//! 曲线区（F04/F05）：工具栏、上下两图（X 轴联动）、缓冲占用 + 区间滑块、统计栏。

use std::time::{Duration, Instant};

use dc3005u_usb::LinkState;
use eframe::egui::{self, Color32, RichText};
use egui_plot::{Line, Plot, Points};

use crate::app::App;
use crate::buffer::{self, Point, Series, Stats};
use crate::format::{self, Scale};
use crate::i18n::{t, tf};
use crate::theme::Palette;
use crate::ui::widgets::{self, hms};

const LINK_GROUP: &str = "graph_x";
/// 指针在数据两端之外超过这个距离（像素）就不显示读数
const HOVER_PX: f64 = 40.0;

struct PlotData {
    series: Series,
    scale: f64,
    unit: &'static str,
    stats: Option<Stats>,
    segs: Vec<Vec<[f64; 2]>>,
}

pub struct GraphView {
    pub series: [Series; 2],
    pub hold: Option<Vec<Point>>,
    pub manual: bool,
    pub confirm_clear: bool,
    /// 指针所指的数据点（上一帧），各图在此处画标记
    pub hover: Option<Point>,
    /// 滑块两端拉满：显示全部
    pub full: bool,
    /// 自定义区间：距最新点的 (start_back, end_back)；None = 跟随最新 display_pts 点
    back: Option<(usize, usize)>,
    seen_total: u64,
    interval_text: String,
    cache: Vec<PlotData>,
    cache_at: Option<Instant>,
    cache_key: Option<(usize, usize, bool, [Series; 2], bool)>,
}

impl Default for GraphView {
    fn default() -> Self {
        Self {
            series: [Series::Voltage, Series::Current],
            hold: None,
            manual: false,
            confirm_clear: false,
            hover: None,
            full: false,
            back: None,
            seen_total: 0,
            interval_text: String::new(),
            cache: Vec::new(),
            cache_at: None,
            cache_key: None,
        }
    }
}

impl GraphView {
    /// 当前显示区间（缓冲下标 lo..hi）。固定区间时随新数据平移，保持看的是同一段数据。
    fn window(&mut self, len: usize, total: u64, display_pts: usize) -> (usize, usize) {
        let added = total.saturating_sub(self.seen_total) as usize;
        self.seen_total = total;
        if self.full {
            return (0, len);
        }
        let span = display_pts.min(len);
        match &mut self.back {
            None => (len - span, len),
            Some((s, e)) => {
                if *e > 0 {
                    *s += added;
                    *e += added;
                }
                *s = (*s).min(len);
                *e = (*e).min(s.saturating_sub(span));
                (len - *s, len - *e)
            }
        }
    }

    fn set_window(&mut self, lo: usize, hi: usize, len: usize) {
        self.full = lo == 0 && hi >= len;
        self.back = if self.full { None } else { Some((len - lo, len - hi)) };
    }

    pub fn reset_window(&mut self) {
        self.full = false;
        self.back = None;
        self.cache_key = None;
    }
}

pub fn series_color(s: Series, pal: &Palette) -> Color32 {
    match s {
        Series::Voltage => pal.lcd_voltage,
        Series::Current => pal.lcd_current,
        Series::Power => pal.lcd_power,
        Series::Resistance => pal.lcd_resistance,
        Series::None => Color32::GRAY,
    }
}

/// 4 位有效数字
pub fn sig4(x: f64) -> String {
    if x == 0.0 || !x.is_finite() {
        return format!("{x:.3}");
    }
    let d = (3 - x.abs().log10().floor() as i32).clamp(0, 6) as usize;
    format!("{x:.d$}")
}

pub fn series_combo(ui: &mut egui::Ui, id: &str, tip: &str, value: &mut Series, enabled: bool) {
    ui.add_enabled_ui(enabled, |ui| {
        egui::ComboBox::from_id_salt(id)
            .selected_text(t(value.key()))
            .width(64.0)
            .show_ui(ui, |ui| {
                for s in Series::ALL {
                    ui.selectable_value(value, s, t(s.key()));
                }
            })
            .response
            .on_hover_text(tip);
    });
}

fn build<'a>(pts: impl Iterator<Item = &'a Point> + Clone, n: usize, shown: &[Series], budget: usize) -> Vec<PlotData> {
    shown
        .iter()
        .map(|&s| {
            let stats = buffer::stats(pts.clone(), s);
            let max_abs = stats.map_or(0.0, |st| st.max.abs().max(st.min.abs()));
            let (scale, unit) = s.unit_for(max_abs);
            let segs = buffer::segments(pts.clone(), n, s, scale, budget);
            PlotData { series: s, scale, unit, stats, segs }
        })
        .collect()
}

