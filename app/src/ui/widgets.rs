//! 公共小控件：双柄区间滑块、数字输入框（按位更改 / 自由更改）、超界红框。

use std::path::Path;
use std::time::Duration;

use eframe::egui::{self, Id, Key, Modifiers, Response, Sense, Stroke, StrokeKind, TextEdit, vec2};
use egui::text::{CCursor, CCursorRange};

use crate::theme::Palette;

pub fn red_frame(ui: &egui::Ui, r: &Response, pal: &Palette) {
    ui.painter().rect_stroke(r.rect, 2.0, Stroke::new(1.5, pal.red), StrokeKind::Outside);
}

/// 双柄区间滑块：`lo..hi` 在 `min..max` 内，跨度不小于 `min_span`。拖柄改一端，拖中间平移。
#[allow(clippy::too_many_arguments)]
pub fn range_slider(
    ui: &mut egui::Ui,
    lo: &mut f64,
    hi: &mut f64,
    min: f64,
    max: f64,
    min_span: f64,
    enabled: bool,
    width: f32,
) -> bool {
    let sense = if enabled { Sense::click_and_drag() } else { Sense::hover() };
    let (rect, resp) = ui.allocate_exact_size(vec2(width.max(60.0), 18.0), sense);
    let span = (max - min).max(1e-12);
    let min_span = min_span.min(span);
    let x_of = |v: f64| rect.left() + ((v - min) / span) as f32 * rect.width();
    let v_of = |x: f32| min + f64::from(((x - rect.left()) / rect.width()).clamp(0.0, 1.0)) * span;
    let mut changed = false;
    if enabled {
        if resp.drag_started()
            && let Some(p) = resp.interact_pointer_pos()
        {
            let (dl, dh) = ((p.x - x_of(*lo)).abs(), (p.x - x_of(*hi)).abs());
            let grab: u8 = if dl.min(dh) <= 8.0 {
                if dl <= dh { 0 } else { 1 }
            } else if p.x > x_of(*lo) && p.x < x_of(*hi) {
                2
            } else if dl < dh {
                0
            } else {
                1
            };
            ui.data_mut(|d| d.insert_temp(resp.id, (grab, v_of(p.x))));
        }
        if resp.dragged()
            && let (Some(p), Some((grab, anchor))) =
                (resp.interact_pointer_pos(), ui.data(|d| d.get_temp::<(u8, f64)>(resp.id)))
        {
            let v = v_of(p.x);
            match grab {
                0 => *lo = v.min(*hi - min_span).max(min),
                1 => *hi = v.max(*lo + min_span).min(max),
                _ => {
                    let w = *hi - *lo;
                    let nlo = (*lo + v - anchor).clamp(min, max - w);
                    *lo = nlo;
                    *hi = nlo + w;
                    ui.data_mut(|d| d.insert_temp(resp.id, (2u8, v)));
                }
            }
            changed = true;
        }
    }
    let vis = ui.visuals();
    let track = rect.shrink2(vec2(0.0, 6.0));
    ui.painter().rect_filled(track, 3.0, vis.widgets.inactive.bg_fill);
    let sel = egui::Rect::from_x_y_ranges(x_of(*lo)..=x_of(*hi), track.y_range());
    let sel_fill = if enabled { vis.selection.bg_fill } else { vis.widgets.noninteractive.bg_stroke.color };
    ui.painter().rect_filled(sel, 3.0, sel_fill);
    let knob = if enabled { vis.widgets.active.fg_stroke.color } else { vis.weak_text_color() };
    for v in [*lo, *hi] {
        ui.painter().circle_filled(egui::pos2(x_of(v), rect.center().y), 6.0, knob);
    }
    changed
}

/// 按位更改：光标左边那一位决定步进（整数位 1/10…，小数第 1 位 0.1 …）
fn digit_step(text: &str, cursor: usize, decimals: usize) -> f64 {
    let dot = text.find('.').unwrap_or(text.len());
    if cursor <= dot {
        10f64.powi(dot.saturating_sub(cursor.max(1)) as i32)
    } else {
        let d = (cursor - dot - 1).clamp(1, decimals.max(1));
        10f64.powi(-(d as i32))
    }
}

/// 自由更改：按数量级自适应
fn adaptive_step(v: f64) -> f64 {
    if v.abs() < 1e-9 {
        return 0.1;
    }
    10f64.powi(v.abs().log10().floor() as i32 - 1).max(0.001)
}

