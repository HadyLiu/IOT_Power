pub mod analysis_window;
pub mod aux_panel;
pub mod float_window;
pub mod graph_panel;
pub mod graphics_dialog;
pub mod output_panel;
pub mod power_dialog;
pub mod result_window;
pub mod status_panel;
pub mod widgets;

use dc3005u_usb::{DeviceError, LinkState};
use eframe::egui::{self, Align2, Color32, RichText};

use crate::app::{App, ExitPhase, ToastKind, kind_key};
use crate::i18n::{t, tf};
use crate::theme::Palette;
use crate::ui::widgets::hms;

pub fn draw(app: &mut App, ui: &mut egui::Ui) {
    let pal = app.pal;
    egui::Panel::top("status").show(ui, |ui| {
        status_panel::draw(app, ui, &pal);
        banners(app, ui, &pal);
    });
    egui::Panel::bottom("statusbar").show(ui, |ui| status_bar(app, ui, &pal));
    egui::Panel::left("left").default_size(330.0).min_size(300.0).show(ui, |ui| {
        output_panel::draw(app, ui, &pal);
    });
    egui::CentralPanel::default().show(ui, |ui| graph_panel::draw(app, ui, &pal));

    let ctx = ui.ctx().clone();
    if let Some(mut d) = app.power_dialog.take()
        && power_dialog::draw(app, &mut d, &ctx, &pal)
    {
        app.power_dialog = Some(d);
    }
    if let Some(mut d) = app.graphics.take()
        && graphics_dialog::draw(app, &mut d, &ctx)
    {
        app.graphics = Some(d);
    }
    result_window::draw(app, &ctx, &pal);
    analysis_window::draw(app, &ctx, &pal);
    float_window::draw(app, &ctx, &pal);
    modals(app, &ctx, &pal);
    toasts(app, &ctx, &pal);
}

fn status_bar(app: &mut App, ui: &mut egui::Ui, pal: &Palette) {
    ui.horizontal(|ui| {
        if let Some((path, since)) = app.recording.clone() {
            let r = ui
                .add(
                    egui::Label::new(
                        RichText::new(format!("● {} {}", t("recording"), hms(since.elapsed())))
                            .color(pal.red)
                            .monospace(),
                    )
                    .sense(egui::Sense::click()),
                )
                .on_hover_text(path.display().to_string());
            if r.clicked() {
                widgets::open_path(&path);
            }
            ui.separator();
        }
        if let Some(k) = app.aux_status.running {
            ui.label(RichText::new(format!("● {}", t(kind_key(k)))).color(pal.green));
            if app.aux_status.clamped > 0 {
                let n = app.aux_status.clamped.to_string();
                ui.label(RichText::new(tf("runtime_clamped", &[("n", &n)])).color(pal.yellow));
            }
            ui.separator();
        }
        if app.http_active() {
            ui.label(RichText::new(t("remote_active")).color(pal.blue))
                .on_hover_text(format!("http://127.0.0.1:{}/api/status", app.settings.http_port));
            ui.separator();
        }
        if let Some(e) = app.last_error.clone() {
            let r = ui
                .add(egui::Label::new(RichText::new(&e).color(pal.red).small()).truncate().sense(egui::Sense::click()))
                .on_hover_text(t("click_for_details"));
            if r.clicked() {
                app.modal_notice = Some(e);
                app.last_error = None;
            }
        }
    });
}

fn banner(ui: &mut egui::Ui, color: Color32, text: &str) {
    egui::Frame::new().fill(color).inner_margin(6.0).corner_radius(4.0).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(RichText::new(text).color(Color32::WHITE).strong());
    });
}

fn banners(app: &App, ui: &mut egui::Ui, pal: &Palette) {
    let reconnecting = matches!(app.link, LinkState::Reconnecting { .. });
    if reconnecting {
        banner(ui, pal.red, t("link_lost_banner"));
        if app.out.pending_off {
            banner(ui, pal.red, t("pending_off_queued"));
        }
    } else if app.out.off_failed || (app.out.pending_off && app.connected()) {
        banner(ui, pal.red, t("cmd_off_failed"));
    }
}

fn device_error_text(e: &DeviceError) -> String {
    match e {
        DeviceError::NotFound => t("err_not_found").into(),
        DeviceError::Busy => t("err_busy").into(),
        DeviceError::Permission => t("err_permission").into(),
        DeviceError::Handshake => t("err_handshake").into(),
        DeviceError::Io(s) => s.clone(),
    }
}

fn ok_cancel(ui: &mut egui::Ui, action: &mut Option<bool>) {
    ui.horizontal(|ui| {
        if ui.button(t("common_ok")).clicked() {
            *action = Some(true);
        }
        if ui.button(t("common_cancel")).clicked() {
            *action = Some(false);
        }
    });
}

