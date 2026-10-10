//! 显示修正 y = kx + b（`Doc/详细设计.md` §5 第 2 条）。
//! 安全边界约束修正前的值（用户看到的）；修正后的下发值另按硬件极限校验。

use dc3005u_proto::{Qty, amp_to_ma, volt_to_mv};

use crate::config::Settings;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cali {
    pub on: bool,
    pub v: (f64, f64),
    pub i: (f64, f64),
    pub vset: (f64, f64),
    pub iset: (f64, f64),
}

impl Default for Cali {
    fn default() -> Self {
        Self { on: false, v: (1.0, 0.0), i: (1.0, 0.0), vset: (1.0, 0.0), iset: (1.0, 0.0) }
    }
}

/// 修正后的下发值超出硬件极限
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OutOfHw {
    pub qty: Qty,
    pub value: f64,
}

impl Cali {
    pub fn from_settings(s: &Settings) -> Self {
        Self {
            on: s.use_cali,
            v: (s.v_cali_k, s.v_cali_b),
            i: (s.i_cali_k, s.i_cali_b),
            vset: (s.vset_cali_k, s.vset_cali_b),
            iset: (s.iset_cali_k, s.iset_cali_b),
        }
    }

    pub fn read_v(&self, v: f64) -> f64 {
        if self.on { self.v.0 * v + self.v.1 } else { v }
    }

    pub fn read_i(&self, i_a: f64) -> f64 {
        if self.on { self.i.0 * i_a + self.i.1 } else { i_a }
    }

    fn set_kb(&self, q: Qty) -> (f64, f64) {
        match (self.on, q) {
            (false, _) => (1.0, 0.0),
            (true, Qty::Voltage) => self.vset,
            (true, Qty::Current) => self.iset,
        }
    }

    /// 编辑值（V / A）→ 下发整数值（mV / mA），并按硬件极限校验
    pub fn encode(&self, q: Qty, value: f64) -> Result<u16, OutOfHw> {
        let (k, b) = self.set_kb(q);
        let sent = k * value + b;
        let hw = q.hw_milli();
        let m = (sent * 1000.0).round();
        if !sent.is_finite() || m < f64::from(*hw.start()) || m > f64::from(*hw.end()) {
            return Err(OutOfHw { qty: q, value: sent });
        }
        Ok(match q {
            Qty::Voltage => volt_to_mv(sent),
            Qty::Current => amp_to_ma(sent),
        })
    }

    /// 设备回显（mV / mA）→ 用户看到的设定值（V / A）
    pub fn decode(&self, q: Qty, milli: u16) -> f64 {
        let (k, b) = self.set_kb(q);
        let raw = f64::from(milli) / 1000.0;
        let v = (raw - b) / k;
        (v * 1000.0).round() / 1000.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_when_off() {
        let c = Cali::default();
        assert_eq!(c.encode(Qty::Voltage, 5.0), Ok(5000));
        assert_eq!(c.decode(Qty::Current, 1234), 1.234);
        assert!(c.encode(Qty::Voltage, 25.0).is_err());
    }

    #[test]
    fn tc57_corrected_value_checked_against_hw() {
        let c = Cali { on: true, vset: (1.1, 0.0), ..Cali::default() };
        assert_eq!(c.encode(Qty::Voltage, 10.0), Ok(11000));
        let e = c.encode(Qty::Voltage, 23.0).unwrap_err();
        assert_eq!(e.qty, Qty::Voltage);
        assert!((c.decode(Qty::Voltage, 11000) - 10.0).abs() < 1e-9);
    }
}
