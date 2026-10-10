//! 主窗口状态与事件处理；各面板的绘制在 `ui/` 下。

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, unbounded};
use dc3005u_proto::{Field, Identity, LimitError, Qty};
use dc3005u_usb::{
    Ack, Cmd, CmdError, CmdId, CommStats, DeviceError, DeviceHandle, Event, LinkOrigin, LinkState, Source,
    sim::SimDevice,
};
use eframe::egui;
use serde_json::json;

use crate::auxfn::{self, AuxCmd, AuxHandle, AuxNotice, AuxStatus, IdGen, Job, Kind, StartReq, StopReason};
use crate::buffer::SampleBuffer;
use crate::cali::{Cali, OutOfHw};
use crate::config::{LoadNotice, Settings};
use crate::data::{DataCmd, DataHandle, DataNotice, DisplayCfg, LogCtx, LogHeader, Processed, RecordCfg};
use crate::format::{Scale, milli3};
use crate::http::{HttpReply, HttpReq, HttpServer, SharedView, UiView};
use crate::i18n::{t, tf};
use crate::output::{OutputNotice, OutputState, OutputView};
use crate::setpoint::{Commit, Outcome, SetError, SetpointModel};
use crate::theme::Palette;
use crate::ui::analysis_window::AnalysisWindow;
use crate::ui::aux_panel::AuxUi;
use crate::ui::float_window::FloatState;
use crate::ui::graph_panel::GraphView;
use crate::ui::graphics_dialog::GraphicsDialog;
use crate::ui::power_dialog::PowerDialog;
use crate::ui::result_window::ResultWindow;

const TOAST_INFO: Duration = Duration::from_secs(4);
const TOAST_ERR: Duration = Duration::from_secs(8);
const RATE_WINDOW: usize = 40;
const EXIT_OFF_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    Info,
    Warn,
    Error,
}

pub struct Toast {
    pub text: String,
    pub kind: ToastKind,
    pub until: Instant,
}

/// 状态区：显示两次刷新间样本的平均值
pub struct StatusDisplay {
    acc_v: f64,
    acc_i: f64,
    acc_n: u32,
    last_flush: Instant,
    pub v: f64,
    pub i_a: f64,
    pub energy_j: f64,
    pub avg_power_w: Option<f64>,
    pub have: bool,
    pub i_scale: Scale,
    pub p_scale: Scale,
    pub avgp_scale: Scale,
}

impl Default for StatusDisplay {
    fn default() -> Self {
        Self {
            acc_v: 0.0,
            acc_i: 0.0,
            acc_n: 0,
            last_flush: Instant::now(),
            v: 0.0,
            i_a: 0.0,
            energy_j: 0.0,
            avg_power_w: None,
            have: false,
            i_scale: Scale::Milli,
            p_scale: Scale::Unit,
            avgp_scale: Scale::Unit,
        }
    }
}

impl StatusDisplay {
    fn push(&mut self, p: &Processed) {
        self.acc_v += p.v;
        self.acc_i += p.i_a;
        self.acc_n += 1;
        self.energy_j = p.energy_j;
        self.avg_power_w = p.avg_power_w;
    }

    fn flush(&mut self, period: Duration) {
        if self.acc_n == 0 || self.last_flush.elapsed() < period {
            return;
        }
        let n = f64::from(self.acc_n);
        self.v = self.acc_v / n;
        self.i_a = self.acc_i / n;
        self.acc_v = 0.0;
        self.acc_i = 0.0;
        self.acc_n = 0;
        self.last_flush = Instant::now();
        self.have = true;
    }

    pub fn power_w(&self) -> f64 {
        self.v * self.i_a
    }
}

pub enum ExitPhase {
    Ask,
    TurningOff { id: CmdId, since: Instant },
    OffFailed,
}

pub struct App {
    pub settings: Settings,
    pub sim: Option<SimDevice>,
    // 字段按声明顺序 drop：HTTP、AUX 先停，然后 IO 线程，最后数据线程（录制随之写盘）
    http: Option<HttpServer>,
    pub aux: AuxHandle,
    pub dev: DeviceHandle,
    pub data: DataHandle,
    ids: IdGen,
    pub cali: Cali,
    pub link: LinkState,
    pub link_error: Option<DeviceError>,
    pub identity: Option<Identity>,
    pub stats: CommStats,
    pub sp: SetpointModel,
    pub out: OutputState,
    pub on_after_set: Option<CmdId>,
    pub status: StatusDisplay,
    pub last_sample: Option<Processed>,
    sample_times: VecDeque<Instant>,
    pub sample_rate_hz: f64,
    pub buffer: SampleBuffer,
    pub graph: GraphView,
    pub toasts: Vec<Toast>,
    pub power_dialog: Option<PowerDialog>,
    pub graphics: Option<GraphicsDialog>,
    pub confirm_on: bool,
    pub exit: Option<ExitPhase>,
    allow_close: bool,
    quit_now: bool,
    pub modal_notice: Option<String>,
    pub dark: bool,
    pub pal: Palette,
    title: String,
    /// false 时不写 settings.json
    pub persist: bool,
    pub recording: Option<(PathBuf, Instant)>,
    pub saved_files: Option<Vec<PathBuf>>,
    pub aux_status: AuxStatus,
    pub aux_ui: AuxUi,
    pub confirm_aux: Option<Box<StartReq>>,
    pub result_win: Option<ResultWindow>,
    pub float: FloatState,
    pub analyses: Vec<AnalysisWindow>,
    next_window: u64,
    http_view: SharedView,
    http_tx: Sender<HttpReq>,
    http_rx: Receiver<HttpReq>,
    http_wait: HashMap<CmdId, Sender<HttpReply>>,
    log_ctx: LogCtx,
    /// 状态栏红字：最近一次错误，点击查看
    pub last_error: Option<String>,
    ctx: egui::Context,
}

fn src_tag(s: Source) -> &'static str {
    match s {
        Source::Manual | Source::OnPrepare => "manual",
        Source::QuickSet => "quickset",
        Source::Preset => "preset",
        Source::Seq => "seq",
        Source::Aux => "aux",
        Source::Http => "http",
        Source::EStop => "estop",
        Source::Connect => "connect",
        Source::PendingOff => "pending_off",
        Source::Exit => "exit",
    }
}

fn opt3(x: Option<f64>) -> String {
    x.map_or_else(String::new, milli3)
}

