//! 数据线程：IO 线程的所有事件先到这里。对样本做显示修正、置零阈值、能量积分、录制，
//! 再转发给 UI。样本走有界通道（UI 卡死太久时丢最旧的显示样本并计数），
//! 其它事件走无界通道，不丢。最新样本与累计能量发布到 [`Snapshot`]，供 AUX 与 HTTP 读。

pub mod recorder;

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, TrySendError, bounded, select, unbounded};
use dc3005u_usb::{Event, LinkOrigin, LinkState, Sample};

use crate::cali::Cali;
use recorder::Recorder;
pub use recorder::{LogCtx, LogHeader, RecordCfg, kv};

const UI_SAMPLE_CAP: usize = 4096;
/// HTTP `/api/samples` 最多返回最近这么多点
pub const RECENT_CAP: usize = 10_000;
const TICK: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, Copy)]
pub struct DisplayCfg {
    pub cali: Cali,
    pub v_threshold: f64,
    pub i_threshold: f64,
}

pub enum DataCmd {
    Zero,
    Display(DisplayCfg),
    StartRecord(Box<RecordCfg>),
    StopRecord,
    /// txt 事件行：类型 + 已拼好的 `key=value` 字段
    LogEvent {
        kind: &'static str,
        fields: String,
    },
    LogCtx(LogCtx),
    /// 把 CmdResult 事件转发一份给 AUX / HTTP 线程
    Tap(Sender<Event>),
    /// panic hook：立即写盘
    Flush(Sender<()>),
}

/// 发给 UI 的录制状态
#[derive(Debug, Clone)]
pub enum DataNotice {
    RecordStarted(PathBuf),
    Saved(Vec<PathBuf>),
    RecordFailed(String),
}

/// 修正与置零之后的样本
#[derive(Debug, Clone, Copy)]
pub struct Processed {
    pub sample: Sample,
    pub v: f64,
    pub i_a: f64,
    /// 显示能量（回零清零）
    pub energy_j: f64,
    pub avg_power_w: Option<f64>,
    /// 连接以来的累计能量，不受回零影响（电池模拟用）
    pub cum_energy_j: f64,
}

/// 数据线程发布的最新状态
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub link: LinkState,
    pub latest: Option<Processed>,
    pub recent: VecDeque<Processed>,
    pub recording: Option<(PathBuf, Instant)>,
}

impl Default for Snapshot {
    fn default() -> Self {
        Self { link: LinkState::Disconnected, latest: None, recent: VecDeque::new(), recording: None }
    }
}

pub type SharedSnapshot = Arc<RwLock<Snapshot>>;

/// E += P_i × (t_i − t_{i−1})；跨过断线时段的那一段不累加、不计入平均功率的分母。
#[derive(Debug, Default)]
pub struct Energy {
    last_t: Option<Instant>,
    energy_j: f64,
    active_s: f64,
}

impl Energy {
    pub fn push(&mut self, t: Instant, p_w: f64) {
        if let Some(lt) = self.last_t {
            let dt = t.saturating_duration_since(lt).as_secs_f64();
            self.energy_j += p_w * dt;
            self.active_s += dt;
        }
        self.last_t = Some(t);
    }

    pub fn link_lost(&mut self) {
        self.last_t = None;
    }

    pub fn zero(&mut self) {
        self.energy_j = 0.0;
        self.active_s = 0.0;
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn energy_j(&self) -> f64 {
        self.energy_j
    }

    pub fn avg_power_w(&self) -> Option<f64> {
        (self.active_s > 0.0).then(|| self.energy_j / self.active_s)
    }
}

pub fn process(cfg: &DisplayCfg, s: &Sample) -> (f64, f64, f64) {
    let mut v = cfg.cali.read_v(s.meas.voltage_v);
    let mut i = cfg.cali.read_i(s.meas.current_a());
    if v.abs() < cfg.v_threshold {
        v = 0.0;
    }
    if i.abs() < cfg.i_threshold {
        i = 0.0;
    }
    (v, i, v * i)
}

pub struct DataHandle {
    pub cmd: Sender<DataCmd>,
    pub events: Receiver<Event>,
    pub samples: Receiver<Processed>,
    pub notices: Receiver<DataNotice>,
    pub snapshot: SharedSnapshot,
    pub dropped: Arc<AtomicU64>,
    join: Option<JoinHandle<()>>,
}

impl DataHandle {
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    pub fn send(&self, c: DataCmd) {
        let _ = self.cmd.send(c);
    }

