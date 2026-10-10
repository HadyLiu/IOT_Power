//! 辅助功能 Tab（`Doc/界面设计.md` §2.4）：预设组、功率闭环、直流扫描、函数发生器、序列执行、电池模拟。
//! 启动前按安全边界整体校验（`Doc/详细设计.md` §4.0），运行中在线修改同样校验，超界标红不生效。

use std::time::Duration;

use chrono::NaiveDateTime;
use dc3005u_proto::{Field, Qty, SafetyLimits};
use eframe::egui::{self, Color32, RichText};

use crate::app::{App, ToastKind, trim_num};
use crate::auxfn::battery::{BUILTIN_NAME, Curve};
use crate::auxfn::sequence::{self, DELAY_MAX_MS, Step, WAIT_FORMAT};
use crate::auxfn::{BatJob, Job, Kind, SeqJob, SweepJob, Update, WaveKind};
use crate::buffer::Series;
use crate::i18n::{t, tf};
use crate::theme::Palette;
use crate::ui::result_window::ResultWindow;
use crate::ui::widgets::{self, hms, param, short_duration};

const PAGES: [(&str, Option<Kind>); 6] = [
    ("tab_preset", None),
    ("tab_keep_power", Some(Kind::KeepPower)),
    ("tab_sweep", Some(Kind::Sweep)),
    ("tab_wavegen", Some(Kind::Wave)),
    ("tab_seq", Some(Kind::Seq)),
    ("tab_batsim", Some(Kind::Battery)),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StepKind {
    Delay,
    Wait,
    Volt,
    Curr,
}

struct SeqEdit {
    kind: StepKind,
    text: String,
    /// 编辑第几行；None = 新增
    replace: Option<usize>,
}

pub struct AuxUi {
    pub page: usize,
    preset_edit: usize,
    preset_text: [String; 2],
    preset_loaded: Option<usize>,
    pub seq: Vec<Step>,
    seq_sel: Option<usize>,
    seq_edit: Option<SeqEdit>,
    seq_confirm_clear: bool,
    pub curve: Curve,
    /// 在线修改被拒绝的提示
    rejected: Option<String>,
}

impl AuxUi {
    pub fn new(curve_setting: &str) -> Self {
        let curve = load_curve(curve_setting).unwrap_or_else(|e| {
            if curve_setting != BUILTIN_NAME {
                log::warn!("battery curve {curve_setting}: {e}; using built-in");
            }
            Curve::builtin()
        });
        Self {
            page: 0,
            preset_edit: 1,
            preset_text: [String::new(), String::new()],
            preset_loaded: None,
            seq: Vec::new(),
            seq_sel: None,
            seq_edit: None,
            seq_confirm_clear: false,
            curve,
            rejected: None,
        }
    }
}

fn load_curve(setting: &str) -> Result<Curve, String> {
    if setting == BUILTIN_NAME || setting.is_empty() {
        return Ok(Curve::builtin());
    }
    let text = std::fs::read_to_string(setting).map_err(|e| e.to_string())?;
    let name = std::path::Path::new(setting).file_stem().map_or(setting.into(), |s| s.to_string_lossy().into_owned());
    Curve::parse(&name, &text)
}

fn r3(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0
}

fn in_range(x: f64, (lo, hi): (f64, f64)) -> bool {
    x.is_finite() && (lo..=hi).contains(&r3(x))
}

fn out_of(param: &str, (min, max): (f64, f64), unit: &str) -> String {
    tf("aux_param_out_of_limits", &[("param", param), ("min", &trim_num(min)), ("max", &trim_num(max)), ("unit", unit)])
}

fn invalid(param: &str, (min, max): (f64, f64), unit: &str) -> String {
    tf("aux_param_invalid", &[("param", param), ("min", &trim_num(min)), ("max", &trim_num(max)), ("unit", unit)])
}

/// 设备当前设定（启动前校验用：未作为参数的另一个量必须在边界内）
fn device_check(app: &App, q: Qty, errs: &mut Vec<String>) {
    let lim = app.settings.safety;
    if let Some(x) = app.sp.device_value(q, &app.cali)
        && !in_range(x, lim.range(q))
    {
        let name = if q == Qty::Voltage { t("set_voltage") } else { t("set_current") };
        errs.push(out_of(name, lim.range(q), q.unit()));
    }
}

fn kp_errors(app: &App) -> Vec<String> {
    let c = &app.settings.aux.keep_power;
    let lim = app.settings.safety;
    let mut e = Vec::new();
    if !in_range(c.max_v, lim.range(Qty::Voltage)) {
        e.push(out_of(t("kp_max_v"), lim.range(Qty::Voltage), "V"));
    }
    if !(c.target_w > 0.0 && c.target_w <= 120.0) {
        e.push(invalid(t("kp_target"), (0.1, 120.0), "W"));
    }
    if !(c.ki > 0.0 && c.ki <= 40.0) {
        e.push(invalid(t("kp_param"), (0.001, 40.0), ""));
    }
    if !(0.1..=100.0).contains(&c.freq_hz) {
        e.push(invalid(t("act_freq"), (0.1, 100.0), "Hz"));
    }
    device_check(app, Qty::Voltage, &mut e);
    device_check(app, Qty::Current, &mut e);
    e
}

fn sweep_target(s: &str) -> Qty {
    if s == "current" { Qty::Current } else { Qty::Voltage }
}

fn sweep_errors(app: &App) -> Vec<String> {
    let c = &app.settings.aux.sweep;
    let lim = app.settings.safety;
    let q = sweep_target(&c.target);
    let mut e = Vec::new();
    for (name, x) in [(t("sweep_start"), c.start), (t("sweep_stop"), c.stop)] {
        if !in_range(x, lim.range(q)) {
            e.push(out_of(name, lim.range(q), q.unit()));
        }
    }
    if r3(c.start) == r3(c.stop) {
        e.push(t("sweep_same_ends").into());
    }
    if !(c.step >= 0.001 && c.step.is_finite()) {
        e.push(invalid(t("sweep_step"), (0.001, 24.0), q.unit()));
    }
    let min_delay = (f64::from(app.settings.setpoint_min_interval_ms) / 1000.0)
        .max(2.0 * f64::from(app.settings.poll_interval_ms) / 1000.0)
        .max(0.1);
    if !(min_delay..=999_999.0).contains(&c.delay_s) {
        e.push(invalid(t("sweep_delay"), (min_delay, 999_999.0), "s"));
    }
    device_check(app, if q == Qty::Voltage { Qty::Current } else { Qty::Voltage }, &mut e);
    e
}

fn wave_online_errors(c: &crate::config::WaveCfg, lim: &SafetyLimits) -> Vec<String> {
    let mut e = Vec::new();
    for (name, x) in [(t("wave_high"), c.high_v), (t("wave_low"), c.low_v)] {
        if !in_range(x, lim.range(Qty::Voltage)) {
            e.push(out_of(name, lim.range(Qty::Voltage), "V"));
        }
    }
    if c.high_v <= c.low_v {
        e.push(t("wave_high_le_low").into());
    }
    if !(c.period_s > 0.0 && c.period_s.is_finite()) {
        e.push(invalid(t("wave_period"), (0.001, 1e6), "s"));
    }
    e
}

fn wave_errors(app: &App) -> Vec<String> {
    let c = &app.settings.aux.wavegen;
    let mut e = wave_online_errors(c, &app.settings.safety);
    if !(0.1..=100.0).contains(&c.freq_hz) {
        e.push(invalid(t("act_freq"), (0.1, 100.0), "Hz"));
    }
    device_check(app, Qty::Current, &mut e);
    e
}

fn bat_errors(app: &App, curve: &Curve) -> Vec<String> {
    let c = &app.settings.aux.batsim;
    let lim = app.settings.safety;
    let s = f64::from(c.cells);
    let mut e = Vec::new();
    if curve.max_v() * s > lim.v_max + 1e-9 {
        e.push(tf("bat_over_vmax", &[("v_max", &trim_num(lim.v_max))]));
    }
    let v0 = curve.ocv(c.soc_now) * s;
    if !in_range(v0, lim.range(Qty::Voltage)) {
        e.push(out_of(t("bat_initial_v"), lim.range(Qty::Voltage), "V"));
    }
    if !(0.0..=100.0).contains(&c.soc_now) {
        e.push(invalid(t("bat_soc_now"), (0.0, 100.0), "%"));
    }
    if !(c.soc_stop >= 0.0 && c.soc_stop < c.soc_now) {
        e.push(invalid(t("bat_soc_stop"), (0.0, c.soc_now.max(0.0)), "%"));
    }
    if !(c.cap_wh > 0.0 && c.cap_wh.is_finite()) {
        e.push(invalid(t("bat_capacity"), (0.001, 1e6), "Wh"));
    }
    if !(c.r_mohm >= 0.0 && c.r_mohm <= 10_000.0) {
        e.push(invalid(t("bat_internal_r"), (0.0, 10_000.0), "mΩ"));
    }
    if !(1..=6).contains(&c.cells) {
        e.push(invalid(t("bat_cells"), (1.0, 6.0), "S"));
    }
    if !(0.1..=100.0).contains(&c.freq_hz) {
        e.push(invalid(t("act_freq"), (0.1, 100.0), "Hz"));
    }
    device_check(app, Qty::Current, &mut e);
    e
}

// ---------- 公共部分 ----------

pub fn draw(app: &mut App, ui: &mut egui::Ui, pal: &Palette) {
    let running = app.aux_status.running;
    ui.label(RichText::new(t("aux_title")).strong());
    let header = ui
        .horizontal(|ui| {
            if ui.small_button("◀").clicked() {
                app.aux_ui.page = (app.aux_ui.page + PAGES.len() - 1) % PAGES.len();
            }
            let (key, kind) = PAGES[app.aux_ui.page];
            let mut title = RichText::new(t(key)).strong();
            if kind.is_some() && kind == running {
                title = RichText::new(format!("● {}", t(key))).strong().color(pal.green);
            }
            ui.add_sized([ui.available_width() - 28.0, 20.0], egui::Label::new(title));
            if ui.small_button("▶").clicked() {
                app.aux_ui.page = (app.aux_ui.page + 1) % PAGES.len();
            }
        })
        .response;
    // 滚轮切换：只在连接后、鼠标在标题行上时
    if app.connected() && header.hovered() {
        let dy = widgets::wheel(ui);
        if dy != 0.0 {
            let n = PAGES.len();
            app.aux_ui.page = if dy > 0.0 { (app.aux_ui.page + n - 1) % n } else { (app.aux_ui.page + 1) % n };
        }
    }
    if let Some(k) = running
        && PAGES[app.aux_ui.page].1 != Some(k)
    {
        let name = PAGES.iter().find(|p| p.1 == Some(k)).map_or("", |p| t(p.0));
        ui.label(RichText::new(tf("aux_other_running", &[("name", name)])).small().color(pal.yellow));
    }
    ui.add_space(2.0);
    match app.aux_ui.page {
        0 => preset_page(app, ui, pal),
        1 => keep_power_page(app, ui, pal),
        2 => sweep_page(app, ui, pal),
        3 => wave_page(app, ui, pal),
        4 => seq_page(app, ui, pal),
        _ => battery_page(app, ui, pal),
    }
    seq_edit_modal(app, ui.ctx(), pal);
}

/// 页顶的「功能已关闭 / 功能已开启」开关 + 急停；返回 true 表示点了启动
fn run_toggle(app: &mut App, ui: &mut egui::Ui, pal: &Palette, kind: Kind, errs: &[String]) -> bool {
    let running = app.aux_status.running;
    let mut start = false;
    ui.horizontal(|ui| {
        if running == Some(kind) {
            let b = egui::Button::new(RichText::new(t("aux_on")).color(Color32::BLACK).strong()).fill(pal.on);
            if ui.add(b).clicked() {
                app.stop_aux();
            }
            let e = egui::Button::new(RichText::new(t("estop")).color(Color32::WHITE).strong()).fill(pal.red);
            if ui.add(e).on_hover_text("Esc").clicked() {
                app.estop("user");
            }
        } else {
            let locked = app.setpoint_locked();
            let enabled = app.connected() && running.is_none() && errs.is_empty() && !locked;
            let label = if errs.is_empty() { t("aux_off") } else { t("aux_illegal") };
            if ui.add_enabled(enabled, egui::Button::new(label)).clicked() {
                start = true;
            }
        }
    });
    if running == Some(kind) {
        status_line(app, ui, pal);
    } else {
        for e in errs {
            ui.label(RichText::new(e).small().color(pal.red));
        }
        if app.setpoint_locked() {
            ui.label(RichText::new(t("locked_when_output")).small().color(pal.yellow));
        }
    }
    if let Some(r) = &app.aux_ui.rejected
        && running == Some(kind)
    {
        ui.label(RichText::new(r).small().color(pal.red));
    }
    start
}

fn status_line(app: &App, ui: &mut egui::Ui, pal: &Palette) {
    let s = &app.aux_status;
    ui.horizontal_wrapped(|ui| {
        if s.waiting_stable {
            ui.spinner();
            ui.label(RichText::new(t("aux_wait_stable")).small());
        }
        if s.paused {
            ui.label(RichText::new(t("aux_read_stalled")).small().color(pal.yellow));
        }
        ui.label(RichText::new(tf("aux_freq_limited", &[("hz", &format!("{:.1}", s.eff_hz))])).small().weak());
        if s.send_failed > 0 {
            ui.label(RichText::new(tf("aux_send_failed", &[("n", &s.send_failed.to_string())])).small().color(pal.red));
        }
        if s.clamped > 0 {
            ui.label(RichText::new(tf("runtime_clamped", &[("n", &s.clamped.to_string())])).small().color(pal.yellow));
        }
    });
}

fn row(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui)) {
    ui.label(label);
    ui.horizontal(add);
    ui.end_row();
}

