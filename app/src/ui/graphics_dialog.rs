//! 图形设置（F08/F15，`Doc/界面设计.md` §3.2）：UI、图表、状态区、录制、显示修正。
//! 改动立即生效，关闭对话框时写盘。

use std::collections::BTreeMap;

use eframe::egui::{self, RichText};

use crate::app::App;
use crate::i18n::{self, Lang, t, tf};
use crate::theme::{BASE_KEY, PALETTE_KEYS, Palette, color_hex};
use crate::ui::widgets;

pub fn apply_language(setting: &str) {
    i18n::set_lang(match setting {
        "zh" => Lang::Zh,
        "en" => Lang::En,
        _ => i18n::detect_system_lang(),
    });
}

pub struct GraphicsDialog {
    data_pts: String,
    display_pts: String,
    new_theme: String,
    new_base_light: bool,
}

impl GraphicsDialog {
    pub fn new(s: &crate::config::Settings) -> Self {
        Self {
            data_pts: s.data_pts.to_string(),
            display_pts: s.display_pts.to_string(),
            new_theme: String::new(),
            new_base_light: false,
        }
    }
}

fn section(ui: &mut egui::Ui, key: &'static str) {
    ui.add_space(4.0);
    ui.label(RichText::new(t(key)).strong());
}

fn theme_name(name: &str) -> String {
    match name {
        "dark" => t("gfx_theme_dark").into(),
        "light" => t("gfx_theme_light").into(),
        other => other.into(),
    }
}

fn palette_ui(app: &mut App, d: &mut GraphicsDialog, ui: &mut egui::Ui) {
    egui::CollapsingHeader::new(t("gfx_palette")).id_salt("gfx_palette").show(ui, |ui| {
        let theme = app.settings.theme.clone();
        let (_, mut pal) = Palette::resolve(&theme, &app.settings.color_palette);
        egui::Grid::new("palette_grid").num_columns(4).spacing([10.0, 4.0]).show(ui, |ui| {
            for (k, key) in PALETTE_KEYS.iter().enumerate() {
                let Some(slot) = pal.slot(key) else { continue };
                let mut c = *slot;
                ui.label(RichText::new(*key).monospace().small());
                if egui::color_picker::color_edit_button_srgba(ui, &mut c, egui::color_picker::Alpha::Opaque).changed()
                {
                    app.settings.color_palette.entry(theme.clone()).or_default().insert((*key).into(), color_hex(c));
                }
                if k % 2 == 1 {
                    ui.end_row();
                }
            }
        });
        ui.horizontal(|ui| {
            if ui.button(t("gfx_palette_reset")).clicked()
                && let Some(m) = app.settings.color_palette.get_mut(&theme)
            {
                m.retain(|k, _| k == BASE_KEY);
                if m.is_empty() {
                    app.settings.color_palette.remove(&theme);
                }
            }
            if theme != "dark" && theme != "light" && ui.button(t("gfx_theme_delete")).clicked() {
                app.settings.color_palette.remove(&theme);
                app.settings.theme = "dark".into();
            }
        });
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut d.new_theme).hint_text(t("gfx_theme_new")).desired_width(110.0));
            ui.selectable_value(&mut d.new_base_light, false, t("gfx_theme_dark"));
            ui.selectable_value(&mut d.new_base_light, true, t("gfx_theme_light"));
            let name = d.new_theme.trim().to_string();
            let ok = !name.is_empty()
                && name != "dark"
                && name != "light"
                && !app.settings.color_palette.contains_key(&name);
            if ui.add_enabled(ok, egui::Button::new("+")).clicked() {
                let mut m = BTreeMap::new();
                m.insert(BASE_KEY.to_string(), if d.new_base_light { "light" } else { "dark" }.to_string());
                app.settings.color_palette.insert(name.clone(), m);
                app.settings.theme = name;
                d.new_theme.clear();
            }
        });
    });
}