    pub fn log(&self, kind: &'static str, fields: &[(&str, String)]) {
        self.send(DataCmd::LogEvent { kind, fields: kv(fields) });
    }
}

impl Drop for DataHandle {
    fn drop(&mut self) {
        if let Some(j) = self.join.take() {
            // IO 线程退出后事件通道关闭，数据线程随之结束（录制在 Recorder::drop 里写盘）
            let _ = j.join();
        }
    }
}

struct DataThread {
    cfg: DisplayCfg,
    energy: Energy,
    cum: Energy,
    recorder: Option<Recorder>,
    ctx: LogCtx,
    taps: Vec<Sender<Event>>,
    attempt: u32,
    snapshot: SharedSnapshot,
    notices: Sender<DataNotice>,
}

impl DataThread {
    fn notice(&self, n: DataNotice) {
        let _ = self.notices.send(n);
    }

    fn stop_record(&mut self) {
        if let Some(mut r) = self.recorder.take() {
            r.event(Instant::now(), "NOTE", "record=stop");
            let (files, res) = r.finish();
            if let Err(e) = res {
                self.notice(DataNotice::RecordFailed(e.to_string()));
            }
            self.notice(DataNotice::Saved(files));
        }
        self.snapshot.write().expect("snapshot").recording = None;
    }

    fn record_io<T>(&mut self, r: std::io::Result<T>) {
        if let Err(e) = r {
            log::error!("record write failed: {e}");
            self.recorder = None;
            self.snapshot.write().expect("snapshot").recording = None;
            self.notice(DataNotice::RecordFailed(e.to_string()));
        }
    }

    fn on_sample(&mut self, s: Sample) -> Processed {
        let (v, i_a, p_w) = process(&self.cfg, &s);
        self.energy.push(s.t, p_w);
        self.cum.push(s.t, p_w);
        let p = Processed {
            sample: s,
            v,
            i_a,
            energy_j: self.energy.energy_j(),
            avg_power_w: self.energy.avg_power_w(),
            cum_energy_j: self.cum.energy_j(),
        };
        if let Some(r) = &mut self.recorder {
            let res = r.sample(&p, &self.ctx);
            self.record_io(res);
        }
        let mut snap = self.snapshot.write().expect("snapshot");
        snap.latest = Some(p);
        snap.recent.push_back(p);
        while snap.recent.len() > RECENT_CAP {
            snap.recent.pop_front();
        }
        p
    }

    fn on_link(&mut self, s: LinkState) {
        let now = Instant::now();
        let restored = format!("state=restored\tattempt={}\tout=unconfirmed", self.attempt.max(1));
        let ev = match s {
            LinkState::Connected { origin: LinkOrigin::FirstConnect } => {
                self.energy.reset();
                self.cum.reset();
                Some("state=connect")
            }
            LinkState::Connected { origin: LinkOrigin::Reconnect } => Some(restored.as_str()),
            LinkState::Reconnecting { attempt } => {
                self.attempt = attempt;
                self.energy.link_lost();
                self.cum.link_lost();
                (attempt == 0).then_some("state=lost")
            }
            LinkState::Disconnected => {
                self.energy.link_lost();
                self.cum.link_lost();
                Some("state=disconnect")
            }
            LinkState::Connecting { .. } => None,
        };
        if let (Some(r), Some(f)) = (&mut self.recorder, ev) {
            r.event(now, "LINK", f);
        }
        {
            let mut snap = self.snapshot.write().expect("snapshot");
            snap.link = s;
            if matches!(s, LinkState::Disconnected | LinkState::Connected { origin: LinkOrigin::FirstConnect }) {
                snap.latest = None;
                snap.recent.clear();
            }
        }
        if s == LinkState::Disconnected {
            // 用户断开：录制停止并写盘
            self.stop_record();
        }
    }

