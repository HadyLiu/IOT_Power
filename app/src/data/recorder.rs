//! 录制：txt 采集日志（F17，`Doc/详细设计.md` §3.7）+ 原版兼容 CSV（F05，§3.1）。
//! 只追加不重写：txt 每 `log_flush_ms` 写一次、每 5 s fsync；CSV 每 30 s 写一次；按时长/大小分文件。

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use chrono::{DateTime, Local};
use dc3005u_proto::SafetyLimits;

use super::Processed;
use crate::cali::Cali;

pub const FORMAT_VERSION: u32 = 1;
const FSYNC_PERIOD: Duration = Duration::from_secs(5);
const CSV_PERIOD: Duration = Duration::from_secs(30);

#[derive(Debug, Clone)]
pub struct LogHeader {
    pub model: String,
    pub serial: String,
    pub firmware: String,
    pub poll_interval_ms: u32,
    pub safety: SafetyLimits,
    pub cali: Cali,
}

#[derive(Debug, Clone)]
pub struct RecordCfg {
    pub dir: PathBuf,
    pub csv: bool,
    pub debug_columns: bool,
    pub split_minutes: u32,
    pub split_mb: u32,
    pub flush_ms: u32,
    pub header: LogHeader,
}

/// txt 数据行里的输出状态与设备设定（由 UI 更新）
#[derive(Debug, Clone, PartialEq)]
pub struct LogCtx {
    /// `1`/`0` 已应答，`1?`/`0?` 未经确认，`U` 未知
    pub out: String,
    pub vset: Option<f64>,
    pub iset: Option<f64>,
}

impl Default for LogCtx {
    fn default() -> Self {
        Self { out: "U".into(), vset: None, iset: None }
    }
}

/// 事件字段：`key=value` 用制表符连接
pub fn kv(fields: &[(&str, String)]) -> String {
    fields.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("\t")
}

fn opt3(x: Option<f64>) -> String {
    x.map_or_else(String::new, |v| format!("{v:.3}"))
}

pub struct Recorder {
    cfg: RecordCfg,
    stem: String,
    part: u32,
    start_wall: DateTime<Local>,
    t0: Instant,
    have_sample: bool,
    part_started: Instant,
    txt: File,
    txt_path: PathBuf,
    txt_buf: String,
    txt_bytes: u64,
    csv: Option<File>,
    csv_buf: String,
    last_flush: Instant,
    last_fsync: Instant,
    last_csv: Instant,
    files: Vec<PathBuf>,
}

impl Recorder {
    pub fn start(cfg: RecordCfg, now: Instant, wall: DateTime<Local>) -> std::io::Result<Self> {
        std::fs::create_dir_all(&cfg.dir)?;
        let stem = format!("record_{}", wall.format("%Y-%m-%d_%H-%M-%S"));
        let (txt, txt_path, csv, files) = Self::open_part(&cfg, &stem, 1)?;
        let mut r = Self {
            cfg,
            stem,
            part: 1,
            start_wall: wall,
            t0: now,
            have_sample: false,
            part_started: now,
            txt,
            txt_path,
            txt_buf: String::new(),
            txt_bytes: 0,
            csv,
            csv_buf: String::new(),
            last_flush: now,
            last_fsync: now,
            last_csv: now,
            files,
        };
        r.write_headers(None);
        Ok(r)
    }

    fn part_name(stem: &str, part: u32, ext: &str) -> String {
        if part == 1 { format!("{stem}.{ext}") } else { format!("{stem}_part{part}.{ext}") }
    }

    #[allow(clippy::type_complexity)]
    fn open_part(
        cfg: &RecordCfg,
        stem: &str,
        part: u32,
    ) -> std::io::Result<(File, PathBuf, Option<File>, Vec<PathBuf>)> {
        let open = |p: &Path| OpenOptions::new().create(true).append(true).open(p);
        let txt_path = cfg.dir.join(Self::part_name(stem, part, "txt"));
        let txt = open(&txt_path)?;
        let mut files = vec![txt_path.clone()];
        let csv = if cfg.csv {
            let p = cfg.dir.join(Self::part_name(stem, part, "csv"));
            let f = open(&p)?;
            files.push(p);
            Some(f)
        } else {
            None
        };
        Ok((txt, txt_path, csv, files))
    }

