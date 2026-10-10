//! 左栏：输出设定（F03）+ 连接（F01）。

use dc3005u_proto::{LimitError, Qty};
use dc3005u_usb::{LinkState, Source};
use eframe::egui::{self, Color32, RichText, Stroke, StrokeKind};

use crate::app::{App, limit_error_text, trim_num};
use crate::format::milli3;
use crate::i18n::{t, tf};
use crate::output::OutputView;
use crate::theme::Palette;
use crate::ui::aux_panel;
use crate::ui::graphics_dialog::GraphicsDialog;
use crate::ui::power_dialog::PowerDialog;
use crate::ui::widgets::{self, num_field};

pub fn draw(app: &mut App, ui: &mut egui::Ui, pal: &Palette) {
    egui::Panel::bottom("left_bottom").show(ui, |ui| {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui.button(t("btn_graphics")).clicked() && app.graphics.is_none() {
                app.graphics = Some(GraphicsDialog::new(&app.settings));
            }
            if ui.button(t("btn_settings")).clicked() && app.power_dialog.is_none() {
                app.power_dialog = Some(PowerDialog::new(&app.settings));
            }
        });
        ui.separator();
        connection_row(app, ui, pal);
        ui.add_space(4.0);
    });
    egui::CentralPanel::default().show(ui, |ui| {
        egui::ScrollArea::vertical().show(ui, |ui| {
            output_section(app, ui, pal);
            ui.separator();
            aux_panel::draw(app, ui, pal);
        });
    });
}

fn connection_row(app: &mut App, ui: &mut egui::Ui, pal: &Palette) {
    ui.horizontal(|ui| {
        let (color, text) = match app.link {
            LinkState::Disconnected => (Color32::GRAY, t("state_disconnected").to_string()),
            LinkState::Connecting { waiting_device: false } => (pal.yellow, t("state_connecting").to_string()),
            LinkState::Connecting { waiting_device: true } => (pal.yellow, t("state_waiting_device").to_string()),
            LinkState::Reconnecting { attempt } => {
                (pal.yellow, tf("state_reconnecting", &[("n", &attempt.max(1).to_string())]))
            }
            LinkState::Connected { .. } => (pal.green, t("state_connected").to_string()),
        };
        ui.label(RichText::new("●").color(color).size(18.0));
        if matches!(app.link, LinkState::Connecting { .. } | LinkState::Reconnecting { .. }) {
            ui.spinner();
        }
        ui.label(text);
        if let (true, Some(id)) = (app.connected(), &app.identity) {
            ui.label(RichText::new(&id.model).strong()).on_hover_text(format!("S/N {}  FW {}", id.serial, id.firmware));
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let label =
                if matches!(app.link, LinkState::Disconnected) { t("btn_connect") } else { t("btn_disconnect") };
            if ui.button(label).clicked() {
                if matches!(app.link, LinkState::Disconnected) {
                    app.connect();
                } else {
                    app.disconnect();
                }
            }
        });
    });
}

fn output_section(app: &mut App, ui: &mut egui::Ui, pal: &Palette) {
    ui.heading(t("output_title"));
    ui.add_space(4.0);

    let lim = app.settings.safety;
    let invalid = [Qty::Voltage, Qty::Current].map(|q| app.sp.check(q, &lim).err());
    let device_bad = app.sp.device_out_of_limits(&lim, &app.cali);
    let locked = app.setpoint_locked();

    ui.horizontal(|ui| {
        ui.label(t("output_state"));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let enabled = app.connected() && !locked && !app.aux_running();
            ui.add_enabled(enabled, egui::Checkbox::new(&mut app.settings.quickset, t("output_quickset")))
                .on_hover_text(t("output_quickset_tip"));
        });
    });

    output_button(app, ui, pal, invalid.iter().any(Option::is_some) || device_bad);

    ui.add_space(6.0);
    let editable = !locked;
    for (k, q) in [Qty::Voltage, Qty::Current].into_iter().enumerate() {
        setpoint_row(app, ui, pal, q, editable, invalid[k]);
    }

    // 设备设定
    let dev_line = match (app.sp.device_value(Qty::Voltage, &app.cali), app.sp.device_value(Qty::Current, &app.cali)) {
        (Some(v), Some(i)) => tf("device_setpoint", &[("v", &milli3(v)), ("i", &milli3(i))]),
        _ => t("device_setpoint_none").into(),
    };
    ui.label(RichText::new(dev_line).small());
    if device_bad {
        ui.label(RichText::new(t("device_setpoint_out_of_limits")).small().color(pal.red));
    }
    if let Some((v, i)) = app.sp.echo_notice {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(tf("echo_changed_draft_kept", &[("v", &milli3(v)), ("i", &milli3(i))]))
                    .small()
                    .color(pal.yellow),
            );
            if ui.small_button("×").clicked() {
                app.sp.echo_notice = None;
            }
        });
    }

    // 安全边界摘要
    let summary = tf(
        "safety_summary",
        &[
            ("vmin", &trim_num(lim.v_min)),
            ("vmax", &trim_num(lim.v_max)),
            ("imin", &trim_num(lim.i_min)),
            ("imax", &trim_num(lim.i_max)),
        ],
    );
    let mut text = RichText::new(format!("{summary} ⓘ")).small();
    if !lim.is_hardware() {
        text = text.color(pal.yellow);
    }
    let r = ui.add(egui::Label::new(text).sense(egui::Sense::click())).on_hover_text(t("safety_summary_tip"));
    if r.clicked() && app.power_dialog.is_none() {
        app.power_dialog = Some(PowerDialog::new(&app.settings));
    }
    if app.aux_running() {
        ui.label(RichText::new(t("aux_locked")).small().color(pal.yellow));
    } else if locked {
        ui.label(RichText::new(t("locked_when_output")).small().color(pal.yellow));
    }

    ui.add_space(6.0);
    progress(app, ui, Qty::Voltage);
    progress(app, ui, Qty::Current);
}