impl App {
    pub fn new(
        ctx: &egui::Context,
        settings: Settings,
        notices: Vec<LoadNotice>,
        sim: Option<SimDevice>,
        dev: DeviceHandle,
        data: DataHandle,
    ) -> Self {
        let cali = Cali::from_settings(&settings);
        let sp = SetpointModel::new(settings.last_vset, settings.last_iset);
        let ids: IdGen = Arc::new(1.into());
        let (tap_tx, tap_rx) = unbounded();
        data.send(DataCmd::Tap(tap_tx));
        let rc = ctx.clone();
        let aux = auxfn::spawn(
            dev.sender(),
            ids.clone(),
            data.snapshot.clone(),
            data.cmd.clone(),
            tap_rx,
            Box::new(move || rc.request_repaint()),
        );
        let (http_tx, http_rx) = unbounded();
        let (dark, pal) = Palette::resolve(&settings.theme, &settings.color_palette);
        let mut app = Self {
            sim,
            http: None,
            aux,
            dev,
            data,
            ids,
            cali,
            link: LinkState::Disconnected,
            link_error: None,
            identity: None,
            stats: CommStats::default(),
            sp,
            out: OutputState::default(),
            on_after_set: None,
            status: StatusDisplay::default(),
            last_sample: None,
            sample_times: VecDeque::new(),
            sample_rate_hz: 0.0,
            buffer: SampleBuffer::new(settings.data_pts as usize),
            graph: GraphView::default(),
            toasts: Vec::new(),
            power_dialog: None,
            graphics: None,
            confirm_on: false,
            exit: None,
            allow_close: false,
            quit_now: false,
            modal_notice: None,
            dark,
            pal,
            title: String::new(),
            persist: true,
            recording: None,
            saved_files: None,
            aux_status: AuxStatus::default(),
            aux_ui: AuxUi::new(&settings.aux.batsim.curve),
            confirm_aux: None,
            result_win: None,
            float: FloatState::default(),
            analyses: Vec::new(),
            next_window: 1,
            http_view: Arc::new(RwLock::new(UiView::default())),
            http_tx,
            http_rx,
            http_wait: HashMap::new(),
            log_ctx: LogCtx::default(),
            last_error: None,
            ctx: ctx.clone(),
            settings,
        };
        app.apply_theme();
        app.dev.send(Cmd::GuardLimits(app.settings.guard_limits()));
        for n in notices {
            match n {
                LoadNotice::LoadFailed(e) => app.toast(ToastKind::Warn, tf("settings_load_failed", &[("err", &e)])),
                LoadNotice::SafetyReset => app.modal_notice = Some(t("safety_settings_reset").into()),
                LoadNotice::PresetsFixed => app.toast(ToastKind::Warn, t("presets_fixed")),
            }
        }
        app.restart_http();
        if app.settings.auto_connect {
            app.dev.send(Cmd::Connect { auto: true });
        }
        app
    }

    pub fn toast(&mut self, kind: ToastKind, text: impl Into<String>) {
        let text = text.into();
        match kind {
            ToastKind::Error => {
                log::error!("{text}");
                self.last_error = Some(text.clone());
            }
            ToastKind::Warn => log::warn!("{text}"),
            ToastKind::Info => log::info!("{text}"),
        }
        let d = if kind == ToastKind::Info { TOAST_INFO } else { TOAST_ERR };
        self.toasts.push(Toast { text, kind, until: Instant::now() + d });
    }

    fn alloc_id(&mut self) -> CmdId {
        auxfn::next_id(&self.ids)
    }

    pub fn connected(&self) -> bool {
        matches!(self.link, LinkState::Connected { .. })
    }

    pub fn aux_running(&self) -> bool {
        self.aux_status.running.is_some()
    }

    /// 输出时锁定参数：用户发起的设定修改被拒绝
    pub fn setpoint_locked(&self) -> bool {
        self.aux_running() || (self.settings.lock_when_output && self.out.view.may_be_on())
    }

    pub fn save_settings(&mut self) {
        if let Some(v) = self.sp.edit_value(Qty::Voltage) {
            self.settings.last_vset = v.clamp(0.8, 24.0);
        }
        if let Some(i) = self.sp.edit_value(Qty::Current) {
            self.settings.last_iset = i.clamp(0.5, 5.0);
        }
        if !self.persist {
            return;
        }
        if let Err(e) = self.settings.save() {
            self.toast(ToastKind::Error, tf("settings_save_failed", &[("err", &e.to_string())]));
        }
    }

    /// 让设置生效但不写盘（图形设置拖动滑块时每帧调用）
    pub fn apply_runtime(&mut self) {
        self.cali = Cali::from_settings(&self.settings);
        self.dev.send(Cmd::Config(self.settings.worker_config()));
        self.dev.send(Cmd::GuardLimits(self.settings.guard_limits()));
        self.data.send(DataCmd::Display(display_cfg(&self.settings)));
        if !self.aux_running() {
            self.buffer.set_cap(self.settings.data_pts as usize);
        }
        self.apply_theme();
    }

    pub fn apply_settings(&mut self) {
        self.apply_runtime();
        self.save_settings();
    }

    pub fn apply_theme(&mut self) {
        let (dark, pal) = Palette::resolve(&self.settings.theme, &self.settings.color_palette);
        self.dark = dark;
        self.pal = pal;
        self.ctx.set_theme(if dark { egui::Theme::Dark } else { egui::Theme::Light });
        let aa = self.settings.antialias;
        self.ctx.tessellation_options_mut(|o| o.feathering = aa);
    }

    /// 按设置启动 / 停止本地 HTTP 接口
    pub fn restart_http(&mut self) {
        self.http = None;
        if !self.settings.http_enabled {
            return;
        }
        if self.settings.http_token.len() < 16 {
            self.settings.http_token = crate::config::new_token();
            self.save_settings();
        }
        let rc = self.ctx.clone();
        match crate::http::start(
            self.settings.http_port,
            self.settings.http_token.clone(),
            self.data.snapshot.clone(),
            self.http_view.clone(),
            self.http_tx.clone(),
            Box::new(move || rc.request_repaint()),
        ) {
            Ok(s) => self.http = Some(s),
            Err(e) => self.toast(ToastKind::Error, tf("http_start_failed", &[("err", &e)])),
        }
    }

    pub fn http_active(&self) -> bool {
        self.http.is_some()
    }

    // ---------- 用户操作 ----------

    pub fn connect(&mut self) {
        self.link_error = None;
        self.dev.send(Cmd::Connect { auto: false });
    }

    /// 断开：AUX 运行中先急停
    pub fn disconnect(&mut self) {
        if self.aux_running() {
            self.estop("user");
        }
        self.dev.send(Cmd::Disconnect);
    }

    pub fn zero_energy(&mut self) {
        self.data.send(DataCmd::Zero);
        self.status.energy_j = 0.0;
        self.status.avg_power_w = None;
    }

    /// 急停：停 AUX → 清空待发 → 关输出（IO 线程最多重试 5 次）
    pub fn estop(&mut self, src: &str) {
        self.aux.send(AuxCmd::Stop(StopReason::EStop));
        self.aux_status.running = None;
        self.dev.send(Cmd::EStop);
        self.data.log("ESTOP", &[("src", src.to_string())]);
    }

    /// 输出按钮：「未知」或可能为开时动作是关输出；AUX 运行中关输出等于急停
    pub fn click_output(&mut self) {
        if self.aux_running() {
            self.estop("user");
        } else if self.out.view.may_be_on() {
            self.send_output(false, Source::Manual);
        } else if self.settings.output_warning {
            self.confirm_on = true;
        } else {
            self.turn_on();
        }
    }

    pub fn turn_on(&mut self) {
        let id = self.alloc_id();
        match self.sp.prepare_on(id, &self.settings.safety, &self.cali) {
            Commit::Send { mv, ma } => {
                self.on_after_set = Some(id);
                self.dev.send(Cmd::SetVI { id, volt_mv: mv, amp_ma: ma, source: Source::OnPrepare });
            }
            Commit::NoChange => {
                self.send_output(true, Source::Manual);
            }
            Commit::Rejected(e) => {
                let msg = self.set_error_text(&e);
                self.toast(ToastKind::Warn, msg);
            }
            Commit::NotConnected => {}
        }
    }

    pub fn send_output(&mut self, on: bool, source: Source) -> CmdId {
        let id = self.alloc_id();
        self.out.track(id, on);
        self.dev.send(Cmd::Output { id, on, source });
        id
    }

    pub fn commit(&mut self, q: Qty, source: Source) {
        if self.aux_running() {
            self.sp.follow_device(&self.cali);
            return;
        }
        if self.setpoint_locked() {
            self.toast(ToastKind::Warn, t("locked_when_output"));
            self.sp.restore(&self.cali);
            return;
        }
        let id = self.alloc_id();
        match self.sp.commit(q, id, &self.settings.safety, &self.cali) {
            Commit::Send { mv, ma } => self.dev.send(Cmd::SetVI { id, volt_mv: mv, amp_ma: ma, source }),
            Commit::Rejected(e) => {
                // 输入框本身已标红；快速设定输入过程中的中间值不弹提示
                if source != Source::QuickSet {
                    let msg = self.set_error_text(&e);
                    self.toast(ToastKind::Warn, msg);
                }
            }
            Commit::NoChange | Commit::NotConnected => {}
        }
    }