    fn write_headers(&mut self, prev: Option<&str>) {
        let h = &self.cfg.header;
        let c = &h.cali;
        let lim = &h.safety;
        let lines = [
            format!("# IOT_Power-Log {FORMAT_VERSION}"),
            format!("# app=IOT_Power {}", env!("CARGO_PKG_VERSION")),
            format!("# device={} sn={} fw={}", h.model, h.serial, h.firmware),
            format!("# start={}", self.start_wall.format("%Y-%m-%dT%H:%M:%S%.3f%:z")),
            format!("# poll_interval_ms={}", h.poll_interval_ms),
            format!("# safety=v:{:.3}-{:.3}V i:{:.3}-{:.3}A", lim.v_min, lim.v_max, lim.i_min, lim.i_max),
            format!(
                "# cali=use:{} v:{},{} i:{},{} vset:{},{} iset:{},{}",
                u8::from(c.on),
                c.v.0,
                c.v.1,
                c.i.0,
                c.i.1,
                c.vset.0,
                c.vset.1,
                c.iset.0,
                c.iset.1
            ),
            format!("# part={} prev={}", self.part, prev.unwrap_or("")),
            "# columns=t_s\tv_V\ti_A\tp_W\tout\tvset_V\tiset_A\tseq".to_string(),
        ];
        for l in lines {
            self.push_txt(&l);
        }
        if self.csv.is_some() {
            self.csv_buf.push_str("time/s,voltage/V,current/A,power/W,seq");
            if self.cfg.debug_columns {
                self.csv_buf.push_str(",power_dev/W");
            }
            self.csv_buf.push('\n');
        }
    }

    fn push_txt(&mut self, line: &str) {
        self.txt_buf.push_str(line);
        self.txt_buf.push('\n');
        self.txt_bytes += line.len() as u64 + 1;
    }

    fn rel(&self, t: Instant) -> f64 {
        t.saturating_duration_since(self.t0).as_secs_f64()
    }

    pub fn current_txt(&self) -> &Path {
        &self.txt_path
    }

    pub fn sample(&mut self, p: &Processed, ctx: &LogCtx) -> std::io::Result<()> {
        if !self.have_sample {
            // 第一个样本 time = 0
            self.have_sample = true;
            self.t0 = p.sample.t;
        }
        self.maybe_split(p.sample.t)?;
        let t = self.rel(p.sample.t);
        let pw = p.v * p.i_a;
        let line = format!(
            "{t:.6}\t{:.3}\t{:.8}\t{pw:.8}\t{}\t{}\t{}\t{}",
            p.v,
            p.i_a,
            ctx.out,
            opt3(ctx.vset),
            opt3(ctx.iset),
            p.sample.meas.seq
        );
        self.push_txt(&line);
        if self.csv.is_some() {
            use std::fmt::Write as _;
            let _ = write!(self.csv_buf, "{t:.6},{:.6},{:.8},{pw:.8},{}", p.v, p.i_a, p.sample.meas.seq);
            if self.cfg.debug_columns {
                let _ = write!(self.csv_buf, ",{:.8}", p.sample.meas.power_dev_w);
            }
            self.csv_buf.push('\n');
        }
        Ok(())
    }

    pub fn event(&mut self, t: Instant, kind: &str, fields: &str) {
        let t = if self.have_sample { self.rel(t) } else { 0.0 };
        let line = if fields.is_empty() { format!("!{t:.6}\t{kind}") } else { format!("!{t:.6}\t{kind}\t{fields}") };
        self.push_txt(&line);
    }

    fn maybe_split(&mut self, now: Instant) -> std::io::Result<()> {
        let by_time = self.cfg.split_minutes > 0
            && now.saturating_duration_since(self.part_started)
                >= Duration::from_secs(u64::from(self.cfg.split_minutes) * 60);
        let by_size = self.cfg.split_mb > 0 && self.txt_bytes >= u64::from(self.cfg.split_mb) * 1024 * 1024;
        if !(by_time || by_size) {
            return Ok(());
        }
        self.flush_txt(true)?;
        self.flush_csv()?;
        let prev = self.txt_path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        self.part += 1;
        let (txt, txt_path, csv, files) = Self::open_part(&self.cfg, &self.stem, self.part)?;
        self.txt = txt;
        self.txt_path = txt_path;
        self.csv = csv;
        self.files.extend(files);
        self.txt_bytes = 0;
        self.part_started = now;
        self.write_headers(Some(&prev));
        Ok(())
    }

    fn flush_txt(&mut self, sync: bool) -> std::io::Result<()> {
        if !self.txt_buf.is_empty() {
            self.txt.write_all(self.txt_buf.as_bytes())?;
            self.txt_buf.clear();
        }
        if sync {
            self.txt.sync_data()?;
            self.last_fsync = Instant::now();
        }
        Ok(())
    }