// ---------- 预设组 ----------

fn preset_label(k: usize, v: f64, i: f64) -> String {
    format!("[{k}] {v:06.3}V {i:06.3}A")
}

fn preset_outside(lim: &SafetyLimits, v: f64, i: f64) -> bool {
    lim.check(Field::Voltage, v).is_err() || lim.check(Field::Current, i).is_err()
}

fn preset_page(app: &mut App, ui: &mut egui::Ui, pal: &Palette) {
    let lim = app.settings.safety;
    let enabled = app.connected() && !app.setpoint_locked() && app.aux_status.running.is_none();
    let mut chosen = None;
    ui.add_enabled_ui(enabled, |ui| {
        egui::ComboBox::from_id_salt("preset_select")
            .selected_text(format!("[>] {}", t("preset_select")))
            .width(ui.available_width() - 8.0)
            .show_ui(ui, |ui| {
                for k in 1..=9 {
                    let Some(&[v, i]) = app.settings.presets.get(&k.to_string()) else { continue };
                    let mut text = RichText::new(preset_label(k, v, i)).monospace();
                    let bad = preset_outside(&lim, v, i);
                    if bad {
                        text = RichText::new(format!("{}  {}", preset_label(k, v, i), t("safety_item_out_of_bounds")))
                            .monospace()
                            .color(pal.red);
                    }
                    if ui.selectable_label(false, text).clicked() {
                        chosen = Some(k);
                    }
                }
            });
    });
    if let Some(k) = chosen {
        app.apply_preset(k);
    }

    ui.add_space(6.0);
    ui.label(RichText::new(t("preset_edit")).strong());
    let k = app.aux_ui.preset_edit;
    if app.aux_ui.preset_loaded != Some(k) {
        let [v, i] = app.settings.presets.get(&k.to_string()).copied().unwrap_or([5.0, 1.0]);
        app.aux_ui.preset_text = [format!("{v:.3}"), format!("{i:.3}")];
        app.aux_ui.preset_loaded = Some(k);
    }
    let hw = SafetyLimits::hardware();
    let pv = app.aux_ui.preset_text[0].trim().parse::<f64>().ok().filter(|x| hw.check(Field::Voltage, *x).is_ok());
    let pi = app.aux_ui.preset_text[1].trim().parse::<f64>().ok().filter(|x| hw.check(Field::Current, *x).is_ok());
    egui::Grid::new("preset_edit").num_columns(2).show(ui, |ui| {
        row(ui, "", |ui| {
            egui::ComboBox::from_id_salt("preset_edit_k").selected_text(format!("Preset-{k}")).show_ui(ui, |ui| {
                for n in 1..=9 {
                    ui.selectable_value(&mut app.aux_ui.preset_edit, n, format!("Preset-{n}"));
                }
            });
        });
        for (idx, (name, unit, ok)) in
            [(t("qty_voltage"), "V", pv.is_some()), (t("qty_current"), "A", pi.is_some())].into_iter().enumerate()
        {
            row(ui, name, |ui| {
                let r = ui.add(
                    egui::TextEdit::singleline(&mut app.aux_ui.preset_text[idx])
                        .desired_width(70.0)
                        .font(egui::TextStyle::Monospace),
                );
                if r.changed() {
                    app.aux_ui.preset_text[idx].retain(|c| c.is_ascii_digit() || c == '.');
                }
                if !ok {
                    widgets::red_frame(ui, &r, pal);
                }
                ui.label(unit);
            });
        }
    });
    if ui.button(t("common_save")).clicked() {
        match (pv, pi) {
            (Some(v), Some(i)) => {
                app.settings.presets.insert(k.to_string(), [r3(v), r3(i)]);
                app.save_settings();
                app.toast(ToastKind::Info, t("preset_save_ok"));
            }
            _ => app.toast(ToastKind::Warn, format!("{}：{}", t("preset_save_fail"), t("hw_limits"))),
        }
    }
}

