//! 配色（`Doc/界面设计.md` §4）与中文字体。

use std::collections::BTreeMap;
use std::sync::Arc;

use eframe::egui::{self, Color32, FontData, FontDefinitions, FontFamily};

#[derive(Debug, Clone, Copy)]
pub struct Palette {
    pub on: Color32,
    pub off: Color32,
    pub green: Color32,
    pub red: Color32,
    pub yellow: Color32,
    pub blue: Color32,
    pub lcd_voltage: Color32,
    pub lcd_current: Color32,
    pub lcd_power: Color32,
    pub lcd_energy: Color32,
    pub lcd_avg_power: Color32,
    pub lcd_resistance: Color32,
    pub line1: Color32,
    pub line2: Color32,
}

/// `color_palette` 里每个主题可覆盖的键（`Doc/详细设计.md` §3.2）
pub const PALETTE_KEYS: [&str; 14] = [
    "off",
    "on",
    "lcd_voltage",
    "lcd_current",
    "lcd_power",
    "lcd_energy",
    "lcd_avg_power",
    "lcd_resistance",
    "general_green",
    "general_red",
    "general_yellow",
    "general_blue",
    "line1",
    "line2",
];

pub const BASE_KEY: &str = "based_on_dark_or_light";

impl Palette {
    pub fn slot(&mut self, key: &str) -> Option<&mut Color32> {
        Some(match key {
            "off" => &mut self.off,
            "on" => &mut self.on,
            "lcd_voltage" => &mut self.lcd_voltage,
            "lcd_current" => &mut self.lcd_current,
            "lcd_power" => &mut self.lcd_power,
            "lcd_energy" => &mut self.lcd_energy,
            "lcd_avg_power" => &mut self.lcd_avg_power,
            "lcd_resistance" => &mut self.lcd_resistance,
            "general_green" => &mut self.green,
            "general_red" => &mut self.red,
            "general_yellow" => &mut self.yellow,
            "general_blue" => &mut self.blue,
            "line1" => &mut self.line1,
            "line2" => &mut self.line2,
            _ => return None,
        })
    }

    /// 主题名 → (是否暗色, 配色)。`dark`/`light` 也可以在 `color_palette` 里覆盖个别颜色；
    /// 其它名字是自定义主题，按 `based_on_dark_or_light` 继承。
    pub fn resolve(theme: &str, custom: &BTreeMap<String, BTreeMap<String, String>>) -> (bool, Self) {
        let map = custom.get(theme);
        let dark = match theme {
            "dark" => true,
            "light" => false,
            _ => map.and_then(|m| m.get(BASE_KEY)).is_none_or(|b| b != "light"),
        };
        let mut p = Self::for_dark(dark);
        if let Some(m) = map {
            for (k, v) in m {
                if let (Some(slot), Some(c)) = (p.slot(k), parse_color(v)) {
                    *slot = c;
                }
            }
        }
        (dark, p)
    }