fn output_button(app: &mut App, ui: &mut egui::Ui, pal: &Palette, setpoint_bad: bool) {
    let view = app.out.view;
    let disconnected = matches!(app.link, LinkState::Disconnected | LinkState::Connecting { .. });
    let reconnecting = matches!(app.link, LinkState::Reconnecting { .. });
    let (label, fill) = match view {
        _ if disconnected => (t("common_na"), Color32::GRAY),
        OutputView::Acked(true) | OutputView::Stale(true) => (t("output_on"), pal.on),
        OutputView::Acked(false) | OutputView::Stale(false) => (t("output_off"), pal.off),
        OutputView::Unknown => (t("output_unknown"), pal.red),
        OutputView::NoInfo => (t("common_na"), Color32::GRAY),
    };
    let turning_on = !view.may_be_on();
    let enabled = if disconnected {
        false
    } else if app.aux_running() {
        true
    } else if reconnecting {
        view.may_be_on()
    } else {
        !(turning_on && setpoint_bad) && app.on_after_set.is_none()
    };

    let width = ui.available_width();
    let btn = egui::Button::new(RichText::new(label).size(22.0).strong().color(Color32::BLACK))
        .fill(fill)
        .min_size(egui::vec2(width, 48.0));
    let mut resp = ui.add_enabled(enabled, btn);
    if matches!(view, OutputView::Stale(_)) && !disconnected {
        ui.painter().rect_stroke(resp.rect, 4.0, Stroke::new(2.0, pal.yellow), StrokeKind::Outside);
    }
    if app.aux_running() {
        resp = resp.on_hover_text(t("output_aux_estop_tip"));
    }
    if resp.clicked() {
        if reconnecting && !app.aux_running() {
            app.send_output(false, Source::Manual);
        } else {
            app.click_output();
        }
    }

    ui.horizontal(|ui| {
        if app.on_after_set.is_some() {
            ui.spinner();
        }
        match view {
            OutputView::Stale(_) if !disconnected => {
                ui.label(RichText::new(t("output_unconfirmed_hint")).small().color(pal.yellow));
            }
            OutputView::Acked(_) if !disconnected => {
                ui.label(RichText::new("ⓘ").weak()).on_hover_text(t("output_acked_tip"));
            }
            _ => {}
        }
        if !disconnected && turning_on && setpoint_bad {
            ui.label(RichText::new(t("safety_adjust_before_on")).small().color(pal.red));
        }
    });
}

fn setpoint_row(app: &mut App, ui: &mut egui::Ui, pal: &Palette, q: Qty, editable: bool, err: Option<LimitError>) {
    let k = if q == Qty::Voltage { 0 } else { 1 };
    let name = if q == Qty::Voltage { t("set_voltage") } else { t("set_current") };
    let mut focused = false;
    ui.horizontal(|ui| {
        ui.add_sized([72.0, 20.0], egui::Label::new(name));
        let id = egui::Id::new(("setpoint", k));
        let f = num_field(ui, id, &mut app.sp.text[k], 96.0, editable, app.settings.bitadjust);
        let resp = f.resp;
        if resp.changed() || f.stepped {
            app.sp.on_user_edit(q);
            if app.settings.quickset && app.connected() && app.sp.check(q, &app.settings.safety).is_ok() {
                app.commit(q, Source::QuickSet);
            }
        }
        focused = resp.has_focus();
        app.sp.focused[k] = focused;
        if resp.lost_focus() {
            app.commit(q, Source::Manual);
        }
        if err.is_some() {
            widgets::red_frame(ui, &resp, pal);
        }
        ui.label(q.unit());
        if app.sp.is_pending() {
            ui.spinner();
        } else if app.sp.uncommitted(q, &app.cali) {
            ui.label(RichText::new(t("setpoint_uncommitted")).small().color(pal.yellow));
        }
    });
    let (min, max) = app.settings.safety.range(q);
    let range = tf("allowed_range", &[("min", &trim_num(min)), ("max", &trim_num(max)), ("unit", q.unit())]);
    if let Some(e) = err {
        let msg = match e {
            LimitError::OutOfSafety { .. } => range,
            other => limit_error_text(&other),
        };
        ui.label(RichText::new(msg).small().color(pal.red));
    } else if focused {
        ui.label(RichText::new(range).small().weak());
    }
}

fn progress(app: &App, ui: &mut egui::Ui, q: Qty) {
    let (meas, set, unit) = match q {
        Qty::Voltage => (app.status.v, app.sp.device_value(q, &app.cali), "V"),
        Qty::Current => (app.status.i_a, app.sp.device_value(q, &app.cali), "A"),
    };
    let have = app.status.have && app.connected();
    let frac = match set {
        Some(s) if have && s > 0.0 => (meas / s).clamp(0.0, 1.0) as f32,
        _ => 0.0,
    };
    let text = match set {
        Some(s) if have => format!("{meas:.3} / {s:.3} {unit}"),
        _ => format!("-- / -- {unit}"),
    };
    ui.add(egui::ProgressBar::new(frac).text(text));
}