// ---------- 功率闭环 ----------

fn keep_power_page(app: &mut App, ui: &mut egui::Ui, pal: &Palette) {
    let running = app.aux_status.running == Some(Kind::KeepPower);
    let errs = kp_errors(app);
    if run_toggle(app, ui, pal, Kind::KeepPower, &errs) {
        let job = Job::KeepPower(app.settings.aux.keep_power.clone());
        app.start_aux(job);
    }
    let lim = app.settings.safety;
    let before = app.settings.aux.keep_power.clone();
    let c = &mut app.settings.aux.keep_power;
    egui::Grid::new("kp_grid").num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
        row(ui, t("kp_target"), |ui| {
            param(ui, &mut c.target_w, 0.01, 3, |x| !(x > 0.0 && x <= 120.0), pal);
            ui.label("W");
        });
        row(ui, t("kp_param"), |ui| {
            param(ui, &mut c.ki, 0.01, 3, |x| !(x > 0.0 && x <= 40.0), pal).on_hover_text("Ki");
        });
        row(ui, t("kp_max_v"), |ui| {
            param(ui, &mut c.max_v, 0.01, 3, |x| !in_range(x, lim.range(Qty::Voltage)), pal);
            ui.label("V");
        });
        row(ui, t("act_freq"), |ui| {
            ui.add_enabled_ui(!running, |ui| param(ui, &mut c.freq_hz, 0.1, 1, |x| !(0.1..=100.0).contains(&x), pal));
            ui.label("Hz");
        });
    });
    let c = app.settings.aux.keep_power.clone();
    if running && c != before {
        let ok = in_range(c.max_v, lim.range(Qty::Voltage)) && c.target_w > 0.0 && c.ki > 0.0;
        if ok {
            app.aux_ui.rejected = None;
            app.aux.send(crate::auxfn::AuxCmd::Update(Update::KeepPower {
                target_w: c.target_w,
                ki: c.ki,
                max_v: c.max_v,
            }));
        } else {
            app.aux_ui.rejected = Some(tf("aux_param_rejected", &[("param", t("tab_keep_power"))]));
        }
    }
}