    pub fn for_dark(dark: bool) -> Self {
        if dark {
            Self {
                on: Color32::from_rgb(0x90, 0xEE, 0x90),     // lightgreen
                off: Color32::from_rgb(0xF0, 0xE6, 0x8C),    // khaki
                green: Color32::from_rgb(0x66, 0xCD, 0xAA),  // mediumaquamarine
                red: Color32::from_rgb(0xFF, 0x45, 0x00),    // orangered
                yellow: Color32::from_rgb(0xFF, 0xFF, 0x00), // yellow
                blue: Color32::from_rgb(0xAD, 0xD8, 0xE6),   // lightblue
                lcd_voltage: Color32::from_rgb(0xFA, 0x80, 0x72),
                lcd_current: Color32::from_rgb(0x40, 0xE0, 0xD0),
                lcd_power: Color32::from_rgb(0x87, 0xCE, 0xFA),
                lcd_energy: Color32::from_rgb(0xDD, 0xA0, 0xDD),
                lcd_avg_power: Color32::from_rgb(0xF0, 0xE6, 0x8C),
                lcd_resistance: Color32::from_rgb(0x98, 0xFB, 0x98),
                line1: Color32::from_rgb(0xFA, 0x80, 0x72), // salmon
                line2: Color32::from_rgb(0x40, 0xE0, 0xD0), // turquoise
            }
        } else {
            Self {
                on: Color32::from_rgb(0x00, 0x64, 0x00),     // darkgreen
                off: Color32::from_rgb(0xB8, 0x86, 0x0B),    // darkgoldenrod
                green: Color32::from_rgb(0x22, 0x8B, 0x22),  // forestgreen
                red: Color32::from_rgb(0xB2, 0x22, 0x22),    // firebrick
                yellow: Color32::from_rgb(0xDA, 0xA5, 0x20), // goldenrod
                blue: Color32::from_rgb(0x00, 0x00, 0x8B),   // darkblue
                lcd_voltage: Color32::from_rgb(0xFF, 0x45, 0x00),
                lcd_current: Color32::from_rgb(0x00, 0x8B, 0x8B),
                lcd_power: Color32::from_rgb(0x1E, 0x90, 0xFF),
                lcd_energy: Color32::from_rgb(0x8B, 0x00, 0x8B),
                lcd_avg_power: Color32::from_rgb(0xB8, 0x86, 0x0B),
                lcd_resistance: Color32::from_rgb(0x2E, 0x8B, 0x57),
                line1: Color32::from_rgb(0xFF, 0x45, 0x00), // orangered
                line2: Color32::from_rgb(0x00, 0x8B, 0x8B), // darkcyan
            }
        }
    }
}

const CSS_COLORS: &[(&str, [u8; 3])] = &[
    ("black", [0, 0, 0]),
    ("white", [255, 255, 255]),
    ("gray", [128, 128, 128]),
    ("grey", [128, 128, 128]),
    ("silver", [192, 192, 192]),
    ("red", [255, 0, 0]),
    ("green", [0, 128, 0]),
    ("blue", [0, 0, 255]),
    ("yellow", [255, 255, 0]),
    ("orange", [255, 165, 0]),
    ("purple", [128, 0, 128]),
    ("cyan", [0, 255, 255]),
    ("magenta", [255, 0, 255]),
    ("khaki", [240, 230, 140]),
    ("darkgoldenrod", [184, 134, 11]),
    ("goldenrod", [218, 165, 32]),
    ("gold", [255, 215, 0]),
    ("lightgreen", [144, 238, 144]),
    ("darkgreen", [0, 100, 0]),
    ("forestgreen", [34, 139, 34]),
    ("mediumaquamarine", [102, 205, 170]),
    ("seagreen", [46, 139, 87]),
    ("palegreen", [152, 251, 152]),
    ("orangered", [255, 69, 0]),
    ("firebrick", [178, 34, 34]),
    ("crimson", [220, 20, 60]),
    ("tomato", [255, 99, 71]),
    ("salmon", [250, 128, 114]),
    ("lightblue", [173, 216, 230]),
    ("lightskyblue", [135, 206, 250]),
    ("skyblue", [135, 206, 235]),
    ("dodgerblue", [30, 144, 255]),
    ("darkblue", [0, 0, 139]),
    ("navy", [0, 0, 128]),
    ("turquoise", [64, 224, 208]),
    ("darkcyan", [0, 139, 139]),
    ("teal", [0, 128, 128]),
    ("plum", [221, 160, 221]),
    ("darkmagenta", [139, 0, 139]),
    ("violet", [238, 130, 238]),
    ("pink", [255, 192, 203]),
    ("hotpink", [255, 105, 180]),
    ("brown", [165, 42, 42]),
    ("chocolate", [210, 105, 30]),
];

