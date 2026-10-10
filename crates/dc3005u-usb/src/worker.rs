//! 设备服务线程：连接状态机、固定节拍轮询、命令优先级与节流、应答确认、重试、重连、pending_off。

use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, TryRecvError, unbounded};
use dc3005u_proto::{
    ConnectReply, Identity, Measurement, POLL_RESP_LEN, SafetyLimits, check_poll_reply, check_short_ack,
    classify_connect_reply, cmd, frame_connect, frame_output, frame_poll, frame_set_vi, guard_before_send,
};

use crate::transport::{Connector, OpenError, Transport, TransportError};

pub type CmdId = u64;

/// 读缓冲：读数帧约 10072 字节，按 64 字节包对齐
const READ_LEN: usize = POLL_RESP_LEN.div_ceil(64) * 64;
const DRAIN_TIMEOUT: Duration = Duration::from_millis(10);
const RECONNECT_PERIOD: Duration = Duration::from_secs(1);
const PENDING_OFF_PERIOD: Duration = Duration::from_secs(1);
const ESTOP_RETRIES: u8 = 4; // 共 5 次
const DUPLICATE_STALL: Duration = Duration::from_secs(1);
const ECHO_CONFIRM_POLLS: u8 = 2;
const MAX_CONSECUTIVE_FAILS: u32 = 3;
const STATS_PERIOD: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Manual,
    QuickSet,
    Preset,
    /// ON 前先下发编辑值
    OnPrepare,
    Seq,
    Aux,
    Http,
    EStop,
    /// 首次连接时的 `off_on_connect`
    Connect,
    /// pending_off 的后台重试
    PendingOff,
    Exit,
}

#[derive(Debug, Clone)]
pub struct WorkerConfig {
    pub poll_interval: Duration,
    pub resp_timeout_poll: Duration,
    pub resp_timeout_cmd: Duration,
    pub usb_timeout: Duration,
    pub cmd_retries: u8,
    pub setpoint_min_interval: Duration,
    pub off_on_connect: bool,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_millis(50),
            resp_timeout_poll: Duration::from_millis(500),
            resp_timeout_cmd: Duration::from_millis(300),
            usb_timeout: Duration::from_millis(2000),
            cmd_retries: 2,
            setpoint_min_interval: Duration::from_millis(100),
            off_on_connect: true,
        }
    }
}

