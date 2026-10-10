//! AUX 线程（`Doc/详细设计.md` §2.4、§4）：序列、功率闭环、直流扫描、函数发生器、电池模拟，
//! 同一时刻最多运行一个。按「下一个到期时刻」阻塞等待，不依赖 UI 刷新；
//! 设定直接发给 IO 线程（来源 `Seq` / `Aux`），结果由数据线程转发回来。

pub mod battery;
pub mod sequence;

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, select, unbounded};
use dc3005u_proto::{Qty, SafetyLimits, clamp_to_limits, cmd};
use dc3005u_usb::{Ack, Cmd, CmdError, CmdId, Event, LinkState, Source};

use crate::buffer::Series;
use crate::cali::Cali;
use crate::config::{KeepPowerCfg, WaveCfg};
use crate::data::{DataCmd, Processed, SharedSnapshot, kv};
use battery::Curve;
use sequence::Step;

pub type IdGen = Arc<AtomicU64>;

pub fn next_id(g: &IdGen) -> CmdId {
    g.fetch_add(1, Ordering::Relaxed)
}

const STABLE_TOL_V: f64 = 0.1;
const STABLE_TIMEOUT: Duration = Duration::from_secs(10);
const STABLE_POLL: Duration = Duration::from_millis(50);
const MAX_FAILS: u32 = 3;
const IDLE_WAIT: Duration = Duration::from_secs(3600);
const WAIT_STEP_MAX: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    KeepPower,
    Sweep,
    Wave,
    Seq,
    Battery,
}

impl Kind {
    pub fn tag(self) -> &'static str {
        match self {
            Kind::KeepPower => "keep_power",
            Kind::Sweep => "sweep",
            Kind::Wave => "wavegen",
            Kind::Seq => "seq",
            Kind::Battery => "batsim",
        }
    }

    fn source(self) -> Source {
        if self == Kind::Seq { Source::Seq } else { Source::Aux }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaveKind {
    Square,
    Sine,
    Triangle,
    Sawtooth,
    Noise,
}

impl WaveKind {
    pub const ALL: [WaveKind; 5] =
        [WaveKind::Square, WaveKind::Sine, WaveKind::Triangle, WaveKind::Sawtooth, WaveKind::Noise];

    pub fn parse(s: &str) -> Self {
        Self::ALL.into_iter().find(|k| k.as_str() == s).unwrap_or(WaveKind::Square)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            WaveKind::Square => "square",
            WaveKind::Sine => "sine",
            WaveKind::Triangle => "triangle",
            WaveKind::Sawtooth => "sawtooth",
            WaveKind::Noise => "noise",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            WaveKind::Square => "wave_square",
            WaveKind::Sine => "wave_sine",
            WaveKind::Triangle => "wave_triangle",
            WaveKind::Sawtooth => "wave_sawtooth",
            WaveKind::Noise => "wave_noise",
        }
    }
}

/// 原版公式（三角波一个完整周期是 2T，保留）
pub fn wave_value(kind: WaveKind, t: f64, period: f64, high: f64, low: f64) -> f64 {
    let x = t / period;
    let v = match kind {
        WaveKind::Sine => low + (high - low) * ((2.0 * std::f64::consts::PI * x).sin() + 1.0) / 2.0,
        WaveKind::Square => {
            if (2.0 * std::f64::consts::PI * x).sin() > 0.0 {
                high
            } else {
                low
            }
        }
        WaveKind::Triangle => {
            let m = x.rem_euclid(2.0);
            let m = if m < 1.0 { m } else { 2.0 - m };
            low + (high - low) * m
        }
        WaveKind::Sawtooth => low + (high - low) * x.rem_euclid(1.0),
        WaveKind::Noise => low + (high - low) * fastrand::f64(),
    };
    v.clamp(low.min(high), high.max(low))
}

#[derive(Debug, Clone)]
pub struct SweepJob {
    pub target: Qty,
    pub start: f64,
    pub stop: f64,
    pub step: f64,
    pub delay_s: f64,
    pub response: Series,
}

