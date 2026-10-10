//! 输出状态的三级可信度（`Doc/详细设计.md` §0 第 4 条、§2.2）。

use std::collections::HashMap;

use dc3005u_usb::{Ack, CmdError, CmdId, LinkOrigin, LinkState, Source};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputView {
    /// 本次运行还没连过设备
    NoInfo,
    /// 本次连接内最后一条 `0x06` 已应答
    Acked(bool),
    /// 断线 / 重连后还没再发过 `0x06`
    Stale(bool),
    /// 从未得知（`off_on_connect` 关闭）或关输出失败
    Unknown,
}

impl OutputView {
    /// 「输出可能为开」= 开（任一可信度）或未知
    pub fn may_be_on(self) -> bool {
        matches!(self, OutputView::Acked(true) | OutputView::Stale(true) | OutputView::Unknown)
    }
}

/// UI 需要提示的结果
#[derive(Debug, Clone, PartialEq)]
pub enum OutputNotice {
    OnFailed(CmdError),
    OffFailed,
}

#[derive(Debug)]
pub struct OutputState {
    pub view: OutputView,
    pub pending_off: bool,
    /// 关输出失败（含急停），显示红色横幅直到关成功
    pub off_failed: bool,
    inflight: HashMap<CmdId, bool>,
}

impl Default for OutputState {
    fn default() -> Self {
        Self { view: OutputView::NoInfo, pending_off: false, off_failed: false, inflight: HashMap::new() }
    }
}

impl OutputState {
    pub fn track(&mut self, id: CmdId, on: bool) {
        self.inflight.insert(id, on);
    }

    pub fn on_link(&mut self, s: LinkState) {
        match s {
            LinkState::Connected { origin: LinkOrigin::FirstConnect } => self.view = OutputView::Unknown,
            LinkState::Reconnecting { .. } | LinkState::Disconnected => {
                if let OutputView::Acked(on) = self.view {
                    self.view = OutputView::Stale(on);
                }
                if s == LinkState::Disconnected {
                    self.pending_off = false;
                }
            }
            _ => {}
        }
    }

    /// `hint` 是事件里带的请求值：AUX / HTTP 直接发给 IO 线程的 `0x06` 不在 `inflight` 里
    pub fn on_result(
        &mut self,
        id: CmdId,
        source: Source,
        hint: Option<bool>,
        result: &Result<Ack, CmdError>,
    ) -> Option<OutputNotice> {
        let on = match source {
            Source::Connect | Source::EStop | Source::PendingOff | Source::Exit => false,
            Source::Seq | Source::Aux | Source::Http => self.inflight.remove(&id).or(hint)?,
            _ => self.inflight.remove(&id)?,
        };
        match result {
            Ok(Ack::Acked | Ack::Confirmed) => {
                self.view = OutputView::Acked(on);
                if !on {
                    self.off_failed = false;
                }
                None
            }
            Ok(Ack::Deferred | Ack::Superseded) => None,
            Err(CmdError::NotConnected) => None,
            Err(e) if on => Some(OutputNotice::OnFailed(e.clone())),
            Err(_) => {
                self.view = OutputView::Unknown;
                self.off_failed = true;
                Some(OutputNotice::OffFailed)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_connect_off_then_reconnect_stale() {
        let mut o = OutputState::default();
        assert!(!o.view.may_be_on());
        o.on_link(LinkState::Connected { origin: LinkOrigin::FirstConnect });
        assert_eq!(o.view, OutputView::Unknown);
        o.on_result(0, Source::Connect, None, &Ok(Ack::Acked));
        assert_eq!(o.view, OutputView::Acked(false));
        o.track(5, true);
        o.on_result(5, Source::Manual, None, &Ok(Ack::Acked));
        o.on_link(LinkState::Reconnecting { attempt: 1 });
        assert_eq!(o.view, OutputView::Stale(true));
        o.on_link(LinkState::Connected { origin: LinkOrigin::Reconnect });
        assert_eq!(o.view, OutputView::Stale(true));
    }

    #[test]
    fn tc63_on_failure_keeps_off() {
        let mut o = OutputState { view: OutputView::Acked(false), ..Default::default() };
        o.track(1, true);
        assert_eq!(
            o.on_result(1, Source::Manual, None, &Err(CmdError::Timeout)),
            Some(OutputNotice::OnFailed(CmdError::Timeout))
        );
        assert_eq!(o.view, OutputView::Acked(false));
    }

    #[test]
    fn tc64_off_failure_unknown() {
        let mut o = OutputState { view: OutputView::Acked(true), ..Default::default() };
        o.track(2, false);
        assert_eq!(o.on_result(2, Source::Manual, None, &Err(CmdError::Timeout)), Some(OutputNotice::OffFailed));
        assert_eq!(o.view, OutputView::Unknown);
        assert!(o.off_failed);
        o.on_result(0, Source::PendingOff, None, &Ok(Ack::Acked));
        assert_eq!(o.view, OutputView::Acked(false));
        assert!(!o.off_failed);
    }
}