fn step_text(ctx: &egui::Context, id: Id, text: &mut String, dir: f64, bit: bool, decimals: usize) -> bool {
    let Ok(v) = text.trim().parse::<f64>() else { return false };
    let state = TextEdit::load_state(ctx, id);
    let len = text.chars().count();
    let cursor = state.as_ref().and_then(|s| s.cursor.char_range()).map_or(len, |r| r.primary.index.into());
    let step = if bit { digit_step(text, cursor, decimals) } else { adaptive_step(v) };
    let nv = (((v + dir * step) * 1000.0).round() / 1000.0).max(0.0);
    let new = format!("{nv:.decimals$}");
    let dot_old = text.find('.').unwrap_or(text.len()) as i64;
    let dot_new = new.find('.').unwrap_or(new.len()) as i64;
    let nc = (cursor as i64 + dot_new - dot_old).clamp(0, new.len() as i64) as usize;
    *text = new;
    if let Some(mut s) = state {
        s.cursor.set_char_range(Some(CCursorRange::one(CCursor::new(nc))));
        s.store(ctx, id);
    }
    true
}

pub struct NumField {
    pub resp: Response,
    /// 被 ↑↓ / 滚轮步进修改
    pub stepped: bool,
}

/// 设定类数字输入框：只允许数字和小数点；获得焦点时 ↑↓ / 滚轮步进
/// （`bit` = 按位更改，否则自适应步进）。不夹值，超界由调用方标红。
pub fn num_field(ui: &mut egui::Ui, id: Id, text: &mut String, width: f32, enabled: bool, bit: bool) -> NumField {
    let focused = enabled && ui.memory(|m| m.has_focus(id));
    let mut dir = 0.0;
    if focused {
        ui.input_mut(|i| {
            if i.consume_key(Modifiers::NONE, Key::ArrowUp) {
                dir = 1.0;
            } else if i.consume_key(Modifiers::NONE, Key::ArrowDown) {
                dir = -1.0;
            }
        });
    }
    let mut stepped = dir != 0.0 && step_text(ui.ctx(), id, text, dir, bit, 3);
    let resp = ui
        .add_enabled(enabled, TextEdit::singleline(text).id(id).desired_width(width).font(egui::TextStyle::Monospace));
    if resp.changed() {
        text.retain(|c| c.is_ascii_digit() || c == '.');
    }
    if focused && resp.hovered() {
        let dy = wheel(ui);
        if dy != 0.0 {
            stepped |= step_text(ui.ctx(), id, text, dy.into(), bit, 3);
        }
    }
    NumField { resp, stepped }
}

/// 本帧滚轮方向（+1 向上 / -1 向下 / 0），并吞掉本帧滚动，避免外层 ScrollArea 跟着滚
pub fn wheel(ui: &egui::Ui) -> f32 {
    let dy: f32 = ui.input(|i| {
        i.events
            .iter()
            .filter_map(|e| match e {
                egui::Event::MouseWheel { delta, .. } => Some(delta.y),
                _ => None,
            })
            .sum()
    });
    if dy == 0.0 {
        return 0.0;
    }
    ui.input_mut(|i| i.smooth_scroll_delta = egui::Vec2::ZERO);
    dy.signum()
}

/// 参数用的数值框（DragValue 不设范围，超界标红而不是夹值）；`bad` 用编辑后的值判断
pub fn param(
    ui: &mut egui::Ui,
    v: &mut f64,
    speed: f64,
    decimals: usize,
    bad: impl FnOnce(f64) -> bool,
    pal: &Palette,
) -> Response {
    let r = ui.add(egui::DragValue::new(v).speed(speed).fixed_decimals(decimals).min_decimals(decimals));
    if bad(*v) {
        red_frame(ui, &r, pal);
    }
    r
}

pub fn hms(d: Duration) -> String {
    let s = d.as_secs();
    format!("{:02}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
}

/// 秒数 → 「1 h 2 min」「5.0 s」之类的简短时长
pub fn short_duration(s: f64) -> String {
    if !s.is_finite() || s < 0.0 {
        return "--".into();
    }
    if s < 60.0 {
        format!("{s:.1} s")
    } else if s < 3600.0 {
        format!("{} min {:.0} s", (s / 60.0).floor(), s % 60.0)
    } else {
        format!("{} h {} min", (s / 3600.0).floor(), ((s % 3600.0) / 60.0).floor())
    }
}

/// 用系统文件管理器打开目录
pub fn open_path(p: &Path) {
    let dir = if p.is_dir() { p } else { p.parent().unwrap_or(p) };
    let cmd = if cfg!(windows) {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    if let Err(e) = std::process::Command::new(cmd).arg(dir).spawn() {
        log::warn!("open {}: {e}", dir.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digit_steps_follow_cursor() {
        assert_eq!(digit_step("12.345", 2, 3), 1.0);
        assert_eq!(digit_step("12.345", 1, 3), 10.0);
        assert_eq!(digit_step("12.345", 0, 3), 10.0);
        assert_eq!(digit_step("12.345", 4, 3), 0.1);
        assert_eq!(digit_step("12.345", 6, 3), 0.001);
        assert_eq!(adaptive_step(12.0), 1.0);
        assert_eq!(adaptive_step(5.0), 0.1);
        assert_eq!(hms(Duration::from_secs(3725)), "01:02:05");
    }
}