// ---------- 直流扫描 ----------

fn sweep_page(app: &mut App, ui: &mut egui::Ui, pal: &Palette) {
    let running = app.aux_status.running == Some(Kind::Sweep);
    let errs = sweep_errors(app);
    if run_toggle(app, ui, pal, Kind::Sweep, &errs) {
        let c = &app.settings.aux.sweep;
        let job = Job::Sweep(SweepJob {
            target: sweep_target(&c.target),
            start: r3(c.start),
            stop: r3(c.stop),
            step: c.step,
            delay_s: c.delay_s,
            response: Series::parse(&c.response),
        });
        app.start_aux(job);
    }
    let lim = app.settings.safety;
    let c = &mut app.settings.aux.sweep;
    let q = sweep_target(&c.target);
    ui.add_enabled_ui(!running, |ui| {
        egui::Grid::new("sweep_grid").num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
            row(ui, t("sweep_target"), |ui| {
                ui.selectable_value(&mut c.target, "voltage".into(), t("qty_voltage"));
                ui.selectable_value(&mut c.target, "current".into(), t("qty_current"));
            });
            for (name, x) in [(t("sweep_start"), &mut c.start), (t("sweep_stop"), &mut c.stop)] {
                row(ui, name, |ui| {
                    param(ui, x, 0.01, 3, |v| !in_range(v, lim.range(q)), pal);
                    ui.label(q.unit());
                });
            }
            row(ui, t("sweep_step"), |ui| {
                param(ui, &mut c.step, 0.001, 3, |x| x < 0.001, pal);
                ui.label(q.unit());
            });
            row(ui, t("sweep_delay"), |ui| {
                param(ui, &mut c.delay_s, 0.01, 2, |x| x < 0.1, pal);
                ui.label("s");
            });
            row(ui, t("sweep_response"), |ui| {
                let cur = Series::parse(&c.response);
                let text = if cur == Series::None { t("sweep_no_record") } else { t(cur.key()) };
                egui::ComboBox::from_id_salt("sweep_resp").selected_text(text).show_ui(ui, |ui| {
                    ui.selectable_value(&mut c.response, "none".into(), t("sweep_no_record"));
                    for s in [Series::Voltage, Series::Current, Series::Power, Series::Resistance] {
                        ui.selectable_value(&mut c.response, s.as_str().into(), t(s.key()));
                    }
                });
            });
        });
    });
    let job = SweepJob {
        target: q,
        start: c.start,
        stop: c.stop,
        step: c.step.max(0.001),
        delay_s: c.delay_s,
        response: Series::None,
    };
    if c.step >= 0.001 && c.start.is_finite() && c.stop.is_finite() {
        let n = job.values().len();
        let eta = short_duration(n as f64 * c.delay_s);
        ui.label(RichText::new(tf("sweep_eta", &[("n", &n.to_string()), ("t", &eta)])).small().weak());
    }
    if ui.button(t("sweep_view")).clicked() {
        match app.aux_status.sweep.clone() {
            Some(r) if !r.points.is_empty() => app.result_win = Some(ResultWindow::sweep(&r)),
            _ => app.toast(ToastKind::Info, t("sweep_empty")),
        }
    }
}