    fn on_cmd(&mut self, c: DataCmd) {
        match c {
            DataCmd::Zero => self.energy.zero(),
            DataCmd::Display(c) => self.cfg = c,
            DataCmd::StartRecord(cfg) => {
                self.stop_record();
                match Recorder::start(*cfg, Instant::now(), chrono::Local::now()) {
                    Ok(r) => {
                        let path = r.current_txt().to_path_buf();
                        self.snapshot.write().expect("snapshot").recording = Some((path.clone(), Instant::now()));
                        self.recorder = Some(r);
                        self.notice(DataNotice::RecordStarted(path));
                    }
                    Err(e) => self.notice(DataNotice::RecordFailed(e.to_string())),
                }
            }
            DataCmd::StopRecord => self.stop_record(),
            DataCmd::LogEvent { kind, fields } => {
                if let Some(r) = &mut self.recorder {
                    r.event(Instant::now(), kind, &fields);
                }
            }
            DataCmd::LogCtx(c) => self.ctx = c,
            DataCmd::Tap(tx) => self.taps.push(tx),
            DataCmd::Flush(done) => {
                if let Some(r) = &mut self.recorder {
                    let res = r.flush_all();
                    self.record_io(res);
                }
                let _ = done.send(());
            }
        }
    }

    fn tick(&mut self) {
        if let Some(r) = &mut self.recorder {
            let res = r.tick(Instant::now());
            self.record_io(res);
        }
    }
}

pub fn spawn(io_events: Receiver<Event>, cfg: DisplayCfg, repaint: Box<dyn Fn() + Send>) -> DataHandle {
    let (cmd_tx, cmd_rx) = unbounded();
    let (ev_tx, ev_rx) = unbounded();
    let (n_tx, n_rx) = unbounded();
    let (s_tx, s_rx) = bounded(UI_SAMPLE_CAP);
    let dropped = Arc::new(AtomicU64::new(0));
    let snapshot: SharedSnapshot = Arc::default();
    let dropped2 = dropped.clone();
    let s_rx2 = s_rx.clone();
    let snap2 = snapshot.clone();
    let join = std::thread::Builder::new()
        .name("data".into())
        .spawn(move || {
            let mut d = DataThread {
                cfg,
                energy: Energy::default(),
                cum: Energy::default(),
                recorder: None,
                ctx: LogCtx::default(),
                taps: Vec::new(),
                attempt: 0,
                snapshot: snap2,
                notices: n_tx,
            };
            let mut cmd_rx = cmd_rx;
            loop {
                select! {
                    recv(io_events) -> ev => {
                        let Ok(ev) = ev else { break };
                        match ev {
                            Event::Sample(s) => {
                                let p = d.on_sample(s);
                                if let Err(TrySendError::Full(p)) = s_tx.try_send(p) {
                                    let _ = s_rx2.try_recv();
                                    dropped2.fetch_add(1, Ordering::Relaxed);
                                    let _ = s_tx.try_send(p);
                                }
                            }
                            other => {
                                match &other {
                                    Event::Link(s) => d.on_link(*s),
                                    Event::CmdResult { .. } => d.taps.retain(|t| t.send(other.clone()).is_ok()),
                                    _ => {}
                                }
                                let _ = ev_tx.send(other);
                                repaint();
                            }
                        }
                    }
                    recv(cmd_rx) -> c => match c {
                        Ok(c) => {
                            let notify = matches!(c, DataCmd::StartRecord(_) | DataCmd::StopRecord);
                            d.on_cmd(c);
                            if notify {
                                repaint();
                            }
                        }
                        Err(_) => cmd_rx = crossbeam_channel::never(),
                    },
                    default(TICK) => {}
                }
                d.tick();
            }
            d.stop_record();
        })
        .expect("spawn data thread");
    DataHandle { cmd: cmd_tx, events: ev_rx, samples: s_rx, notices: n_rx, snapshot, dropped, join: Some(join) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn tc84_energy_skips_link_gap() {
        let t0 = Instant::now();
        let mut e = Energy::default();
        e.push(t0, 2.0);
        e.push(t0 + Duration::from_secs(1), 2.0);
        e.link_lost();
        e.push(t0 + Duration::from_secs(11), 2.0);
        e.push(t0 + Duration::from_secs(12), 2.0);
        assert!((e.energy_j() - 4.0).abs() < 1e-9);
        assert!((e.avg_power_w().unwrap() - 2.0).abs() < 1e-9);
        e.zero();
        assert_eq!(e.energy_j(), 0.0);
        assert!(e.avg_power_w().is_none());
    }

    #[test]
    fn microwatt_energy_keeps_precision() {
        let t0 = Instant::now();
        let mut e = Energy::default();
        for k in 0..=72_000u64 {
            e.push(t0 + Duration::from_millis(50 * k), 5e-6);
        }
        assert!((e.energy_j() - 5e-6 * 3600.0).abs() < 1e-12);
    }
}