    /// 预设：两个量一帧下发；超出安全边界拒绝
    pub fn apply_preset(&mut self, k: usize) {
        let Some(&[v, i]) = self.settings.presets.get(&k.to_string()) else { return };
        if self.aux_running() {
            self.toast(ToastKind::Warn, t("aux_locked"));
            return;
        }
        if self.setpoint_locked() {
            self.toast(ToastKind::Warn, t("locked_when_output"));
            return;
        }
        let lim = self.settings.safety;
        for (f, x) in [(Field::Voltage, v), (Field::Current, i)] {
            if let Err(LimitError::OutOfSafety { field, min, max, .. }) = lim.check(f, x) {
                let msg = tf(
                    "preset_rejected",
                    &[
                        ("n", &k.to_string()),
                        ("field", field_name(field)),
                        ("min", &trim_num(min)),
                        ("max", &trim_num(max)),
                        ("unit", field.qty().unit()),
                    ],
                );
                self.toast(ToastKind::Warn, msg);
                return;
            }
        }
        let id = self.alloc_id();
        match self.sp.commit_both(id, v, i, &lim, &self.cali) {
            Commit::Send { mv, ma } => {
                self.dev.send(Cmd::SetVI { id, volt_mv: mv, amp_ma: ma, source: Source::Preset })
            }
            Commit::Rejected(e) => {
                let msg = self.set_error_text(&e);
                self.toast(ToastKind::Warn, msg);
            }
            Commit::NoChange | Commit::NotConnected => {}
        }
    }

    pub fn set_error_text(&self, e: &SetError) -> String {
        match e {
            SetError::Limit(le) => limit_error_text(le),
            SetError::Cali(OutOfHw { qty, value }) => {
                let hw = qty.hw_milli();
                tf(
                    "cali_out_of_hw",
                    &[
                        ("value", &milli3(*value)),
                        ("unit", qty.unit()),
                        ("min", &trim_num(f64::from(*hw.start()) / 1000.0)),
                        ("max", &trim_num(f64::from(*hw.end()) / 1000.0)),
                    ],
                )
            }
        }
    }

    // ---------- AUX ----------

    /// 启动前的提示（不阻止启动）
    fn aux_warnings(&mut self, job: &Job) {
        let max_hz = 1000.0 / f64::from(self.settings.setpoint_min_interval_ms.max(1));
        let freq = match job {
            Job::KeepPower(c) => Some(c.freq_hz),
            Job::Wave(c) => Some(c.freq_hz),
            Job::Battery(b) => Some(b.freq_hz),
            _ => None,
        };
        if let Some(f) = freq {
            let poll = f64::from(self.settings.poll_interval_ms);
            if poll > 1000.0 / f {
                self.toast(ToastKind::Warn, tf("aux_poll_slow", &[("poll", &poll.to_string()), ("hz", &trim_num(f))]));
            }
            if f > max_hz {
                self.toast(ToastKind::Info, tf("aux_freq_limited", &[("hz", &format!("{max_hz:.1}"))]));
            }
            if let Job::Wave(c) = job
                && c.period_s < 10.0 / f.min(max_hz)
            {
                self.toast(ToastKind::Warn, t("wave_period_too_short"));
            }
        }
        match job {
            Job::Seq(s) => {
                let n = s.steps.iter().filter(|x| x.outside(&self.settings.safety)).count();
                if n > 0 {
                    let action = if s.skip_oob { t("seq_oob_skip") } else { t("seq_oob_stop") };
                    self.toast(ToastKind::Warn, tf("seq_oob_warning", &[("n", &n.to_string()), ("action", action)]));
                }
            }
            Job::Battery(b) => {
                let s = f64::from(b.cells);
                if b.curve.min_v() * s - b.r_ohm * s * self.settings.safety.i_max < self.settings.safety.v_min {
                    self.toast(ToastKind::Warn, t("bat_low_clamp"));
                }
            }
            _ => {}
        }
    }

    /// 启动 AUX：锁定检查、提示，输出关着且开了输出警告时先确认
    pub fn start_aux(&mut self, job: Job) {
        if !self.connected() || self.aux_running() {
            return;
        }
        if self.setpoint_locked() {
            self.toast(ToastKind::Warn, t("locked_when_output"));
            return;
        }
        self.aux_warnings(&job);
        let req = StartReq {
            job,
            safety: self.settings.safety,
            cali: self.cali,
            output_on: self.out.view.may_be_on(),
            set_interval: Duration::from_millis(u64::from(self.settings.setpoint_min_interval_ms)),
        };
        if !req.output_on && self.settings.output_warning {
            self.confirm_aux = Some(Box::new(req));
        } else {
            self.send_aux(req);
        }
    }

    pub fn send_aux(&mut self, req: StartReq) {
        let kind = req.job.kind();
        self.confirm_aux = None;
        self.aux.send(AuxCmd::Start(Box::new(req)));
        self.aux_status.running = Some(kind);
        self.sp.follow_device(&self.cali);
    }

    pub fn stop_aux(&mut self) {
        self.aux.send(AuxCmd::Stop(StopReason::User));
    }

    fn on_aux_notice(&mut self, n: AuxNotice) {
        match n {
            AuxNotice::Stopped(kind, reason) => {
                let name = t(kind_key(kind));
                match reason {
                    StopReason::Finished => self.toast(ToastKind::Info, tf("aux_finished", &[("name", name)])),
                    StopReason::SendFailed => self.toast(ToastKind::Error, t("aux_stopped_send_failed")),
                    StopReason::SeqRejected(line) => {
                        self.toast(ToastKind::Error, tf("seq_line_rejected", &[("line", &line.to_string())]))
                    }
                    StopReason::BatteryCutoff => self.toast(ToastKind::Info, t("bat_cutoff")),
                    StopReason::LinkLost => self.toast(ToastKind::Warn, tf("aux_stopped_link", &[("name", name)])),
                    StopReason::User | StopReason::EStop | StopReason::Shutdown => {}
                }
                self.sp.follow_device(&self.cali);
            }
            AuxNotice::ClampedFirst(_) => self.toast(ToastKind::Warn, tf("runtime_clamped", &[("n", "1")])),
            AuxNotice::SeqSkipped(line) => {
                self.toast(ToastKind::Warn, tf("seq_line_skipped", &[("line", &line.to_string())]))
            }
            AuxNotice::Stalled => self.toast(ToastKind::Warn, t("aux_read_stalled")),
        }
    }

    // ---------- 录制 / 导出 / 分析 ----------

    pub fn start_record(&mut self) {
        let id = self.identity.clone();
        let header = LogHeader {
            model: id.as_ref().map_or("?".into(), |i| i.model.clone()),
            serial: id.as_ref().map_or("?".into(), |i| i.serial.clone()),
            firmware: id.as_ref().map_or("?".into(), |i| i.firmware.clone()),
            poll_interval_ms: self.settings.poll_interval_ms,
            safety: self.settings.safety,
            cali: self.cali,
        };
        let s = &self.settings;
        let cfg = RecordCfg {
            dir: s.record_dir(),
            csv: s.record_csv,
            debug_columns: s.debug_columns,
            split_minutes: s.log_split_minutes,
            split_mb: s.log_split_mb,
            flush_ms: s.log_flush_ms,
            header,
        };
        self.data.send(DataCmd::StartRecord(Box::new(cfg)));
    }

    pub fn stop_record(&mut self) {
        self.data.send(DataCmd::StopRecord);
    }

