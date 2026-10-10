//! 设定值模型：编辑值 / 设备设定 / 待确认（`Doc/详细设计.md` §2.6）。

use dc3005u_proto::{Field, LimitError, Qty, SafetyLimits};
use dc3005u_usb::{Ack, CmdError, CmdId};

use crate::cali::{Cali, OutOfHw};
use crate::format::milli3;

const QS: [Qty; 2] = [Qty::Voltage, Qty::Current];

fn idx(q: Qty) -> usize {
    match q {
        Qty::Voltage => 0,
        Qty::Current => 1,
    }
}

fn field(q: Qty) -> Field {
    match q {
        Qty::Voltage => Field::Voltage,
        Qty::Current => Field::Current,
    }
}

fn milli(x: f64) -> i64 {
    (x * 1000.0).round() as i64
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Pending {
    id: CmdId,
    mv: u16,
    ma: u16,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SetError {
    Limit(LimitError),
    Cali(OutOfHw),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Commit {
    /// 下发 `0x03`
    Send {
        mv: u16,
        ma: u16,
    },
    /// 与设备设定相同，不下发
    NoChange,
    /// 未连接：保留编辑值，标「未提交」
    NotConnected,
    Rejected(SetError),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Confirmed,
    /// 失败，编辑值已恢复为设备设定
    Failed(CmdError),
}

#[derive(Debug, Default)]
pub struct SetpointModel {
    /// 设定框文本（V、A）
    pub text: [String; 2],
    pub focused: [bool; 2],
    /// 用户改过、尚未提交成功
    touched: [bool; 2],
    /// 设备设定（回显原始值 mV / mA）
    device: Option<(u16, u16)>,
    pending: Option<Pending>,
    /// 设备设定在用户编辑期间变化时的提示
    pub echo_notice: Option<(f64, f64)>,
}

impl SetpointModel {
    pub fn new(v: f64, i: f64) -> Self {
        Self { text: [milli3(v), milli3(i)], ..Default::default() }
    }

    pub fn edit_value(&self, q: Qty) -> Option<f64> {
        self.text[idx(q)].trim().parse::<f64>().ok().filter(|x| x.is_finite())
    }

    /// 解析 + 安全边界校验
    pub fn check(&self, q: Qty, lim: &SafetyLimits) -> Result<f64, LimitError> {
        let f = field(q);
        let v = self.edit_value(q).ok_or(LimitError::NotFinite { field: f })?;
        lim.check(f, v).map(|_| v)
    }

    pub fn device_value(&self, q: Qty, cali: &Cali) -> Option<f64> {
        let (mv, ma) = self.device?;
        Some(cali.decode(q, if q == Qty::Voltage { mv } else { ma }))
    }

    pub fn is_pending(&self) -> bool {
        self.pending.is_some()
    }

    /// 编辑值与设备设定不同（未连接时：用户改过即算）
    pub fn uncommitted(&self, q: Qty, cali: &Cali) -> bool {
        match (self.edit_value(q), self.device_value(q, cali)) {
            (Some(e), Some(d)) => milli(e) != milli(d),
            (_, None) => self.touched[idx(q)],
            (None, Some(_)) => true,
        }
    }

    pub fn device_out_of_limits(&self, lim: &SafetyLimits, cali: &Cali) -> bool {
        QS.iter().any(|&q| self.device_value(q, cali).is_some_and(|v| lim.check(field(q), v).is_err()))
    }

    pub fn on_user_edit(&mut self, q: Qty) {
        self.touched[idx(q)] = true;
    }

    fn to_device(&self, q: Qty, lim: &SafetyLimits, cali: &Cali) -> Result<u16, SetError> {
        let v = self.check(q, lim).map_err(SetError::Limit)?;
        cali.encode(q, v).map_err(SetError::Cali)
    }

    /// 规则 1：提交一个量，另一个量取设备设定
    pub fn commit(&mut self, q: Qty, id: CmdId, lim: &SafetyLimits, cali: &Cali) -> Commit {
        let sent = match self.to_device(q, lim, cali) {
            Ok(x) => x,
            Err(e) => return Commit::Rejected(e),
        };
        let Some((dmv, dma)) = self.device else { return Commit::NotConnected };
        let (mv, ma) = if q == Qty::Voltage { (sent, dma) } else { (dmv, sent) };
        self.send(id, mv, ma)
    }

    /// 预设 / HTTP：两个量一起下发，编辑值同时改为新值
    pub fn commit_both(&mut self, id: CmdId, v: f64, i: f64, lim: &SafetyLimits, cali: &Cali) -> Commit {
        let old = self.text.clone();
        self.text = [milli3(v), milli3(i)];
        let r = self.prepare_on(id, lim, cali);
        match r {
            Commit::Send { .. } => self.touched = [true; 2],
            Commit::NoChange => {}
            Commit::NotConnected | Commit::Rejected(_) => self.text = old,
        }
        r
    }

    /// 设备设定原始值（mV, mA）
    pub fn device_raw(&self) -> Option<(u16, u16)> {
        self.device
    }

    /// 规则 6：AUX 运行中编辑值跟随设备设定
    pub fn follow_device(&mut self, cali: &Cali) {
        if self.device.is_some() {
            self.restore(cali);
            self.echo_notice = None;
        }
    }

    /// 规则 5：点 ON 前两个编辑值都要合法；与设备设定不同则一起下发
    pub fn prepare_on(&mut self, id: CmdId, lim: &SafetyLimits, cali: &Cali) -> Commit {
        let mv = match self.to_device(Qty::Voltage, lim, cali) {
            Ok(x) => x,
            Err(e) => return Commit::Rejected(e),
        };
        let ma = match self.to_device(Qty::Current, lim, cali) {
            Ok(x) => x,
            Err(e) => return Commit::Rejected(e),
        };
        if self.device.is_none() {
            return Commit::NotConnected;
        }
        self.send(id, mv, ma)
    }

    fn send(&mut self, id: CmdId, mv: u16, ma: u16) -> Commit {
        if self.device == Some((mv, ma)) && self.pending.is_none() {
            return Commit::NoChange;
        }
        self.pending = Some(Pending { id, mv, ma });
        Commit::Send { mv, ma }
    }

    /// `0x03` 的结果。返回 None 表示不是本模型关心的结果（已被取代 / 中间状态）。
    pub fn on_result(&mut self, id: CmdId, result: &Result<Ack, CmdError>, cali: &Cali) -> Option<Outcome> {
        let p = self.pending.filter(|p| p.id == id)?;
        match result {
            Ok(Ack::Acked | Ack::Deferred) => None,
            Ok(Ack::Superseded) => {
                self.pending = None;
                None
            }
            Ok(Ack::Confirmed) => {
                self.pending = None;
                self.device = Some((p.mv, p.ma));
                for q in QS {
                    if !self.uncommitted(q, cali) {
                        self.touched[idx(q)] = false;
                    }
                }
                Some(Outcome::Confirmed)
            }
            Err(e) => {
                self.pending = None;
                self.restore(cali);
                Some(Outcome::Failed(e.clone()))
            }
        }
    }

    /// 编辑值恢复为设备设定
    pub fn restore(&mut self, cali: &Cali) {
        for q in QS {
            if let Some(d) = self.device_value(q, cali) {
                self.text[idx(q)] = milli3(d);
            }
            self.touched[idx(q)] = false;
        }
    }

    /// 规则 3 / 4：读数回显同步设备设定。返回 true 表示设备设定变了但编辑值被保留（需提示）。
    pub fn on_echo(&mut self, mv: u16, ma: u16, cali: &Cali) -> bool {
        if self.pending.is_some() || self.device == Some((mv, ma)) {
            return false;
        }
        let first = self.device.is_none();
        self.device = Some((mv, ma));
        let mut kept = false;
        for q in QS {
            let i = idx(q);
            let d = self.device_value(q, cali).unwrap_or_default();
            if !self.touched[i] && !self.focused[i] {
                self.text[i] = milli3(d);
            } else if !first && self.uncommitted(q, cali) {
                kept = true;
            }
        }
        if kept {
            self.echo_notice = self.device_value(Qty::Voltage, cali).zip(self.device_value(Qty::Current, cali));
        }
        kept
    }

    pub fn on_disconnected(&mut self) {
        self.device = None;
        self.pending = None;
        self.echo_notice = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lim() -> SafetyLimits {
        SafetyLimits::hardware()
    }

    fn connected(v: u16, i: u16) -> SetpointModel {
        let mut m = SetpointModel::new(5.0, 2.0);
        m.on_echo(v, i, &Cali::default());
        m
    }

    #[test]
    fn first_echo_sets_device_and_untouched_edit() {
        let m = connected(12000, 1000);
        assert_eq!(m.text, ["12.000".to_string(), "1.000".to_string()]);
        assert!(!m.uncommitted(Qty::Voltage, &Cali::default()));
    }

    #[test]
    fn edit_before_connect_kept_as_uncommitted() {
        let c = Cali::default();
        let mut m = SetpointModel::new(5.0, 2.0);
        m.text[0] = "7.5".into();
        m.on_user_edit(Qty::Voltage);
        assert!(!m.on_echo(12000, 1000, &c));
        assert_eq!(m.text[0], "7.5");
        assert_eq!(m.text[1], "1.000");
        assert!(m.uncommitted(Qty::Voltage, &c));
    }

    #[test]
    fn commit_packs_other_from_device() {
        let c = Cali::default();
        let mut m = connected(12000, 1000);
        m.text[1] = "3".into();
        m.on_user_edit(Qty::Current);
        m.text[0] = "9".into();
        m.on_user_edit(Qty::Voltage);
        assert_eq!(m.commit(Qty::Current, 1, &lim(), &c), Commit::Send { mv: 12000, ma: 3000 });
        assert_eq!(m.on_result(1, &Ok(Ack::Confirmed), &c), Some(Outcome::Confirmed));
        assert!(!m.uncommitted(Qty::Current, &c));
        assert!(m.uncommitted(Qty::Voltage, &c));
    }

    #[test]
    fn tc12_out_of_limits_rejected() {
        let c = Cali::default();
        let mut m = connected(12000, 1000);
        m.text[0] = "30".into();
        assert!(matches!(
            m.commit(Qty::Voltage, 1, &lim(), &c),
            Commit::Rejected(SetError::Limit(LimitError::OutOfSafety { .. }))
        ));
        m.text[0] = "abc".into();
        assert!(matches!(
            m.commit(Qty::Voltage, 1, &lim(), &c),
            Commit::Rejected(SetError::Limit(LimitError::NotFinite { .. }))
        ));
    }

    #[test]
    fn unchanged_is_not_sent() {
        let mut m = connected(12000, 1000);
        m.text[0] = "12.0000".into();
        assert_eq!(m.commit(Qty::Voltage, 1, &lim(), &Cali::default()), Commit::NoChange);
    }

    #[test]
    fn failure_restores_device_value() {
        let c = Cali::default();
        let mut m = connected(12000, 1000);
        m.text[0] = "9".into();
        m.on_user_edit(Qty::Voltage);
        m.commit(Qty::Voltage, 4, &lim(), &c);
        assert_eq!(m.on_result(4, &Err(CmdError::Timeout), &c), Some(Outcome::Failed(CmdError::Timeout)));
        assert_eq!(m.text[0], "12.000");
    }

    #[test]
    fn echo_change_follows_or_keeps_draft() {
        let c = Cali::default();
        let mut m = connected(12000, 1000);
        assert!(!m.on_echo(6000, 1000, &c));
        assert_eq!(m.text[0], "6.000");
        m.text[0] = "8".into();
        m.on_user_edit(Qty::Voltage);
        assert!(m.on_echo(7000, 1000, &c));
        assert_eq!(m.text[0], "8");
        assert_eq!(m.echo_notice, Some((7.0, 1.0)));
    }

    #[test]
    fn echo_ignored_while_pending() {
        let c = Cali::default();
        let mut m = connected(12000, 1000);
        m.text[0] = "9".into();
        m.commit(Qty::Voltage, 2, &lim(), &c);
        assert!(!m.on_echo(12000, 1000, &c) && !m.on_echo(9000, 1000, &c));
        assert_eq!(m.device, Some((12000, 1000)));
    }

    #[test]
    fn prepare_on() {
        let c = Cali::default();
        let mut m = connected(12000, 1000);
        assert_eq!(m.prepare_on(1, &lim(), &c), Commit::NoChange);
        m.text = ["5".into(), "0.8".into()];
        assert_eq!(m.prepare_on(2, &lim(), &c), Commit::Send { mv: 5000, ma: 800 });
    }

    #[test]
    fn device_outside_safety_flagged() {
        let m = connected(20000, 1000);
        let l = SafetyLimits { v_min: 3.0, v_max: 12.0, i_min: 0.5, i_max: 5.0 };
        assert!(m.device_out_of_limits(&l, &Cali::default()));
    }
}