impl SweepJob {
    /// 起点到终点（含），越过终点截断为终点
    pub fn values(&self) -> Vec<f64> {
        let dir = if self.stop >= self.start { 1.0 } else { -1.0 };
        let n = ((self.stop - self.start).abs() / self.step - 1e-9).ceil().max(0.0) as usize;
        let mut v: Vec<f64> = (0..n).map(|k| self.start + dir * self.step * k as f64).collect();
        v.push(self.stop);
        v.iter().map(|x| (x * 1000.0).round() / 1000.0).collect()
    }
}

#[derive(Debug, Clone)]
pub struct SeqJob {
    pub steps: Vec<Step>,
    pub looped: bool,
    pub skip_oob: bool,
}

#[derive(Debug, Clone)]
pub struct BatJob {
    pub curve: Curve,
    pub soc_now: f64,
    pub soc_stop: f64,
    pub cap_wh: f64,
    pub r_ohm: f64,
    pub cells: u32,
    pub freq_hz: f64,
}

#[derive(Debug, Clone)]
pub enum Job {
    KeepPower(KeepPowerCfg),
    Sweep(SweepJob),
    Wave(WaveCfg),
    Seq(SeqJob),
    Battery(BatJob),
}

impl Job {
    pub fn kind(&self) -> Kind {
        match self {
            Job::KeepPower(_) => Kind::KeepPower,
            Job::Sweep(_) => Kind::Sweep,
            Job::Wave(_) => Kind::Wave,
            Job::Seq(_) => Kind::Seq,
            Job::Battery(_) => Kind::Battery,
        }
    }
}

#[derive(Debug, Clone)]
pub struct StartReq {
    pub job: Job,
    pub safety: SafetyLimits,
    pub cali: Cali,
    /// 启动时输出可能为开
    pub output_on: bool,
    pub set_interval: Duration,
}

/// 运行中在线修改（UI 已按安全边界校验）
#[derive(Debug, Clone)]
pub enum Update {
    KeepPower { target_w: f64, ki: f64, max_v: f64 },
    Wave { kind: WaveKind, period_s: f64, high_v: f64, low_v: f64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    User,
    Finished,
    EStop,
    LinkLost,
    SendFailed,
    /// 序列执行到超出安全边界的行（1 起）
    SeqRejected(usize),
    BatteryCutoff,
    Shutdown,
}

impl StopReason {
    fn tag(self) -> &'static str {
        match self {
            StopReason::User => "user",
            StopReason::Finished => "finished",
            StopReason::EStop => "estop",
            StopReason::LinkLost => "link_lost",
            StopReason::SendFailed => "send_failed",
            StopReason::SeqRejected(_) => "seq_out_of_limits",
            StopReason::BatteryCutoff => "battery_cutoff",
            StopReason::Shutdown => "exit",
        }
    }
}

pub enum AuxCmd {
    Start(Box<StartReq>),
    Stop(StopReason),
    Update(Update),
    Shutdown,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AuxNotice {
    Stopped(Kind, StopReason),
    ClampedFirst(Kind),
    SeqSkipped(usize),
    Stalled,
}

#[derive(Debug, Clone)]
pub struct SweepResult {
    pub target: Qty,
    pub response: Series,
    pub points: Vec<[f64; 2]>,
}

#[derive(Debug, Clone, Default)]
pub struct AuxStatus {
    pub running: Option<Kind>,
    pub waiting_stable: bool,
    pub send_failed: u32,
    pub clamped: u32,
    pub eff_hz: f64,
    pub seq_index: Option<usize>,
    pub soc: Option<f64>,
    pub discharge: Duration,
    /// 电池模拟当前点 (SOC, OCV)
    pub bat_point: Option<[f64; 2]>,
    pub paused: bool,
    pub sweep: Option<SweepResult>,
}

pub struct AuxHandle {
    pub cmd: Sender<AuxCmd>,
    pub notices: Receiver<AuxNotice>,
    pub status: Arc<Mutex<AuxStatus>>,
    join: Option<JoinHandle<()>>,
}

impl AuxHandle {
    pub fn send(&self, c: AuxCmd) {
        let _ = self.cmd.send(c);
    }