    pub fn dump_buffer(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .set_title(t("dump_dialog"))
            .add_filter(t("csv_filter"), &["csv"])
            .set_file_name("iot_power_buffer_dump.csv")
            .save_file()
        else {
            return;
        };
        match crate::analysis::write_points_csv(&path, self.buffer.range(0, self.buffer.len())) {
            Ok(()) => self.toast(ToastKind::Info, format!("{} {}", t("dump_done"), path.display())),
            Err(e) => self.toast(ToastKind::Error, format!("{}: {e}", path.display())),
        }
    }

    pub fn open_analysis_dialog(&mut self) {
        if let Some(paths) = crate::ui::analysis_window::pick() {
            self.open_analysis(paths);
        }
    }

    pub fn open_analysis(&mut self, paths: Vec<PathBuf>) {
        let id = self.next_window;
        self.next_window += 1;
        self.analyses.push(AnalysisWindow::open(id, paths, self.ctx.clone()));
    }

    pub fn log_safety(&self) {
        let l = self.settings.safety;
        self.data.log(
            "SAFETY",
            &[
                ("v_min", milli3(l.v_min)),
                ("v_max", milli3(l.v_max)),
                ("i_min", milli3(l.i_min)),
                ("i_max", milli3(l.i_max)),
            ],
        );
    }

    /// 急停热键：Esc 只在 AUX 运行时触发（不吞掉按键，弹窗照常关闭）；Ctrl+Shift+X 连接后任何时候（仅窗口有焦点时）
    pub fn hotkeys(&mut self, ctx: &egui::Context) {
        let (esc, ctrl_shift_x) = ctx.input(|i| {
            (i.key_pressed(egui::Key::Escape), i.modifiers.ctrl && i.modifiers.shift && i.key_pressed(egui::Key::X))
        });
        if (esc && self.aux_running()) || (ctrl_shift_x && self.connected()) {
            self.estop("user");
        }
    }

    pub fn quit(&mut self) {
        self.exit = None;
        self.quit_now = true;
    }

    pub fn exit_turn_off(&mut self) {
        if self.aux_running() {
            self.aux.send(AuxCmd::Stop(StopReason::Shutdown));
        }
        let id = self.send_output(false, Source::Exit);
        self.exit = Some(ExitPhase::TurningOff { id, since: Instant::now() });
    }

    // ---------- HTTP 写请求 ----------

    fn handle_http(&mut self, req: HttpReq) {
        match req {
            HttpReq::Setpoint { v, i, reply } => {
                let r = self.http_setpoint(v, i);
                match r {
                    Ok(Some(id)) => {
                        self.http_wait.insert(id, reply);
                    }
                    Ok(None) => {
                        let _ = reply.send(HttpReply::ok(json!({ "result": "unchanged" })));
                    }
                    Err(e) => {
                        let _ = reply.send(e);
                    }
                }
            }
            HttpReq::Output { on, reply } => {
                if !self.connected() {
                    let _ = reply.send(HttpReply::err(409, "not connected"));
                } else if !on {
                    if self.aux_running() {
                        self.estop("http");
                        let _ = reply.send(HttpReply::ok(json!({ "result": "estop" })));
                    } else {
                        let id = self.send_output(false, Source::Http);
                        self.http_wait.insert(id, reply);
                    }
                } else if !self.settings.http_allow_output_on {
                    let _ = reply.send(HttpReply::err(403, "remote output-on is not allowed"));
                } else if self.aux_running() {
                    let _ = reply.send(HttpReply::err(409, "an AUX function is running"));
                } else if self.sp.device_out_of_limits(&self.settings.safety, &self.cali) {
                    let _ = reply.send(HttpReply::err(400, "device setpoint is outside safety limits"));
                } else {
                    let id = self.send_output(true, Source::Http);
                    self.http_wait.insert(id, reply);
                }
            }
        }
    }

    fn http_setpoint(&mut self, v: Option<f64>, i: Option<f64>) -> Result<Option<CmdId>, HttpReply> {
        if !self.connected() {
            return Err(HttpReply::err(409, "not connected"));
        }
        if self.aux_running() {
            return Err(HttpReply::err(409, "an AUX function is running"));
        }
        if self.setpoint_locked() {
            return Err(HttpReply::err(409, t("locked_when_output")));
        }
        let (Some(dv), Some(di)) =
            (self.sp.device_value(Qty::Voltage, &self.cali), self.sp.device_value(Qty::Current, &self.cali))
        else {
            return Err(HttpReply::err(409, "device setpoint unknown"));
        };
        let (v, i) = (v.unwrap_or(dv), i.unwrap_or(di));
        let lim = self.settings.safety;
        for (f, x) in [(Field::Voltage, v), (Field::Current, i)] {
            if let Err(e) = lim.check(f, x) {
                let (min, max) = lim.range(f.qty());
                return Err(HttpReply {
                    status: 400,
                    body: json!({
                        "error": limit_error_text(&e),
                        "field": if f == Field::Voltage { "voltage" } else { "current" },
                        "min": min,
                        "max": max,
                    }),
                });
            }
        }
        let id = self.alloc_id();
        match self.sp.commit_both(id, v, i, &lim, &self.cali) {
            Commit::Send { mv, ma } => {
                self.dev.send(Cmd::SetVI { id, volt_mv: mv, amp_ma: ma, source: Source::Http });
                Ok(Some(id))
            }
            Commit::NoChange => Ok(None),
            Commit::NotConnected => Err(HttpReply::err(409, "not connected")),
            Commit::Rejected(e) => Err(HttpReply::err(400, &self.set_error_text(&e))),
        }
    }

    fn http_result(&mut self, id: CmdId, result: &Result<Ack, CmdError>) {
        let Some(reply) = self.http_wait.remove(&id) else { return };
        let r = match result {
            Ok(Ack::Confirmed | Ack::Acked) => HttpReply::ok(json!({ "result": "ok" })),
            Ok(Ack::Superseded) => HttpReply::err(409, "superseded"),
            Ok(Ack::Deferred) => {
                self.http_wait.insert(id, reply);
                return;
            }
            Err(e) => HttpReply::err(502, &cmd_error_text(e)),
        };
        let _ = reply.send(r);
    }

    fn publish_view(&mut self) {
        let out = match self.out.view {
            OutputView::Acked(true) => "on",
            OutputView::Acked(false) => "off",
            OutputView::Stale(true) => "on_unconfirmed",
            OutputView::Stale(false) => "off_unconfirmed",
            OutputView::Unknown => "unknown",
            OutputView::NoInfo => "n/a",
        };
        let l = self.settings.safety;
        let view = UiView {
            output: out,
            vset: self.sp.device_value(Qty::Voltage, &self.cali),
            iset: self.sp.device_value(Qty::Current, &self.cali),
            aux: self.aux_status.running.map(Kind::tag),
            safety: [l.v_min, l.v_max, l.i_min, l.i_max],
        };
        *self.http_view.write().expect("http view") = view;

        let ctx = LogCtx {
            out: match self.out.view {
                OutputView::Acked(true) => "1",
                OutputView::Acked(false) => "0",
                OutputView::Stale(true) => "1?",
                OutputView::Stale(false) => "0?",
                _ => "U",
            }
            .into(),
            vset: self.sp.device_value(Qty::Voltage, &self.cali),
            iset: self.sp.device_value(Qty::Current, &self.cali),
        };
        if ctx != self.log_ctx {
            self.log_ctx = ctx.clone();
            self.data.send(DataCmd::LogCtx(ctx));
        }
    }

    // ---------- 事件 ----------

