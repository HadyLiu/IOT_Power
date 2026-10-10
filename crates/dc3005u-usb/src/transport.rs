use std::time::Duration;

use dc3005u_proto::{EP_IN, EP_OUT, INTERFACE, PID, VID};
use nusb::MaybeFuture;
use nusb::transfer::{Buffer, Bulk, In, Out, TransferError};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TransportError {
    #[error("超时")]
    Timeout,
    #[error("设备已断开")]
    Disconnected,
    #[error("{0}")]
    Io(String),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OpenError {
    #[error("未找到设备")]
    NotFound,
    #[error("设备被占用")]
    Busy,
    #[error("没有 USB 访问权限")]
    Permission,
    #[error("{0}")]
    Io(String),
}

/// 一个已打开并 claim 了接口 0 的设备连接。
pub trait Transport: Send {
    fn write(&mut self, data: &[u8], timeout: Duration) -> Result<(), TransportError>;
    /// 读到短包或读满 `max_len` 为止；超时返回 `Err(Timeout)`。
    fn read(&mut self, max_len: usize, timeout: Duration) -> Result<Vec<u8>, TransportError>;
}

/// 负责枚举并打开设备；IO 线程在首次连接和重连时调用。
pub trait Connector: Send {
    fn open(&mut self) -> Result<Box<dyn Transport>, OpenError>;
}

/// 真实设备：nusb（Windows 走 WinUSB，Linux 走 usbfs）。多设备时连第一台。
#[derive(Default)]
pub struct NusbConnector;

fn map_open_err(e: nusb::Error) -> OpenError {
    match e.kind() {
        nusb::ErrorKind::Busy => OpenError::Busy,
        nusb::ErrorKind::PermissionDenied => OpenError::Permission,
        nusb::ErrorKind::NotFound | nusb::ErrorKind::Disconnected => OpenError::NotFound,
        _ => OpenError::Io(e.to_string()),
    }
}

impl Connector for NusbConnector {
    fn open(&mut self) -> Result<Box<dyn Transport>, OpenError> {
        let info = nusb::list_devices()
            .wait()
            .map_err(|e| OpenError::Io(e.to_string()))?
            .find(|d| d.vendor_id() == VID && d.product_id() == PID)
            .ok_or(OpenError::NotFound)?;
        let device = info.open().wait().map_err(map_open_err)?;
        let interface = device.claim_interface(INTERFACE).wait().map_err(map_open_err)?;
        let ep_out = interface.endpoint::<Bulk, Out>(EP_OUT).map_err(|e| OpenError::Io(e.to_string()))?;
        let ep_in = interface.endpoint::<Bulk, In>(EP_IN).map_err(|e| OpenError::Io(e.to_string()))?;
        Ok(Box::new(NusbTransport { ep_out, ep_in, _interface: interface }))
    }
}

struct NusbTransport {
    ep_out: nusb::Endpoint<Bulk, Out>,
    ep_in: nusb::Endpoint<Bulk, In>,
    // 端点释放后接口才会 release，所以字段顺序放最后
    _interface: nusb::Interface,
}

fn map_transfer_err(e: TransferError) -> TransportError {
    match e {
        TransferError::Cancelled => TransportError::Timeout,
        TransferError::Disconnected => TransportError::Disconnected,
        other => TransportError::Io(other.to_string()),
    }
}

impl Transport for NusbTransport {
    fn write(&mut self, data: &[u8], timeout: Duration) -> Result<(), TransportError> {
        let c = self.ep_out.transfer_blocking(Buffer::from(data.to_vec()), timeout);
        c.status.map_err(map_transfer_err)
    }

    fn read(&mut self, max_len: usize, timeout: Duration) -> Result<Vec<u8>, TransportError> {
        // IN 传输长度必须是 wMaxPacketSize 的整数倍
        let mps = self.ep_in.max_packet_size().max(1);
        let len = max_len.max(1).div_ceil(mps) * mps;
        let c = self.ep_in.transfer_blocking(Buffer::new(len), timeout);
        if let Err(e) = c.status {
            if matches!(e, TransferError::Stall) {
                let _ = self.ep_in.clear_halt().wait();
            }
            return Err(map_transfer_err(e));
        }
        Ok(c.buffer.into_vec())
    }
}
