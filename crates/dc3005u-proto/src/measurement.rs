/// 读数帧头部至少要有 56 字节（到 @54 限流回显为止）
pub const MEASUREMENT_MIN_LEN: usize = 56;

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Measurement {
    /// @16 帧序号
    pub seq: u32,
    /// u16@30 / 1000
    pub voltage_v: f64,
    /// u32@20 / 100000，瞬时值，分辨率 10 nA
    pub current_ma: f64,
    /// u32@24 / 1e8；u32 最多只能表示 42.95 W，仅调试/校验用，界面功率用 V×I
    pub power_dev_w: f64,
    /// @52 电压设定回显，mV
    pub set_voltage_mv: u16,
    /// @54 限流设定回显，mA
    pub set_current_ma: u16,
}

impl Measurement {
    pub fn current_a(&self) -> f64 {
        self.current_ma / 1000.0
    }

    /// 主机计算功率 V×I（W）
    pub fn power_calc_w(&self) -> f64 {
        self.voltage_v * self.current_a()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ParseError {
    #[error("读数帧过短：{0} 字节")]
    ShortFrame(usize),
}

pub fn parse_measurement(data: &[u8]) -> Result<Measurement, ParseError> {
    if data.len() < MEASUREMENT_MIN_LEN {
        return Err(ParseError::ShortFrame(data.len()));
    }
    let u16_at = |o: usize| u16::from_le_bytes([data[o], data[o + 1]]);
    let u32_at = |o: usize| u32::from_le_bytes([data[o], data[o + 1], data[o + 2], data[o + 3]]);
    Ok(Measurement {
        seq: u32_at(16),
        voltage_v: f64::from(u16_at(30)) / 1000.0,
        current_ma: f64::from(u32_at(20)) / 100_000.0,
        power_dev_w: f64::from(u32_at(24)) / 1e8,
        set_voltage_mv: u16_at(52),
        set_current_ma: u16_at(54),
    })
}

/// 按协议布局合成一帧读数（测试与模拟设备用）。
pub fn synth_measurement_frame(
    len: usize,
    seq: u32,
    voltage_mv: u16,
    current_raw: u32,
    power_raw: u32,
    set_mv: u16,
    set_ma: u16,
) -> Vec<u8> {
    let mut d = vec![0u8; len.max(MEASUREMENT_MIN_LEN)];
    d[16..20].copy_from_slice(&seq.to_le_bytes());
    d[20..24].copy_from_slice(&current_raw.to_le_bytes());
    d[24..28].copy_from_slice(&power_raw.to_le_bytes());
    d[30..32].copy_from_slice(&voltage_mv.to_le_bytes());
    d[52..54].copy_from_slice(&set_mv.to_le_bytes());
    d[54..56].copy_from_slice(&set_ma.to_le_bytes());
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tc04_condition_a() {
        // 工况 A：4.999 V、≈575 mA、≈2.874 W
        let f = synth_measurement_frame(10072, 1203456, 4999, 57_512_345, 287_400_000, 5000, 2000);
        let m = parse_measurement(&f).unwrap();
        assert_eq!(m.seq, 1203456);
        assert!((m.voltage_v - 4.999).abs() < 1e-9);
        assert!((m.current_ma - 575.12345).abs() < 1e-9);
        assert!((m.power_dev_w - 2.874).abs() < 1e-9);
        assert_eq!((m.set_voltage_mv, m.set_current_ma), (5000, 2000));
    }

    #[test]
    fn tc05_power_self_check() {
        // 工况 B：6.999 V、≈13.84 mA、≈0.097 W
        let f = synth_measurement_frame(10072, 7, 6999, 1_384_000, 9_686_616, 7000, 1000);
        let m = parse_measurement(&f).unwrap();
        let rel = (m.power_dev_w - m.power_calc_w()).abs() / m.power_dev_w;
        assert!(rel < 0.05, "rel = {rel}");
    }

    #[test]
    fn tc06_short_frame() {
        assert_eq!(parse_measurement(&[0u8; 55]), Err(ParseError::ShortFrame(55)));
        assert_eq!(parse_measurement(&[]), Err(ParseError::ShortFrame(0)));
    }

    #[test]
    fn tc79_power_beyond_device_field() {
        // 24 V × 4 A = 96 W，设备功率字段 u32 已溢出，界面只用 V×I
        let f = synth_measurement_frame(56, 1, 24000, 400_000_000, u32::MAX, 24000, 5000);
        let m = parse_measurement(&f).unwrap();
        assert!((m.power_calc_w() - 96.0).abs() < 1e-9);
    }

    #[test]
    fn resolution_10na() {
        let f = synth_measurement_frame(56, 1, 5000, 1, 0, 5000, 500);
        let m = parse_measurement(&f).unwrap();
        assert!((m.current_a() - 1e-8).abs() < 1e-15);
    }
}