/// CSS 颜色名或 `#rgb` / `#rrggbb`；`default` 或无法识别返回 None（用主题默认色）
pub fn parse_color(s: &str) -> Option<Color32> {
    let s = s.trim().to_ascii_lowercase();
    if let Some(hex) = s.strip_prefix('#') {
        let h = |a: &str| u8::from_str_radix(a, 16).ok();
        return match hex.len() {
            6 => Some(Color32::from_rgb(h(&hex[0..2])?, h(&hex[2..4])?, h(&hex[4..6])?)),
            3 => {
                let d = |k: usize| h(&hex[k..=k]).map(|x| x * 17);
                Some(Color32::from_rgb(d(0)?, d(1)?, d(2)?))
            }
            _ => None,
        };
    }
    CSS_COLORS.iter().find(|(n, _)| *n == s).map(|(_, [r, g, b])| Color32::from_rgb(*r, *g, *b))
}

pub fn color_hex(c: Color32) -> String {
    format!("#{:02x}{:02x}{:02x}", c.r(), c.g(), c.b())
}

/// 中文字体候选：(路径, ttc 内的字形索引)。先找程序旁的 `assets/font.ttf`（发布包内置），再找系统字体。
fn font_candidates() -> Vec<(std::path::PathBuf, u32)> {
    let mut v = Vec::new();
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        v.push((dir.join("assets").join("font.ttf"), 0));
    }
    v.push(("app/assets/font.ttf".into(), 0));
    if cfg!(windows) {
        let windir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
        let fonts = std::path::Path::new(&windir).join("Fonts");
        v.push((fonts.join("msyh.ttc"), 0));
        v.push((fonts.join("simhei.ttf"), 0));
        v.push((fonts.join("simsun.ttc"), 0));
    } else if cfg!(target_os = "macos") {
        v.push(("/System/Library/Fonts/PingFang.ttc".into(), 0));
        v.push(("/System/Library/Fonts/STHeiti Light.ttc".into(), 0));
    } else {
        // NotoSansCJK-Regular.ttc 的第 2 个字形是简体中文（SC）
        for p in [
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc",
        ] {
            v.push((p.into(), 2));
        }
        v.push(("/usr/share/fonts/truetype/droid/DroidSansFallbackFull.ttf".into(), 0));
        v.push(("/usr/share/fonts/truetype/wqy/wqy-microhei.ttc".into(), 0));
        v.push(("/usr/share/fonts/wenquanyi/wqy-microhei/wqy-microhei.ttc".into(), 0));
    }
    v
}

/// 注册为 Proportional 与 Monospace 的回退字体；返回实际使用的字体路径。
pub fn install_cjk_font(ctx: &egui::Context) -> Option<String> {
    for (path, index) in font_candidates() {
        let Ok(bytes) = std::fs::read(&path) else { continue };
        let mut fonts = FontDefinitions::default();
        let mut data = FontData::from_owned(bytes);
        data.index = index;
        fonts.font_data.insert("cjk".into(), Arc::new(data));
        for fam in [FontFamily::Proportional, FontFamily::Monospace] {
            fonts.families.entry(fam).or_default().push("cjk".into());
        }
        ctx.set_fonts(fonts);
        let shown = path.display().to_string();
        log::info!("CJK font: {shown} (index {index})");
        return Some(shown);
    }
    log::warn!("no CJK font found; Chinese text will not render");
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_theme_inherits_and_overrides() {
        let mut custom = BTreeMap::new();
        let mut m = BTreeMap::new();
        m.insert(BASE_KEY.to_string(), "light".to_string());
        m.insert("on".to_string(), "#ff0000".to_string());
        m.insert("off".to_string(), "default".to_string());
        custom.insert("mine".to_string(), m);
        let (dark, p) = Palette::resolve("mine", &custom);
        assert!(!dark);
        assert_eq!(p.on, Color32::from_rgb(255, 0, 0));
        assert_eq!(p.off, Palette::for_dark(false).off);
        assert_eq!(parse_color("Salmon"), Some(Color32::from_rgb(250, 128, 114)));
        assert_eq!(parse_color("#abc"), Some(Color32::from_rgb(0xaa, 0xbb, 0xcc)));
    }
}
