//! 状态区（F02）：电压、电流、功率、能量、平均功率、负载阻值。

use eframe::egui::{self, Color32, RichText};

use crate::app::App;
use crate::format::{self, Scale, auto_scale};
use crate::i18n::t;
use crate::theme::Palette;

const BIG: f32 = 30.0;
const MIN: f32 = 14.0;
/// 等宽字体字宽约为字号的 0.62 倍
const MONO_ASPECT: f32 = 0.62;

fn cell(ui: &mut egui::Ui, label: String, value: &str, color: Color32) {
    let chars = value.chars().count().max(4) as f32;
    let size = (ui.available_width() / (chars * MONO_ASPECT)).clamp(MIN, BIG);
    ui.add(egui::Label::new(RichText::new(label).small()).wrap_mode(egui::TextWrapMode::Truncate));
    ui.add(
        egui::Label::new(RichText::new(value).monospace().size(size).color(color))
            .wrap_mode(egui::TextWrapMode::Extend),
    );
}

fn row(ui: &mut egui::Ui, cells: [(String, &str, Color32); 3]) {
    ui.columns(3, |cols| {
        for (col, (label, value, color)) in cols.iter_mut().zip(cells) {
            cell(col, label, value, color);
        }
    });
}

fn fixed_scale(unit: &str) -> Option<Scale> {
    Scale::parse_unit(unit)
}

pub fn draw(app: &mut App, ui: &mut egui::Ui, pal: &Palette) {
    let s = &mut app.status;
    let interp = app.settings.interp;
    let have = s.have && !matches!(app.link, dc3005u_usb::LinkState::Disconnected);

    s.i_scale = fixed_scale(&app.settings.current_unit).unwrap_or_else(|| auto_scale(s.i_scale, s.i_a.abs()));
    let p = s.power_w();
    s.p_scale = auto_scale(s.p_scale, p.abs());
    if let Some(avg) = s.avg_power_w {
        s.avgp_scale = auto_scale(s.avgp_scale, avg.abs());
    }

    let dash = "--";
    let v_text = if have { format!("{:.*}", 3 + usize::from(interp), s.v) } else { dash.into() };
    let (i_text, i_unit) = format::current(s.i_a, s.i_scale, interp);
    let (p_text, p_unit) = format::power(p, s.p_scale, interp);
    let (r_text, r_unit) = format::resistance(s.v, s.i_a, interp);
    let e_text = if have { format!("{:.3}", s.energy_j) } else { dash.into() };
    let (a_text, a_unit) = match s.avg_power_w {
        Some(a) => format::power(a, s.avgp_scale, interp),
        None => (dash.into(), "W".into()),
    };
    let pick = |x: String| if have { x } else { dash.to_string() };
    let held = if app.graph.hold.is_some() { format!(" · {}", t("btn_hold")) } else { String::new() };

    ui.add_space(4.0);
    let (p_text, i_text, r_text) = (pick(p_text), pick(i_text), pick(r_text));
    row(
        ui,
        [
            (format!("{} (V)", t("lcd_voltage")), &v_text, pal.lcd_voltage),
            (format!("{} ({p_unit})", t("lcd_power")), &p_text, pal.lcd_power),
            (format!("{} ({a_unit}){held}", t("lcd_avg_power")), &a_text, pal.lcd_avg_power),
        ],
    );
    ui.add_space(2.0);
    row(
        ui,
        [
            (format!("{} ({i_unit})", t("lcd_current")), &i_text, pal.lcd_current),
            (format!("{}{held}", t("lcd_energy")), &e_text, pal.lcd_energy),
            (format!("{} ({r_unit})", t("lcd_load_r")), &r_text, pal.lcd_resistance),
        ],
    );
    ui.add_space(4.0);
}
