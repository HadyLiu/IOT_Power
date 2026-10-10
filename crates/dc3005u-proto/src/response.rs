//! 应答校验（架构设计 §4.1）。

use crate::frame::{EOF, SOF};
use crate::identity::{Identity, parse_identity};
use crate::measurement::{MEASUREMENT_MIN_LEN, Measurement, parse_measurement};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ResponseError {
    #[error("空应答")]
    Empty,
    #[error("应答帧格式错误")]
    BadFraming,
    #[error("应答 CRC 错误")]
    BadCrc,
    #[error("应答命令字不符：期望 0x{expected:02X}，收到 0x{got:02X}")]
    WrongCmd { expected: u8, got: u8 },
    #[error("应答过短：{0} 字节")]
    Short(usize),
    #[error("无法识别的应答")]
    NotRecognized,
}

/// 以 0x55 开头的应答必须是完整的 `55|cmd..|AA|CRC32`，且命令字与请求相同。
fn check_framed(resp: &[u8], cmd: u8) -> Result<(), ResponseError> {
    if resp.len() < 7 || resp[resp.len() - 5] != EOF {
        return Err(ResponseError::BadFraming);
    }
    let (payload, crc) = resp.split_at(resp.len() - 4);
    if crc32fast::hash(payload).to_le_bytes() != crc {
        return Err(ResponseError::BadCrc);
    }
    if resp[1] != cmd {
        return Err(ResponseError::WrongCmd { expected: cmd, got: resp[1] });
    }
    Ok(())
}

/// `0x03` / `0x06` 的短应答。内容格式未知（PRD §8 #2）：
/// 非空即算已应答；若以 0x55 开头则按封装帧严格校验。
pub fn check_short_ack(resp: &[u8], cmd: u8) -> Result<(), ResponseError> {
    match resp.first() {
        None => Err(ResponseError::Empty),
        Some(&SOF) => check_framed(resp, cmd),
        Some(_) => Ok(()),
    }
}

/// `0x05` 应答：裸二进制读数帧，按长度校验。
pub fn check_poll_reply(resp: &[u8]) -> Result<Measurement, ResponseError> {
    if resp.is_empty() {
        return Err(ResponseError::Empty);
    }
    parse_measurement(resp).map_err(|_| ResponseError::Short(resp.len()))
}

#[derive(Debug, Clone, PartialEq)]
pub enum ConnectReply {
    Identity(Identity),
    /// 已连接状态下 `0x01` 可能直接返回读数类数据（协议 §4.1），身份沿用上次
    Measurement(Measurement),
}

/// 身份帧以可见 ASCII（型号）开头；读数帧前 16 字节多为 0。
pub fn classify_connect_reply(resp: &[u8]) -> Result<ConnectReply, ResponseError> {
    let first = *resp.first().ok_or(ResponseError::Empty)?;
    if first == SOF {
        check_framed(resp, crate::cmd::CONNECT)?;
    }
    if first.is_ascii_graphic()
        && first != SOF
        && resp.len() < 256
        && let Some(id) = parse_identity(resp)
    {
        return Ok(ConnectReply::Identity(id));
    }
    if resp.len() >= MEASUREMENT_MIN_LEN {
        return parse_measurement(resp).map(ConnectReply::Measurement).map_err(|_| ResponseError::NotRecognized);
    }
    Err(ResponseError::NotRecognized)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::build_frame;
    use crate::identity::synth_identity;
    use crate::measurement::synth_measurement_frame;

    #[test]
    fn tc69_framed_ack_validation() {
        let ok = build_frame(&[0x03, 0x00]);
        assert_eq!(check_short_ack(&ok, 0x03), Ok(()));

        let mut bad_crc = ok.clone();
        *bad_crc.last_mut().unwrap() ^= 0xFF;
        assert_eq!(check_short_ack(&bad_crc, 0x03), Err(ResponseError::BadCrc));

        let wrong = build_frame(&[0x06, 0x00]);
        assert_eq!(check_short_ack(&wrong, 0x03), Err(ResponseError::WrongCmd { expected: 0x03, got: 0x06 }));
        assert_eq!(check_short_ack(&[0x55, 0x03], 0x03), Err(ResponseError::BadFraming));
    }

    #[test]
    fn unknown_short_ack_accepted() {
        assert_eq!(check_short_ack(&[0x00], 0x06), Ok(()));
        assert_eq!(check_short_ack(&[], 0x06), Err(ResponseError::Empty));
    }

    #[test]
    fn tc68_connect_reply_classification() {
        let id = synth_identity("DC3005U", "8B63A8F401E4", "1.9.5");
        assert!(matches!(classify_connect_reply(&id), Ok(ConnectReply::Identity(_))));

        let meas = synth_measurement_frame(10072, 9, 5000, 0, 0, 5000, 1000);
        assert!(matches!(classify_connect_reply(&meas), Ok(ConnectReply::Measurement(m)) if m.seq == 9));

        assert_eq!(classify_connect_reply(&[0u8; 10]), Err(ResponseError::NotRecognized));
    }

    #[test]
    fn poll_reply_length() {
        assert_eq!(check_poll_reply(&[0u8; 40]), Err(ResponseError::Short(40)));
        assert!(check_poll_reply(&synth_measurement_frame(10072, 1, 0, 0, 0, 0, 0)).is_ok());
    }
}
