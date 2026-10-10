//! 结果曲线窗（RW，`Doc/详细设计.md` §4.6）：扫描响应 / 电池曲线；X/Y 交换、多项式拟合、十字线、导出。

use dc3005u_proto::Qty;
use eframe::egui::{self, RichText};
use egui_plot::{Line, Plot, Points};

use crate::app::{App, ToastKind};
use crate::auxfn::battery::Curve;
use crate::auxfn::{Kind, SweepResult};
use crate::buffer::Series;
use crate::fit;
use crate::i18n::t;
use crate::theme::Palette;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    Sweep,
    Battery,
}

pub struct ResultWindow {
    source: Source,
    title: &'static str,
    x_label: String,
    y_label: String,
    points: Vec<[f64; 2]>,
    swapped: bool,
    order: usize,
    fit: Option<(Vec<[f64; 2]>, String)>,
    fit_key: Option<(usize, bool, usize)>,
}

fn qty_label(q: Qty) -> String {
    if q == Qty::Voltage { format!("{}/V", t("qty_voltage")) } else { format!("{}/A", t("qty_current")) }
}

fn series_label(s: Series) -> String {
    format!("{}/{}", t(s.key()), s.base_unit())
}

impl ResultWindow {
    pub fn sweep(r: &SweepResult) -> Self {
        Self {
            source: Source::Sweep,
            title: "sweep_result_title",
            x_label: qty_label(r.target),
            y_label: series_label(r.response),
            points: r.points.clone(),
            swapped: false,
            order: 0,
            fit: None,
            fit_key: None,
        }
    }

    pub fn battery(c: &Curve) -> Self {
        Self {
            source: Source::Battery,
            title: "bat_view_curve",
            x_label: "SOC/%".into(),
            y_label: "OCV/V".into(),
            points: c.resampled(),
            swapped: false,
            order: 0,
            fit: None,
            fit_key: None,
        }
    }

    fn data(&self) -> (Vec<[f64; 2]>, &str, &str) {
        if self.swapped {
            (self.points.iter().map(|p| [p[1], p[0]]).collect(), &self.y_label, &self.x_label)
        } else {
            (self.points.clone(), &self.x_label, &self.y_label)
        }
    }

    fn refit(&mut self, pts: &[[f64; 2]]) {
        let key = (self.order, self.swapped, pts.len());
        if self.fit_key == Some(key) {
            return;
        }
        self.fit_key = Some(key);
        self.fit = None;
        if self.order < 1 || pts.len() < 2 {
            return;
        }
        let x: Vec<f64> = pts.iter().map(|p| p[0]).collect();
        let y: Vec<f64> = pts.iter().map(|p| p[1]).collect();
        if pts.len() <= self.order {
            self.fit = Some((Vec::new(), t("result_not_enough").into()));
            return;
        }
        let Some(p) = fit::polyfit(&x, &y, self.order) else {
            self.fit = Some((Vec::new(), t("result_not_enough").into()));
            return;
        };
        let (lo, hi) = x.iter().fold((f64::MAX, f64::MIN), |(a, b), &v| (a.min(v), b.max(v)));
        let curve =
            (0..1000).map(|k| lo + (hi - lo) * f64::from(k) / 999.0).map(|xv| [xv, fit::polyval(&p, xv)]).collect();
        self.fit = Some((curve, fit::describe(&p, fit::rmse(&p, &x, &y))));
    }
}

fn export(app: &mut App, pts: &[[f64; 2]], xl: &str, yl: &str) {
    let Some(path) =
        rfd::FileDialog::new().add_filter(t("csv_filter"), &["csv"]).set_file_name("iot_power_result.csv").save_file()
    else {
        return;
    };
    let mut s = format!("{xl},{yl}\n");
    for p in pts {
        s.push_str(&format!("{:.6},{:.8}\n", p[0], p[1]));
    }
    match std::fs::write(&path, s) {
        Ok(()) => app.toast(ToastKind::Info, format!("{} {}", t("dump_done"), path.display())),
        Err(e) => app.toast(ToastKind::Error, format!("{}: {e}", path.display())),
    }
}

/// 返回 false 表示窗口已关闭
pub fn draw(app: &mut App, ctx: &egui::Context, pal: &Palette) -> bool {
    let Some(mut w) = app.result_win.take() else { return false };
    // 扫描进行中：实时追加点
    if w.source == Source::Sweep
        && let Some(r) = &app.aux_status.sweep
        && r.points.len() != w.points.len()
    {
        w.points = r.points.clone();
    }
    let crosshair = (w.source == Source::Battery && app.aux_status.running == Some(Kind::Battery))
        .then_some(app.aux_status.bat_point)
        .flatten();
    let mut open = true;
    let mut do_export = None;
    egui::Window::new(t(w.title)).id(egui::Id::new("result_window")).open(&mut open).default_size([560.0, 420.0]).show(
        ctx,
        |ui| {
            ui.horizontal(|ui| {
                let swap_ok = w.source != Source::Battery;
                if ui.add_enabled(swap_ok, egui::Button::new("X ⇄ Y").selected(w.swapped)).clicked() {
                    w.swapped = !w.swapped;
                }
                ui.label(t("result_fit_order"));
                ui.add(egui::DragValue::new(&mut w.order).range(0..=8));
                if ui.button(t("result_export")).clicked() {
                    let (pts, xl, yl) = w.data();
                    do_export = Some((pts, xl.to_string(), yl.to_string()));
                }
            });
            let (pts, xl, yl) = w.data();
            let (xl, yl) = (xl.to_string(), yl.to_string());
            w.refit(&pts);
            let fit_text = match &w.fit {
                Some((_, s)) => s.clone(),
                None => "N/A".into(),
            };
            ui.label(RichText::new(fit_text).monospace().small());
            Plot::new("result_plot").x_axis_label(xl).y_axis_label(yl).show(ui, |pui| {
                pui.line(Line::new("data", pts.clone()).color(pal.line1).width(1.5));
                if w.source == Source::Sweep {
                    pui.points(Points::new("pts", pts).color(pal.line1).radius(2.5));
                }
                if let Some((curve, _)) = &w.fit
                    && !curve.is_empty()
                {
                    pui.line(Line::new("fit", curve.clone()).color(pal.line2).width(1.5));
                }
                if let Some([x, y]) = crosshair {
                    pui.vline(egui_plot::VLine::new("soc", x).color(pal.yellow));
                    pui.hline(egui_plot::HLine::new("ocv", y).color(pal.yellow));
                    pui.points(Points::new("now", vec![[x, y]]).color(pal.yellow).radius(4.0));
                }
            });
        },
    );
    if let Some((pts, xl, yl)) = do_export {
        export(app, &pts, &xl, &yl);
    }
    if open {
        app.result_win = Some(w);
    }
    open
}