fn stats_text(d: &PlotData) -> String {
    let l = d.series.letter();
    match d.stats {
        Some(st) => format!(
            "{l}avg {a}{u}  {l}max {mx}{u}  {l}min {mn}{u}  {l}pp {pp}{u}",
            a = sig4(st.avg * d.scale),
            mx = sig4(st.max * d.scale),
            mn = sig4(st.min * d.scale),
            pp = sig4(st.pp() * d.scale),
            u = d.unit,
        ),
        None => format!("{l}: --"),
    }
}

/// 指针处的一个采样点：时间、电压、电流、功率、阻值
fn hover_readout(ui: &mut egui::Ui, p: &Point, pal: &Palette, interp: u8) {
    let pw = p.v * p.i;
    let (i_text, i_unit) = format::current(p.i, format::auto_scale(Scale::Unit, p.i.abs()), interp);
    let (p_text, p_unit) = format::power(pw, format::auto_scale(Scale::Unit, pw.abs()), interp);
    let (r_text, r_unit) = format::resistance(p.v, p.i, interp);
    let rows = [
        (Series::Voltage, format!("{:.*}", 3 + usize::from(interp), p.v), "V".to_string()),
        (Series::Current, i_text, i_unit),
        (Series::Power, p_text, p_unit),
        (Series::Resistance, r_text, r_unit),
    ];
    ui.label(RichText::new(format!("{}  {:.3}", t("graph_time"), p.t)).weak());
    egui::Grid::new("graph_hover").num_columns(3).spacing([12.0, 2.0]).show(ui, |ui| {
        for (s, value, unit) in rows {
            let c = series_color(s, pal);
            ui.label(RichText::new(t(s.key())).color(c));
            ui.label(RichText::new(value).monospace().color(c));
            ui.label(unit);
            ui.end_row();
        }
    });
}

fn toolbar(app: &mut App, ui: &mut egui::Ui, pal: &Palette) {
    let connected = app.connected();
    let held = app.graph.hold.is_some();
    ui.horizontal_wrapped(|ui| {
        ui.label(t("graph_data"));
        series_combo(ui, "graph_upper", t("graph1_tip"), &mut app.graph.series[0], !held);
        series_combo(ui, "graph_lower", t("graph2_tip"), &mut app.graph.series[1], !held);
        ui.separator();
        ui.label(t("sample_interval"));
        let g = &mut app.graph;
        let id = egui::Id::new("sample_interval");
        if !ui.memory(|m| m.has_focus(id)) {
            g.interval_text = app.settings.poll_interval_ms.to_string();
        }
        let r = ui.add_enabled(
            connected,
            egui::TextEdit::singleline(&mut g.interval_text)
                .id(id)
                .desired_width(48.0)
                .font(egui::TextStyle::Monospace),
        );
        if r.changed() {
            g.interval_text.retain(|c| c.is_ascii_digit());
        }
        let parsed = g.interval_text.trim().parse::<u32>().ok().filter(|x| (50..=65535).contains(x));
        if parsed.is_none() {
            widgets::red_frame(ui, &r, pal);
        }
        if r.lost_focus()
            && let Some(ms) = parsed
            && ms != app.settings.poll_interval_ms
        {
            app.settings.poll_interval_ms = ms;
            app.apply_settings();
        }
        ui.label("ms");
        let hz = if connected { format!("{:.1}", app.sample_rate_hz) } else { "0.0".into() };
        ui.label(tf("sample_rate", &[("hz", &hz)])).on_hover_text(t("sample_rate_tip"));
        ui.label(RichText::new("ⓘ").weak()).on_hover_text(t("sampling_note"));
        let dropped = app.data.dropped();
        if dropped > 0 {
            ui.label(RichText::new(tf("ui_dropped_samples", &[("n", &dropped.to_string())])).color(pal.yellow));
        }
        if app.sim.is_some() {
            ui.label(RichText::new(t("sim_mode")).color(pal.blue));
        }
    });

    let link = app.link;
    let connecting = matches!(link, LinkState::Connecting { .. });
    let reconnecting = matches!(link, LinkState::Reconnecting { .. });
    let normal = connected;
    ui.horizontal_wrapped(|ui| {
        let recording = app.recording.clone();
        if let Some((_, since)) = &recording {
            let stop = ui
                .add_enabled(
                    normal || reconnecting,
                    egui::Button::new(RichText::new(t("btn_record_stop")).color(pal.red)),
                )
                .on_hover_text(t("record_tip"));
            if stop.clicked() {
                app.stop_record();
            }
            ui.label(RichText::new(format!("● {}", hms(since.elapsed()))).color(pal.red).monospace());
        } else if ui.add_enabled(normal, egui::Button::new(t("btn_record"))).on_hover_text(t("record_tip")).clicked() {
            app.start_record();
        }
        let can_dump = !connecting && !reconnecting && !app.buffer.is_empty();
        if ui.add_enabled(can_dump, egui::Button::new(t("btn_dump"))).on_hover_text(t("dump_tip")).clicked() {
            app.dump_buffer();
        }
        let auto_label = if app.graph.manual { t("btn_manual") } else { t("btn_auto") };
        if ui.add_enabled(normal, egui::Button::new(auto_label)).on_hover_text(t("auto_tip")).clicked() {
            app.graph.manual = !app.graph.manual;
        }
        let hold_label = if held { t("btn_release") } else { t("btn_hold") };
        if ui.add_enabled(normal, egui::Button::new(hold_label).selected(held)).on_hover_text(t("hold_tip")).clicked() {
            toggle_hold(app);
        }
        if ui.add_enabled(normal, egui::Button::new(t("btn_clear"))).on_hover_text(t("clear_tip")).clicked() {
            app.graph.confirm_clear = true;
        }
        let float_on = app.float.open;
        if ui
            .add_enabled(normal || float_on, egui::Button::new(t("btn_float")).selected(float_on))
            .on_hover_text(t("float_tip"))
            .clicked()
        {
            app.float.open = !float_on;
        }
        if ui.add_enabled(normal, egui::Button::new(t("btn_zero"))).on_hover_text(t("zero_tip")).clicked() {
            app.zero_energy();
        }
        if ui.button(t("ana_open")).clicked() {
            app.open_analysis_dialog();
        }
    });
}

