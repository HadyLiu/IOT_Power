//! DC3005U 协议编解码（纯函数，无 IO）。
//!
//! 协议依据 `Doc/Communication/协议文档.md` v0.4。

pub mod frame;
pub mod identity;
pub mod limits;
pub mod measurement;
pub mod response;

pub use frame::{
    amp_to_ma, build_frame, frame_connect, frame_output, frame_poll, frame_set_vi, ma_to_amp, mv_to_volt, volt_to_mv,
};
pub use identity::{Identity, parse_identity, synth_identity};
pub use limits::{
    Field, HW_I_MA, HW_V_MV, LimitError, Qty, SafetyLimits, clamp_to_limits, guard_before_send, validate_setpoint,
};
pub use measurement::{MEASUREMENT_MIN_LEN, Measurement, ParseError, parse_measurement, synth_measurement_frame};
pub use response::{ConnectReply, ResponseError, check_poll_reply, check_short_ack, classify_connect_reply};

pub const VID: u16 = 0x0408;
pub const PID: u16 = 0x1043;
pub const INTERFACE: u8 = 0;
pub const EP_OUT: u8 = 0x01;
pub const EP_IN: u8 = 0x82;
/// 读数帧约 10072 字节，按实际读取长度处理
pub const POLL_RESP_LEN: usize = 10072;

pub mod cmd {
    pub const CONNECT: u8 = 0x01;
    pub const SET_VI: u8 = 0x03;
    pub const POLL: u8 = 0x05;
    pub const OUTPUT: u8 = 0x06;
}
