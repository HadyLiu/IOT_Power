//! settings.json（`Doc/详细设计.md` §3.2）。未知字段原样保留在 `extra` 里。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use dc3005u_proto::{HW_I_MA, HW_V_MV, SafetyLimits};
use dc3005u_usb::WorkerConfig;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// 原版字段，新版不再使用，读到就丢弃
const REMOVED_KEYS: [&str; 3] = ["baudrate", "comport", "opengl"];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub presets: BTreeMap<String, [f64; 2]>,
    pub device_serial: String,
    pub poll_interval_ms: u32,
    pub usb_timeout_ms: u32,
    pub resp_timeout_poll_ms: u32,
    pub resp_timeout_cmd_ms: u32,
    pub cmd_retries: u8,
    pub auto_connect: bool,
    pub off_on_connect: bool,
    pub safety: SafetyLimits,
    pub current_unit: String,
    pub setpoint_min_interval_ms: u32,
    pub state_fps: f64,
    /// 曲线缓冲容量（点）
    pub data_pts: u32,
    /// 跟随模式显示的最新点数
    pub display_pts: u32,
    pub interp: u8,
    pub last_vset: f64,
    pub last_iset: f64,
    pub v_threshold: f64,
    pub i_threshold: f64,
    pub use_cali: bool,
    pub v_cali_k: f64,
    pub v_cali_b: f64,
    pub i_cali_k: f64,
    pub i_cali_b: f64,
    pub vset_cali_k: f64,
    pub vset_cali_b: f64,
    pub iset_cali_k: f64,
    pub iset_cali_b: f64,
    pub output_warning: bool,
    pub lock_when_output: bool,
    pub quickset: bool,
    /// stop / skip：序列执行到超出安全边界的行时
    pub seq_oob_action: String,
    /// 录制 CSV 附加 `power_dev/W` 调试列
    pub debug_columns: bool,
    pub graph_max_fps: f64,
    pub downsample: bool,
    pub antialias: bool,
    /// true = 按位更改，false = 自由更改
    pub bitadjust: bool,
    pub theme: String,
    /// 自定义主题：主题名 → (键 → CSS 颜色名 / #rrggbb / default)，`based_on_dark_or_light` 指定继承
    pub color_palette: BTreeMap<String, BTreeMap<String, String>>,
    pub language: String,
    /// 空 = 文档/IOT_Power
    pub record_dir: String,
    pub record_csv: bool,
    pub log_auto_start: bool,
    pub log_split_minutes: u32,
    pub log_split_mb: u32,
    pub log_flush_ms: u32,
    pub aux: AuxSettings,
    pub http_enabled: bool,
    pub http_port: u16,
    pub http_token: String,
    pub http_allow_output_on: bool,
    pub window: Option<WindowGeom>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WindowGeom {
    pub w: f32,
    pub h: f32,
    #[serde(default)]
    pub x: Option<f32>,
    #[serde(default)]
    pub y: Option<f32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct KeepPowerCfg {
    pub target_w: f64,
    pub ki: f64,
    pub max_v: f64,
    pub freq_hz: f64,
}

impl Default for KeepPowerCfg {
    fn default() -> Self {
        Self { target_w: 1.0, ki: 2.5, max_v: 24.0, freq_hz: 10.0 }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SweepCfg {
    /// voltage / current
    pub target: String,
    pub start: f64,
    pub stop: f64,
    pub step: f64,
    pub delay_s: f64,
    /// none / voltage / current / power / resistance
    pub response: String,
}

impl Default for SweepCfg {
    fn default() -> Self {
        Self { target: "voltage".into(), start: 1.0, stop: 5.0, step: 0.1, delay_s: 1.0, response: "none".into() }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WaveCfg {
    /// square / sine / triangle / sawtooth / noise
    #[serde(rename = "type")]
    pub kind: String,
    pub period_s: f64,
    pub high_v: f64,
    pub low_v: f64,
    pub freq_hz: f64,
}

impl Default for WaveCfg {
    fn default() -> Self {
        Self { kind: "square".into(), period_s: 1.0, high_v: 5.0, low_v: 2.0, freq_hz: 10.0 }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BatCfg {
    /// 内置曲线名或 CSV 文件路径
    pub curve: String,
    pub soc_now: f64,
    pub soc_stop: f64,
    pub cap_wh: f64,
    pub r_mohm: f64,
    pub cells: u32,
    pub freq_hz: f64,
}

impl Default for BatCfg {
    fn default() -> Self {
        Self {
            curve: "Li-ion".into(),
            soc_now: 100.0,
            soc_stop: 0.0,
            cap_wh: 10.0,
            r_mohm: 0.0,
            cells: 1,
            freq_hz: 10.0,
        }
    }
}

/// 各 AUX 页的参数（原版不保存）
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AuxSettings {
    pub keep_power: KeepPowerCfg,
    pub sweep: SweepCfg,
    pub wavegen: WaveCfg,
    pub batsim: BatCfg,
}

impl AuxSettings {
    /// 首次运行（settings 里没有 `aux`）时，电压/电流类默认值先截取到安全边界内
    fn fit_defaults(&mut self, lim: &SafetyLimits) {
        let v = |x: f64| x.clamp(lim.v_min, lim.v_max);
        self.keep_power.max_v = v(self.keep_power.max_v);
        self.sweep.start = v(self.sweep.start);
        self.sweep.stop = v(self.sweep.stop);
        self.wavegen.high_v = v(self.wavegen.high_v);
        self.wavegen.low_v = v(self.wavegen.low_v);
    }
}

pub fn default_presets() -> BTreeMap<String, [f64; 2]> {
    [(3.3, 2.0), (3.3, 5.0), (5.0, 2.0), (5.0, 5.0), (9.0, 5.0), (12.0, 5.0), (20.0, 5.0), (24.0, 3.0), (24.0, 5.0)]
        .iter()
        .enumerate()
        .map(|(k, &(v, i))| ((k + 1).to_string(), [v, i]))
        .collect()
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            presets: default_presets(),
            device_serial: String::new(),
            poll_interval_ms: 50,
            usb_timeout_ms: 2000,
            resp_timeout_poll_ms: 500,
            resp_timeout_cmd_ms: 300,
            cmd_retries: 2,
            auto_connect: false,
            off_on_connect: true,
            safety: SafetyLimits::hardware(),
            current_unit: "auto".into(),
            setpoint_min_interval_ms: 100,
            state_fps: 10.0,
            data_pts: 100_000,
            display_pts: 400,
            interp: 0,
            last_vset: 5.0,
            last_iset: 2.0,
            v_threshold: 0.002,
            i_threshold: 0.0,
            use_cali: false,
            v_cali_k: 1.0,
            v_cali_b: 0.0,
            i_cali_k: 1.0,
            i_cali_b: 0.0,
            vset_cali_k: 1.0,
            vset_cali_b: 0.0,
            iset_cali_k: 1.0,
            iset_cali_b: 0.0,
            output_warning: false,
            lock_when_output: false,
            quickset: false,
            seq_oob_action: "stop".into(),
            debug_columns: false,
            graph_max_fps: 50.0,
            downsample: true,
            antialias: true,
            bitadjust: false,
            theme: "dark".into(),
            color_palette: BTreeMap::new(),
            language: "auto".into(),
            record_dir: String::new(),
            record_csv: true,
            log_auto_start: false,
            log_split_minutes: 60,
            log_split_mb: 100,
            log_flush_ms: 1000,
            aux: AuxSettings::default(),
            http_enabled: false,
            http_port: 17321,
            http_token: String::new(),
            http_allow_output_on: false,
            window: None,
            extra: Map::new(),
        }
    }
}

/// 读入时发生、需要提示用户的修正
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadNotice {
    LoadFailed(String),
    SafetyReset,
    PresetsFixed,
}

fn clamp_u32(name: &str, x: &mut u32, lo: u32, hi: u32) {
    let c = (*x).clamp(lo, hi);
    if c != *x {
        log::warn!("settings.{name}={x} out of {lo}..={hi}, clamped to {c}");
        *x = c;
    }
}

fn clamp_f64(name: &str, x: &mut f64, lo: f64, hi: f64, default: f64) {
    let c = if x.is_finite() { x.clamp(lo, hi) } else { default };
    if c != *x {
        log::warn!("settings.{name}={x} out of {lo}..={hi}, using {c}");
        *x = c;
    }
}

impl Settings {
    pub fn path() -> Option<PathBuf> {
        let base = directories::BaseDirs::new()?;
        let dir = if cfg!(windows) { "IOT_Power" } else { "iot_power" };
        Some(base.config_dir().join(dir).join("settings.json"))
    }

    pub fn load() -> (Self, Vec<LoadNotice>) {
        match Self::path() {
            Some(p) => Self::load_from(&p),
            None => (Self::default(), Vec::new()),
        }
    }

    pub fn load_from(path: &Path) -> (Self, Vec<LoadNotice>) {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (Self::default(), Vec::new()),
            Err(e) => return (Self::default(), vec![LoadNotice::LoadFailed(e.to_string())]),
        };
        Self::from_json(&text)
    }

    /// 逐字段合并到默认值上：某个字段类型不对只丢弃该字段，不影响其它字段。
    pub fn from_json(text: &str) -> (Self, Vec<LoadNotice>) {
        let mut notices = Vec::new();
        let file: Map<String, Value> = match serde_json::from_str(text) {
            Ok(Value::Object(m)) => m,
            Ok(_) => return (Self::default(), vec![LoadNotice::LoadFailed("not a JSON object".into())]),
            Err(e) => return (Self::default(), vec![LoadNotice::LoadFailed(e.to_string())]),
        };
        let Ok(Value::Object(mut merged)) = serde_json::to_value(Self::default()) else {
            unreachable!("Settings serializes to an object")
        };
        let file_has_aux = file.contains_key("aux");
        for (k, v) in file {
            if REMOVED_KEYS.contains(&k.as_str()) {
                continue;
            }
            let mut trial = merged.clone();
            trial.insert(k.clone(), v.clone());
            if serde_json::from_value::<Self>(Value::Object(trial)).is_ok() {
                merged.insert(k, v);
            } else {
                log::warn!("settings.{k} has an invalid value, using default");
            }
        }
        let has_aux = file_has_aux;
        let mut s: Self = serde_json::from_value(Value::Object(merged)).unwrap_or_default();
        s.sanitize(&mut notices);
        if !has_aux {
            let lim = s.safety;
            s.aux.fit_defaults(&lim);
        }
        (s, notices)
    }

    fn sanitize(&mut self, notices: &mut Vec<LoadNotice>) {
        if self.poll_interval_ms < 50 {
            log::warn!("settings.poll_interval_ms={} below 50, using 50", self.poll_interval_ms);
        }
        clamp_u32("poll_interval_ms", &mut self.poll_interval_ms, 50, 65535);
        clamp_u32("usb_timeout_ms", &mut self.usb_timeout_ms, 500, 5000);
        clamp_u32("resp_timeout_poll_ms", &mut self.resp_timeout_poll_ms, 100, 5000);
        clamp_u32("resp_timeout_cmd_ms", &mut self.resp_timeout_cmd_ms, 50, 5000);
        clamp_u32("setpoint_min_interval_ms", &mut self.setpoint_min_interval_ms, 50, 1000);
        self.cmd_retries = self.cmd_retries.min(5);
        self.interp = self.interp.min(3);
        clamp_f64("state_fps", &mut self.state_fps, 1.0, 120.0, 10.0);
        clamp_u32("data_pts", &mut self.data_pts, 100, 1_000_000);
        let max_display = self.data_pts;
        clamp_u32("display_pts", &mut self.display_pts, 100, max_display);
        clamp_f64("v_threshold", &mut self.v_threshold, 0.0, 1.0, 0.002);
        clamp_f64("i_threshold", &mut self.i_threshold, 0.0, 1.0, 0.0);
        clamp_f64("last_vset", &mut self.last_vset, 0.8, 24.0, 5.0);
        clamp_f64("last_iset", &mut self.last_iset, 0.5, 5.0, 2.0);
        for (name, x, default) in [
            ("v_cali_k", &mut self.v_cali_k, 1.0),
            ("v_cali_b", &mut self.v_cali_b, 0.0),
            ("i_cali_k", &mut self.i_cali_k, 1.0),
            ("i_cali_b", &mut self.i_cali_b, 0.0),
            ("vset_cali_k", &mut self.vset_cali_k, 1.0),
            ("vset_cali_b", &mut self.vset_cali_b, 0.0),
            ("iset_cali_k", &mut self.iset_cali_k, 1.0),
            ("iset_cali_b", &mut self.iset_cali_b, 0.0),
        ] {
            clamp_f64(name, x, -100.0, 100.0, default);
        }
        for (name, k) in [("vset_cali_k", &mut self.vset_cali_k), ("iset_cali_k", &mut self.iset_cali_k)] {
            if *k == 0.0 {
                log::warn!("settings.{name}=0 is not allowed, using 1");
                *k = 1.0;
            }
        }
        if !["auto", "A", "mA", "µA"].contains(&self.current_unit.as_str()) {
            self.current_unit = "auto".into();
        }
        if !["auto", "zh", "en"].contains(&self.language.as_str()) {
            self.language = "auto".into();
        }
        if !["stop", "skip"].contains(&self.seq_oob_action.as_str()) {
            self.seq_oob_action = "stop".into();
        }
        clamp_f64("graph_max_fps", &mut self.graph_max_fps, 1.0, 120.0, 50.0);
        clamp_u32("log_split_minutes", &mut self.log_split_minutes, 0, 1440);
        clamp_u32("log_split_mb", &mut self.log_split_mb, 0, 2048);
        clamp_u32("log_flush_ms", &mut self.log_flush_ms, 200, 10_000);
        if self.http_port < 1024 {
            log::warn!("settings.http_port={} below 1024, using 17321", self.http_port);
            self.http_port = 17321;
        }
        if self.http_token.len() < 16 {
            self.http_token = new_token();
        }
        let a = &mut self.aux;
        clamp_f64("aux.keep_power.target_w", &mut a.keep_power.target_w, 0.1, 120.0, 1.0);
        clamp_f64("aux.keep_power.ki", &mut a.keep_power.ki, 0.001, 40.0, 2.5);
        clamp_f64("aux.keep_power.freq_hz", &mut a.keep_power.freq_hz, 0.1, 100.0, 10.0);
        clamp_f64("aux.sweep.step", &mut a.sweep.step, 0.001, 24.0, 0.1);
        clamp_f64("aux.sweep.delay_s", &mut a.sweep.delay_s, 0.1, 999_999.0, 1.0);
        clamp_f64("aux.wavegen.period_s", &mut a.wavegen.period_s, 0.001, 100_000.0, 1.0);
        clamp_f64("aux.wavegen.freq_hz", &mut a.wavegen.freq_hz, 0.1, 100.0, 10.0);
        clamp_f64("aux.batsim.soc_now", &mut a.batsim.soc_now, 0.0, 100.0, 100.0);
        clamp_f64("aux.batsim.soc_stop", &mut a.batsim.soc_stop, 0.0, 100.0, 0.0);
        clamp_f64("aux.batsim.cap_wh", &mut a.batsim.cap_wh, 0.001, 100_000.0, 10.0);
        clamp_f64("aux.batsim.r_mohm", &mut a.batsim.r_mohm, 0.0, 100_000.0, 0.0);
        clamp_f64("aux.batsim.freq_hz", &mut a.batsim.freq_hz, 0.1, 100.0, 10.0);
        a.batsim.cells = a.batsim.cells.clamp(1, 6);

        // 安全边界：任一值不合法则四个值全部恢复为硬件极限（不做部分钳位）
        if self.safety.validate().is_err() {
            log::warn!("settings.safety {:?} invalid, reset to hardware limits", self.safety);
            self.safety = SafetyLimits::hardware();
            notices.push(LoadNotice::SafetyReset);
        } else {
            self.safety = self.safety.rounded();
        }

        let (v_lo, v_hi) = (f64::from(*HW_V_MV.start()) / 1000.0, f64::from(*HW_V_MV.end()) / 1000.0);
        let (i_lo, i_hi) = (f64::from(*HW_I_MA.start()) / 1000.0, f64::from(*HW_I_MA.end()) / 1000.0);
        let mut fixed = false;
        let mut presets = default_presets();
        for (k, [v, i]) in std::mem::take(&mut self.presets) {
            if !presets.contains_key(&k) {
                continue;
            }
            let fv = if v.is_finite() { v.clamp(v_lo, v_hi) } else { v_lo };
            let fi = if i.is_finite() { i.clamp(i_lo, i_hi) } else { i_lo };
            fixed |= fv != v || fi != i;
            presets.insert(k, [fv, fi]);
        }
        self.presets = presets;
        if fixed {
            notices.push(LoadNotice::PresetsFixed);
        }
    }

    /// 先写临时文件再 rename。
    pub fn save(&self) -> std::io::Result<()> {
        let Some(path) = Self::path() else { return Ok(()) };
        self.save_to(&path)
    }

    pub fn save_to(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        let text = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(&tmp, text)?;
        // 文件里有 HTTP token：只允许当前用户读
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
        }
        std::fs::rename(&tmp, path)
    }

    pub fn record_dir(&self) -> PathBuf {
        if !self.record_dir.trim().is_empty() {
            return PathBuf::from(self.record_dir.trim());
        }
        directories::UserDirs::new()
            .and_then(|u| u.document_dir().map(Path::to_path_buf))
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
            .join("IOT_Power")
    }

    pub fn worker_config(&self) -> WorkerConfig {
        WorkerConfig {
            poll_interval: Duration::from_millis(u64::from(self.poll_interval_ms)),
            resp_timeout_poll: Duration::from_millis(u64::from(self.resp_timeout_poll_ms)),
            resp_timeout_cmd: Duration::from_millis(u64::from(self.resp_timeout_cmd_ms)),
            usb_timeout: Duration::from_millis(u64::from(self.usb_timeout_ms)),
            cmd_retries: self.cmd_retries,
            setpoint_min_interval: Duration::from_millis(u64::from(self.setpoint_min_interval_ms)),
            off_on_connect: self.off_on_connect,
        }
    }

    /// IO 线程兜底钳位用的边界：显示修正开启时只按硬件极限钳位
    pub fn guard_limits(&self) -> Option<SafetyLimits> {
        (!self.use_cali).then_some(self.safety)
    }
}

/// 32 位十六进制随机 token
pub fn new_token() -> String {
    let mut b = [0u8; 16];
    if getrandom::fill(&mut b).is_err() {
        b.iter_mut().for_each(|x| *x = fastrand::u8(..));
    }
    b.iter().map(|x| format!("{x:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_fields_get_defaults_and_unknown_fields_survive() {
        let (s, n) = Settings::from_json(r#"{"poll_interval_ms": 100, "future_key": 18000, "baudrate": 115200}"#);
        assert!(n.is_empty());
        assert_eq!(s.poll_interval_ms, 100);
        assert_eq!(s.resp_timeout_cmd_ms, 300);
        assert_eq!(s.extra.get("future_key"), Some(&Value::from(18000)));
        assert!(!s.extra.contains_key("baudrate"));
        let text = serde_json::to_string(&s).unwrap();
        assert!(text.contains("future_key"));
    }

    #[test]
    fn bad_field_type_only_drops_that_field() {
        let (s, _) = Settings::from_json(r#"{"poll_interval_ms": "fast", "auto_connect": true}"#);
        assert_eq!(s.poll_interval_ms, 50);
        assert!(s.auto_connect);
    }

    #[test]
    fn tc44_invalid_safety_resets_all_four() {
        let (s, n) = Settings::from_json(r#"{"safety": {"v_min": 3.0, "v_max": 30.0, "i_min": 1.0, "i_max": 2.0}}"#);
        assert_eq!(s.safety, SafetyLimits::hardware());
        assert_eq!(n, [LoadNotice::SafetyReset]);
    }

    #[test]
    fn aux_defaults_fit_safety_only_on_first_run() {
        let (s, _) = Settings::from_json(r#"{"safety": {"v_min": 3.0, "v_max": 12.0, "i_min": 0.5, "i_max": 5.0}}"#);
        assert_eq!((s.aux.keep_power.max_v, s.aux.sweep.start), (12.0, 3.0));
        let (s, _) = Settings::from_json(
            r#"{"safety": {"v_min": 3.0, "v_max": 12.0, "i_min": 0.5, "i_max": 5.0}, "aux": {"sweep": {"start": 1.5}}}"#,
        );
        assert_eq!(s.aux.sweep.start, 1.5);
        assert_eq!(s.aux.keep_power.max_v, 24.0);
        assert_eq!(s.http_token.len(), 32);
    }

    #[test]
    fn values_clamped() {
        let (s, _) = Settings::from_json(r#"{"poll_interval_ms": 10, "cmd_retries": 9, "vset_cali_k": 0}"#);
        assert_eq!(s.poll_interval_ms, 50);
        assert_eq!(s.cmd_retries, 5);
        assert_eq!(s.vset_cali_k, 1.0);
    }

    #[test]
    fn legacy_presets_fixed_to_hardware() {
        let (s, n) = Settings::from_json(r#"{"presets": {"9": [30.0, 10.0]}}"#);
        assert_eq!(s.presets["9"], [24.0, 5.0]);
        assert_eq!(s.presets["1"], [3.3, 2.0]);
        assert_eq!(n, [LoadNotice::PresetsFixed]);
    }

    #[test]
    fn save_roundtrip() {
        let dir = std::env::temp_dir().join(format!("iot_power_test_{}", std::process::id()));
        let p = dir.join("settings.json");
        let s = Settings {
            safety: SafetyLimits { v_min: 3.0, v_max: 12.0, i_min: 0.5, i_max: 2.0 },
            http_token: new_token(),
            ..Default::default()
        };
        s.save_to(&p).unwrap();
        let (r, n) = Settings::load_from(&p);
        assert!(n.is_empty());
        assert_eq!(r, s);
        let _ = std::fs::remove_dir_all(dir);
    }
}