// ---------- 函数发生器 ----------

fn wave_page(app: &mut App, ui: &mut egui::Ui, pal: &Palette) {
    let running = app.aux_status.running == Some(Kind::Wave);
    let errs = wave_errors(app);
    if run_toggle(app, ui, pal, Kind::Wave, &errs) {
        let job = Job::Wave(app.settings.aux.wavegen.clone());
        app.start_aux(job);
    }
    let lim = app.settings.safety;
    let before = app.settings.aux.wavegen.clone();
    let c = &mut app.settings.aux.wavegen;
    egui::Grid::new("wave_grid").num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
        row(ui, t("wave_type"), |ui| {
            let cur = WaveKind::parse(&c.kind);
            egui::ComboBox::from_id_salt("wave_kind").selected_text(t(cur.key())).show_ui(ui, |ui| {
                for k in WaveKind::ALL {
                    ui.selectable_value(&mut c.kind, k.as_str().into(), t(k.key()));
                }
            });
        });
        row(ui, t("wave_period"), |ui| {
            param(ui, &mut c.period_s, 0.01, 3, |x| x <= 0.0, pal).on_hover_text(t("wave_triangle_tip"));
            ui.label("s");
        });
        let bad_order = c.high_v <= c.low_v;
        row(ui, t("wave_high"), |ui| {
            param(ui, &mut c.high_v, 0.01, 3, |x| bad_order || !in_range(x, lim.range(Qty::Voltage)), pal);
            ui.label("V");
        });
        row(ui, t("wave_low"), |ui| {
            param(ui, &mut c.low_v, 0.01, 3, |x| bad_order || !in_range(x, lim.range(Qty::Voltage)), pal);
            ui.label("V");
        });
        row(ui, t("act_freq"), |ui| {
            ui.add_enabled_ui(!running, |ui| param(ui, &mut c.freq_hz, 0.1, 1, |x| !(0.1..=100.0).contains(&x), pal));
            ui.label("Hz");
        });
    });
    let c = app.settings.aux.wavegen.clone();
    if running && c != before {
        if wave_online_errors(&c, &lim).is_empty() {
            app.aux_ui.rejected = None;
            app.aux.send(crate::auxfn::AuxCmd::Update(Update::Wave {
                kind: WaveKind::parse(&c.kind),
                period_s: c.period_s,
                high_v: r3(c.high_v),
                low_v: r3(c.low_v),
            }));
        } else {
            app.aux_ui.rejected = Some(tf("aux_param_rejected", &[("param", t("tab_wavegen"))]));
        }
    }
}