#[derive(Debug, Clone)]
pub enum Cmd {
    /// `auto` = 启动时自动连接：找不到设备时等待而不是报错
    Connect {
        auto: bool,
    },
    Disconnect,
    /// 已做显示修正与换算；调用方已校验
    SetVI {
        id: CmdId,
        volt_mv: u16,
        amp_ma: u16,
        source: Source,
    },
    Output {
        id: CmdId,
        on: bool,
        source: Source,
    },
    EStop,
    Config(WorkerConfig),
    /// 兜底钳位用的安全边界；显示修正开启时为 None（只按硬件极限钳位）
    GuardLimits(Option<SafetyLimits>),
    Shutdown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkOrigin {
    FirstConnect,
    Reconnect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkState {
    Disconnected,
    Connecting { waiting_device: bool },
    Connected { origin: LinkOrigin },
    Reconnecting { attempt: u32 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceError {
    NotFound,
    Busy,
    Permission,
    /// 打开成功但 `0x01` 握手无应答
    Handshake,
    Io(String),
}

impl From<OpenError> for DeviceError {
    fn from(e: OpenError) -> Self {
        match e {
            OpenError::NotFound => DeviceError::NotFound,
            OpenError::Busy => DeviceError::Busy,
            OpenError::Permission => DeviceError::Permission,
            OpenError::Io(s) => DeviceError::Io(s),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ack {
    /// 收到应答
    Acked,
    /// 读数回显已确认（仅 0x03）
    Confirmed,
    /// 确认前已有更新的同类命令，本条不再确认，不算失败
    Superseded,
    /// 重连中的关输出：已记为 pending_off，重连后立即发送
    Deferred,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CmdError {
    #[error("设备无应答")]
    Timeout,
    #[error("设备应答无效：{0}")]
    BadResponse(String),
    #[error("USB 错误：{0}")]
    Usb(String),
    #[error("设定回显不一致")]
    EchoMismatch,
    #[error("设备未连接")]
    NotConnected,
    #[error("有未完成的关输出请求")]
    PendingOff,
}

impl CmdError {
    /// 计入「连续 3 条命令失败 → 重连」的错误
    fn counts_as_link_failure(&self) -> bool {
        matches!(self, CmdError::Timeout | CmdError::BadResponse(_) | CmdError::Usb(_))
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Sample {
    /// 上位机样本计数（连接以来单调递增）
    pub n: u64,
    /// 应答读完的时刻
    pub t: Instant,
    pub meas: Measurement,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct CmdStats {
    pub ok: u64,
    pub timeout: u64,
    pub bad: u64,
    rtt_total: Duration,
}

impl CmdStats {
    pub fn avg_rtt_ms(&self) -> f64 {
        if self.ok == 0 { 0.0 } else { self.rtt_total.as_secs_f64() * 1000.0 / self.ok as f64 }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct CommStats {
    pub connect: CmdStats,
    pub set: CmdStats,
    pub poll: CmdStats,
    pub output: CmdStats,
    pub duplicate_frames: u64,
    pub drained_bytes: u64,
}

impl CommStats {
    fn of(&mut self, c: u8) -> &mut CmdStats {
        match c {
            cmd::CONNECT => &mut self.connect,
            cmd::SET_VI => &mut self.set,
            cmd::OUTPUT => &mut self.output,
            _ => &mut self.poll,
        }
    }

    pub fn total(&self) -> CmdStats {
        let mut t = CmdStats::default();
        for s in [self.connect, self.set, self.poll, self.output] {
            t.ok += s.ok;
            t.timeout += s.timeout;
            t.bad += s.bad;
            t.rtt_total += s.rtt_total;
        }
        t
    }
}

#[derive(Debug, Clone)]
pub enum Event {
    Link(LinkState),
    Identity(Identity),
    Sample(Sample),
    Error(DeviceError),
    CmdResult {
        id: CmdId,
        cmd: u8,
        source: Source,
        result: Result<Ack, CmdError>,
        /// `0x06` 请求的开关值；其它命令为 None
        output_on: Option<bool>,
    },
    PendingOff(bool),
    /// 兜底钳位触发（正常路径不应发生）
    GuardTriggered {
        source: Source,
        from: (u16, u16),
        to: (u16, u16),
    },
    Stats(CommStats),
}

pub struct DeviceHandle {
    tx: Sender<Cmd>,
    join: Option<JoinHandle<()>>,
}

impl DeviceHandle {
    pub fn send(&self, c: Cmd) {
        let _ = self.tx.send(c);
    }

    pub fn sender(&self) -> Sender<Cmd> {
        self.tx.clone()
    }
}

impl Drop for DeviceHandle {
    fn drop(&mut self) {
        let _ = self.tx.send(Cmd::Shutdown);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

pub fn spawn(connector: Box<dyn Connector>, cfg: WorkerConfig, events: Sender<Event>) -> DeviceHandle {
    let (tx, rx) = unbounded();
    let join = std::thread::Builder::new()
        .name("dc3005u-io".into())
        .spawn(move || Worker::new(connector, cfg, rx, events).run())
        .expect("spawn IO thread");
    DeviceHandle { tx, join: Some(join) }
}

#[derive(Debug, Clone, Copy)]
struct SetReq {
    id: CmdId,
    mv: u16,
    ma: u16,
    source: Source,
}

#[derive(Debug, Clone, Copy)]
struct OutReq {
    id: CmdId,
    on: bool,
    source: Source,
}

#[derive(Debug, Clone, Copy)]
struct Confirm {
    req: SetReq,
    polls_left: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Link {
    Idle,
    /// 首次连接（自动连接）等待设备插入
    Waiting {
        next_try: Instant,
    },
    Up,
    Reconnecting {
        attempt: u32,
        next_try: Instant,
    },
}

struct Worker {
    connector: Box<dyn Connector>,
    cfg: WorkerConfig,
    guard: Option<SafetyLimits>,
    rx: Receiver<Cmd>,
    tx: Sender<Event>,
    link: Link,
    transport: Option<Box<dyn Transport>>,
    /// 当前事务里检测到设备已断开
    lost: bool,
    shutdown: bool,
    pending_set: Option<SetReq>,
    pending_out: Option<OutReq>,
    estop: bool,
    pending_off: bool,
    pending_off_next: Instant,
    confirm: Option<Confirm>,
    last_set_sent: Option<Instant>,
    next_poll: Instant,
    last_seq: Option<u32>,
    last_new_seq_at: Instant,
    consecutive_fails: u32,
    sample_n: u64,
    stats: CommStats,
    stats_next: Instant,
}

impl Worker {
    fn new(connector: Box<dyn Connector>, cfg: WorkerConfig, rx: Receiver<Cmd>, tx: Sender<Event>) -> Self {
        let now = Instant::now();
        Self {
            connector,
            cfg,
            guard: None,
            rx,
            tx,
            link: Link::Idle,
            transport: None,
            lost: false,
            shutdown: false,
            pending_set: None,
            pending_out: None,
            estop: false,
            pending_off: false,
            pending_off_next: now,
            confirm: None,
            last_set_sent: None,
            next_poll: now,
            last_seq: None,
            last_new_seq_at: now,
            consecutive_fails: 0,
            sample_n: 0,
            stats: CommStats::default(),
            stats_next: now + STATS_PERIOD,
        }
    }

    fn emit(&self, e: Event) {
        let _ = self.tx.send(e);
    }

    fn result(&self, id: CmdId, c: u8, source: Source, result: Result<Ack, CmdError>) {
        self.emit(Event::CmdResult { id, cmd: c, source, result, output_on: None });
    }

    fn out_result(&self, id: CmdId, source: Source, on: bool, result: Result<Ack, CmdError>) {
        self.emit(Event::CmdResult { id, cmd: cmd::OUTPUT, source, result, output_on: Some(on) });
    }

    fn set_link(&mut self, link: Link, state: LinkState) {
        self.link = link;
        self.emit(Event::Link(state));
    }

    fn set_pending_off(&mut self, on: bool) {
        if self.pending_off != on {
            self.pending_off = on;
            self.emit(Event::PendingOff(on));
        }
    }

    fn run(mut self) {
        while !self.shutdown {
            match self.link {
                Link::Idle => match self.rx.recv() {
                    Ok(c) => self.handle_cmd(c),
                    Err(_) => break,
                },
                Link::Waiting { next_try } | Link::Reconnecting { next_try, .. } => {
                    if !self.wait_cmd_until(next_try) {
                        continue;
                    }
                    if matches!(self.link, Link::Waiting { .. }) {
                        self.try_first_connect(true);
                    } else {
                        self.try_reconnect();
                    }
                }
                Link::Up => self.step_connected(),
            }
        }
        self.transport = None;
    }

    /// 等命令直到 `deadline`；返回 true 表示到点（期间状态没被命令改掉）
    fn wait_cmd_until(&mut self, deadline: Instant) -> bool {
        let link_before = self.link;
        loop {
            let now = Instant::now();
            if now >= deadline {
                return self.link == link_before;
            }
            match self.rx.recv_timeout(deadline - now) {
                Ok(c) => {
                    self.handle_cmd(c);
                    if self.shutdown || self.link != link_before {
                        return false;
                    }
                }
                Err(RecvTimeoutError::Timeout) => return self.link == link_before,
                Err(RecvTimeoutError::Disconnected) => {
                    self.shutdown = true;
                    return false;
                }
            }
        }
    }

    fn drain_cmds(&mut self) {
        loop {
            match self.rx.try_recv() {
                Ok(c) => self.handle_cmd(c),
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => {
                    self.shutdown = true;
                    return;
                }
            }
        }
    }

    fn handle_cmd(&mut self, c: Cmd) {
        match c {
            Cmd::Shutdown => self.shutdown = true,
            Cmd::Config(cfg) => {
                if cfg.poll_interval != self.cfg.poll_interval {
                    self.next_poll = Instant::now();
                }
                self.cfg = cfg;
            }
            Cmd::GuardLimits(g) => self.guard = g,
            Cmd::Connect { auto } => {
                if matches!(self.link, Link::Idle) {
                    self.try_first_connect(auto);
                }
            }
            Cmd::Disconnect => self.disconnect(),
            Cmd::SetVI { id, volt_mv, amp_ma, source } => {
                if self.link != Link::Up {
                    self.result(id, cmd::SET_VI, source, Err(CmdError::NotConnected));
                    return;
                }
                if let Some(old) = self.pending_set.replace(SetReq { id, mv: volt_mv, ma: amp_ma, source }) {
                    self.result(old.id, cmd::SET_VI, old.source, Ok(Ack::Superseded));
                }
            }
            Cmd::Output { id, on, source } => self.queue_output(OutReq { id, on, source }),
            Cmd::EStop => match self.link {
                Link::Up => {
                    self.estop = true;
                    self.drop_queued(Ok(Ack::Superseded));
                }
                Link::Reconnecting { .. } => self.set_pending_off(true),
                _ => {}
            },
        }
    }

    fn queue_output(&mut self, req: OutReq) {
        match (self.link, req.on) {
            (Link::Up, true) if self.pending_off => {
                self.out_result(req.id, req.source, req.on, Err(CmdError::PendingOff));
            }
            (Link::Up, _) => {
                if let Some(old) = self.pending_out.replace(req) {
                    self.out_result(old.id, old.source, old.on, Ok(Ack::Superseded));
                }
            }
            (Link::Reconnecting { .. }, false) => {
                self.set_pending_off(true);
                self.out_result(req.id, req.source, req.on, Ok(Ack::Deferred));
            }
            _ => self.out_result(req.id, req.source, req.on, Err(CmdError::NotConnected)),
        }
    }

    /// 清空待发设定 / 开关和待确认，逐条回报 `result`
    fn drop_queued(&mut self, result: Result<Ack, CmdError>) {
        if let Some(s) = self.pending_set.take() {
            self.result(s.id, cmd::SET_VI, s.source, result.clone());
        }
        if let Some(c) = self.confirm.take() {
            self.result(c.req.id, cmd::SET_VI, c.req.source, result.clone());
        }
        if let Some(o) = self.pending_out.take() {
            self.out_result(o.id, o.source, o.on, result);
        }
    }

    fn disconnect(&mut self) {
        self.transport = None;
        self.estop = false;
        self.drop_queued(Err(CmdError::NotConnected));
        self.set_pending_off(false);
        self.set_link(Link::Idle, LinkState::Disconnected);
    }

    // ---------- 连接 ----------

    fn try_first_connect(&mut self, auto: bool) {
        if !matches!(self.link, Link::Waiting { .. }) {
            self.emit(Event::Link(LinkState::Connecting { waiting_device: false }));
        }
        match self.open_and_handshake() {
            Ok(()) => {
                self.on_link_up(LinkOrigin::FirstConnect);
                if self.cfg.off_on_connect {
                    self.do_output(OutReq { id: 0, on: false, source: Source::Connect });
                }
            }
            Err(DeviceError::NotFound) if auto => {
                let next_try = Instant::now() + RECONNECT_PERIOD;
                self.set_link(Link::Waiting { next_try }, LinkState::Connecting { waiting_device: true });
            }
            Err(e) => {
                self.emit(Event::Error(e));
                self.set_link(Link::Idle, LinkState::Disconnected);
            }
        }
    }

    fn try_reconnect(&mut self) {
        let Link::Reconnecting { attempt, .. } = self.link else { return };
        let attempt = attempt + 1;
        self.emit(Event::Link(LinkState::Reconnecting { attempt }));
        match self.open_and_handshake() {
            Ok(()) => {
                self.on_link_up(LinkOrigin::Reconnect);
                if self.pending_off {
                    self.pending_off_next = Instant::now();
                }
            }
            Err(e) => {
                log::debug!("reconnect attempt {attempt} failed: {e:?}");
                self.link = Link::Reconnecting { attempt, next_try: Instant::now() + RECONNECT_PERIOD };
            }
        }
    }

    fn open_and_handshake(&mut self) -> Result<(), DeviceError> {
        let t = self.connector.open().map_err(DeviceError::from)?;
        self.transport = Some(t);
        self.lost = false;
        match self.run_cmd(cmd::CONNECT, &frame_connect(), self.cfg.cmd_retries) {
            Ok(resp) => {
                self.handle_connect_reply(&resp);
                Ok(())
            }
            Err(_) => {
                self.transport = None;
                Err(DeviceError::Handshake)
            }
        }
    }

    fn handle_connect_reply(&mut self, resp: &[u8]) {
        match classify_connect_reply(resp) {
            Ok(ConnectReply::Identity(id)) => self.emit(Event::Identity(id)),
            Ok(ConnectReply::Measurement(m)) => self.accept_measurement(m),
            Err(e) => log::warn!("unrecognized 0x01 reply ({} bytes): {e}", resp.len()),
        }
    }

    fn on_link_up(&mut self, origin: LinkOrigin) {
        let now = Instant::now();
        self.consecutive_fails = 0;
        self.last_seq = None;
        self.last_new_seq_at = now;
        self.next_poll = now;
        self.last_set_sent = None;
        self.set_link(Link::Up, LinkState::Connected { origin });
    }

    fn go_reconnect(&mut self) {
        log::warn!("link lost, reconnecting");
        self.transport = None;
        self.lost = false;
        self.estop = false;
        self.drop_queued(Err(CmdError::NotConnected));
        self.set_link(
            Link::Reconnecting { attempt: 0, next_try: Instant::now() + RECONNECT_PERIOD },
            LinkState::Reconnecting { attempt: 0 },
        );
    }

    // ---------- 已连接：调度 ----------

    fn step_connected(&mut self) {
        self.drain_cmds();
        if self.shutdown || self.link != Link::Up {
            return;
        }
        let now = Instant::now();
        if now >= self.stats_next {
            self.stats_next = now + STATS_PERIOD;
            self.emit(Event::Stats(self.stats));
        }

        if self.estop {
            self.do_estop();
        } else if self.pending_off && now >= self.pending_off_next {
            self.do_pending_off();
        } else if let Some(o) = self.pending_out.take() {
            self.do_output(o);
        } else if self.pending_set.is_some() && now >= self.set_ready_at(now) {
            self.do_set();
        } else if now >= self.next_poll {
            self.do_poll();
        } else {
            let mut deadline = self.next_poll;
            if self.pending_set.is_some() {
                deadline = deadline.min(self.set_ready_at(now));
            }
            if self.pending_off {
                deadline = deadline.min(self.pending_off_next);
            }
            match self.rx.recv_timeout(deadline.saturating_duration_since(now)) {
                Ok(c) => self.handle_cmd(c),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => self.shutdown = true,
            }
        }

        if self.link == Link::Up && (self.lost || self.consecutive_fails >= MAX_CONSECUTIVE_FAILS) {
            self.go_reconnect();
        }
    }

    fn set_ready_at(&self, now: Instant) -> Instant {
        match self.last_set_sent {
            Some(t) => t + self.cfg.setpoint_min_interval,
            None => now,
        }
    }

    fn note_cmd_outcome(&mut self, r: &Result<Vec<u8>, CmdError>) {
        match r {
            Ok(_) => self.consecutive_fails = 0,
            Err(e) if e.counts_as_link_failure() => self.consecutive_fails += 1,
            Err(_) => {}
        }
    }

    // ---------- 事务 ----------

    /// 一次事务：写前排空 → 写 → 读应答。
    fn transact(&mut self, frame: &[u8], timeout: Duration) -> Result<Vec<u8>, TransportError> {
        let usb_timeout = self.cfg.usb_timeout;
        let t = self.transport.as_mut().ok_or(TransportError::Disconnected)?;
        loop {
            match t.read(READ_LEN, DRAIN_TIMEOUT) {
                Ok(d) if !d.is_empty() => {
                    log::debug!("drained {} stale bytes", d.len());
                    self.stats.drained_bytes += d.len() as u64;
                }
                Ok(_) | Err(TransportError::Timeout) => break,
                Err(TransportError::Disconnected) => return Err(TransportError::Disconnected),
                Err(TransportError::Io(e)) => {
                    log::debug!("drain read error: {e}");
                    break;
                }
            }
        }
        t.write(frame, usb_timeout)?;
        t.read(READ_LEN, timeout)
    }

    /// 发送一条命令并校验应答，失败重试 `retries` 次。
    fn run_cmd(&mut self, c: u8, frame: &[u8], retries: u8) -> Result<Vec<u8>, CmdError> {
        let timeout = if c == cmd::POLL { self.cfg.resp_timeout_poll } else { self.cfg.resp_timeout_cmd };
        let mut last = CmdError::Timeout;
        for attempt in 0..=retries {
            let t0 = Instant::now();
            match self.transact(frame, timeout) {
                Ok(resp) => {
                    let valid = match c {
                        cmd::POLL => check_poll_reply(&resp).map(|_| ()),
                        cmd::CONNECT => classify_connect_reply(&resp).map(|_| ()),
                        _ => check_short_ack(&resp, c),
                    };
                    match valid {
                        Ok(()) => {
                            let s = self.stats.of(c);
                            s.ok += 1;
                            s.rtt_total += t0.elapsed();
                            return Ok(resp);
                        }
                        Err(e) => {
                            self.stats.of(c).bad += 1;
                            last = CmdError::BadResponse(e.to_string());
                        }
                    }
                }
                Err(TransportError::Timeout) => {
                    self.stats.of(c).timeout += 1;
                    last = CmdError::Timeout;
                }
                Err(TransportError::Disconnected) => {
                    self.lost = true;
                    return Err(CmdError::Usb("设备已断开".into()));
                }
                Err(TransportError::Io(e)) => last = CmdError::Usb(e),
            }
            log::debug!("cmd 0x{c:02X} attempt {} failed: {last}", attempt + 1);
        }
        Err(last)
    }

    // ---------- 各命令 ----------

    fn do_output(&mut self, o: OutReq) {
        let r = self.run_cmd(cmd::OUTPUT, &frame_output(o.on), self.cfg.cmd_retries);
        self.note_cmd_outcome(&r);
        match r {
            Ok(_) => {
                if !o.on {
                    self.set_pending_off(false);
                }
                self.out_result(o.id, o.source, o.on, Ok(Ack::Acked));
            }
            Err(e) => {
                if !o.on {
                    self.set_pending_off(true);
                    self.pending_off_next = Instant::now() + PENDING_OFF_PERIOD;
                }
                self.out_result(o.id, o.source, o.on, Err(e));
            }
        }
    }

    fn do_estop(&mut self) {
        self.estop = false;
        self.drop_queued(Ok(Ack::Superseded));
        let r = self.run_cmd(cmd::OUTPUT, &frame_output(false), ESTOP_RETRIES);
        match r {
            Ok(_) => {
                self.set_pending_off(false);
                self.out_result(0, Source::EStop, false, Ok(Ack::Acked));
            }
            Err(e) => {
                self.set_pending_off(true);
                self.pending_off_next = Instant::now() + PENDING_OFF_PERIOD;
                self.out_result(0, Source::EStop, false, Err(e));
            }
        }
    }

    fn do_pending_off(&mut self) {
        let r = self.run_cmd(cmd::OUTPUT, &frame_output(false), 0);
        match r {
            Ok(_) => {
                self.set_pending_off(false);
                self.out_result(0, Source::PendingOff, false, Ok(Ack::Acked));
            }
            Err(e) => {
                log::warn!("pending off retry failed: {e}");
                self.pending_off_next = Instant::now() + PENDING_OFF_PERIOD;
            }
        }
    }

    fn do_set(&mut self) {
        let Some(req) = self.pending_set.take() else { return };
        if let Some(c) = self.confirm.take() {
            self.result(c.req.id, cmd::SET_VI, c.req.source, Ok(Ack::Superseded));
        }
        let (mv, ma, triggered) = guard_before_send(req.mv, req.ma, self.guard.as_ref());
        if triggered {
            log::error!("guard_before_send clamped {:?}: {}/{} -> {mv}/{ma}", req.source, req.mv, req.ma);
            self.emit(Event::GuardTriggered { source: req.source, from: (req.mv, req.ma), to: (mv, ma) });
        }
        let sent = SetReq { mv, ma, ..req };
        let r = self.run_cmd(cmd::SET_VI, &frame_set_vi(mv, ma), self.cfg.cmd_retries);
        self.last_set_sent = Some(Instant::now());
        self.note_cmd_outcome(&r);
        match r {
            Ok(_) => {
                self.result(req.id, cmd::SET_VI, req.source, Ok(Ack::Acked));
                self.confirm = Some(Confirm { req: sent, polls_left: ECHO_CONFIRM_POLLS });
            }
            Err(e) => self.result(req.id, cmd::SET_VI, req.source, Err(e)),
        }
    }

    fn do_poll(&mut self) {
        let now = Instant::now();
        self.next_poll += self.cfg.poll_interval;
        if self.next_poll <= now {
            // 错过的节拍直接跳过，不补发
            self.next_poll = now + self.cfg.poll_interval;
        }
        let r = self.run_cmd(cmd::POLL, &frame_poll(), self.cfg.cmd_retries);
        if self.lost {
            return;
        }
        match r.map(|resp| check_poll_reply(&resp)) {
            Ok(Ok(m)) => {
                if self.last_seq == Some(m.seq) {
                    self.stats.duplicate_frames += 1;
                    if self.last_new_seq_at.elapsed() >= DUPLICATE_STALL {
                        log::warn!("frame seq stuck at {} for 1 s", m.seq);
                        self.last_new_seq_at = Instant::now();
                        self.poll_failed();
                    }
                } else {
                    self.accept_measurement(m);
                }
            }
            _ => self.poll_failed(),
        }
    }

    /// `0x05` 重试用尽：发 `0x01`，仍失败则重连。
    fn poll_failed(&mut self) {
        match self.run_cmd(cmd::CONNECT, &frame_connect(), self.cfg.cmd_retries) {
            Ok(resp) => self.handle_connect_reply(&resp),
            Err(_) => {
                if self.link == Link::Up {
                    self.go_reconnect();
                }
            }
        }
    }

    fn accept_measurement(&mut self, m: Measurement) {
        self.last_seq = Some(m.seq);
        self.last_new_seq_at = Instant::now();
        self.sample_n += 1;
        self.emit(Event::Sample(Sample { n: self.sample_n, t: Instant::now(), meas: m }));
        if let Some(mut c) = self.confirm.take() {
            if m.set_voltage_mv == c.req.mv && m.set_current_ma == c.req.ma {
                self.result(c.req.id, cmd::SET_VI, c.req.source, Ok(Ack::Confirmed));
            } else {
                c.polls_left -= 1;
                if c.polls_left == 0 {
                    self.result(c.req.id, cmd::SET_VI, c.req.source, Err(CmdError::EchoMismatch));
                } else {
                    self.confirm = Some(c);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::{IoKind, SimDevice};

    fn fast_cfg() -> WorkerConfig {
        WorkerConfig {
            poll_interval: Duration::from_millis(20),
            resp_timeout_poll: Duration::from_millis(30),
            resp_timeout_cmd: Duration::from_millis(30),
            usb_timeout: Duration::from_millis(100),
            cmd_retries: 2,
            setpoint_min_interval: Duration::from_millis(10),
            off_on_connect: true,
        }
    }

    struct Rig {
        sim: SimDevice,
        dev: DeviceHandle,
        rx: Receiver<Event>,
    }

    impl Rig {
        fn new(cfg: WorkerConfig) -> Self {
            let sim = SimDevice::new();
            let (tx, rx) = unbounded();
            let dev = spawn(Box::new(sim.connector()), cfg, tx);
            Self { sim, dev, rx }
        }

        fn connected(cfg: WorkerConfig) -> Self {
            let r = Self::new(cfg);
            r.dev.send(Cmd::Connect { auto: false });
            r.wait(|e| matches!(e, Event::Sample(_)));
            r
        }

        fn wait(&self, mut pred: impl FnMut(&Event) -> bool) -> Event {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                let left = deadline.saturating_duration_since(Instant::now());
                match self.rx.recv_timeout(left) {
                    Ok(e) if pred(&e) => return e,
                    Ok(_) => {}
                    Err(_) => panic!("timed out waiting for event"),
                }
            }
        }
    }

    #[test]
    fn tc07_first_connect_turns_output_off() {
        let r = Rig::new(fast_cfg());
        r.sim.state().output_on = true;
        r.dev.send(Cmd::Connect { auto: false });
        assert!(matches!(r.wait(|e| matches!(e, Event::Identity(_))), Event::Identity(id) if id.model == "DC3005U"));
        r.wait(|e| matches!(e, Event::CmdResult { source: Source::Connect, result: Ok(Ack::Acked), .. }));
        assert!(!r.sim.state().output_on);
        r.wait(|e| matches!(e, Event::Sample(_)));
    }

    #[test]
    fn tc61_no_off_when_disabled() {
        let r = Rig::connected(WorkerConfig { off_on_connect: false, ..fast_cfg() });
        assert!(r.sim.writes_of(cmd::OUTPUT).is_empty());
    }

    #[test]
    fn set_is_acked_then_confirmed() {
        let r = Rig::connected(fast_cfg());
        r.dev.send(Cmd::SetVI { id: 7, volt_mv: 6000, amp_ma: 1000, source: Source::Manual });
        r.wait(|e| matches!(e, Event::CmdResult { id: 7, result: Ok(Ack::Acked), .. }));
        r.wait(|e| matches!(e, Event::CmdResult { id: 7, result: Ok(Ack::Confirmed), .. }));
        let s = r.sim.state();
        assert_eq!((s.set_mv, s.set_ma), (6000, 1000));
    }

    #[test]
    fn tc59_echo_mismatch() {
        let r = Rig::connected(fast_cfg());
        r.sim.state().echo_follows_set = false;
        r.dev.send(Cmd::SetVI { id: 8, volt_mv: 6000, amp_ma: 1000, source: Source::Manual });
        r.wait(|e| matches!(e, Event::CmdResult { id: 8, result: Err(CmdError::EchoMismatch), .. }));
    }

    #[test]
    fn tc62_set_timeout_retries_then_fails() {
        let r = Rig::connected(fast_cfg());
        r.sim.state().drop_set_ack = true;
        r.dev.send(Cmd::SetVI { id: 9, volt_mv: 6000, amp_ma: 1000, source: Source::Manual });
        r.wait(|e| matches!(e, Event::CmdResult { id: 9, result: Err(CmdError::Timeout), .. }));
        assert_eq!(r.sim.writes_of(cmd::SET_VI).len(), 3);
    }

    #[test]
    fn tc100_superseded_set() {
        let r = Rig::connected(WorkerConfig { poll_interval: Duration::from_millis(300), ..fast_cfg() });
        r.dev.send(Cmd::SetVI { id: 1, volt_mv: 5100, amp_ma: 1000, source: Source::QuickSet });
        r.wait(|e| matches!(e, Event::CmdResult { id: 1, result: Ok(Ack::Acked), .. }));
        r.dev.send(Cmd::SetVI { id: 2, volt_mv: 5200, amp_ma: 1000, source: Source::QuickSet });
        r.wait(|e| matches!(e, Event::CmdResult { id: 1, result: Ok(Ack::Superseded), .. }));
        r.wait(|e| matches!(e, Event::CmdResult { id: 2, result: Ok(Ack::Confirmed), .. }));
    }

    #[test]
    fn tc63_tc64_output_failures() {
        let r = Rig::connected(WorkerConfig { off_on_connect: false, ..fast_cfg() });
        r.sim.state().drop_output_ack = true;
        r.dev.send(Cmd::Output { id: 3, on: true, source: Source::Manual });
        r.wait(|e| matches!(e, Event::CmdResult { id: 3, result: Err(CmdError::Timeout), .. }));

        r.dev.send(Cmd::Output { id: 4, on: false, source: Source::Manual });
        r.wait(|e| matches!(e, Event::PendingOff(true)));
        r.wait(|e| matches!(e, Event::CmdResult { id: 4, result: Err(_), .. }));
        r.sim.state().drop_output_ack = false;
        r.wait(|e| matches!(e, Event::PendingOff(false)));
        assert!(!r.sim.state().output_on);
    }

    #[test]
    fn tc99_duplicate_frames_dropped_then_stall_handled() {
        let r = Rig::connected(fast_cfg());
        r.sim.state().freeze_seq = true;
        let connects_before = r.sim.writes_of(cmd::CONNECT).len();
        std::thread::sleep(Duration::from_millis(1300));
        // 重复帧不产生样本；1 s 后按读超时处理，发 0x01
        while r.rx.try_recv().is_ok() {}
        std::thread::sleep(Duration::from_millis(100));
        assert!(!r.rx.try_iter().any(|e| matches!(e, Event::Sample(_))));
        assert!(r.sim.writes_of(cmd::CONNECT).len() > connects_before);
    }

    #[test]
    fn tc67_stale_bytes_drained() {
        let r = Rig::connected(fast_cfg());
        r.sim.state().stray = Some(vec![0xEE; 40]);
        r.wait(|e| matches!(e, Event::Stats(s) if s.drained_bytes >= 40));
        r.wait(|e| matches!(e, Event::Sample(_)));
    }

    #[test]
    fn tc65_strict_request_response() {
        let r = Rig::connected(fast_cfg());
        r.dev.send(Cmd::SetVI { id: 5, volt_mv: 7000, amp_ma: 1000, source: Source::QuickSet });
        r.dev.send(Cmd::Output { id: 6, on: true, source: Source::Manual });
        r.wait(|e| matches!(e, Event::CmdResult { id: 5, result: Ok(Ack::Confirmed), .. }));
        let log = r.sim.state().io_log.clone();
        for w in log.windows(2) {
            assert!(!(w[0] == IoKind::Write && w[1] == IoKind::Write), "two writes without a read");
        }
    }

    #[test]
    fn tc20_reconnect_keeps_output_and_tc101_pending_off() {
        let r = Rig::connected(WorkerConfig { off_on_connect: false, ..fast_cfg() });
        r.dev.send(Cmd::Output { id: 10, on: true, source: Source::Manual });
        r.wait(|e| matches!(e, Event::CmdResult { id: 10, result: Ok(Ack::Acked), .. }));

        // 拔插：重连后不发 0x06
        r.sim.unplug();
        r.wait(|e| matches!(e, Event::Link(LinkState::Reconnecting { .. })));
        let outputs_before = r.sim.writes_of(cmd::OUTPUT).len();
        r.sim.plug();
        r.wait(|e| matches!(e, Event::Link(LinkState::Connected { origin: LinkOrigin::Reconnect })));
        r.wait(|e| matches!(e, Event::Sample(_)));
        assert_eq!(r.sim.writes_of(cmd::OUTPUT).len(), outputs_before);
        assert!(r.sim.state().output_on);

        // 重连中点 OFF：记为 pending_off，重连后第一条命令就是 0x06 00
        r.sim.unplug();
        r.wait(|e| matches!(e, Event::Link(LinkState::Reconnecting { .. })));
        r.dev.send(Cmd::Output { id: 11, on: false, source: Source::Manual });
        r.wait(|e| matches!(e, Event::PendingOff(true)));
        r.wait(|e| matches!(e, Event::CmdResult { id: 11, result: Ok(Ack::Deferred), .. }));
        let n = r.sim.state().writes.len();
        r.sim.plug();
        r.wait(|e| matches!(e, Event::PendingOff(false)));
        let writes = r.sim.state().writes[n..].to_vec();
        assert_eq!(writes[0][1], cmd::CONNECT);
        assert_eq!(writes[1], frame_output(false));
        assert!(!r.sim.state().output_on);
    }

    #[test]
    fn tc109_auto_connect_waits_for_device() {
        let r = Rig::new(fast_cfg());
        r.sim.unplug();
        r.dev.send(Cmd::Connect { auto: true });
        r.wait(|e| matches!(e, Event::Link(LinkState::Connecting { waiting_device: true })));
        r.sim.plug();
        r.wait(|e| matches!(e, Event::Link(LinkState::Connected { origin: LinkOrigin::FirstConnect })));
    }

    #[test]
    fn tc08_busy_and_tc09_not_found() {
        let r = Rig::new(fast_cfg());
        r.sim.state().busy = true;
        r.dev.send(Cmd::Connect { auto: false });
        r.wait(|e| matches!(e, Event::Error(DeviceError::Busy)));
        r.sim.state().busy = false;
        r.sim.unplug();
        r.dev.send(Cmd::Connect { auto: false });
        r.wait(|e| matches!(e, Event::Error(DeviceError::NotFound)));
    }

    #[test]
    fn estop_clears_queue_and_turns_off() {
        let r = Rig::connected(WorkerConfig { off_on_connect: false, ..fast_cfg() });
        r.dev.send(Cmd::Output { id: 20, on: true, source: Source::Manual });
        r.wait(|e| matches!(e, Event::CmdResult { id: 20, result: Ok(Ack::Acked), .. }));
        r.dev.send(Cmd::SetVI { id: 21, volt_mv: 9000, amp_ma: 1000, source: Source::Aux });
        r.dev.send(Cmd::EStop);
        r.wait(|e| matches!(e, Event::CmdResult { source: Source::EStop, result: Ok(Ack::Acked), .. }));
        assert!(!r.sim.state().output_on);
    }

    #[test]
    fn guard_clamps_out_of_range() {
        let r = Rig::connected(fast_cfg());
        r.dev.send(Cmd::SetVI { id: 30, volt_mv: 30000, amp_ma: 6000, source: Source::Manual });
        r.wait(|e| matches!(e, Event::GuardTriggered { to: (24000, 5000), .. }));
        r.wait(|e| matches!(e, Event::CmdResult { id: 30, result: Ok(Ack::Confirmed), .. }));
    }
}