fn modals(app: &mut App, ctx: &egui::Context, pal: &Palette) {
    if let Some(phase) = &app.exit {
        let connected = app.connected();
        let mut action = None;
        egui::Modal::new(egui::Id::new("exit_modal")).show(ctx, |ui| {
            ui.set_max_width(380.0);
            ui.heading(t("common_warning"));
            match phase {
                ExitPhase::TurningOff { .. } => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(t("exit_turning_off"));
                    });
                }
                ExitPhase::Ask | ExitPhase::OffFailed => {
                    if matches!(phase, ExitPhase::OffFailed) {
                        ui.label(RichText::new(t("cmd_off_failed")).color(pal.red));
                    }
                    if connected {
                        ui.label(t("exit_output_on"));
                        ui.horizontal(|ui| {
                            if ui.button(t("exit_off_and_quit")).clicked() {
                                action = Some(0);
                            }
                            if ui.button(t("exit_keep_and_quit")).clicked() {
                                action = Some(1);
                            }
                            if ui.button(t("common_cancel")).clicked() {
                                action = Some(2);
                            }
                        });
                    } else {
                        ui.label(t("exit_disconnected"));
                        ui.horizontal(|ui| {
                            if ui.button(t("common_quit")).clicked() {
                                action = Some(1);
                            }
                            if ui.button(t("common_cancel")).clicked() {
                                action = Some(2);
                            }
                        });
                    }
                }
            }
        });
        match action {
            Some(0) => app.exit_turn_off(),
            Some(1) => app.quit(),
            Some(2) => app.exit = None,
            _ => {}
        }
        return;
    }

    if app.confirm_on {
        let mut action = None;
        egui::Modal::new(egui::Id::new("confirm_on")).show(ctx, |ui| {
            ui.heading(t("common_warning"));
            ui.label(t("confirm_output_on"));
            ui.label(
                RichText::new(format!("{} V / {} A", app.sp.text[0].trim(), app.sp.text[1].trim()))
                    .monospace()
                    .size(18.0),
            );
            ok_cancel(ui, &mut action);
        });
        if let Some(ok) = action {
            app.confirm_on = false;
            if ok {
                app.turn_on();
            }
        }
        return;
    }

    if let Some(req) = &app.confirm_aux {
        let mut action = None;
        let name = t(kind_key(req.job.kind()));
        egui::Modal::new(egui::Id::new("confirm_aux")).show(ctx, |ui| {
            ui.set_max_width(360.0);
            ui.heading(t("common_warning"));
            ui.label(t("confirm_output_on"));
            ui.label(RichText::new(tf("aux_will_turn_on", &[("name", name)])).small());
            ok_cancel(ui, &mut action);
        });
        if let Some(ok) = action {
            let req = app.confirm_aux.take();
            if ok
                && let Some(r) = req
                && app.connected()
            {
                app.send_aux(*r);
            }
        }
        return;
    }

    if let Some(e) = app.link_error.clone() {
        let mut action = None;
        egui::Modal::new(egui::Id::new("link_error")).show(ctx, |ui| {
            ui.set_max_width(420.0);
            ui.heading(t("connect_failed"));
            ui.label(device_error_text(&e));
            ui.horizontal(|ui| {
                if ui.button(t("common_retry")).clicked() {
                    action = Some(true);
                }
                if ui.button(t("common_close")).clicked() {
                    action = Some(false);
                }
            });
        });
        match action {
            Some(true) => app.connect(),
            Some(false) => app.link_error = None,
            None => {}
        }
        return;
    }

    if let Some(files) = app.saved_files.clone() {
        let mut close = false;
        egui::Modal::new(egui::Id::new("saved_files")).show(ctx, |ui| {
            ui.set_max_width(460.0);
            ui.heading(t("record_done"));
            ui.label(t("data_saved_to"));
            for f in &files {
                ui.label(RichText::new(f.display().to_string()).monospace().small());
            }
            ui.horizontal(|ui| {
                if ui.button(t("open_path")).clicked()
                    && let Some(f) = files.first()
                {
                    widgets::open_path(f);
                }
                if ui.button(t("ana_open")).clicked() {
                    let txt: Vec<_> =
                        files.iter().filter(|p| p.extension().is_some_and(|e| e == "txt")).cloned().collect();
                    if !txt.is_empty() {
                        app.open_analysis(txt);
                    }
                    close = true;
                }
                if ui.button(t("common_ok")).clicked() {
                    close = true;
                }
            });
        });
        if close {
            app.saved_files = None;
        }
        return;
    }

    if let Some(text) = app.modal_notice.clone() {
        let mut close = false;
        egui::Modal::new(egui::Id::new("notice")).show(ctx, |ui| {
            ui.set_max_width(380.0);
            ui.heading(t("common_warning"));
            ui.label(text);
            close = ui.button(t("common_ok")).clicked();
        });
        if close {
            app.modal_notice = None;
        }
    }
}

fn toasts(app: &mut App, ctx: &egui::Context, pal: &Palette) {
    if app.toasts.is_empty() {
        return;
    }
    egui::Area::new(egui::Id::new("toasts"))
        .anchor(Align2::RIGHT_BOTTOM, [-12.0, -36.0])
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            ui.set_max_width(420.0);
            let mut dismiss = None;
            for (k, toast) in app.toasts.iter().enumerate() {
                let fill = match toast.kind {
                    ToastKind::Info => pal.blue.gamma_multiply(0.85),
                    ToastKind::Warn => pal.yellow.gamma_multiply(0.75),
                    ToastKind::Error => pal.red,
                };
                let r = egui::Frame::new().fill(fill).inner_margin(8.0).corner_radius(6.0).show(ui, |ui| {
                    ui.label(RichText::new(&toast.text).color(Color32::BLACK));
                });
                if r.response.interact(egui::Sense::click()).clicked() {
                    dismiss = Some(k);
                }
            }
            if let Some(k) = dismiss {
                app.toasts.remove(k);
            }
        });
}
