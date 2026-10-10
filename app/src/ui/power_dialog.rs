//! 电源设置（F08 基础 + F16 安全边界），`Doc/界面设计.md` §3.1。

use dc3005u_proto::{Field, LimitError, Qty, SafetyLimits};
use eframe::egui::{self, RichText, Stroke, StrokeKind};

use crate::app::{App, limit_error_text};
use crate::config::Settings;
use crate::format::milli3;
use crate::i18n::{t, tf};
use crate::theme::Palette;

const SAFETY_FIELDS: [Field; 4] = [Field::VMin, Field::VMax, Field::IMin, Field::IMax];

pub struct PowerDialog {
    poll: String,
    usb_timeout: String,
    resp_cmd: String,
    resp_poll: String,
    auto_connect: bool,
    off_on_connect: bool,
    safety: [String; 4],
    output_warning: bool,
    lock_when_output: bool,
    seq_skip: bool,
    http_enabled: bool,
    http_port: String,
    http_token: String,
    http_allow_on: bool,
}

impl PowerDialog {
    pub fn new(s: &Settings) -> Self {
        let l = s.safety;
        Self {
            poll: s.poll_interval_ms.to_string(),
            usb_timeout: s.usb_timeout_ms.to_string(),
            resp_cmd: s.resp_timeout_cmd_ms.to_string(),
            resp_poll: s.resp_timeout_poll_ms.to_string(),
            auto_connect: s.auto_connect,
            off_on_connect: s.off_on_connect,
            safety: [l.v_min, l.v_max, l.i_min, l.i_max].map(milli3),
            output_warning: s.output_warning,
            lock_when_output: s.lock_when_output,
            seq_skip: s.seq_oob_action == "skip",
            http_enabled: s.http_enabled,
            http_port: s.http_port.to_string(),
            http_token: s.http_token.clone(),
            http_allow_on: s.http_allow_output_on,
        }
    }

    fn parsed_safety(&self) -> SafetyLimits {
        let p = |s: &String| s.trim().parse::<f64>().unwrap_or(f64::NAN);
        SafetyLimits {
            v_min: p(&self.safety[0]),
            v_max: p(&self.safety[1]),
            i_min: p(&self.safety[2]),
            i_max: p(&self.safety[3]),
        }
    }

    fn safety_errors(&self) -> Vec<LimitError> {
        self.parsed_safety().validate().err().unwrap_or_default()
    }
}

fn parse_ms(s: &str, lo: u32, hi: u32) -> Option<u32> {
    s.trim().parse::<u32>().ok().filter(|x| (lo..=hi).contains(x))
}

fn field_has_error(errs: &[LimitError], f: Field) -> bool {
    errs.iter().any(|e| match *e {
        LimitError::Order { qty } => f.qty() == qty,
        other => other.field() == Some(f),
    })
}

fn num_edit(ui: &mut egui::Ui, text: &mut String, ok: bool, pal: &Palette, width: f32) {
    let r = ui.add(egui::TextEdit::singleline(text).desired_width(width).font(egui::TextStyle::Monospace));
    if r.changed() {
        text.retain(|c| c.is_ascii_digit() || c == '.');
    }
    if !ok {
        ui.painter().rect_stroke(r.rect, 2.0, Stroke::new(1.5, pal.red), StrokeKind::Outside);
    }
}

