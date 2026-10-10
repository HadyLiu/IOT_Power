//! 状态区数值格式：电流 / 功率自动量程（5% 回差）、阻值规则（`Doc/详细设计.md` §3.6）。

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scale {
    Unit,
    Milli,
    Micro,
}

impl Scale {
    fn factor(self) -> f64 {
        match self {
            Scale::Unit => 1.0,
            Scale::Milli => 1e3,
            Scale::Micro => 1e6,
        }
    }

    fn prefix(self) -> &'static str {
        match self {
            Scale::Unit => "",
            Scale::Milli => "m",
            Scale::Micro => "µ",
        }
    }

    pub fn parse_unit(s: &str) -> Option<Self> {
        match s {
            "A" => Some(Scale::Unit),
            "mA" => Some(Scale::Milli),
            "µA" => Some(Scale::Micro),
            _ => None,
        }
    }
}

const HYSTERESIS: f64 = 0.05;

/// 带回差的量程选择：≥1 → 单位，1e-3~1 → 毫，<1e-3 → 微。
/// 往小量程切要低于门限的 95%，往大量程切要达到门限。
pub fn auto_scale(prev: Scale, abs_value: f64) -> Scale {
    let x = abs_value;
    let target = if x >= 1.0 {
        Scale::Unit
    } else if x >= 1e-3 {
        Scale::Milli
    } else {
        Scale::Micro
    };
    let down_ok = |thr: f64| x < thr * (1.0 - HYSTERESIS);
    match (prev, target) {
        (Scale::Unit, Scale::Milli) if !down_ok(1.0) => Scale::Unit,
        (Scale::Unit, Scale::Micro) if !down_ok(1.0) => Scale::Unit,
        (Scale::Unit, Scale::Micro) if !down_ok(1e-3) => Scale::Milli,
        (Scale::Milli, Scale::Micro) if !down_ok(1e-3) => Scale::Milli,
        _ => target,
    }
}

/// 电流：小数位 = 3 + interp，但不超过 10 nA 分辨率（A 8 位、mA 5 位、µA 2 位）
pub fn current_decimals(scale: Scale, interp: u8) -> usize {
    let want = 3 + usize::from(interp);
    match scale {
        Scale::Unit => want.min(8),
        Scale::Milli => want.min(5),
        Scale::Micro => 2,
    }
}

/// 返回 (数值文本, 单位)
pub fn current(i_a: f64, scale: Scale, interp: u8) -> (String, String) {
    let d = current_decimals(scale, interp);
    (format!("{:.*}", d, i_a * scale.factor()), format!("{}A", scale.prefix()))
}

pub fn power(p_w: f64, scale: Scale, interp: u8) -> (String, String) {
    let d = 3 + usize::from(interp);
    (format!("{:.*}", d, p_w * scale.factor()), format!("{}W", scale.prefix()))
}

/// R = V/I；I = 0 显示「∞」，R > 10 MΩ 显示「--」，其余 Ω/kΩ/MΩ
pub fn resistance(v: f64, i_a: f64, interp: u8) -> (String, String) {
    if i_a == 0.0 {
        return ("∞".into(), "Ω".into());
    }
    let r = (v / i_a).abs();
    if !r.is_finite() || r > 10e6 {
        return ("--".into(), "Ω".into());
    }
    let d = 3 + usize::from(interp);
    if r >= 1e6 {
        (format!("{:.*}", d, r / 1e6), "MΩ".into())
    } else if r >= 1e3 {
        (format!("{:.*}", d, r / 1e3), "kΩ".into())
    } else {
        (format!("{:.*}", d, r), "Ω".into())
    }
}

/// 能量：≥ 1000 J 显示 kJ
pub fn energy(j: f64) -> (String, String) {
    if j.abs() >= 1e3 { (format!("{:.3}", j / 1e3), "kJ".into()) } else { (format!("{j:.3}"), "J".into()) }
}

/// 电量：≥ 1000 mAh 显示 Ah
pub fn charge(mah: f64, interp: u8) -> (String, String) {
    let d = 3 + usize::from(interp);
    if mah.abs() >= 1e3 {
        (format!("{:.*}", d, mah / 1e3), "Ah".into())
    } else {
        (format!("{:.*}", d, mah), "mAh".into())
    }
}

/// 设定值统一 3 位小数
pub fn milli3(x: f64) -> String {
    format!("{x:.3}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_with_hysteresis() {
        assert_eq!(auto_scale(Scale::Milli, 1.0), Scale::Unit);
        assert_eq!(auto_scale(Scale::Unit, 0.97), Scale::Unit);
        assert_eq!(auto_scale(Scale::Unit, 0.94), Scale::Milli);
        assert_eq!(auto_scale(Scale::Milli, 0.00097), Scale::Milli);
        assert_eq!(auto_scale(Scale::Milli, 0.0009), Scale::Micro);
        assert_eq!(auto_scale(Scale::Micro, 0.001), Scale::Milli);
        assert_eq!(auto_scale(Scale::Unit, 0.0009), Scale::Micro);
    }

    #[test]
    fn tc05_small_current_resolution() {
        // 10 nA 在 µA 量程显示为 0.01
        assert_eq!(current(1e-8, Scale::Micro, 0), ("0.01".into(), "µA".into()));
        assert_eq!(current(0.012345, Scale::Milli, 3), ("12.34500".into(), "mA".into()));
    }

    #[test]
    fn energy_and_charge_units() {
        assert_eq!(energy(999.9994), ("999.999".into(), "J".into()));
        assert_eq!(energy(1000.0), ("1.000".into(), "kJ".into()));
        assert_eq!(energy(123_456.0), ("123.456".into(), "kJ".into()));
        assert_eq!(charge(999.5, 0), ("999.500".into(), "mAh".into()));
        assert_eq!(charge(1000.0, 0), ("1.000".into(), "Ah".into()));
        assert_eq!(charge(2500.0, 1), ("2.5000".into(), "Ah".into()));
    }

    #[test]
    fn resistance_rules() {
        assert_eq!(resistance(5.0, 0.0, 0).0, "∞");
        assert_eq!(resistance(5.0, 1e-7, 0).0, "--");
        assert_eq!(resistance(5.0, 0.5, 0), ("10.000".into(), "Ω".into()));
        assert_eq!(resistance(5.0, 0.001, 0), ("5.000".into(), "kΩ".into()));
    }
}