/// 返回 false 表示对话框已关闭
pub fn draw(app: &mut App, d: &mut GraphicsDialog, ctx: &egui::Context) -> bool {
    let mut open = true;
    let before = app.settings.clone();
    let aux_running = app.aux_running();
    let pal = app.pal;
    egui::Window::new(t("dlg_graphics_title"))
        .open(&mut open)
        .collapsible(false)
        .default_width(460.0)
        .default_height(560.0)
        .show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                section(ui, "gfx_section_ui");
                egui::Grid::new("gfx_ui").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                    let s = &mut app.settings;
                    ui.label(t("gfx_language"));
                    egui::ComboBox::from_id_salt("lang")
                        .selected_text(match s.language.as_str() {
                            "zh" => "中文",
                            "en" => "English",
                            _ => t("current_unit_auto"),
                        })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut s.language, "auto".into(), t("current_unit_auto"));
                            ui.selectable_value(&mut s.language, "zh".into(), "中文");
                            ui.selectable_value(&mut s.language, "en".into(), "English");
                        });
                    ui.end_row();

                    ui.label(t("gfx_theme"));
                    let mut names = vec!["dark".to_string(), "light".to_string()];
                    names.extend(s.color_palette.keys().filter(|k| *k != "dark" && *k != "light").cloned());
                    egui::ComboBox::from_id_salt("theme").selected_text(theme_name(&s.theme)).show_ui(ui, |ui| {
                        for n in names {
                            let label = theme_name(&n);
                            ui.selectable_value(&mut s.theme, n, label);
                        }
                    });
                    ui.end_row();

                    ui.label(t("gfx_input"));
                    ui.horizontal(|ui| {
                        ui.selectable_value(&mut s.bitadjust, true, t("gfx_input_bit"))
                            .on_hover_text(t("gfx_input_bit_tip"));
                        ui.selectable_value(&mut s.bitadjust, false, t("gfx_input_free"));
                    });
                    ui.end_row();
                });
                palette_ui(app, d, ui);

                ui.separator();
                section(ui, "gfx_section_graph");
                egui::Grid::new("gfx_graph").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                    ui.label(t("gfx_max_fps"));
                    ui.add(egui::Slider::new(&mut app.settings.graph_max_fps, 1.0..=120.0).integer());
                    ui.end_row();

                    let data_ok = d.data_pts.trim().parse::<u32>().ok().filter(|x| (100..=1_000_000).contains(x));
                    ui.label(t("gfx_data_pts"));
                    ui.horizontal(|ui| {
                        let r = ui.add_enabled(
                            !aux_running,
                            egui::TextEdit::singleline(&mut d.data_pts)
                                .desired_width(80.0)
                                .font(egui::TextStyle::Monospace),
                        );
                        if data_ok.is_none() {
                            widgets::red_frame(ui, &r, &pal);
                        }
                        if r.lost_focus()
                            && let Some(n) = data_ok
                        {
                            app.settings.data_pts = n;
                            app.settings.display_pts = app.settings.display_pts.min(n);
                            d.display_pts = app.settings.display_pts.to_string();
                        }
                        ui.label("pts").on_hover_text(t("gfx_data_pts_tip"));
                    });
                    ui.end_row();

                    let disp_ok =
                        d.display_pts.trim().parse::<u32>().ok().filter(|x| (100..=app.settings.data_pts).contains(x));
                    ui.label(t("gfx_display_pts"));
                    ui.horizontal(|ui| {
                        let r = ui.add(
                            egui::TextEdit::singleline(&mut d.display_pts)
                                .desired_width(80.0)
                                .font(egui::TextStyle::Monospace),
                        );
                        if disp_ok.is_none() {
                            widgets::red_frame(ui, &r, &pal);
                        }
                        if r.lost_focus()
                            && let Some(n) = disp_ok
                        {
                            app.settings.display_pts = n;
                        }
                        ui.label("pts");
                    });
                    ui.end_row();

                    ui.label("");
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut app.settings.antialias, t("gfx_antialias"));
                        ui.checkbox(&mut app.settings.downsample, t("gfx_downsample"));
                    });
                    ui.end_row();
                });

                ui.separator();
                section(ui, "gfx_section_state");
                egui::Grid::new("gfx_state").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                    let s = &mut app.settings;
                    ui.label(t("gfx_state_fps"));
                    ui.add(egui::Slider::new(&mut s.state_fps, 1.0..=120.0).integer());
                    ui.end_row();

                    ui.label(t("gfx_interp"));
                    let keys = ["gfx_interp_0", "gfx_interp_1", "gfx_interp_2", "gfx_interp_3"];
                    egui::ComboBox::from_id_salt("interp")
                        .selected_text(t(keys[usize::from(s.interp.min(3))]))
                        .show_ui(ui, |ui| {
                            for (k, key) in keys.iter().enumerate() {
                                ui.selectable_value(&mut s.interp, k as u8, t(key));
                            }
                        });
                    ui.end_row();

                    ui.label(t("gfx_zero_threshold"));
                    ui.horizontal(|ui| {
                        ui.add(egui::DragValue::new(&mut s.v_threshold).range(0.0..=1.0).speed(0.001).max_decimals(3));
                        ui.label("V");
                        ui.add(
                            egui::DragValue::new(&mut s.i_threshold).range(0.0..=1.0).speed(0.00001).max_decimals(5),
                        );
                        ui.label("A");
                    });
                    ui.end_row();

                    ui.label(t("current_unit"));
                    egui::ComboBox::from_id_salt("cur_unit")
                        .selected_text(if s.current_unit == "auto" {
                            t("current_unit_auto").to_string()
                        } else {
                            s.current_unit.clone()
                        })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut s.current_unit, "auto".into(), t("current_unit_auto"));
                            for u in ["A", "mA", "µA"] {
                                ui.selectable_value(&mut s.current_unit, u.into(), u);
                            }
                        });
                    ui.end_row();
                });

                ui.separator();
                section(ui, "gfx_section_record");
                egui::Grid::new("gfx_rec").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                    let s = &mut app.settings;
                    ui.label(t("log_dir"));
                    ui.horizontal(|ui| {
                        let shown = s.record_dir().display().to_string();
                        ui.label(RichText::new(shown).monospace().small());
                        if ui.small_button("…").clicked()
                            && let Some(p) = rfd::FileDialog::new().set_directory(s.record_dir()).pick_folder()
                        {
                            s.record_dir = p.to_string_lossy().into_owned();
                        }
                        if ui.small_button("📂").on_hover_text(t("open_path")).clicked() {
                            widgets::open_path(&s.record_dir());
                        }
                    });
                    ui.end_row();

                    ui.label("");
                    ui.vertical(|ui| {
                        ui.checkbox(&mut s.record_csv, t("log_record_csv"));
                        ui.checkbox(&mut s.debug_columns, t("log_debug_columns"));
                        ui.checkbox(&mut s.log_auto_start, t("log_auto_start"));
                    });
                    ui.end_row();

                    ui.label(t("log_split_label"));
                    ui.horizontal(|ui| {
                        ui.add(egui::DragValue::new(&mut s.log_split_minutes).range(0..=1440).suffix(" min"));
                        ui.add(egui::DragValue::new(&mut s.log_split_mb).range(0..=2048).suffix(" MB"));
                    });
                    ui.end_row();
                    ui.label(t("log_flush"));
                    ui.add(egui::DragValue::new(&mut s.log_flush_ms).range(200..=10_000).suffix(" ms"));
                    ui.end_row();
                });
                ui.label(
                    RichText::new(tf(
                        "log_split",
                        &[
                            ("min", &app.settings.log_split_minutes.to_string()),
                            ("mb", &app.settings.log_split_mb.to_string()),
                        ],
                    ))
                    .small()
                    .weak(),
                );

                ui.separator();
                section(ui, "gfx_section_correction");
                let s = &mut app.settings;
                ui.checkbox(&mut s.use_cali, t("gfx_apply_correction")).on_hover_text(t("gfx_correction_tip"));
                ui.add_enabled_ui(s.use_cali, |ui| {
                    egui::Grid::new("gfx_cali").num_columns(5).spacing([8.0, 4.0]).show(ui, |ui| {
                        ui.label("");
                        for h in ["Vk", "Vb", "Ik", "Ib"] {
                            ui.label(RichText::new(h).monospace());
                        }
                        ui.end_row();
                        fn dv(v: &mut f64) -> egui::DragValue<'_> {
                            egui::DragValue::new(v).range(-100.0..=100.0).speed(0.0001).max_decimals(6)
                        }
                        ui.label(t("gfx_cali_read"));
                        for v in [&mut s.v_cali_k, &mut s.v_cali_b, &mut s.i_cali_k, &mut s.i_cali_b] {
                            ui.add(dv(v));
                        }
                        ui.end_row();
                        ui.label(t("gfx_cali_set"));
                        for v in [&mut s.vset_cali_k, &mut s.vset_cali_b, &mut s.iset_cali_k, &mut s.iset_cali_b] {
                            ui.add(dv(v));
                        }
                        ui.end_row();
                    });
                });
            });
        });

    // 设定修正 k 不能为 0（无法反算）
    for (k, old) in
        [(&mut app.settings.vset_cali_k, before.vset_cali_k), (&mut app.settings.iset_cali_k, before.iset_cali_k)]
    {
        if *k == 0.0 {
            *k = old;
        }
    }
    if app.settings != before {
        if app.settings.language != before.language {
            apply_language(&app.settings.language);
        }
        if app.settings.data_pts != before.data_pts {
            app.graph.reset_window();
        }
        app.apply_runtime();
    }
    if !open {
        app.save_settings();
    }
    open
}