/// 返回 false 表示对话框已关闭
pub fn draw(app: &mut App, d: &mut PowerDialog, ctx: &egui::Context, pal: &Palette) -> bool {
    let mut open = true;
    let mut ok_clicked = false;
    let mut cancel = false;
    let safety_locked = app.out.view.may_be_on() || app.aux_running();

    let poll = parse_ms(&d.poll, 50, 65535);
    let usb = parse_ms(&d.usb_timeout, 500, 5000);
    let rcmd = parse_ms(&d.resp_cmd, 50, 5000);
    let rpoll = parse_ms(&d.resp_poll, 100, 5000);
    let port = d.http_port.trim().parse::<u16>().ok().filter(|p| *p >= 1024);
    let errs = if safety_locked { Vec::new() } else { d.safety_errors() };
    let all_ok =
        poll.is_some() && usb.is_some() && rcmd.is_some() && rpoll.is_some() && port.is_some() && errs.is_empty();

    egui::Window::new(t("dlg_device_title"))
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .default_width(440.0)
        .show(ctx, |ui| {
            ui.label(RichText::new(t("dev_section_conn")).strong());
            egui::Grid::new("conn_grid").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                ui.label(t("dev_poll_interval"));
                ui.horizontal(|ui| {
                    num_edit(ui, &mut d.poll, poll.is_some(), pal, 60.0);
                    ui.label("ms (50~65535)");
                });
                ui.end_row();
                ui.label(t("dev_timeout"));
                ui.horizontal(|ui| {
                    num_edit(ui, &mut d.usb_timeout, usb.is_some(), pal, 60.0);
                    ui.label("ms (500~5000)");
                });
                ui.end_row();
                ui.label(t("dev_resp_timeout")).on_hover_text(t("dev_resp_timeout_tip"));
                ui.horizontal(|ui| {
                    num_edit(ui, &mut d.resp_cmd, rcmd.is_some(), pal, 50.0);
                    ui.label("/");
                    num_edit(ui, &mut d.resp_poll, rpoll.is_some(), pal, 50.0);
                    ui.label("ms");
                });
                ui.end_row();
            });
            ui.horizontal(|ui| {
                ui.checkbox(&mut d.auto_connect, t("dev_auto_connect"));
                ui.checkbox(&mut d.off_on_connect, t("dev_off_on_connect"));
            });
            if let Some(id) = &app.identity {
                ui.label(tf("dev_identity", &[("model", &id.model), ("serial", &id.serial), ("fw", &id.firmware)]));
            }
            let s = app.stats.total();
            ui.label(
                RichText::new(tf(
                    "dev_cmd_stats",
                    &[
                        ("ok", &s.ok.to_string()),
                        ("to", &s.timeout.to_string()),
                        ("bad", &s.bad.to_string()),
                        ("rtt", &format!("{:.1}", s.avg_rtt_ms())),
                    ],
                ))
                .small(),
            );

            ui.separator();
            ui.horizontal(|ui| {
                ui.label(RichText::new(t("safety_section")).strong());
                ui.label(RichText::new(t("hw_limits")).small().weak());
            });
            ui.add_enabled_ui(!safety_locked, |ui| {
                egui::Grid::new("safety_grid").num_columns(4).spacing([12.0, 6.0]).show(ui, |ui| {
                    for (row, chunk) in SAFETY_FIELDS.chunks(2).enumerate() {
                        for &f in chunk {
                            let k = SAFETY_FIELDS.iter().position(|x| *x == f).unwrap_or(0);
                            let name = match f {
                                Field::VMin => t("safety_v_min"),
                                Field::VMax => t("safety_v_max"),
                                Field::IMin => t("safety_i_min"),
                                _ => t("safety_i_max"),
                            };
                            ui.label(name);
                            ui.horizontal(|ui| {
                                num_edit(ui, &mut d.safety[k], !field_has_error(&errs, f), pal, 70.0);
                                ui.label(if row == 0 { Qty::Voltage.unit() } else { Qty::Current.unit() });
                            });
                        }
                        ui.end_row();
                    }
                });
                for e in &errs {
                    ui.label(RichText::new(limit_error_text(e)).small().color(pal.red));
                }
                if ui.button(t("safety_reset")).clicked() {
                    let h = SafetyLimits::hardware();
                    d.safety = [h.v_min, h.v_max, h.i_min, h.i_max].map(milli3);
                }
            });
            if safety_locked {
                let key = if app.aux_running() { "safety_locked_aux" } else { "safety_locked_output_on" };
                ui.label(RichText::new(t(key)).color(pal.yellow));
            }
            ui.horizontal(|ui| {
                ui.label(t("seq_oob_action"));
                ui.selectable_value(&mut d.seq_skip, false, t("seq_oob_stop"));
                ui.selectable_value(&mut d.seq_skip, true, t("seq_oob_skip"));
            });

            ui.separator();
            ui.label(RichText::new(t("dev_section_other")).strong());
            ui.horizontal(|ui| {
                ui.checkbox(&mut d.output_warning, t("dev_output_warn")).on_hover_text(t("dev_output_warn_tip"));
                ui.checkbox(&mut d.lock_when_output, t("dev_lock_output")).on_hover_text(t("dev_lock_output_tip"));
            });

            ui.separator();
            ui.label(RichText::new(t("http_section")).strong());
            ui.horizontal(|ui| {
                ui.checkbox(&mut d.http_enabled, t("http_enabled")).on_hover_text(t("http_enabled_tip"));
                ui.label(t("http_port"));
                num_edit(ui, &mut d.http_port, port.is_some(), pal, 60.0);
            });
            ui.horizontal(|ui| {
                ui.label("token");
                ui.label(RichText::new(format!("{}…", &d.http_token[..d.http_token.len().min(6)])).monospace());
                if ui.small_button(t("http_copy_token")).clicked() {
                    ui.ctx().copy_text(d.http_token.clone());
                }
                if ui.small_button(t("http_new_token")).clicked() {
                    d.http_token = crate::config::new_token();
                }
            });
            ui.checkbox(&mut d.http_allow_on, t("http_allow_output_on"));
            if app.http_active() {
                ui.label(
                    RichText::new(format!(
                        "{} · http://127.0.0.1:{}/api/status",
                        t("remote_active"),
                        app.settings.http_port
                    ))
                    .small()
                    .color(pal.blue),
                );
            }

            ui.separator();
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(t("common_cancel")).clicked() {
                    cancel = true;
                }
                if ui.add_enabled(all_ok, egui::Button::new(t("common_ok"))).clicked() {
                    ok_clicked = true;
                }
            });
        });

    if ok_clicked && let (Some(poll), Some(usb), Some(rcmd), Some(rpoll), Some(port)) = (poll, usb, rcmd, rpoll, port) {
        let s = &mut app.settings;
        s.poll_interval_ms = poll;
        s.usb_timeout_ms = usb;
        s.resp_timeout_cmd_ms = rcmd;
        s.resp_timeout_poll_ms = rpoll;
        s.auto_connect = d.auto_connect;
        s.off_on_connect = d.off_on_connect;
        s.output_warning = d.output_warning;
        s.lock_when_output = d.lock_when_output;
        s.seq_oob_action = if d.seq_skip { "skip" } else { "stop" }.into();
        let http_before = (s.http_enabled, s.http_port, s.http_token.clone());
        s.http_enabled = d.http_enabled;
        s.http_port = port;
        s.http_token = d.http_token.clone();
        s.http_allow_output_on = d.http_allow_on;
        let http_changed = http_before != (s.http_enabled, s.http_port, s.http_token.clone());
        // 输出可能为开或 AUX 运行时安全边界不可改（对话框打开期间状态变化也一样）
        if !app.out.view.may_be_on() && !app.aux_running() {
            let new = d.parsed_safety().rounded();
            if new.validate().is_ok() && new != app.settings.safety {
                app.settings.safety = new;
                app.log_safety();
                let edit_bad = [Qty::Voltage, Qty::Current].iter().any(|&q| app.sp.check(q, &new).is_err());
                if edit_bad || app.sp.device_out_of_limits(&new, &app.cali) {
                    app.modal_notice = Some(t("safety_setpoint_outside_new").into());
                }
            }
        }
        app.apply_settings();
        if http_changed {
            app.restart_http();
        }
        return false;
    }
    open && !cancel
}