    pub fn handle_events(&mut self, ctx: &egui::Context) {
        while let Ok(ev) = self.data.events.try_recv() {
            self.handle_event(ev);
        }
        while let Ok(n) = self.data.notices.try_recv() {
            match n {
                DataNotice::RecordStarted(p) => self.recording = Some((p, Instant::now())),
                DataNotice::Saved(files) => {
                    self.recording = None;
                    if !files.is_empty() {
                        self.saved_files = Some(files);
                    }
                }
                DataNotice::RecordFailed(e) => {
                    self.recording = None;
                    self.toast(ToastKind::Error, tf("record_failed", &[("err", &e)]));
                }
            }
        }
        self.aux_status = self.aux.status();
        while let Ok(n) = self.aux.notices.try_recv() {
            self.on_aux_notice(n);
        }
        while let Ok(r) = self.http_rx.try_recv() {
            self.handle_http(r);
        }

        let mut latest_echo = None;
        while let Ok(p) = self.data.samples.try_recv() {
            self.status.push(&p);
            self.buffer.push(p.sample.t, p.v, p.i_a);
            self.sample_times.push_back(p.sample.t);
            while self.sample_times.len() > RATE_WINDOW + 1 {
                self.sample_times.pop_front();
            }
            latest_echo = Some((p.sample.meas.set_voltage_mv, p.sample.meas.set_current_ma));
            self.last_sample = Some(p);
        }
        if let Some((mv, ma)) = latest_echo
            && self.connected()
        {
            let before = self.sp.device_raw();
            self.sp.on_echo(mv, ma, &self.cali);
            let after = self.sp.device_raw();
            if before.is_some() && after != before && !self.aux_running() {
                self.data.log(
                    "SETPOINT",
                    &[
                        ("vset", opt3(self.sp.device_value(Qty::Voltage, &self.cali))),
                        ("iset", opt3(self.sp.device_value(Qty::Current, &self.cali))),
                        ("src", "device".into()),
                        ("result", "echo".into()),
                    ],
                );
            }
        }
        if self.aux_running() {
            self.sp.follow_device(&self.cali);
        }
        if let (Some(first), Some(last)) = (self.sample_times.front(), self.sample_times.back()) {
            let n = self.sample_times.len().saturating_sub(1);
            let span = last.saturating_duration_since(*first).as_secs_f64();
            self.sample_rate_hz = if n > 0 && span > 0.0 { n as f64 / span } else { 0.0 };
        }
        self.status.flush(Duration::from_secs_f64(1.0 / self.settings.state_fps));
        self.toasts.retain(|t| t.until > Instant::now());
        self.publish_view();

        if let Some(ExitPhase::TurningOff { since, .. }) = self.exit
            && since.elapsed() > EXIT_OFF_TIMEOUT
        {
            self.exit = Some(ExitPhase::OffFailed);
        }

        let title = match &self.identity {
            Some(id) if self.connected() => {
                format!("{} v{} - {} {}", crate::i18n::window_title(), env!("CARGO_PKG_VERSION"), id.model, id.serial)
            }
            _ => format!("{} v{}", crate::i18n::window_title(), env!("CARGO_PKG_VERSION")),
        };
        if title != self.title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.title = title;
        }
    }

    fn handle_event(&mut self, ev: Event) {
        match ev {
            Event::Link(s) => {
                log::info!("link: {s:?}");
                self.out.on_link(s);
                match s {
                    LinkState::Disconnected => {
                        self.sp.on_disconnected();
                        self.on_after_set = None;
                        self.status = StatusDisplay::default();
                        self.last_sample = None;
                        self.sample_times.clear();
                        self.sample_rate_hz = 0.0;
                        self.buffer.mark_gap();
                    }
                    LinkState::Reconnecting { .. } => {
                        self.buffer.mark_gap();
                        self.on_after_set = None;
                        self.sample_times.clear();
                        self.sample_rate_hz = 0.0;
                    }
                    LinkState::Connected { origin: LinkOrigin::FirstConnect } => {
                        self.status = StatusDisplay::default();
                        self.buffer.clear();
                        self.graph.hold = None;
                        self.graph.reset_window();
                        if self.settings.log_auto_start && self.recording.is_none() {
                            self.start_record();
                        }
                    }
                    _ => {}
                }
                self.link = s;
            }
            Event::Identity(id) => {
                let last = &self.settings.device_serial;
                if !last.is_empty() && *last != id.serial {
                    let msg = tf("serial_changed", &[("sn", &id.serial), ("last", last)]);
                    self.toast(ToastKind::Warn, msg);
                }
                if self.settings.device_serial != id.serial {
                    self.settings.device_serial = id.serial.clone();
                    self.save_settings();
                }
                self.identity = Some(id);
            }
            Event::Error(e) => {
                log::warn!("device error: {e:?}");
                self.link_error = Some(e);
            }
            Event::Sample(_) => {}
            Event::CmdResult { id, cmd, source, result, output_on } => {
                self.handle_result(id, cmd, source, result, output_on)
            }
            Event::PendingOff(b) => self.out.pending_off = b,
            Event::GuardTriggered { from, to, .. } => {
                let f = |(mv, ma): (u16, u16)| {
                    format!("{} V / {} A", milli3(f64::from(mv) / 1000.0), milli3(f64::from(ma) / 1000.0))
                };
                self.toast(ToastKind::Error, tf("guard_triggered", &[("from", &f(from)), ("to", &f(to))]));
            }
            Event::Stats(s) => self.stats = s,
        }
    }

    fn log_result(&self, cmd: u8, source: Source, result: &Result<Ack, CmdError>, output_on: Option<bool>) {
        let src = src_tag(source);
        let set = cmd == dc3005u_proto::cmd::SET_VI;
        match result {
            Ok(Ack::Confirmed) if set && source != Source::Aux => self.data.log(
                "SETPOINT",
                &[
                    ("vset", opt3(self.sp.device_value(Qty::Voltage, &self.cali))),
                    ("iset", opt3(self.sp.device_value(Qty::Current, &self.cali))),
                    ("src", src.into()),
                    ("result", "confirmed".into()),
                ],
            ),
            Ok(Ack::Acked) if !set => self.data.log(
                "OUTPUT",
                &[
                    ("on", if output_on == Some(true) { "1" } else { "0" }.into()),
                    ("src", src.into()),
                    ("result", "acked".into()),
                ],
            ),
            Err(e) if !matches!(e, CmdError::NotConnected | CmdError::PendingOff) => {
                let mut f = vec![
                    ("cmd", format!("0x{cmd:02x}")),
                    ("src", src.to_string()),
                    ("reason", format!("{e:?}")),
                    ("retries", self.settings.cmd_retries.to_string()),
                ];
                if set {
                    f.push((
                        "rollback",
                        format!(
                            "vset:{} iset:{}",
                            opt3(self.sp.device_value(Qty::Voltage, &self.cali)),
                            opt3(self.sp.device_value(Qty::Current, &self.cali))
                        ),
                    ));
                }
                self.data.log("CMD_FAIL", &f);
            }
            _ => {}
        }
    }

    fn handle_result(
        &mut self,
        id: CmdId,
        cmd: u8,
        source: Source,
        result: Result<Ack, CmdError>,
        output_on: Option<bool>,
    ) {
        if cmd == dc3005u_proto::cmd::SET_VI {
            let outcome = self.sp.on_result(id, &result, &self.cali);
            self.log_result(cmd, source, &result, output_on);
            self.http_result(id, &result);
            let is_on_prep = self.on_after_set == Some(id);
            match outcome {
                Some(Outcome::Confirmed) if is_on_prep => {
                    self.on_after_set = None;
                    self.send_output(true, Source::Manual);
                }
                Some(Outcome::Failed(e)) => {
                    if is_on_prep {
                        self.on_after_set = None;
                        self.toast(ToastKind::Error, format!("{}：{}", t("on_setpoint_failed"), cmd_error_text(&e)));
                    } else if !matches!(e, CmdError::NotConnected) {
                        let v = self.sp.device_value(Qty::Voltage, &self.cali).map(milli3).unwrap_or_default();
                        let i = self.sp.device_value(Qty::Current, &self.cali).map(milli3).unwrap_or_default();
                        let msg = tf("cmd_set_failed", &[("reason", &cmd_error_text(&e)), ("v", &v), ("i", &i)]);
                        self.toast(ToastKind::Error, msg);
                    }
                }
                _ => {
                    if is_on_prep && result == Ok(Ack::Superseded) {
                        self.on_after_set = None;
                    }
                }
            }
            return;
        }
        if cmd == dc3005u_proto::cmd::OUTPUT {
            self.log_result(cmd, source, &result, output_on);
            self.http_result(id, &result);
            if let Some(ExitPhase::TurningOff { id: exit_id, .. }) = self.exit
                && exit_id == id
            {
                if matches!(result, Ok(Ack::Acked)) {
                    self.exit = None;
                    self.quit_now = true;
                } else {
                    self.exit = Some(ExitPhase::OffFailed);
                }
            }
            if source == Source::EStop && result == Ok(Ack::Acked) {
                self.toast(ToastKind::Info, t("estop_done"));
            }
            match self.out.on_result(id, source, output_on, &result) {
                Some(OutputNotice::OnFailed(e)) => {
                    self.toast(ToastKind::Error, tf("cmd_on_failed", &[("reason", &cmd_error_text(&e))]));
                }
                Some(OutputNotice::OffFailed) => {
                    let key = if source == Source::EStop { "estop_failed" } else { "cmd_off_failed" };
                    self.toast(ToastKind::Error, t(key));
                }
                None => {}
            }
        }
    }
}

