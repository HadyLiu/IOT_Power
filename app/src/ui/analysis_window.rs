//! 记录分析窗口（F17，`Doc/界面设计.md` §3.4）：后台解析、两图 + 事件竖线、拖选区间、回放、区间统计、导出。

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Instant;

use crossbeam_channel::Receiver;
use eframe::egui::{self, Color32, RichText};
use egui_plot::{Line, Plot, Span, VLine};

use crate::analysis::{self, LogData, LogEvent, RangeStats};
use crate::app::{App, ToastKind};
use crate::buffer::{self, Series};
use crate::i18n::{t, tf};
use crate::theme::Palette;
use crate::ui::graph_panel::{series_color, series_combo, sig4};
use crate::ui::widgets::{self, short_duration};

const SPEEDS: [f64; 3] = [1.0, 10.0, 100.0];
const EVENTS_W: f32 = 230.0;

enum State {
    Loading { rx: Receiver<Result<LogData, String>>, progress: Arc<AtomicU32> },
    Ready(Box<LogData>),
    Failed(String),
}

pub struct AnalysisWindow {
    id: u64,
    title: String,
    state: State,
    series: [Series; 2],
    sel: (f64, f64),
    speed: f64,
    playing: bool,
    cursor: Option<f64>,
    last_tick: Option<Instant>,
    drag_from: Option<f64>,
    stats: Option<((f64, f64), Option<RangeStats>)>,
    pub open: bool,
}

impl AnalysisWindow {
    pub fn open(id: u64, paths: Vec<PathBuf>, repaint: egui::Context) -> Self {
        let title =
            paths.first().and_then(|p| p.file_name()).map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let progress = Arc::new(AtomicU32::new(0));
        let (tx, rx) = crossbeam_channel::bounded(1);
        let pr = progress.clone();
        std::thread::Builder::new()
            .name("log-parse".into())
            .spawn(move || {
                let r = analysis::parse_files(&paths, &pr);
                let _ = tx.send(r);
                repaint.request_repaint();
            })
            .expect("spawn parser");
        Self {
            id,
            title,
            state: State::Loading { rx, progress },
            series: [Series::Voltage, Series::Current],
            sel: (0.0, 1.0),
            speed: 1.0,
            playing: false,
            cursor: None,
            last_tick: None,
            drag_from: None,
            stats: None,
            open: true,
        }
    }
}

fn event_color(e: &LogEvent, pal: &Palette) -> Color32 {
    match e.kind.as_str() {
        "SETPOINT" => pal.blue,
        "OUTPUT" if e.field("on") == Some("1") => pal.green,
        "OUTPUT" => pal.yellow,
        "LINK" | "ESTOP" => pal.red,
        "CMD_FAIL" => Color32::from_rgb(0xFF, 0x8C, 0x00),
        _ => Color32::GRAY,
    }
}

fn event_text(e: &LogEvent) -> String {
    format!("{:.1} s  {}  {}", e.t, e.kind, e.fields.replace('\t', " "))
}

fn pick_logs() -> Option<Vec<PathBuf>> {
    rfd::FileDialog::new().add_filter(t("txt_filter"), &["txt"]).pick_files().filter(|v| !v.is_empty())
}

fn stats_grid(ui: &mut egui::Ui, s: &RangeStats) {
    ui.horizontal_wrapped(|ui| {
        ui.label(
            RichText::new(format!(
                "{}  {:.3} ~ {:.3} s · {} · {} pts",
                t("ana_stats"),
                s.t0,
                s.t1,
                tf("ana_duration", &[("t", &short_duration(s.t1 - s.t0))]),
                s.n
            ))
            .strong(),
        );
    });
    ui.horizontal(|ui| {
        egui::Grid::new("ana_stats").num_columns(5).spacing([16.0, 2.0]).striped(true).show(ui, |ui| {
            ui.label("");
            for k in ["ana_max", "ana_min", "ana_avg", "ana_rms"] {
                ui.label(RichText::new(t(k)).strong());
            }
            ui.end_row();
            for (name, c, s) in [("V", &s.v, Series::Voltage), ("I", &s.i, Series::Current), ("P", &s.p, Series::Power)]
            {
                let (scale, unit) = s.unit_for(c.max.abs().max(c.min.abs()));
                ui.label(name);
                for x in [c.max, c.min, c.avg, c.rms] {
                    ui.label(RichText::new(format!("{}{unit}", sig4(x * scale))).monospace());
                }
                ui.end_row();
            }
        });
        ui.separator();
        ui.vertical(|ui| {
            ui.label(tf("ana_energy", &[("wh", &format!("{:.4}", s.wh))]));
            ui.label(tf("ana_capacity", &[("mah", &format!("{:.2}", s.mah))]));
            ui.label(tf("ana_gaps", &[("n", &s.gaps.to_string()), ("t", &short_duration(s.gap_s))]));
        });
    });
}