// ---------- 序列执行 ----------

fn seq_page(app: &mut App, ui: &mut egui::Ui, pal: &Palette) {
    let running = app.aux_status.running == Some(Kind::Seq);
    let lim = app.settings.safety;
    let current = if running { app.aux_status.seq_index } else { None };

    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.set_width(ui.available_width());
        egui::ScrollArea::vertical().id_salt("seq_list").max_height(160.0).auto_shrink([false, true]).show(ui, |ui| {
            if app.aux_ui.seq.is_empty() {
                ui.label(RichText::new("—").weak());
            }
            let mut action: Option<(usize, u8)> = None;
            for (k, st) in app.aux_ui.seq.iter().enumerate() {
                let bad = st.outside(&lim);
                let prefix = if current == Some(k) { "▶ " } else { "   " };
                let mut text = RichText::new(format!("{prefix}{:>3}  {}", k + 1, st.to_line())).monospace();
                if bad {
                    text = text.color(pal.red);
                } else if current == Some(k) {
                    text = text.color(pal.green);
                }
                let mut r = ui.selectable_label(app.aux_ui.seq_sel == Some(k), text);
                if bad {
                    r = r.on_hover_text(t("safety_item_out_of_bounds"));
                }
                if running {
                    continue;
                }
                if r.clicked() {
                    action = Some((k, 0));
                }
                if r.double_clicked() {
                    action = Some((k, 1));
                }
                r.context_menu(|ui| {
                    if ui.button(t("seq_menu_edit")).clicked() {
                        action = Some((k, 1));
                        ui.close();
                    }
                    if ui.button(t("seq_menu_delete")).clicked() {
                        action = Some((k, 2));
                        ui.close();
                    }
                    if ui.button(t("seq_menu_clear")).clicked() {
                        action = Some((k, 3));
                        ui.close();
                    }
                });
            }
            match action {
                Some((k, 0)) => app.aux_ui.seq_sel = Some(k),
                Some((k, 1)) => open_seq_edit(app, Some(k)),
                Some((k, 2)) => {
                    app.aux_ui.seq.remove(k);
                    app.aux_ui.seq_sel = None;
                }
                Some((_, 3)) => app.aux_ui.seq_confirm_clear = true,
                _ => {}
            }
        });
    });

    if running {
        ui.horizontal(|ui| {
            if ui.button(t("seq_stop")).clicked() {
                app.stop_aux();
            }
            let e = egui::Button::new(RichText::new(t("estop")).color(Color32::WHITE).strong()).fill(pal.red);
            if ui.add(e).on_hover_text("Esc").clicked() {
                app.estop("user");
            }
        });
        status_line(app, ui, pal);
        return;
    }

    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(t("seq_add_action")).small());
        for (kind, key) in [
            (StepKind::Delay, "seq_delay"),
            (StepKind::Wait, "seq_wait"),
            (StepKind::Volt, "seq_voltage"),
            (StepKind::Curr, "seq_current"),
        ] {
            if ui.small_button(t(key)).clicked() {
                app.aux_ui.seq_edit = Some(SeqEdit { kind, text: default_text(app, kind), replace: None });
            }
        }
    });
    ui.horizontal(|ui| {
        if ui.button(t("common_save")).clicked() {
            save_seq(app);
        }
        if ui.button(t("seq_load")).clicked() {
            load_seq(app);
        }
        ui.separator();
        let other = app.aux_status.running.is_some();
        let can = app.connected() && !other && !app.setpoint_locked() && !app.aux_ui.seq.is_empty();
        for (looped, key) in [(false, "seq_single"), (true, "seq_loop")] {
            if ui.add_enabled(can, egui::Button::new(t(key))).clicked() {
                if looped && !app.aux_ui.seq.iter().any(Step::is_timed) {
                    app.toast(ToastKind::Warn, t("seq_loop_no_delay"));
                } else {
                    let job = Job::Seq(SeqJob {
                        steps: app.aux_ui.seq.clone(),
                        looped,
                        skip_oob: app.settings.seq_oob_action == "skip",
                    });
                    app.start_aux(job);
                }
            }
        }
    });
    if app.setpoint_locked() {
        ui.label(RichText::new(t("locked_when_output")).small().color(pal.yellow));
    }

    if app.aux_ui.seq_confirm_clear {
        let mut action = None;
        egui::Modal::new(egui::Id::new("seq_clear")).show(ui.ctx(), |ui| {
            ui.label(t("seq_confirm_clear"));
            ui.horizontal(|ui| {
                if ui.button(t("common_ok")).clicked() {
                    action = Some(true);
                }
                if ui.button(t("common_cancel")).clicked() {
                    action = Some(false);
                }
            });
        });
        if let Some(ok) = action {
            app.aux_ui.seq_confirm_clear = false;
            if ok {
                app.aux_ui.seq.clear();
                app.aux_ui.seq_sel = None;
            }
        }
    }
}