    pub fn status(&self) -> AuxStatus {
        self.status.lock().expect("aux status").clone()
    }
}

impl Drop for AuxHandle {
    fn drop(&mut self) {
        let _ = self.cmd.send(AuxCmd::Shutdown);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

struct Ctx {
    dev: Sender<Cmd>,
    ids: IdGen,
    snap: SharedSnapshot,
    data: Sender<DataCmd>,
    status: Arc<Mutex<AuxStatus>>,
    notices: Sender<AuxNotice>,
    repaint: Box<dyn Fn() + Send>,
}

impl Ctx {
    fn latest(&self) -> Option<Processed> {
        self.snap.read().expect("snapshot").latest
    }

    fn link(&self) -> LinkState {
        self.snap.read().expect("snapshot").link
    }

    fn status(&self, f: impl FnOnce(&mut AuxStatus)) {
        f(&mut self.status.lock().expect("aux status"));
        (self.repaint)();
    }

    fn notice(&self, n: AuxNotice) {
        let _ = self.notices.send(n);
        (self.repaint)();
    }

    fn log(&self, kind: &'static str, fields: &[(&str, String)]) {
        let _ = self.data.send(DataCmd::LogEvent { kind, fields: kv(fields) });
    }
}

struct Stable {
    v: f64,
    since: Instant,
}

enum JobState {
    KeepPower { cfg: KeepPowerCfg, u: f64, last: Option<Instant>, last_n: Option<u64>, stale: u32 },
    Sweep { job: SweepJob, values: Vec<f64>, k: usize, deadline: Option<Instant>, points: Vec<[f64; 2]> },
    Wave { cfg: WaveCfg, t0: Option<Instant> },
    Seq { job: SeqJob, idx: usize, entered: Option<Instant> },
    Battery { job: BatJob, total: f64, used0: f64, cutoff: f64, e0: Option<f64>, run_start: Option<Instant> },
}

struct Run {
    kind: Kind,
    state: JobState,
    safety: SafetyLimits,
    cali: Cali,
    out_on: bool,
    pending: HashSet<CmdId>,
    then_on: Option<CmdId>,
    fails: u32,
    clamped: u32,
    stable: Option<Stable>,
    last_sent: Option<(f64, f64)>,
    period: Duration,
    next_tick: Instant,
}

enum Flow {
    Go,
    Stop(StopReason),
}

impl Run {
    fn start(req: StartReq, ctx: &Ctx) -> Self {
        let now = Instant::now();
        let kind = req.job.kind();
        let max_hz = 1.0 / req.set_interval.as_secs_f64().max(1e-3);
        let (state, hz) = match req.job {
            Job::KeepPower(cfg) => {
                let hz = cfg.freq_hz;
                (JobState::KeepPower { cfg, u: 0.0, last: None, last_n: None, stale: 0 }, hz)
            }
            Job::Sweep(job) => {
                let values = job.values();
                let hz = 1.0 / job.delay_s;
                (JobState::Sweep { job, values, k: 0, deadline: None, points: Vec::new() }, hz)
            }
            Job::Wave(cfg) => {
                let hz = cfg.freq_hz;
                (JobState::Wave { cfg, t0: None }, hz)
            }
            Job::Seq(job) => (JobState::Seq { job, idx: 0, entered: None }, max_hz),
            Job::Battery(job) => {
                let s = f64::from(job.cells);
                let total = job.cap_wh * 3600.0 * s;
                let used0 = (1.0 - job.soc_now / 100.0) * total;
                let cutoff = total - job.soc_stop / 100.0 * total;
                let hz = job.freq_hz;
                (JobState::Battery { job, total, used0, cutoff, e0: None, run_start: None }, hz)
            }
        };
        let mut r = Run {
            kind,
            state,
            safety: req.safety,
            cali: req.cali,
            out_on: req.output_on,
            pending: HashSet::new(),
            then_on: None,
            fails: 0,
            clamped: 0,
            stable: None,
            last_sent: None,
            period: Duration::from_secs_f64(1.0 / hz.max(1e-6)),
            next_tick: now,
        };
        ctx.status(|s| {
            *s = AuxStatus {
                running: Some(kind),
                eff_hz: hz.min(max_hz),
                sweep: if kind == Kind::Sweep { None } else { s.sweep.take() },
                ..AuxStatus::default()
            };
        });
        ctx.log("AUX", &[("fn", kind.tag().into()), ("action", "start".into())]);
        r.begin(ctx, now);
        r
    }

    fn device_set(&self, ctx: &Ctx) -> Option<(f64, f64)> {
        let p = ctx.latest()?;
        let m = p.sample.meas;
        Some((self.cali.decode(Qty::Voltage, m.set_voltage_mv), self.cali.decode(Qty::Current, m.set_current_ma)))
    }

    fn cur_set(&self, ctx: &Ctx) -> Option<(f64, f64)> {
        self.last_sent.or_else(|| self.device_set(ctx))
    }

    fn send_set(&mut self, ctx: &Ctx, v: f64, i: f64) -> Option<CmdId> {
        let (mv, ma) = match (self.cali.encode(Qty::Voltage, v), self.cali.encode(Qty::Current, i)) {
            (Ok(mv), Ok(ma)) => (mv, ma),
            _ => {
                self.fail(ctx);
                return None;
            }
        };
        let id = next_id(&ctx.ids);
        self.pending.insert(id);
        self.last_sent = Some((v, i));
        let _ = ctx.dev.send(Cmd::SetVI { id, volt_mv: mv, amp_ma: ma, source: self.kind.source() });
        Some(id)
    }

    fn send_out(&mut self, ctx: &Ctx, on: bool) {
        let id = next_id(&ctx.ids);
        self.pending.insert(id);
        self.out_on = on;
        let _ = ctx.dev.send(Cmd::Output { id, on, source: self.kind.source() });
    }

    /// 下发设定；输出关着时等回显确认后再开输出
    fn set_and_on(&mut self, ctx: &Ctx, v: f64, i: f64) {
        let id = self.send_set(ctx, v, i);
        if !self.out_on {
            self.then_on = id;
        }
    }

    fn fail(&mut self, ctx: &Ctx) {
        self.fails += 1;
        ctx.status(|s| s.send_failed += 1);
    }

    /// 运行中瞬时值钳位：每次运行只告警一次
    fn clamp(&mut self, ctx: &Ctx, v: f64, i: f64) -> (f64, f64) {
        let (cv, ci, hit) = clamp_to_limits(v, i, &self.safety);
        if hit {
            self.clamped += 1;
            let n = self.clamped;
            ctx.status(|s| s.clamped = n);
            if n == 1 {
                log::warn!("{} clamped {v:.3} V / {i:.3} A to safety limits", self.kind.tag());
                ctx.log(
                    "CLAMP",
                    &[("fn", self.kind.tag().into()), ("from", format!("{v:.3}")), ("to", format!("{cv:.3}"))],
                );
                ctx.notice(AuxNotice::ClampedFirst(self.kind));
            }
        }
        (cv, ci)
    }

    fn wait_stable(&mut self, ctx: &Ctx, v: f64, now: Instant) {
        self.stable = Some(Stable { v, since: now });
        ctx.status(|s| s.waiting_stable = true);
    }

    fn begin(&mut self, ctx: &Ctx, now: Instant) {
        let dev = self.cur_set(ctx);
        let (v_dev, i_dev) = dev.unwrap_or((self.safety.v_min, self.safety.i_min));
        match &mut self.state {
            JobState::KeepPower { u, .. } => {
                *u = v_dev;
                if !self.out_on {
                    self.send_out(ctx, true);
                }
            }
            JobState::Sweep { job, values, .. } => {
                let first = values[0];
                let (v, i) = if job.target == Qty::Voltage { (first, i_dev) } else { (v_dev, first) };
                self.set_and_on(ctx, v, i);
                self.wait_stable(ctx, v, now);
            }
            JobState::Wave { cfg, .. } => {
                let low = cfg.low_v;
                self.set_and_on(ctx, low, i_dev);
                self.wait_stable(ctx, low, now);
            }
            JobState::Seq { .. } => {
                if !self.out_on {
                    self.send_out(ctx, true);
                }
            }
            JobState::Battery { job, .. } => {
                let v0 = job.curve.ocv(job.soc_now) * f64::from(job.cells);
                self.set_and_on(ctx, v0, i_dev);
                self.wait_stable(ctx, v0, now);
            }
        }
        self.next_tick = now;
    }

    fn update(&mut self, u: Update) {
        match (&mut self.state, u) {
            (JobState::KeepPower { cfg, .. }, Update::KeepPower { target_w, ki, max_v }) => {
                cfg.target_w = target_w;
                cfg.ki = ki;
                cfg.max_v = max_v;
            }
            (JobState::Wave { cfg, .. }, Update::Wave { kind, period_s, high_v, low_v }) => {
                cfg.kind = kind.as_str().into();
                cfg.period_s = period_s;
                cfg.high_v = high_v;
                cfg.low_v = low_v;
            }
            _ => {}
        }
    }

    fn on_result(&mut self, ctx: &Ctx, id: CmdId, c: u8, result: &Result<Ack, CmdError>) -> Flow {
        if !self.pending.contains(&id) {
            return Flow::Go;
        }
        let is_set = c == cmd::SET_VI;
        let done = match result {
            Ok(Ack::Acked) => !is_set,
            _ => true,
        };
        if done {
            self.pending.remove(&id);
        }
        match result {
            Err(CmdError::NotConnected) => return Flow::Stop(StopReason::LinkLost),
            Err(_) => {
                if self.then_on == Some(id) {
                    self.then_on = None;
                }
                self.fail(ctx);
                if self.fails >= MAX_FAILS {
                    return Flow::Stop(StopReason::SendFailed);
                }
            }
            Ok(Ack::Confirmed) => {
                self.fails = 0;
                if self.then_on == Some(id) {
                    self.then_on = None;
                    self.send_out(ctx, true);
                }
            }
            Ok(Ack::Acked) if !is_set => self.fails = 0,
            Ok(Ack::Superseded) if self.then_on == Some(id) => {
                // 被本功能更新的设定取代：开输出跟着最新一条走
                self.then_on = self.pending.iter().copied().max();
            }
            _ => {}
        }
        Flow::Go
    }

    fn tick(&mut self, ctx: &Ctx, now: Instant) -> Flow {
        if !matches!(ctx.link(), LinkState::Connected { .. }) {
            return Flow::Stop(StopReason::LinkLost);
        }
        if let Some(st) = &self.stable {
            let ok = ctx.latest().is_some_and(|p| (p.v - st.v).abs() < STABLE_TOL_V && self.out_on);
            if ok || now.saturating_duration_since(st.since) >= STABLE_TIMEOUT {
                self.stable = None;
                ctx.status(|s| s.waiting_stable = false);
                self.next_tick = now;
            } else {
                self.next_tick = now + STABLE_POLL;
                return Flow::Go;
            }
        }
        let flow = match self.kind {
            Kind::KeepPower => self.tick_keep_power(ctx, now),
            Kind::Sweep => self.tick_sweep(ctx, now),
            Kind::Wave => self.tick_wave(ctx, now),
            Kind::Seq => self.tick_seq(ctx, now),
            Kind::Battery => self.tick_battery(ctx, now),
        };
        if self.next_tick <= now {
            // 错过的节拍不补
            self.next_tick = now + self.period;
        }
        flow
    }

    fn tick_keep_power(&mut self, ctx: &Ctx, now: Instant) -> Flow {
        let i_dev = self.cur_set(ctx).map_or(self.safety.i_min, |s| s.1);
        let period = self.period;
        let v_min = self.safety.v_min;
        let JobState::KeepPower { cfg, u, last, last_n, stale } = &mut self.state else { return Flow::Go };
        self.next_tick = now + period;
        let Some(p) = ctx.latest() else { return Flow::Go };
        if *last_n == Some(p.sample.n) {
            *stale += 1;
            if *stale == 4 {
                ctx.status(|s| s.paused = true);
                ctx.notice(AuxNotice::Stalled);
            }
            *last = Some(now);
            return Flow::Go;
        }
        if *stale > 3 {
            ctx.status(|s| s.paused = false);
        }
        *stale = 0;
        *last_n = Some(p.sample.n);
        let pm = p.v * p.i_a;
        if !pm.is_finite() {
            return Flow::Go;
        }
        let dt = last.map_or(period.as_secs_f64(), |l| now.saturating_duration_since(l).as_secs_f64());
        *last = Some(now);
        *u = (*u + cfg.ki * (cfg.target_w - pm) * dt).clamp(v_min, cfg.max_v.max(v_min));
        let target = *u;
        self.send_set(ctx, target, i_dev);
        Flow::Go
    }

    fn tick_sweep(&mut self, ctx: &Ctx, now: Instant) -> Flow {
        let cur = self.cur_set(ctx);
        let JobState::Sweep { job, values, k, deadline, points } = &mut self.state else { return Flow::Go };
        let delay = Duration::from_secs_f64(job.delay_s);
        let Some(d) = *deadline else {
            *deadline = Some(now + delay);
            self.next_tick = now + delay;
            return Flow::Go;
        };
        if now < d {
            self.next_tick = d;
            return Flow::Go;
        }
        if job.response != Series::None
            && let Some(p) = ctx.latest()
        {
            let x = if job.target == Qty::Voltage { p.v } else { p.i_a };
            let pt = crate::buffer::Point { t: 0.0, v: p.v, i: p.i_a, gap: false };
            points.push([x, job.response.value(&pt)]);
            let res = SweepResult { target: job.target, response: job.response, points: points.clone() };
            ctx.status(|s| s.sweep = Some(res));
        }
        *k += 1;
        if *k >= values.len() {
            return Flow::Stop(StopReason::Finished);
        }
        let x = values[*k];
        let (v_dev, i_dev) = cur.unwrap_or((self.safety.v_min, self.safety.i_min));
        let (v, i) = if job.target == Qty::Voltage { (x, i_dev) } else { (v_dev, x) };
        *deadline = Some(now + delay);
        self.next_tick = now + delay;
        self.send_set(ctx, v, i);
        Flow::Go
    }

    fn tick_wave(&mut self, ctx: &Ctx, now: Instant) -> Flow {
        let i_dev = self.cur_set(ctx).map_or(self.safety.i_min, |s| s.1);
        let JobState::Wave { cfg, t0 } = &mut self.state else { return Flow::Go };
        let t0 = *t0.get_or_insert(now);
        let t = now.saturating_duration_since(t0).as_secs_f64();
        let v = wave_value(WaveKind::parse(&cfg.kind), t, cfg.period_s, cfg.high_v, cfg.low_v);
        let (v, i) = self.clamp(ctx, v, i_dev);
        self.send_set(ctx, v, i);
        Flow::Go
    }

    fn tick_battery(&mut self, ctx: &Ctx, now: Instant) -> Flow {
        let i_dev = self.cur_set(ctx).map_or(self.safety.i_min, |s| s.1);
        let Some(p) = ctx.latest() else { return Flow::Go };
        let JobState::Battery { job, total, used0, cutoff, e0, run_start } = &mut self.state else { return Flow::Go };
        let e0 = *e0.get_or_insert(p.cum_energy_j);
        let start = *run_start.get_or_insert(now);
        let used = *used0 + (p.cum_energy_j - e0);
        let soc = ((*total - used) / *total * 100.0).clamp(0.0, 100.0);
        let ocv = job.curve.ocv(soc);
        let s = f64::from(job.cells);
        let v = (ocv - job.r_ohm * p.i_a) * s;
        let elapsed = now.saturating_duration_since(start);
        ctx.status(|st| {
            st.soc = Some(soc);
            st.discharge = elapsed;
            st.bat_point = Some([soc, ocv]);
        });
        if used >= *cutoff {
            self.send_out(ctx, false);
            return Flow::Stop(StopReason::BatteryCutoff);
        }
        let (v, i) = self.clamp(ctx, v, i_dev);
        self.send_set(ctx, v, i);
        Flow::Go
    }

    fn tick_seq(&mut self, ctx: &Ctx, now: Instant) -> Flow {
        let safety = self.safety;
        loop {
            let JobState::Seq { job, idx, entered } = &mut self.state else { return Flow::Go };
            if *idx >= job.steps.len() {
                if job.looped && !job.steps.is_empty() {
                    *idx = 0;
                    *entered = None;
                } else {
                    return Flow::Stop(StopReason::Finished);
                }
            }
            let i0 = *idx;
            ctx.status(|s| s.seq_index = Some(i0));
            match job.steps[i0].clone() {
                Step::Delay(ms) => {
                    let start = *entered.get_or_insert(now);
                    let due = start + Duration::from_millis(u64::from(ms));
                    if now < due {
                        self.next_tick = due;
                        return Flow::Go;
                    }
                    *idx += 1;
                    *entered = None;
                }
                Step::Wait(at) => {
                    let wall = chrono::Local::now().naive_local();
                    if wall < at {
                        let left = (at - wall).to_std().unwrap_or_default().min(WAIT_STEP_MAX);
                        self.next_tick = now + left.max(Duration::from_millis(1));
                        return Flow::Go;
                    }
                    *idx += 1;
                    *entered = None;
                }
                Step::SetV(_) | Step::SetI(_) => {
                    // 连续的 SET-V / SET-I 合并成一次 0x03
                    let mut v = None;
                    let mut i = None;
                    let mut k = i0;
                    let skip = job.skip_oob;
                    while let Some(st @ (Step::SetV(_) | Step::SetI(_))) = job.steps.get(k) {
                        if st.outside(&safety) {
                            if !skip {
                                return Flow::Stop(StopReason::SeqRejected(k + 1));
                            }
                            log::warn!("sequence line {} outside safety limits, skipped", k + 1);
                            ctx.notice(AuxNotice::SeqSkipped(k + 1));
                        } else {
                            match *st {
                                Step::SetV(x) => v = Some(x),
                                Step::SetI(x) => i = Some(x),
                                _ => {}
                            }
                        }
                        k += 1;
                    }
                    *idx = k;
                    *entered = None;
                    let (v_dev, i_dev) = self.cur_set(ctx).unwrap_or((safety.v_min, safety.i_min));
                    match v {
                        Some(0.0) => {
                            if let Some(i) = i {
                                self.send_set(ctx, v_dev, i);
                            }
                            self.send_out(ctx, false);
                        }
                        Some(x) => self.set_and_on(ctx, x, i.unwrap_or(i_dev)),
                        None => {
                            if let Some(i) = i {
                                self.send_set(ctx, v_dev, i);
                            }
                        }
                    }
                }
            }
        }
    }
}

pub fn spawn(
    dev: Sender<Cmd>,
    ids: IdGen,
    snap: SharedSnapshot,
    data: Sender<DataCmd>,
    results: Receiver<Event>,
    repaint: Box<dyn Fn() + Send>,
) -> AuxHandle {
    let (cmd_tx, cmd_rx) = unbounded();
    let (n_tx, n_rx) = unbounded();
    let status = Arc::new(Mutex::new(AuxStatus::default()));
    let ctx = Ctx { dev, ids, snap, data, status: status.clone(), notices: n_tx, repaint };
    let join = std::thread::Builder::new()
        .name("aux".into())
        .spawn(move || run_thread(&ctx, &cmd_rx, &results))
        .expect("spawn aux thread");
    AuxHandle { cmd: cmd_tx, notices: n_rx, status, join: Some(join) }
}

fn finish(ctx: &Ctx, run: Run, reason: StopReason) {
    ctx.log(
        "AUX",
        &[
            ("fn", run.kind.tag().into()),
            ("action", "stop".into()),
            ("reason", reason.tag().into()),
            ("clamped", run.clamped.to_string()),
        ],
    );
    if run.clamped > 0 {
        log::warn!("{} clamped {} times during run", run.kind.tag(), run.clamped);
    }
    ctx.status(|s| {
        s.running = None;
        s.waiting_stable = false;
        s.paused = false;
    });
    ctx.notice(AuxNotice::Stopped(run.kind, reason));
}

fn run_thread(ctx: &Ctx, cmd_rx: &Receiver<AuxCmd>, results: &Receiver<Event>) {
    let mut run: Option<Run> = None;
    let mut results = results.clone();
    loop {
        let timeout = run.as_ref().map_or(IDLE_WAIT, |r| r.next_tick.saturating_duration_since(Instant::now()));
        let mut stop = None;
        select! {
            recv(cmd_rx) -> c => match c {
                Ok(AuxCmd::Start(req)) => {
                    if let Some(r) = run.take() {
                        finish(ctx, r, StopReason::User);
                    }
                    run = Some(Run::start(*req, ctx));
                }
                Ok(AuxCmd::Stop(reason)) => stop = Some(reason),
                Ok(AuxCmd::Update(u)) => {
                    if let Some(r) = &mut run {
                        r.update(u);
                    }
                }
                Ok(AuxCmd::Shutdown) | Err(_) => {
                    if let Some(r) = run.take() {
                        finish(ctx, r, StopReason::Shutdown);
                    }
                    return;
                }
            },
            recv(results) -> ev => match ev {
                Ok(Event::CmdResult { id, cmd: c, result, .. }) => {
                    if let Some(r) = &mut run
                        && let Flow::Stop(reason) = r.on_result(ctx, id, c, &result)
                    {
                        stop = Some(reason);
                    }
                }
                Ok(_) => {}
                Err(_) => results = crossbeam_channel::never(),
            },
            default(timeout) => {}
        }
        if stop.is_none()
            && let Some(r) = &mut run
        {
            let now = Instant::now();
            if now >= r.next_tick
                && let Flow::Stop(reason) = r.tick(ctx, now)
            {
                stop = Some(reason);
            }
        }
        if let Some(reason) = stop
            && let Some(r) = run.take()
        {
            finish(ctx, r, reason);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wave_formulas() {
        let (h, l, t) = (5.0, 2.0, 1.0);
        assert_eq!(wave_value(WaveKind::Square, 0.25, t, h, l), h);
        assert_eq!(wave_value(WaveKind::Square, 0.75, t, h, l), l);
        assert!((wave_value(WaveKind::Sine, 0.25, t, h, l) - h).abs() < 1e-12);
        // 三角波一个完整周期是 2T
        assert!((wave_value(WaveKind::Triangle, 1.0, t, h, l) - h).abs() < 1e-12);
        assert!((wave_value(WaveKind::Triangle, 2.0, t, h, l) - l).abs() < 1e-12);
        assert!((wave_value(WaveKind::Sawtooth, 0.5, t, h, l) - 3.5).abs() < 1e-12);
        let n = wave_value(WaveKind::Noise, 0.3, t, h, l);
        assert!((l..=h).contains(&n));
    }

    #[test]
    fn sweep_values_include_end() {
        let j =
            SweepJob { target: Qty::Voltage, start: 1.0, stop: 1.25, step: 0.1, delay_s: 1.0, response: Series::None };
        assert_eq!(j.values(), vec![1.0, 1.1, 1.2, 1.25]);
        let j = SweepJob { start: 5.0, stop: 4.0, step: 0.5, ..j };
        assert_eq!(j.values(), vec![5.0, 4.5, 4.0]);
    }
}
