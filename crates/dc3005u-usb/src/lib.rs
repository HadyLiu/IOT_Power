//! DC3005U 传输层与设备服务（IO 线程）。
//!
//! IO 线程独占 USB 句柄，严格按「请求 → 应答 → 下一条」串行执行事务，
//! 规则见 `Doc/架构设计.md` §4.1 与 §5。

pub mod sim;
pub mod transport;
pub mod worker;

pub use transport::{Connector, NusbConnector, OpenError, Transport, TransportError};
pub use worker::{
    Ack, Cmd, CmdError, CmdId, CmdStats, CommStats, DeviceError, DeviceHandle, Event, LinkOrigin, LinkState, Sample,
    Source, WorkerConfig, spawn,
};