fn default_text(app: &App, kind: StepKind) -> String {
    match kind {
        StepKind::Delay => "1000".into(),
        StepKind::Wait => (chrono::Local::now() + chrono::Duration::minutes(1)).format(WAIT_FORMAT).to_string(),
        StepKind::Volt => format!("{:.3}", app.sp.device_value(Qty::Voltage, &app.cali).unwrap_or(5.0)),
        StepKind::Curr => format!("{:.3}", app.sp.device_value(Qty::Current, &app.cali).unwrap_or(1.0)),
    }
}

fn open_seq_edit(app: &mut App, k: Option<usize>) {
    let Some(k) = k else { return };
    let Some(st) = app.aux_ui.seq.get(k) else { return };
    let (kind, text) = match st {
        Step::Delay(ms) => (StepKind::Delay, ms.to_string()),
        Step::Wait(at) => (StepKind::Wait, at.format(WAIT_FORMAT).to_string()),
        Step::SetV(v) => (StepKind::Volt, format!("{v:.3}")),
        Step::SetI(i) => (StepKind::Curr, format!("{i:.3}")),
    };
    app.aux_ui.seq_sel = Some(k);
    app.aux_ui.seq_edit = Some(SeqEdit { kind, text, replace: Some(k) });
}

/// 新增/编辑动作：输入范围为当前安全边界（`SET-V 0` 表示关输出）
fn parse_step(kind: StepKind, text: &str, lim: &SafetyLimits) -> Option<Step> {
    let s = text.trim();
    match kind {
        StepKind::Delay => s.parse::<u32>().ok().filter(|ms| *ms <= DELAY_MAX_MS).map(Step::Delay),
        StepKind::Wait => NaiveDateTime::parse_from_str(s, WAIT_FORMAT).ok().map(Step::Wait),
        StepKind::Volt => s
            .parse::<f64>()
            .ok()
            .filter(|v| *v == 0.0 || lim.check(Field::Voltage, *v).is_ok())
            .map(|v| Step::SetV(r3(v))),
        StepKind::Curr => {
            s.parse::<f64>().ok().filter(|i| lim.check(Field::Current, *i).is_ok()).map(|i| Step::SetI(r3(i)))
        }
    }
}

fn seq_edit_modal(app: &mut App, ctx: &egui::Context, pal: &Palette) {
    let lim = app.settings.safety;
    let Some(ed) = &mut app.aux_ui.seq_edit else { return };
    let parsed = parse_step(ed.kind, &ed.text, &lim);
    let mut action = None;
    egui::Modal::new(egui::Id::new("seq_edit")).show(ctx, |ui| {
        ui.set_max_width(320.0);
        ui.heading(if ed.replace.is_some() { t("seq_edit_action") } else { t("seq_add_action") });
        let (prompt, unit, hint) = match ed.kind {
            StepKind::Delay => (t("seq_input_delay"), "ms", "0~100000".to_string()),
            StepKind::Wait => (t("seq_input_wait"), "", t("seq_wait_format").to_string()),
            StepKind::Volt => (
                t("seq_input_voltage"),
                "V",
                tf("allowed_range", &[("min", &trim_num(lim.v_min)), ("max", &trim_num(lim.v_max)), ("unit", "V")]),
            ),
            StepKind::Curr => (
                t("seq_input_current"),
                "A",
                tf("allowed_range", &[("min", &trim_num(lim.i_min)), ("max", &trim_num(lim.i_max)), ("unit", "A")]),
            ),
        };
        ui.label(prompt);
        ui.horizontal(|ui| {
            let r =
                ui.add(egui::TextEdit::singleline(&mut ed.text).desired_width(180.0).font(egui::TextStyle::Monospace));
            if parsed.is_none() {
                widgets::red_frame(ui, &r, pal);
            }
            if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && parsed.is_some() {
                action = Some(true);
            }
            ui.label(unit);
        });
        ui.label(RichText::new(hint).small().weak());
        ui.horizontal(|ui| {
            if ui.add_enabled(parsed.is_some(), egui::Button::new(t("common_ok"))).clicked() {
                action = Some(true);
            }
            if ui.button(t("common_cancel")).clicked() {
                action = Some(false);
            }
        });
    });
    match action {
        Some(true) => {
            if let (Some(st), Some(ed)) = (parsed, app.aux_ui.seq_edit.take()) {
                let seq = &mut app.aux_ui.seq;
                match ed.replace {
                    Some(k) if k < seq.len() => seq[k] = st,
                    _ => {
                        let at = app.aux_ui.seq_sel.map_or(seq.len(), |k| (k + 1).min(seq.len()));
                        seq.insert(at, st);
                        app.aux_ui.seq_sel = Some(at);
                    }
                }
            }
        }
        Some(false) => app.aux_ui.seq_edit = None,
        None => {}
    }
}