    fn flush_csv(&mut self) -> std::io::Result<()> {
        if let Some(f) = &mut self.csv
            && !self.csv_buf.is_empty()
        {
            f.write_all(self.csv_buf.as_bytes())?;
            self.csv_buf.clear();
        }
        self.last_csv = Instant::now();
        Ok(())
    }

    /// 周期写盘
    pub fn tick(&mut self, now: Instant) -> std::io::Result<()> {
        if now.saturating_duration_since(self.last_flush) >= Duration::from_millis(u64::from(self.cfg.flush_ms)) {
            self.last_flush = now;
            let sync = now.saturating_duration_since(self.last_fsync) >= FSYNC_PERIOD;
            self.flush_txt(sync)?;
        }
        if now.saturating_duration_since(self.last_csv) >= CSV_PERIOD {
            self.flush_csv()?;
        }
        Ok(())
    }

    /// 立即写盘（panic hook / 退出）
    pub fn flush_all(&mut self) -> std::io::Result<()> {
        self.flush_txt(true)?;
        self.flush_csv()?;
        if let Some(f) = &mut self.csv {
            f.sync_data()?;
        }
        Ok(())
    }

    pub fn finish(mut self) -> (Vec<PathBuf>, std::io::Result<()>) {
        let r = self.flush_all();
        (std::mem::take(&mut self.files), r)
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        let _ = self.flush_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dc3005u_proto::Measurement;
    use dc3005u_usb::Sample;

    fn processed(t: Instant, seq: u32, v: f64, i: f64) -> Processed {
        let meas = Measurement { seq, voltage_v: v, ..Measurement::default() };
        Processed {
            sample: Sample { n: u64::from(seq), t, meas },
            v,
            i_a: i,
            energy_j: 0.0,
            avg_power_w: None,
            cum_energy_j: 0.0,
        }
    }

    fn cfg(dir: &Path) -> RecordCfg {
        RecordCfg {
            dir: dir.to_path_buf(),
            csv: true,
            debug_columns: false,
            split_minutes: 0,
            split_mb: 0,
            flush_ms: 1000,
            header: LogHeader {
                model: "DC3005U".into(),
                serial: "SN1".into(),
                firmware: "1.0".into(),
                poll_interval_ms: 50,
                safety: SafetyLimits::hardware(),
                cali: Cali::default(),
            },
        }
    }

    #[test]
    fn writes_header_samples_and_events() {
        let dir = std::env::temp_dir().join(format!("iot_rec_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let t0 = Instant::now();
        let mut r = Recorder::start(cfg(&dir), t0, Local::now()).unwrap();
        let ctx = LogCtx { out: "1".into(), vset: Some(5.0), iset: Some(2.0) };
        r.sample(&processed(t0, 7, 5.001, 0.57512345), &ctx).unwrap();
        r.event(t0 + Duration::from_millis(20), "SETPOINT", &kv(&[("vset", "5.000".into()), ("src", "manual".into())]));
        r.sample(&processed(t0 + Duration::from_millis(50), 8, 5.0, 0.5), &ctx).unwrap();
        let (files, res) = r.finish();
        res.unwrap();
        assert_eq!(files.len(), 2);
        let txt = std::fs::read_to_string(&files[0]).unwrap();
        assert!(txt.starts_with("# IOT_Power-Log 1\n"));
        assert!(txt.contains("0.000000\t5.001\t0.57512345\t2.87619237\t1\t5.000\t2.000\t7\n"));
        assert!(txt.contains("!0.020000\tSETPOINT\tvset=5.000\tsrc=manual\n"));
        let csv = std::fs::read_to_string(&files[1]).unwrap();
        assert!(csv.starts_with("time/s,voltage/V,current/A,power/W,seq\n0.000000,5.001000,0.57512345,"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn splits_by_size_with_prev_link() {
        let dir = std::env::temp_dir().join(format!("iot_rec_split_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut c = cfg(&dir);
        c.split_mb = 1;
        let t0 = Instant::now();
        let mut r = Recorder::start(c, t0, Local::now()).unwrap();
        let ctx = LogCtx::default();
        for k in 0..30_000u32 {
            r.sample(&processed(t0 + Duration::from_millis(u64::from(k)), k, 5.0, 0.1), &ctx).unwrap();
        }
        let (files, _) = r.finish();
        assert_eq!(files.len(), 4);
        let p2 = std::fs::read_to_string(&files[2]).unwrap();
        assert!(files[2].to_string_lossy().ends_with("_part2.txt"));
        assert!(p2.contains("# part=2 prev=record_"));
        assert!(std::fs::read_to_string(&files[3]).unwrap().starts_with("time/s,"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
