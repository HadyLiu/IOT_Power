#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Identity {
    pub model: String,
    pub serial: String,
    pub firmware: String,
}

/// 身份应答约 63 字节：ASCII 字符串按 `\0` 分隔，依次为型号、序列号、固件版本。
/// 精确字段边界未标定，取前三段非空的可见字符串；一段都没有则返回 None。
pub fn parse_identity(data: &[u8]) -> Option<Identity> {
    let mut parts = data
        .split(|b| !(b.is_ascii_graphic() || *b == b' '))
        .map(|s| String::from_utf8_lossy(s).trim().to_string())
        .filter(|s| !s.is_empty());
    let model = parts.next()?;
    Some(Identity { model, serial: parts.next().unwrap_or_default(), firmware: parts.next().unwrap_or_default() })
}

/// 合成身份应答（测试与模拟设备用）。
pub fn synth_identity(model: &str, serial: &str, firmware: &str) -> Vec<u8> {
    let mut d = Vec::with_capacity(63);
    for s in [model, serial, firmware] {
        d.extend_from_slice(s.as_bytes());
        d.push(0);
    }
    d.resize(63, 0);
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_three_fields() {
        let id = parse_identity(&synth_identity("DC3005U", "8B63A8F401E4", "1.9.5")).unwrap();
        assert_eq!(id.model, "DC3005U");
        assert_eq!(id.serial, "8B63A8F401E4");
        assert_eq!(id.firmware, "1.9.5");
    }

    #[test]
    fn tolerates_padding_and_missing_fields() {
        let mut d = vec![0u8; 5];
        d.extend_from_slice(b"DC3005U\0\0\0");
        let id = parse_identity(&d).unwrap();
        assert_eq!(id.model, "DC3005U");
        assert!(id.serial.is_empty());
        assert_eq!(parse_identity(&[0u8; 63]), None);
    }
}