fn save_seq(app: &mut App) {
    let Some(path) =
        rfd::FileDialog::new().add_filter(t("txt_filter"), &["txt"]).set_file_name("sequence.txt").save_file()
    else {
        return;
    };
    if let Err(e) = std::fs::write(&path, sequence::to_text(&app.aux_ui.seq)) {
        app.toast(ToastKind::Error, format!("{}: {e}", path.display()));
    }
}

fn load_seq(app: &mut App) {
    let Some(path) = rfd::FileDialog::new().add_filter(t("txt_filter"), &["txt"]).pick_file() else { return };
    let text = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => {
            app.toast(ToastKind::Error, format!("{}: {e}", path.display()));
            return;
        }
    };
    match sequence::parse_text(&text) {
        Ok(steps) => {
            app.aux_ui.seq = steps;
            app.aux_ui.seq_sel = None;
        }
        Err(line) => app.modal_notice = Some(format!("{}{line}", t("seq_validation_error"))),
    }
}

// ---------- 电池模拟 ----------

fn battery_page(app: &mut App, ui: &mut egui::Ui, pal: &Palette) {
    let running = app.aux_status.running == Some(Kind::Battery);
    let errs = bat_errors(app, &app.aux_ui.curve);
    if run_toggle(app, ui, pal, Kind::Battery, &errs) {
        let c = &app.settings.aux.batsim;
        let job = Job::Battery(BatJob {
            curve: app.aux_ui.curve.clone(),
            soc_now: c.soc_now,
            soc_stop: c.soc_stop,
            cap_wh: c.cap_wh,
            r_ohm: c.r_mohm / 1000.0,
            cells: c.cells,
            freq_hz: c.freq_hz,
        });
        app.start_aux(job);
    }
    if running && let Some(soc) = app.aux_status.soc {
        app.settings.aux.batsim.soc_now = (soc * 10.0).round() / 10.0;
    }

    ui.horizontal(|ui| {
        ui.label(t("bat_curve"));
        ui.label(RichText::new(&app.aux_ui.curve.name).monospace());
    });
    ui.horizontal(|ui| {
        ui.add_enabled_ui(!running, |ui| {
            if ui.button(t("bat_load_curve")).clicked()
                && let Some(p) = rfd::FileDialog::new().add_filter(t("csv_filter"), &["csv"]).pick_file()
            {
                match load_curve(&p.to_string_lossy()) {
                    Ok(c) => {
                        app.aux_ui.curve = c;
                        app.settings.aux.batsim.curve = p.to_string_lossy().into_owned();
                    }
                    Err(e) => app.toast(ToastKind::Error, tf("bat_curve_bad", &[("err", &e)])),
                }
            }
            if app.aux_ui.curve.name != BUILTIN_NAME && ui.button(BUILTIN_NAME).clicked() {
                app.aux_ui.curve = Curve::builtin();
                app.settings.aux.batsim.curve = BUILTIN_NAME.into();
            }
        });
        if ui.button(t("bat_view_curve")).clicked() {
            app.result_win = Some(ResultWindow::battery(&app.aux_ui.curve));
        }
    });

    let c = &mut app.settings.aux.batsim;
    ui.add_enabled_ui(!running, |ui| {
        egui::Grid::new("bat_grid").num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
            row(ui, t("bat_soc_now"), |ui| {
                param(ui, &mut c.soc_now, 0.1, 1, |x| !(0.0..=100.0).contains(&x), pal);
                ui.label("%");
            });
            row(ui, t("bat_soc_stop"), |ui| {
                param(ui, &mut c.soc_stop, 0.1, 1, |x| !(x >= 0.0 && x < c.soc_now), pal);
                ui.label("%");
            });
            row(ui, t("bat_capacity"), |ui| {
                param(ui, &mut c.cap_wh, 0.1, 2, |x| x <= 0.0, pal);
                ui.label("Wh");
            });
            row(ui, t("bat_internal_r"), |ui| {
                param(ui, &mut c.r_mohm, 1.0, 0, |x| x < 0.0, pal);
                ui.label("mΩ");
            });
            row(ui, t("bat_cells"), |ui| {
                ui.add(egui::DragValue::new(&mut c.cells).range(1..=6));
                ui.label("S");
            });
            row(ui, t("act_freq"), |ui| {
                param(ui, &mut c.freq_hz, 0.1, 1, |x| !(0.1..=100.0).contains(&x), pal);
                ui.label("Hz");
            });
        });
    });
    let d = if running { app.aux_status.discharge } else { Duration::ZERO };
    ui.label(tf("bat_discharge_time", &[("time", &hms(d))]));
}
