use crate::cmd;

pub const SOF: u8 = 0x55;
pub const EOF: u8 = 0xAA;

/// `0x55 | body | 0xAA | CRC32_LE`，CRC 覆盖 0x55..=0xAA。
pub fn build_frame(body: &[u8]) -> Vec<u8> {
    let mut f = Vec::with_capacity(body.len() + 6);
    f.push(SOF);
    f.extend_from_slice(body);
    f.push(EOF);
    let crc = crc32fast::hash(&f);
    f.extend_from_slice(&crc.to_le_bytes());
    f
}

pub fn frame_connect() -> Vec<u8> {
    build_frame(&[cmd::CONNECT, 0x00])
}

pub fn frame_poll() -> Vec<u8> {
    build_frame(&[cmd::POLL, 0x00])
}

pub fn frame_output(on: bool) -> Vec<u8> {
    build_frame(&[cmd::OUTPUT, 0x04, on as u8, 0, 0, 0])
}

/// 参数 4 字节 = voltage_mV(u16 LE) | current_limit_mA(u16 LE)。
/// 例：24 V / 1 A → `03 04 C0 5D E8 03`
pub fn frame_set_vi(voltage_mv: u16, current_ma: u16) -> Vec<u8> {
    let mut b = vec![cmd::SET_VI, 0x04];
    b.extend_from_slice(&voltage_mv.to_le_bytes());
    b.extend_from_slice(&current_ma.to_le_bytes());
    build_frame(&b)
}

/// 四舍五入到 1 mV；超出 u16 的值饱和（调用方应已校验范围）。
pub fn volt_to_mv(v: f64) -> u16 {
    to_u16_milli(v)
}

/// 四舍五入到 1 mA。
pub fn amp_to_ma(a: f64) -> u16 {
    to_u16_milli(a)
}

pub fn mv_to_volt(mv: u16) -> f64 {
    f64::from(mv) / 1000.0
}

pub fn ma_to_amp(ma: u16) -> f64 {
    f64::from(ma) / 1000.0
}

fn to_u16_milli(x: f64) -> u16 {
    if !x.is_finite() {
        return 0;
    }
    (x * 1000.0).round().clamp(0.0, f64::from(u16::MAX)) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02X}")).collect::<Vec<_>>().join(" ")
    }

    #[test]
    fn tc01_connect_and_poll_frames() {
        assert_eq!(hex(&frame_connect()), "55 01 00 AA 4D 00 56 EA");
        assert_eq!(hex(&frame_poll()), "55 05 00 AA 91 A8 5F ED");
    }

    #[test]
    fn tc02_output_frames() {
        assert_eq!(hex(&frame_output(true)), "55 06 04 01 00 00 00 AA A1 36 C6 8B");
        assert_eq!(hex(&frame_output(false)), "55 06 04 00 00 00 00 AA 11 1F A6 B6");
    }

    #[test]
    fn tc03_set_vi_frame() {
        assert_eq!(hex(&frame_set_vi(24000, 1000)), "55 03 04 C0 5D E8 03 AA C9 DD 30 B8");
    }

    #[test]
    fn tc58_unit_conversion() {
        let cases = [
            (0.8, 0.5, [0x20, 0x03, 0xF4, 0x01]),
            (5.0, 2.0, [0x88, 0x13, 0xD0, 0x07]),
            (24.0, 5.0, [0xC0, 0x5D, 0x88, 0x13]),
            (3.3004, 1.0006, [0xE4, 0x0C, 0xE9, 0x03]),
        ];
        for (v, a, bytes) in cases {
            let f = frame_set_vi(volt_to_mv(v), amp_to_ma(a));
            assert_eq!(&f[3..7], &bytes, "{v} V / {a} A");
        }
    }

    #[test]
    fn conversion_saturates_and_rejects_nan() {
        assert_eq!(volt_to_mv(100.0), u16::MAX);
        assert_eq!(volt_to_mv(-1.0), 0);
        assert_eq!(volt_to_mv(f64::NAN), 0);
    }
}