fn ready_ui(w: &mut AnalysisWindow, ui: &mut egui::Ui, pal: &Palette, toast: &mut Option<(ToastKind, String)>) {
    let State::Ready(d) = &w.state else { return };
    let span = d.span();

    ui.horizontal_wrapped(|ui| {
        if ui.button(t("ana_export")).clicked() {
            let (lo, hi) = d.index_range(w.sel.0, w.sel.1);
            if let Some(p) = rfd::FileDialog::new()
                .add_filter(t("csv_filter"), &["csv"])
                .set_file_name("iot_power_range.csv")
                .save_file()
            {
                *toast = Some(match analysis::write_points_csv(&p, &d.points[lo..hi]) {
                    Ok(()) => (ToastKind::Info, format!("{} {}", t("dump_done"), p.display())),
                    Err(e) => (ToastKind::Error, format!("{}: {e}", p.display())),
                });
            }
        }
        ui.separator();
        ui.label(t("ana_play"));
        let label = if w.playing { "⏸" } else { "▶" };
        if ui.button(label).on_hover_text(if w.playing { t("ana_pause") } else { t("ana_play") }).clicked() {
            w.playing = !w.playing;
            w.last_tick = None;
            if w.playing && w.cursor.is_none_or(|c| c >= span.1) {
                w.cursor = Some(w.sel.0);
            }
        }
        ui.label(t("ana_speed"));
        egui::ComboBox::from_id_salt(("ana_speed", w.id)).selected_text(format!("{}×", w.speed)).width(56.0).show_ui(
            ui,
            |ui| {
                for s in SPEEDS {
                    ui.selectable_value(&mut w.speed, s, format!("{s}×"));
                }
            },
        );
        if ui.button(t("ana_all")).clicked() {
            w.sel = span;
        }
    });
    let hdr = |k: &str| d.header(k).unwrap_or("—").to_string();
    ui.label(
        RichText::new(format!(
            "{} · {} · {} ms · {}",
            hdr("device"),
            hdr("start"),
            hdr("poll_interval_ms"),
            hdr("safety")
        ))
        .small()
        .weak(),
    );
    if d.truncated {
        ui.label(RichText::new(t("ana_truncated")).small().color(pal.yellow));
    }
    if d.newer_version {
        ui.label(RichText::new(t("ana_newer_version")).small().color(pal.yellow));
    }

    // 回放：游标按时间推进，越过区间右端时窗口跟着平移
    if w.playing {
        let now = Instant::now();
        let dt = w.last_tick.map_or(0.0, |l| now.duration_since(l).as_secs_f64());
        w.last_tick = Some(now);
        let c = w.cursor.unwrap_or(w.sel.0) + dt * w.speed;
        if c >= span.1 {
            w.cursor = Some(span.1);
            w.playing = false;
        } else {
            w.cursor = Some(c);
            if c > w.sel.1 {
                let width = w.sel.1 - w.sel.0;
                w.sel = (c - width * 0.1, c - width * 0.1 + width);
            }
        }
        ui.ctx().request_repaint();
    }

    let (lo, hi) = d.index_range(w.sel.0, w.sel.1);
    let pts = &d.points[lo..hi];
    let budget = (ui.available_width() * 2.0) as usize;
    let stats_h = 110.0;
    let plot_h = ((ui.available_height() - stats_h - 60.0) / 2.0).max(80.0);
    let mut jump = None;
    let mut new_sel = None;

    ui.horizontal_top(|ui| {
        ui.vertical(|ui| {
            ui.set_width((ui.available_width() - EVENTS_W).max(200.0));
            for k in 0..2 {
                series_combo(ui, &format!("ana_s{}_{k}", w.id), "", &mut w.series[k], true);
                let s = w.series[k];
                if s == Series::None {
                    continue;
                }
                let st = buffer::stats(pts, s);
                let (scale, unit) = s.unit_for(st.map_or(0.0, |x| x.max.abs().max(x.min.abs())));
                let segs = buffer::segments(pts, pts.len(), s, scale, budget);
                let c = series_color(s, pal);
                let events = &d.events;
                let sel = w.sel;
                let cursor = w.cursor;
                let drag_from = w.drag_from;
                let resp = Plot::new(("ana_plot", w.id, k))
                    .height(plot_h)
                    .link_axis(format!("ana_x{}", w.id), [true, false])
                    .link_cursor(format!("ana_x{}", w.id), [true, false])
                    .y_axis_label(format!("{} ({unit})", t(s.key())))
                    .y_axis_min_width(56.0)
                    .allow_drag(false)
                    .allow_zoom(false)
                    .allow_scroll(false)
                    .allow_boxed_zoom(false)
                    .allow_double_click_reset(false)
                    .show(ui, |pui| {
                        pui.set_plot_bounds_x(sel.0..=sel.1);
                        pui.set_auto_bounds([false, true]);
                        for seg in segs {
                            pui.line(Line::new(t(s.key()), seg).color(c).width(1.5));
                        }
                        for e in events.iter().filter(|e| e.t >= sel.0 && e.t <= sel.1) {
                            pui.vline(VLine::new(&e.kind, e.t).color(event_color(e, pal)).width(1.0));
                        }
                        if let Some(x) = cursor {
                            pui.vline(VLine::new("cursor", x).color(pal.yellow).width(2.0));
                        }
                        let x = pui.pointer_coordinate().map(|p| p.x);
                        let r = pui.response().clone();
                        let mut drag = drag_from;
                        let mut done = None;
                        if r.drag_started() {
                            drag = x;
                        }
                        if let (Some(a), Some(b)) = (drag, x) {
                            pui.span(Span::new("sel", a.min(b)..=a.max(b)).fill(pal.blue.gamma_multiply(0.2)));
                            if r.drag_stopped() {
                                done = Some((a.min(b), a.max(b)));
                                drag = None;
                            }
                        }
                        if !r.dragged() && !r.drag_stopped() {
                            drag = None;
                        }
                        let tol = (sel.1 - sel.0) * 0.004;
                        let hover = x.and_then(|x| events.iter().find(|e| (e.t - x).abs() <= tol)).map(event_text);
                        (drag, done, hover)
                    });
                let (drag, done, hover) = resp.inner;
                w.drag_from = drag;
                if let Some((a, b)) = done
                    && b - a > (sel.1 - sel.0) * 0.002
                {
                    new_sel = Some((a, b));
                }
                if let Some(h) = hover {
                    resp.response.on_hover_text(h);
                }
            }
        });
        ui.vertical(|ui| {
            ui.set_width(EVENTS_W);
            ui.label(RichText::new(t("ana_events")).strong());
            egui::ScrollArea::vertical().id_salt(("ana_ev", w.id)).max_height(plot_h * 2.0 + 40.0).show(ui, |ui| {
                for e in &d.events {
                    let r = ui.add(
                        egui::Label::new(RichText::new(event_text(e)).small().color(event_color(e, pal)))
                            .truncate()
                            .sense(egui::Sense::click()),
                    );
                    if r.on_hover_text(event_text(e)).clicked() {
                        jump = Some(e.t);
                    }
                }
            });
        });
    });

    if let Some(s) = new_sel {
        w.sel = s;
    }
    if let Some(tj) = jump {
        let width = (w.sel.1 - w.sel.0).min(span.1 - span.0);
        let a = (tj - width / 2.0).clamp(span.0, span.1 - width);
        w.sel = (a, a + width);
        w.cursor = Some(tj);
    }
    let (mut a, mut b) = w.sel;
    let width = ui.available_width();
    if widgets::range_slider(ui, &mut a, &mut b, span.0, span.1, (span.1 - span.0) / 10_000.0, true, width) {
        w.sel = (a, b);
    }

    if w.stats.as_ref().is_none_or(|(k, _)| *k != w.sel) {
        let (lo, hi) = d.index_range(w.sel.0, w.sel.1);
        w.stats = Some((w.sel, analysis::range_stats(&d.points[lo..hi])));
    }
    if let Some((_, Some(s))) = &w.stats {
        stats_grid(ui, s);
    }
}