pub fn kind_key(k: Kind) -> &'static str {
    match k {
        Kind::KeepPower => "tab_keep_power",
        Kind::Sweep => "tab_sweep",
        Kind::Wave => "tab_wavegen",
        Kind::Seq => "tab_seq",
        Kind::Battery => "tab_batsim",
    }
}

pub fn display_cfg(s: &Settings) -> DisplayCfg {
    DisplayCfg { cali: Cali::from_settings(s), v_threshold: s.v_threshold, i_threshold: s.i_threshold }
}

/// 去掉多余的 0：24.000 → 24，0.800 → 0.8
pub fn trim_num(x: f64) -> String {
    let s = format!("{x:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    s.to_string()
}

pub fn cmd_error_text(e: &CmdError) -> String {
    match e {
        CmdError::Timeout => t("cmd_timeout").into(),
        CmdError::BadResponse(d) => format!("{}（{d}）", t("cmd_bad_response")),
        CmdError::Usb(d) => format!("{}：{d}", t("cmd_usb_error")),
        CmdError::EchoMismatch => t("cmd_echo_mismatch").into(),
        CmdError::NotConnected => t("cmd_not_connected").into(),
        CmdError::PendingOff => t("pending_off_queued").into(),
    }
}

pub fn field_name(f: dc3005u_proto::Field) -> &'static str {
    use dc3005u_proto::Field::*;
    match f {
        VMin => t("safety_v_min"),
        VMax => t("safety_v_max"),
        IMin => t("safety_i_min"),
        IMax => t("safety_i_max"),
        Voltage => t("set_voltage"),
        Current => t("set_current"),
    }
}

pub fn limit_error_text(e: &LimitError) -> String {
    use dc3005u_proto::Field::*;
    match *e {
        LimitError::NotFinite { .. } => t("safety_not_number").into(),
        LimitError::OutOfHw { field, .. } => t(match field {
            VMin => "safety_v_min_hw",
            VMax => "safety_v_max_hw",
            IMin => "safety_i_min_hw",
            _ => "safety_i_max_hw",
        })
        .into(),
        LimitError::Order { qty } => t(if qty == Qty::Voltage { "safety_v_order" } else { "safety_i_order" }).into(),
        LimitError::OutOfSafety { field, min, max, .. } => tf(
            "setpoint_out_of_limits",
            &[
                ("field", field_name(field)),
                ("min", &trim_num(min)),
                ("max", &trim_num(max)),
                ("unit", field.qty().unit()),
            ],
        ),
    }
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.handle_events(ctx);
        self.hotkeys(ctx);
        if std::env::var_os("IOT_DBG_PANEL").is_some() {
            if matches!(self.link, LinkState::Disconnected) && self.sim.is_some() {
                self.connect();
            }
            let w = egui::containers::panel::PanelState::load(ctx, egui::Id::new("left")).map(|s| s.size().x);
            log::warn!("DBG left={w:?} screen={:?}", ctx.content_rect().width());
        }

        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("txt")))
                .collect()
        });
        if !dropped.is_empty() {
            self.open_analysis(dropped);
        }

        if self.quit_now {
            self.quit_now = false;
            self.allow_close = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        } else if ctx.input(|i| i.viewport().close_requested()) && !self.allow_close && self.out.view.may_be_on() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            if self.exit.is_none() {
                self.exit = Some(ExitPhase::Ask);
            }
        }

        if let Some(r) = ctx.input(|i| i.viewport().inner_rect) {
            let pos = ctx.input(|i| i.viewport().outer_rect).map(|o| o.min);
            self.settings.window = Some(crate::config::WindowGeom {
                w: r.width(),
                h: r.height(),
                x: pos.map(|p| p.x),
                y: pos.map(|p| p.y),
            });
        }

        let busy = !matches!(self.link, LinkState::Disconnected)
            || !self.toasts.is_empty()
            || self.recording.is_some()
            || self.aux_running();
        if busy {
            ctx.request_repaint_after(Duration::from_secs_f64(1.0 / self.settings.state_fps));
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        crate::ui::draw(self, ui);
    }

    fn on_exit(&mut self) {
        self.aux.send(AuxCmd::Stop(StopReason::Shutdown));
        self.stop_record();
        self.save_settings();
    }
}

/// 无窗口跑真实 UI 代码 + 模拟设备，走一遍主流程
#[cfg(test)]
mod smoke {
    use super::*;
    use crate::output::OutputView;
    use crate::ui::power_dialog::PowerDialog;
    use dc3005u_usb::WorkerConfig;

    fn rig() -> (App, SimDevice, egui::Context) {
        let ctx = egui::Context::default();
        crate::theme::install_cjk_font(&ctx);
        let sim = SimDevice::new();
        let cfg = WorkerConfig {
            poll_interval: Duration::from_millis(20),
            resp_timeout_poll: Duration::from_millis(50),
            resp_timeout_cmd: Duration::from_millis(50),
            setpoint_min_interval: Duration::from_millis(10),
            ..WorkerConfig::default()
        };
        let (tx, rx) = crossbeam_channel::unbounded();
        let dev = dc3005u_usb::spawn(Box::new(sim.connector()), cfg, tx);
        let settings = Settings { setpoint_min_interval_ms: 10, poll_interval_ms: 20, ..Settings::default() };
        let data = crate::data::spawn(rx, display_cfg(&settings), Box::new(|| {}));
        let mut app = App::new(&ctx, settings, Vec::new(), Some(sim.clone()), dev, data);
        app.persist = false;
        (app, sim, ctx)
    }

