//! 硬件极限与安全边界（F16）。所有比较都先四舍五入到 mV / mA 整数，避免浮点误差。

use std::ops::RangeInclusive;

use serde::{Deserialize, Serialize};

/// 硬件极限：电压 0.8~24 V（mV）
pub const HW_V_MV: RangeInclusive<u16> = 800..=24000;
/// 硬件极限：限流 0.5~5.0 A（mA）
pub const HW_I_MA: RangeInclusive<u16> = 500..=5000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Qty {
    Voltage,
    Current,
}

impl Qty {
    pub fn unit(self) -> &'static str {
        match self {
            Qty::Voltage => "V",
            Qty::Current => "A",
        }
    }

    pub fn hw_milli(self) -> RangeInclusive<u16> {
        match self {
            Qty::Voltage => HW_V_MV,
            Qty::Current => HW_I_MA,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Field {
    VMin,
    VMax,
    IMin,
    IMax,
    Voltage,
    Current,
}

impl Field {
    pub fn qty(self) -> Qty {
        match self {
            Field::VMin | Field::VMax | Field::Voltage => Qty::Voltage,
            Field::IMin | Field::IMax | Field::Current => Qty::Current,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LimitError {
    /// NaN / inf / 无法解析
    NotFinite { field: Field },
    /// 安全边界的某个值超出硬件极限
    OutOfHw { field: Field, min: f64, max: f64 },
    /// 下限 > 上限
    Order { qty: Qty },
    /// 设定值超出安全边界
    OutOfSafety { field: Field, value: f64, min: f64, max: f64 },
}

impl LimitError {
    pub fn field(&self) -> Option<Field> {
        match *self {
            LimitError::NotFinite { field }
            | LimitError::OutOfHw { field, .. }
            | LimitError::OutOfSafety { field, .. } => Some(field),
            LimitError::Order { .. } => None,
        }
    }
}

/// 用户在硬件极限内自定的工作范围，单位 V / A，默认等于硬件极限。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SafetyLimits {
    pub v_min: f64,
    pub v_max: f64,
    pub i_min: f64,
    pub i_max: f64,
}

impl Default for SafetyLimits {
    fn default() -> Self {
        Self::hardware()
    }
}

pub(crate) fn milli(x: f64) -> i64 {
    (x * 1000.0).round() as i64
}

fn hw_bounds(q: Qty) -> (f64, f64) {
    let r = q.hw_milli();
    (f64::from(*r.start()) / 1000.0, f64::from(*r.end()) / 1000.0)
}

impl SafetyLimits {
    pub fn hardware() -> Self {
        let (v_min, v_max) = hw_bounds(Qty::Voltage);
        let (i_min, i_max) = hw_bounds(Qty::Current);
        Self { v_min, v_max, i_min, i_max }
    }

    /// 四个值都按 0.001 取整后的副本。
    pub fn rounded(&self) -> Self {
        let r = |x: f64| milli(x) as f64 / 1000.0;
        Self { v_min: r(self.v_min), v_max: r(self.v_max), i_min: r(self.i_min), i_max: r(self.i_max) }
    }

    pub fn range(&self, q: Qty) -> (f64, f64) {
        match q {
            Qty::Voltage => (self.v_min, self.v_max),
            Qty::Current => (self.i_min, self.i_max),
        }
    }

    pub fn is_hardware(&self) -> bool {
        self.rounded() == Self::hardware()
    }

    /// 第 1 层：设置安全边界时校验，返回全部错误；任一错误则不保存。
    pub fn validate(&self) -> Result<(), Vec<LimitError>> {
        let mut errs = Vec::new();
        let items = [
            (Field::VMin, self.v_min),
            (Field::VMax, self.v_max),
            (Field::IMin, self.i_min),
            (Field::IMax, self.i_max),
        ];
        for (field, value) in items {
            if !value.is_finite() {
                errs.push(LimitError::NotFinite { field });
                continue;
            }
            let hw = field.qty().hw_milli();
            if !(i64::from(*hw.start())..=i64::from(*hw.end())).contains(&milli(value)) {
                let (min, max) = hw_bounds(field.qty());
                errs.push(LimitError::OutOfHw { field, min, max });
            }
        }
        for (qty, lo, hi) in [(Qty::Voltage, self.v_min, self.v_max), (Qty::Current, self.i_min, self.i_max)] {
            if lo.is_finite() && hi.is_finite() && milli(lo) > milli(hi) {
                errs.push(LimitError::Order { qty });
            }
        }
        if errs.is_empty() { Ok(()) } else { Err(errs) }
    }

    /// 校验单个设定值是否在安全边界内（四舍五入到 mV/mA 后比较）。
    pub fn check(&self, field: Field, value: f64) -> Result<u16, LimitError> {
        if !value.is_finite() {
            return Err(LimitError::NotFinite { field });
        }
        let (min, max) = self.range(field.qty());
        let m = milli(value);
        if m < milli(min) || m > milli(max) {
            return Err(LimitError::OutOfSafety { field, value, min, max });
        }
        Ok(m as u16)
    }
}

/// 第 2 层：所有设定来源调用。v、i 是用户看到的值（显示修正之前）。
/// 通过时返回四舍五入后的 (mV, mA)。
pub fn validate_setpoint(v: f64, i: f64, lim: &SafetyLimits) -> Result<(u16, u16), Vec<LimitError>> {
    match (lim.check(Field::Voltage, v), lim.check(Field::Current, i)) {
        (Ok(mv), Ok(ma)) => Ok((mv, ma)),
        (a, b) => Err([a.err(), b.err()].into_iter().flatten().collect()),
    }
}

/// 仅用于 AUX 运行中的瞬时值；返回 (v, i, 是否发生钳位)。
pub fn clamp_to_limits(v: f64, i: f64, lim: &SafetyLimits) -> (f64, f64, bool) {
    let cv = v.clamp(lim.v_min, lim.v_max);
    let ci = i.clamp(lim.i_min, lim.i_max);
    (cv, ci, milli(cv) != milli(v) || milli(ci) != milli(i))
}

/// 最后一道保险：IO 线程编码 0x03 前调用。先按安全边界（显示修正开启时传 None），
/// 再按硬件极限钳位；返回 (mV, mA, 是否触发)。
pub fn guard_before_send(mv: u16, ma: u16, lim: Option<&SafetyLimits>) -> (u16, u16, bool) {
    let mut v = mv;
    let mut i = ma;
    if let Some(l) = lim {
        v = v.clamp(milli(l.v_min) as u16, milli(l.v_max) as u16);
        i = i.clamp(milli(l.i_min) as u16, milli(l.i_max) as u16);
    }
    v = v.clamp(*HW_V_MV.start(), *HW_V_MV.end());
    i = i.clamp(*HW_I_MA.start(), *HW_I_MA.end());
    (v, i, v != mv || i != ma)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lim(v_min: f64, v_max: f64, i_min: f64, i_max: f64) -> SafetyLimits {
        SafetyLimits { v_min, v_max, i_min, i_max }
    }

    #[test]
    fn default_is_hardware() {
        let l = SafetyLimits::default();
        assert_eq!(l, lim(0.8, 24.0, 0.5, 5.0));
        assert!(l.validate().is_ok());
        assert!(l.is_hardware());
    }

    #[test]
    fn tc42_out_of_hardware() {
        let errs = lim(0.5, 30.0, 0.1, 6.0).validate().unwrap_err();
        let fields: Vec<_> = errs.iter().filter_map(|e| e.field()).collect();
        assert_eq!(fields, [Field::VMin, Field::VMax, Field::IMin, Field::IMax]);
    }

    #[test]
    fn tc43_order() {
        let errs = lim(12.0, 5.0, 3.0, 1.0).validate().unwrap_err();
        assert!(errs.contains(&LimitError::Order { qty: Qty::Voltage }));
        assert!(errs.contains(&LimitError::Order { qty: Qty::Current }));
        assert!(lim(5.0, 5.0, 1.0, 1.0).validate().is_ok());
    }

    #[test]
    fn nan_is_rejected() {
        let errs = lim(f64::NAN, 24.0, 0.5, 5.0).validate().unwrap_err();
        assert_eq!(errs, [LimitError::NotFinite { field: Field::VMin }]);
    }

    #[test]
    fn tc106_rounding_before_compare() {
        let l = SafetyLimits::default();
        assert_eq!(validate_setpoint(0.7996, 1.0, &l), Ok((800, 1000)));
        assert!(validate_setpoint(0.7994, 1.0, &l).is_err());
        assert_eq!(validate_setpoint(24.0004, 5.0004, &l), Ok((24000, 5000)));
    }

    #[test]
    fn tc45_setpoint_outside_safety() {
        let l = lim(3.0, 12.0, 0.5, 2.0);
        let errs = validate_setpoint(20.0, 3.0, &l).unwrap_err();
        assert_eq!(errs.len(), 2);
        assert!(matches!(errs[0], LimitError::OutOfSafety { field: Field::Voltage, .. }));
        assert!(matches!(errs[1], LimitError::OutOfSafety { field: Field::Current, .. }));
        assert_eq!(validate_setpoint(5.0, 1.0, &l), Ok((5000, 1000)));
    }

    #[test]
    fn clamp_reports_change() {
        let l = lim(3.0, 12.0, 0.5, 2.0);
        assert_eq!(clamp_to_limits(2.0, 1.0, &l), (3.0, 1.0, true));
        assert_eq!(clamp_to_limits(5.0, 1.0, &l), (5.0, 1.0, false));
    }

    #[test]
    fn tc56_guard() {
        let l = lim(3.0, 12.0, 0.5, 2.0);
        assert_eq!(guard_before_send(30000, 1000, Some(&l)), (12000, 1000, true));
        assert_eq!(guard_before_send(30000, 6000, None), (24000, 5000, true));
        assert_eq!(guard_before_send(5000, 1000, Some(&l)), (5000, 1000, false));
    }
}