pub fn draw(app: &mut App, ctx: &egui::Context, pal: &Palette) {
    let mut toast = None;
    let mut reload = Vec::new();
    for w in &mut app.analyses {
        if let State::Loading { rx, .. } = &w.state
            && let Ok(r) = rx.try_recv()
        {
            w.state = match r {
                Ok(d) => {
                    let span = d.span();
                    w.sel = span;
                    State::Ready(Box::new(d))
                }
                Err(e) => State::Failed(e),
            };
        }
        let mut open = w.open;
        egui::Window::new(format!("{} - {}", t("ana_title"), w.title))
            .id(egui::Id::new(("analysis", w.id)))
            .open(&mut open)
            .default_size([900.0, 620.0])
            .show(ctx, |ui| {
                if matches!(w.state, State::Ready(_)) {
                    ready_ui(w, ui, pal, &mut toast);
                    return;
                }
                match &w.state {
                    State::Loading { progress, .. } => {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(tf("ana_loading", &[("pct", &progress.load(Ordering::Relaxed).to_string())]));
                        });
                        ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
                    }
                    State::Failed(e) => {
                        ui.label(RichText::new(tf("ana_bad_file", &[("reason", e)])).color(pal.red));
                        if ui.button(t("ana_load")).clicked() {
                            reload.push(w.id);
                        }
                    }
                    State::Ready(_) => {}
                }
            });
        w.open = open;
    }
    app.analyses.retain(|w| w.open);
    for id in reload {
        if let Some(paths) = pick_logs()
            && let Some(w) = app.analyses.iter_mut().find(|w| w.id == id)
        {
            *w = AnalysisWindow::open(id, paths, ctx.clone());
        }
    }
    if let Some((k, s)) = toast {
        app.toast(k, s);
    }
}

pub fn pick() -> Option<Vec<PathBuf>> {
    pick_logs()
}