    fn frame(app: &mut App, ctx: &egui::Context) {
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            app.handle_events(ui.ctx());
            crate::ui::draw(app, ui);
        });
        out.textures_delta.clear();
    }

    fn until(app: &mut App, ctx: &egui::Context, what: &str, mut cond: impl FnMut(&App) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(8);
        while !cond(app) {
            assert!(
                Instant::now() < deadline,
                "timed out waiting for: {what} (link {:?}, output {:?}, aux {:?})",
                app.link,
                app.out.view,
                app.aux_status.running
            );
            frame(app, ctx);
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn connect(app: &mut App, ctx: &egui::Context) {
        app.connect();
        until(app, ctx, "connected", |a| {
            a.connected()
                && a.sp.device_value(Qty::Voltage, &a.cali).is_some()
                && a.out.view == OutputView::Acked(false)
        });
    }

    #[test]
    fn m1_flow_against_sim() {
        let (mut app, sim, ctx) = rig();
        frame(&mut app, &ctx);

        // 连接：首次连接关输出，读数到达，编辑值取设备设定
        sim.state().output_on = true;
        app.connect();
        until(&mut app, &ctx, "connected with readings", |a| a.connected() && a.status.have);
        until(&mut app, &ctx, "output off on connect", |a| a.out.view == OutputView::Acked(false));
        assert!(!sim.state().output_on);
        assert_eq!(app.sp.text[0], "5.000");
        assert_eq!(app.identity.as_ref().map(|i| i.model.as_str()), Some("DC3005U"));

        // 设定电压：回显确认后设备设定更新
        app.sp.text[0] = "6".into();
        app.sp.on_user_edit(Qty::Voltage);
        app.commit(Qty::Voltage, Source::Manual);
        until(&mut app, &ctx, "setpoint confirmed", |a| a.sp.device_value(Qty::Voltage, &a.cali) == Some(6.0));
        assert_eq!(sim.state().set_mv, 6000);

        // 超出安全边界：拒绝，不下发
        let writes = sim.writes_of(dc3005u_proto::cmd::SET_VI).len();
        app.sp.text[0] = "30".into();
        app.commit(Qty::Voltage, Source::Manual);
        frame(&mut app, &ctx);
        assert_eq!(sim.writes_of(dc3005u_proto::cmd::SET_VI).len(), writes);
        assert!(!app.toasts.is_empty());
        app.sp.restore(&app.cali);

        // ON 前编辑值与设备设定不同：先下发 0x03，确认后再开输出
        app.sp.text[1] = "0.5".into();
        app.sp.on_user_edit(Qty::Current);
        app.click_output();
        until(&mut app, &ctx, "output on", |a| a.out.view == OutputView::Acked(true));
        {
            let s = sim.state();
            assert_eq!((s.set_ma, s.output_on), (500, true));
        }
        // 6 V / 10 Ω 被限流到 0.5 A
        until(&mut app, &ctx, "CC reading", |a| (a.status.i_a - 0.5).abs() < 1e-6);
        until(&mut app, &ctx, "energy grows", |a| a.status.energy_j > 0.0);

        // 打开对话框、浮窗跑几帧，确保绘制不出错
        app.power_dialog = Some(PowerDialog::new(&app.settings));
        app.graphics = Some(GraphicsDialog::new(&app.settings));
        app.float.open = true;
        frame(&mut app, &ctx);
        frame(&mut app, &ctx);
        app.power_dialog = None;
        app.graphics = None;
        app.float.open = false;

        // 拔线：重连中输出「未经确认」；点 OFF 记为 pending_off，重连后先关输出
        sim.unplug();
        until(&mut app, &ctx, "reconnecting", |a| matches!(a.link, LinkState::Reconnecting { .. }));
        assert_eq!(app.out.view, OutputView::Stale(true));
        app.send_output(false, Source::Manual);
        until(&mut app, &ctx, "pending off", |a| a.out.pending_off);
        frame(&mut app, &ctx);
        sim.plug();
        until(&mut app, &ctx, "pending off done", |a| !a.out.pending_off && a.out.view == OutputView::Acked(false));
        assert!(!sim.state().output_on);

        // 退出：输出可能为开时弹确认，选择关闭输出并退出
        app.click_output();
        until(&mut app, &ctx, "output on again", |a| a.out.view == OutputView::Acked(true));
        app.exit = Some(ExitPhase::Ask);
        frame(&mut app, &ctx);
        app.exit_turn_off();
        until(&mut app, &ctx, "exit off acked", |a| a.quit_now);
        assert!(!sim.state().output_on);
    }

    #[test]
    fn graph_buffers_and_breaks_at_reconnect() {
        use crate::buffer::Series;
        let (mut app, sim, ctx) = rig();
        app.connect();
        until(&mut app, &ctx, "connected", |a| a.connected());
        sim.state().output_on = true;
        until(&mut app, &ctx, "samples with current", |a| {
            a.buffer.latest(None).iter().filter(|p| p.i > 0.0).count() >= 5
        });

        for s in Series::ALL {
            app.graph.series = [s, Series::Resistance];
            frame(&mut app, &ctx);
        }
        app.graph.series = [Series::None, Series::None];
        frame(&mut app, &ctx);
        app.graph.series = [Series::Voltage, Series::Current];
        app.graph.full = true;
        frame(&mut app, &ctx);

        app.graph.hold = Some(app.buffer.latest(None));
        let held = app.graph.hold.as_ref().map(Vec::len);
        until(&mut app, &ctx, "buffer grows while held", |a| Some(a.buffer.latest(None).len()) > held);
        assert_eq!(app.graph.hold.as_ref().map(Vec::len), held);
        app.graph.hold = None;

        sim.unplug();
        until(&mut app, &ctx, "reconnecting", |a| matches!(a.link, LinkState::Reconnecting { .. }));
        sim.plug();
        until(&mut app, &ctx, "gap after reconnect", |a| a.buffer.latest(None).iter().any(|p| p.gap));
        frame(&mut app, &ctx);
    }

    #[test]
    fn narrowed_limits_block_on() {
        let (mut app, _sim, ctx) = rig();
        connect(&mut app, &ctx);
        app.settings.safety = dc3005u_proto::SafetyLimits { v_min: 1.0, v_max: 3.3, i_min: 0.5, i_max: 5.0 };
        app.apply_settings();
        frame(&mut app, &ctx);
        // 设备设定 5 V 超出新边界：ON 不下发
        app.click_output();
        frame(&mut app, &ctx);
        assert!(app.sp.device_out_of_limits(&app.settings.safety, &app.cali));
        assert_eq!(app.out.view, OutputView::Acked(false));
        // 超界预设被拒绝
        let writes = _sim.writes_of(dc3005u_proto::cmd::SET_VI).len();
        app.apply_preset(6);
        frame(&mut app, &ctx);
        assert_eq!(_sim.writes_of(dc3005u_proto::cmd::SET_VI).len(), writes);
    }

    #[test]
    fn preset_and_every_aux_page_draws() {
        let (mut app, sim, ctx) = rig();
        connect(&mut app, &ctx);
        app.apply_preset(1);
        until(&mut app, &ctx, "preset applied", |a| a.sp.device_value(Qty::Voltage, &a.cali) == Some(3.3));
        assert_eq!(sim.state().set_ma, 2000);
        for page in 0..6 {
            app.aux_ui.page = page;
            frame(&mut app, &ctx);
        }
        // 结果窗（扫描 / 电池曲线）与各弹窗也画一遍，缺文案会在 debug 下断言
        let sweep = crate::auxfn::SweepResult {
            target: Qty::Voltage,
            response: crate::buffer::Series::Current,
            points: (0..20).map(|k| [f64::from(k), f64::from(k * k) * 0.01]).collect(),
        };
        app.result_win = Some(ResultWindow::sweep(&sweep));
        frame(&mut app, &ctx);
        frame(&mut app, &ctx);
        app.result_win = Some(ResultWindow::battery(&crate::auxfn::battery::Curve::builtin()));
        frame(&mut app, &ctx);
        app.result_win = None;
        app.saved_files = Some(vec![PathBuf::from("/tmp/a.txt")]);
        frame(&mut app, &ctx);
        app.saved_files = None;
        app.last_error = Some("x".into());
        frame(&mut app, &ctx);
    }

    #[test]
    fn left_panel_width_is_stable() {
        let (mut app, _sim, ctx) = rig();
        connect(&mut app, &ctx);
        let input = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 800.0))),
            ..Default::default()
        };
        let width = |ctx: &egui::Context| {
            egui::containers::panel::PanelState::load(ctx, egui::Id::new("left")).map(|s| s.size().x)
        };
        for page in 0..6 {
            app.aux_ui.page = page;
            let mut widths = Vec::new();
            for _ in 0..30 {
                let mut out = ctx.run_ui(input(), |ui| {
                    app.handle_events(ui.ctx());
                    crate::ui::draw(&mut app, ui);
                });
                out.textures_delta.clear();
                widths.push(width(&ctx).expect("left panel state"));
            }
            let (first, last) = (widths[2], widths[29]);
            assert!((last - first).abs() < 1.0, "page {page}: left panel grew {first} -> {last}");
            assert!(last <= 1280.0 * 0.45 + 1.0, "page {page}: left panel too wide {last}");
        }
    }

    #[test]
    fn sequence_runs_merges_and_estop_stops_it() {
        use crate::auxfn::SeqJob;
        use crate::auxfn::sequence::Step;
        let (mut app, sim, ctx) = rig();
        connect(&mut app, &ctx);
        app.aux_ui.page = 4;
        let steps = vec![Step::SetV(4.0), Step::SetI(1.0), Step::Delay(50), Step::SetV(7.0), Step::Delay(100_000)];
        app.start_aux(Job::Seq(SeqJob { steps, looped: false, skip_oob: false }));
        until(&mut app, &ctx, "seq reaches 7 V with output on", |_| {
            let s = sim.state();
            s.set_mv == 7000 && s.output_on
        });
        assert_eq!(sim.state().set_ma, 1000);
        assert!(app.aux_running());
        // 设定框跟随设备设定且禁用
        until(&mut app, &ctx, "edit follows device", |a| a.sp.text[0] == "7.000");
        // Esc = 急停：停序列并关输出
        let mut input = egui::RawInput::default();
        input.events.push(egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        });
        let mut out = ctx.run_ui(input, |ui| {
            app.hotkeys(ui.ctx());
            crate::ui::draw(&mut app, ui);
        });
        out.textures_delta.clear();
        assert!(!app.aux_running());
        until(&mut app, &ctx, "estopped", |a| !a.aux_running() && a.out.view == OutputView::Acked(false));
        assert!(!sim.state().output_on);
    }

    #[test]
    fn wavegen_clamps_nothing_and_stops_on_user() {
        let (mut app, sim, ctx) = rig();
        connect(&mut app, &ctx);
        app.settings.aux.wavegen =
            crate::config::WaveCfg { kind: "square".into(), period_s: 0.2, high_v: 6.0, low_v: 3.0, freq_hz: 50.0 };
        app.aux_ui.page = 3;
        app.start_aux(Job::Wave(app.settings.aux.wavegen.clone()));
        until(&mut app, &ctx, "wave toggles", |_| sim.state().set_mv == 6000);
        until(&mut app, &ctx, "wave low", |_| sim.state().set_mv == 3000);
        assert_eq!(app.aux_status.clamped, 0);
        app.stop_aux();
        until(&mut app, &ctx, "stopped", |a| !a.aux_running());
        // 停止不关输出
        assert!(sim.state().output_on);
    }

    #[test]
    fn recording_writes_txt_and_csv() {
        let dir = std::env::temp_dir().join(format!("iot_power_rec_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (mut app, sim, ctx) = rig();
        app.settings.record_dir = dir.to_string_lossy().into_owned();
        connect(&mut app, &ctx);
        app.start_record();
        until(&mut app, &ctx, "recording", |a| a.recording.is_some());
        app.sp.text[0] = "6".into();
        app.sp.on_user_edit(Qty::Voltage);
        app.commit(Qty::Voltage, Source::Manual);
        until(&mut app, &ctx, "setpoint confirmed", |a| a.sp.device_value(Qty::Voltage, &a.cali) == Some(6.0));
        sim.unplug();
        until(&mut app, &ctx, "reconnecting", |a| matches!(a.link, LinkState::Reconnecting { .. }));
        sim.plug();
        until(&mut app, &ctx, "reconnected", |a| a.connected());
        std::thread::sleep(Duration::from_millis(200));
        frame(&mut app, &ctx);
        app.stop_record();
        until(&mut app, &ctx, "saved", |a| a.saved_files.is_some());
        let files = app.saved_files.clone().unwrap_or_default();
        assert!(files.iter().any(|p| p.extension().is_some_and(|e| e == "csv")));
        let txt = files.iter().find(|p| p.extension().is_some_and(|e| e == "txt")).expect("txt");
        let mut d = crate::analysis::LogData::default();
        crate::analysis::parse_text(&std::fs::read_to_string(txt).unwrap(), true, &mut d, |_| {}).unwrap();
        assert!(d.points.len() >= 3, "{} points", d.points.len());
        let has =
            |kind: &str, key: &str, val: &str| d.events.iter().any(|e| e.kind == kind && e.field(key) == Some(val));
        assert!(has("SETPOINT", "src", "manual"), "{:?}", d.events);
        assert!(has("LINK", "state", "lost") && has("LINK", "state", "restored"), "{:?}", d.events);
        assert!(d.points.iter().any(|p| p.gap));
        // 分析窗口能载入
        app.open_analysis(vec![txt.clone()]);
        until(&mut app, &ctx, "analysis loaded", |a| a.analyses.len() == 1);
        for _ in 0..20 {
            frame(&mut app, &ctx);
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn http_api_end_to_end() {
        use std::io::{Read, Write};
        let (mut app, sim, ctx) = rig();
        let port = 20000 + (std::process::id() % 20000) as u16;
        app.settings.http_enabled = true;
        app.settings.http_port = port;
        app.settings.http_allow_output_on = true;
        app.restart_http();
        assert!(app.http_active());
        connect(&mut app, &ctx);
        let token = app.settings.http_token.clone();

        let call = move |method: &str, path: &str, body: &str, tok: &str| -> String {
            let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
            s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
            write!(
                s,
                "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {tok}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
            let mut out = String::new();
            let _ = s.read_to_string(&mut out);
            out
        };
        assert!(call("GET", "/api/status", "", "wrong").starts_with("HTTP/1.1 401"));

        // 写请求需要 UI 线程处理：后台线程发请求，这里跑帧
        let c2 = call;
        let tok = token.clone();
        let h = std::thread::spawn(move || {
            let a = c2("POST", "/api/setpoint", r#"{"voltage": 30}"#, &tok);
            let b = c2("POST", "/api/setpoint", r#"{"voltage": 4.5}"#, &tok);
            let c = c2("POST", "/api/output", r#"{"on": true}"#, &tok);
            let d = c2("GET", "/api/status", "", &tok);
            let e = c2("GET", "/api/samples?n=5", "", &tok);
            (a, b, c, d, e)
        });
        until(&mut app, &ctx, "http calls done", |_| h.is_finished());
        let (a, b, c, d, e) = h.join().unwrap();
        assert!(a.starts_with("HTTP/1.1 400") && a.contains("\"field\":\"voltage\""), "{a}");
        assert!(b.starts_with("HTTP/1.1 200"), "{b}");
        assert!(c.starts_with("HTTP/1.1 200"), "{c}");
        assert!(d.contains("\"link\":\"connected\""), "{d}");
        assert!(e.contains("\"samples\""), "{e}");
        let s = sim.state();
        assert_eq!((s.set_mv, s.output_on), (4500, true));
    }
}