fn toggle_hold(app: &mut App) {
    let g = &mut app.graph;
    if g.hold.take().is_none() {
        let len = app.buffer.len();
        let (lo, hi) = g.window(len, app.buffer.total(), app.settings.display_pts as usize);
        g.hold = Some(app.buffer.range(lo, hi).copied().collect());
    }
    g.cache_key = None;
}

fn clear_modal(app: &mut App, ui: &egui::Ui) {
    if !app.graph.confirm_clear {
        return;
    }
    let mut action = None;
    egui::Modal::new(egui::Id::new("confirm_clear")).show(ui.ctx(), |ui| {
        ui.label(t("confirm_clear_buffer"));
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
        app.graph.confirm_clear = false;
        if ok {
            app.buffer.clear();
            app.graph.hold = None;
            app.graph.reset_window();
        }
    }
}

pub fn draw(app: &mut App, ui: &mut egui::Ui, pal: &Palette) {
    clear_modal(app, ui);
    toolbar(app, ui, pal);
    ui.separator();

    let dp = app.settings.display_pts as usize;
    let len = app.buffer.len();
    let held = app.graph.hold.is_some();
    let (lo, hi) =
        if let Some(h) = &app.graph.hold { (0, h.len()) } else { app.graph.window(len, app.buffer.total(), dp) };
    let shown: Vec<Series> = app.graph.series.iter().copied().filter(|s| *s != Series::None).collect();
    let budget = if app.settings.downsample { (ui.available_width() * 2.0) as usize } else { usize::MAX };

    // 曲线最大刷新率：区间、选择不变时，按 graph_max_fps 节流重算
    let key = (lo, hi, held, app.graph.series, app.settings.downsample);
    let period = Duration::from_secs_f64(1.0 / app.settings.graph_max_fps.max(1.0));
    let stale = app.graph.cache_at.is_none_or(|t| t.elapsed() >= period);
    let g = &mut app.graph;
    if g.cache_key != Some(key) || (stale && !held) {
        g.cache = match &g.hold {
            Some(h) => build(h.iter(), h.len(), &shown, budget),
            None => build(app.buffer.range(lo, hi), hi - lo, &shown, budget),
        };
        g.cache_key = Some(key);
        g.cache_at = Some(Instant::now());
    } else if !held {
        ui.ctx().request_repaint_after(period);
    }

    let row_h = ui.text_style_height(&egui::TextStyle::Body) + ui.spacing().item_spacing.y;
    let bottom_h = 2.0 * row_h + 18.0 + 3.0 * ui.spacing().item_spacing.y;
    let n_plots = app.graph.cache.len();
    if n_plots == 0 || (hi == lo && !held) {
        app.graph.hover = None;
        let h = (ui.available_height() - bottom_h).max(40.0);
        ui.allocate_ui(egui::vec2(ui.available_width(), h), |ui| {
            ui.centered_and_justified(|ui| {
                ui.label(RichText::new(t("graph_no_data")).weak());
            });
        });
    } else {
        let n = n_plots as f32;
        let plot_h = ((ui.available_height() - bottom_h - n * row_h) / n - ui.spacing().item_spacing.y).max(60.0);
        let manual = app.graph.manual;
        let interp = app.settings.interp;
        let (g, buf) = (&app.graph, &app.buffer);
        let lookup = |x: f64| match &g.hold {
            Some(h) => buffer::nearest(h.len(), |k| h[k], x),
            None => buffer::nearest(hi - lo, |k| *buf.get(lo + k).expect("window in buffer"), x),
        };
        let mut hover = None;
        for (k, d) in g.cache.iter().enumerate() {
            let name = t(d.series.key());
            let c = series_color(d.series, pal);
            ui.label(RichText::new(format!("{name} ({})", d.unit)).color(c).strong());
            let mut plot = Plot::new(("graph", k))
                .height(plot_h)
                .link_axis(LINK_GROUP, [true, false])
                .link_cursor(LINK_GROUP, [true, false])
                .y_axis_min_width(56.0)
                .allow_drag(manual)
                .allow_zoom(manual)
                .allow_scroll(manual)
                .allow_boxed_zoom(manual)
                .allow_double_click_reset(manual);
            plot = if k + 1 == n_plots { plot.x_axis_label(t("graph_time")) } else { plot.show_axes([false, true]) };
            let resp = plot.show(ui, |pui| {
                if !manual {
                    pui.set_auto_bounds([true, true]);
                }
                for seg in &d.segs {
                    pui.line(Line::new(name, seg.clone()).color(c).width(1.5));
                }
                if let Some(p) = g.hover
                    && let y = d.series.value(&p)
                    && y.is_finite()
                {
                    pui.add(Points::new("", vec![[p.t, y * d.scale]]).radius(4.0).color(c).filled(true));
                }
                pui.pointer_coordinate().map(|pos| (pos.x, pui.transform().dpos_dvalue_x().abs()))
            });
            if let Some((x, px_per_s)) = resp.inner
                && resp.response.hovered()
                && let (Some(first), Some(last)) = (lookup(f64::NEG_INFINITY), lookup(f64::INFINITY))
                && x > first.t - HOVER_PX / px_per_s
                && x < last.t + HOVER_PX / px_per_s
                && let Some(p) = lookup(x)
            {
                hover = Some(p);
                resp.response.on_hover_ui_at_pointer(|ui| hover_readout(ui, &p, pal, interp));
            }
        }
        app.graph.hover = hover;
    }

    // 缓冲占用 + 区间滑块
    ui.horizontal(|ui| {
        let usage = app.buffer.usage();
        let mut text = RichText::new(tf("buffer_usage", &[("pct", &format!("{:.1}", usage * 100.0))]));
        if usage > 0.95 {
            text = text.color(pal.yellow);
        }
        ui.label(text).on_hover_text(t("buffer_usage_tip"));
        let range_text = if held { format!("HOLD {}", hi - lo) } else { format!("{lo}~{hi} / {len}") };
        let label_w = 150.0;
        let enabled = app.connected() && len > dp + 5 && !app.graph.manual && !held;
        let (mut flo, mut fhi) = (lo as f64, hi as f64);
        let w = ui.available_width() - label_w - ui.spacing().item_spacing.x;
        let span = dp.min(len) as f64;
        if widgets::range_slider(ui, &mut flo, &mut fhi, 0.0, len as f64, span, enabled, w) {
            let (nlo, nhi) = (flo.round() as usize, (fhi.round() as usize).min(len));
            app.graph.set_window(nlo, nhi.max(nlo), len);
        }
        ui.label(RichText::new(range_text).monospace().small()).on_hover_text(t("display_range_tip"));
    });
    let line = app.graph.cache.iter().map(stats_text).collect::<Vec<_>>().join("  |  ");
    ui.label(RichText::new(if line.is_empty() { "--".into() } else { line }).monospace().small());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_follow_fixed_and_full() {
        let mut g = GraphView::default();
        assert_eq!(g.window(1000, 1000, 400), (600, 1000));
        // 固定区间：新数据进来时跟着平移，看的还是同一段数据
        g.set_window(100, 600, 1000);
        assert_eq!(g.window(1000, 1000, 400), (100, 600));
        assert_eq!(g.window(1010, 1010, 400), (100, 600));
        // 拖到最右：跟随但保留宽度
        g.set_window(500, 1010, 1010);
        assert_eq!(g.window(1020, 1020, 400), (510, 1020));
        g.set_window(0, 1020, 1020);
        assert!(g.full);
        assert_eq!(g.window(1030, 1030, 400), (0, 1030));
    }
}
