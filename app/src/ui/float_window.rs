//! 悬浮窗（F09）：置顶、无边框，显示 U/I/P。左键拖动，双击折叠，滚轮调透明度，右键菜单。

use eframe::egui::{self, Color32, RichText, Sense, ViewportClass, ViewportCommand};

use crate::app::App;
use crate::format;
use crate::i18n::t;
use crate::theme::Palette;
use crate::ui::widgets;

const SIZE: [f32; 2] = [150.0, 140.0];
const SIZE_COLLAPSED: [f32; 2] = [150.0, 80.0];

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FloatState {
    pub open: bool,
    pub collapsed: bool,
    pub opacity: f32,
}

impl Default for FloatState {
    fn default() -> Self {
        Self { open: false, collapsed: false, opacity: 0.95 }
    }
}

struct Values {
    rows: [(&'static str, String, String, Color32); 3],
}

fn values(app: &App, pal: &Palette) -> Values {
    let s = &app.status;
    let have = s.have && app.connected();
    let interp = app.settings.interp;
    let dash = || ("--".to_string(), String::new());
    let (v, vu) = if have { (format!("{:.*}", 3 + usize::from(interp), s.v), "V".into()) } else { dash() };
    let (i, iu) = if have { format::current(s.i_a, s.i_scale, interp) } else { dash() };
    let (p, pu) = if have { format::power(s.power_w(), s.p_scale, interp) } else { dash() };
    Values {
        rows: [
            ("float_voltage", v, vu, pal.lcd_voltage),
            ("float_current", i, iu, pal.lcd_current),
            ("float_power", p, pu, pal.lcd_power),
        ],
    }
}

fn body(ui: &mut egui::Ui, vals: &Values, st: &mut FloatState, embedded: bool) {
    let rect = ui.max_rect();
    let bg = ui.visuals().panel_fill.gamma_multiply(st.opacity);
    ui.painter().rect_filled(rect, 6.0, bg);
    let resp = ui.interact(rect, ui.id().with("float_bg"), Sense::click_and_drag());
    if !embedded && resp.drag_started() {
        ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
    }
    if resp.double_clicked() {
        st.collapsed = !st.collapsed;
    }
    if resp.hovered() {
        let dy = widgets::wheel(ui);
        if dy != 0.0 {
            st.opacity = (st.opacity + dy * 0.05).clamp(0.1, 1.0);
        }
    }
    resp.context_menu(|ui| {
        let (key, collapse) = if st.collapsed { ("float_expand", false) } else { ("float_collapse", true) };
        if ui.button(t(key)).clicked() {
            st.collapsed = collapse;
            ui.close();
        }
        if ui.button(t("common_close")).clicked() {
            st.open = false;
            ui.close();
        }
    });
    ui.set_opacity(st.opacity.max(0.3));
    ui.add_space(4.0);
    for (key, val, unit, color) in &vals.rows {
        if !st.collapsed {
            ui.label(RichText::new(t(key)).small().weak());
        }
        ui.horizontal(|ui| {
            ui.label(RichText::new(val).monospace().size(18.0).strong().color(*color));
            ui.label(RichText::new(unit).small().color(*color));
        });
    }
}

pub fn draw(app: &mut App, ctx: &egui::Context, pal: &Palette) {
    if !app.float.open {
        return;
    }
    let vals = values(app, pal);
    let mut st = app.float;
    let size = if st.collapsed { SIZE_COLLAPSED } else { SIZE };
    let builder = egui::ViewportBuilder::default()
        .with_title("IOT_Power")
        .with_inner_size(size)
        .with_resizable(false)
        .with_always_on_top()
        .with_decorations(false)
        .with_transparent(true);
    ctx.show_viewport_immediate(egui::ViewportId::from_hash_of("float_window"), builder, |ui, class| {
        let embedded = class == ViewportClass::EmbeddedWindow;
        if embedded {
            body(ui, &vals, &mut st, true);
        } else {
            egui::CentralPanel::default().frame(egui::Frame::new().inner_margin(6.0)).show(ui, |ui| {
                body(ui, &vals, &mut st, false);
            });
            if ui.input(|i| i.viewport().close_requested()) {
                st.open = false;
            }
        }
    });
    app.float = st;
}
