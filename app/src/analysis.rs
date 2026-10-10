//! 记录分析（F17，`Doc/详细设计.md` §3.7）：解析 txt 采集日志（可拼接多个 part）、区间统计、导出 CSV。

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use crate::buffer::Point;
use crate::data::recorder::FORMAT_VERSION;

const MAGIC: &str = "# IOT_Power-Log";

#[derive(Debug, Clone, PartialEq)]
pub struct LogEvent {
    pub t: f64,
    pub kind: String,
    pub fields: String,
}

impl LogEvent {
    pub fn field(&self, key: &str) -> Option<&str> {
        self.fields.split('\t').find_map(|kv| kv.strip_prefix(key)?.strip_prefix('='))
    }
}

#[derive(Debug, Clone, Default)]
pub struct LogData {
    /// 第一个文件的文件头（`# key=value`）
    pub header: Vec<(String, String)>,
    pub points: Vec<Point>,
    pub events: Vec<LogEvent>,
    /// 最后一行不完整（已忽略）
    pub truncated: bool,
    /// 格式版本高于当前支持的版本
    pub newer_version: bool,
    pub bad_lines: usize,
}

impl LogData {
    pub fn header(&self, key: &str) -> Option<&str> {
        self.header.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    pub fn span(&self) -> (f64, f64) {
        match (self.points.first(), self.points.last()) {
            (Some(a), Some(b)) => (a.t, b.t.max(a.t + 1e-6)),
            _ => (0.0, 1.0),
        }
    }

    /// 时间区间对应的下标范围
    pub fn index_range(&self, t0: f64, t1: f64) -> (usize, usize) {
        let lo = self.points.partition_point(|p| p.t < t0);
        let hi = self.points.partition_point(|p| p.t <= t1);
        (lo, hi.max(lo))
    }
}

/// `record_x.txt`, `record_x_part2.txt`, … 按录制与分段编号排序
pub fn order_parts(paths: &mut [PathBuf]) {
    fn key(p: &Path) -> (String, u32) {
        let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        match stem.rsplit_once("_part") {
            Some((base, n)) if n.parse::<u32>().is_ok() => (base.to_string(), n.parse().unwrap_or(1)),
            _ => (stem, 1),
        }
    }
    paths.sort_by_key(|p| key(p));
}

pub fn parse_files(paths: &[PathBuf], progress: &AtomicU32) -> Result<LogData, String> {
    let mut paths = paths.to_vec();
    order_parts(&mut paths);
    let total: u64 = paths.iter().map(|p| std::fs::metadata(p).map_or(0, |m| m.len())).sum::<u64>().max(1);
    let mut out = LogData::default();
    let mut done = 0u64;
    for (k, p) in paths.iter().enumerate() {
        let text = std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))?;
        parse_text(&text, k == 0, &mut out, |n| {
            progress.store(((done + n as u64) * 100 / total) as u32, Ordering::Relaxed);
        })
        .map_err(|e| format!("{}: {e}", p.display()))?;
        done += text.len() as u64;
    }
    progress.store(100, Ordering::Relaxed);
    if out.points.is_empty() && out.events.is_empty() {
        return Err("no data".into());
    }
    Ok(out)
}

/// 解析一个文件，追加到 `out`。`first` 时保存文件头。
pub fn parse_text(text: &str, first: bool, out: &mut LogData, mut progress: impl FnMut(usize)) -> Result<(), String> {
    if !text.starts_with(MAGIC) {
        return Err("missing `# IOT_Power-Log` header".into());
    }
    let mut cols: Option<(usize, usize, usize)> = None;
    let mut gap_next = false;
    let mut pos = 0usize;
    for (n, line) in text.split_inclusive('\n').enumerate() {
        pos += line.len();
        if n % 20_000 == 0 {
            progress(pos);
        }
        let Some(line) = line.strip_suffix('\n') else {
            if !line.trim().is_empty() {
                out.truncated = true;
            }
            break;
        };
        let line = line.strip_suffix('\r').unwrap_or(line);
        if let Some(h) = line.strip_prefix('#') {
            let h = h.trim();
            if let Some(v) = h.strip_prefix("IOT_Power-Log") {
                if v.trim().parse::<u32>().is_ok_and(|x| x > FORMAT_VERSION) {
                    out.newer_version = true;
                }
                continue;
            }
            let Some((k, v)) = h.split_once('=') else { continue };
            if k == "columns" {
                let names: Vec<&str> = v.split('\t').collect();
                let find = |c: &str| names.iter().position(|x| *x == c);
                cols = Some((
                    find("t_s").ok_or("no t_s column")?,
                    find("v_V").ok_or("no v_V column")?,
                    find("i_A").ok_or("no i_A column")?,
                ));
            }
            if first {
                out.header.push((k.to_string(), v.to_string()));
            }
            continue;
        }
        if let Some(ev) = line.strip_prefix('!') {
            let mut it = ev.splitn(3, '\t');
            let (Some(t), Some(kind)) = (it.next().and_then(|t| t.parse::<f64>().ok()), it.next()) else {
                out.bad_lines += 1;
                continue;
            };
            let e = LogEvent { t, kind: kind.to_string(), fields: it.next().unwrap_or("").to_string() };
            if e.kind == "LINK" && e.field("state") == Some("lost") {
                gap_next = true;
            }
            out.events.push(e);
            continue;
        }
        if line.is_empty() {
            continue;
        }
        let Some((ct, cv, ci)) = cols else { return Err("data before `# columns=`".into()) };
        let f: Vec<&str> = line.split('\t').collect();
        let num = |k: usize| f.get(k).and_then(|s| s.parse::<f64>().ok());
        match (num(ct), num(cv), num(ci)) {
            (Some(t), Some(v), Some(i)) => {
                let gap = std::mem::take(&mut gap_next) && !out.points.is_empty();
                out.points.push(Point { t, v, i, gap });
            }
            _ => out.bad_lines += 1,
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChanStats {
    pub max: f64,
    pub min: f64,
    pub avg: f64,
    pub rms: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RangeStats {
    pub n: usize,
    pub t0: f64,
    pub t1: f64,
    pub v: ChanStats,
    pub i: ChanStats,
    pub p: ChanStats,
    pub wh: f64,
    pub mah: f64,
    pub gaps: usize,
    pub gap_s: f64,
}

fn chan(pts: &[Point], f: impl Fn(&Point) -> f64) -> ChanStats {
    let (mut max, mut min, mut sum, mut sq) = (f64::NEG_INFINITY, f64::INFINITY, 0.0, 0.0);
    for p in pts {
        let x = f(p);
        max = max.max(x);
        min = min.min(x);
        sum += x;
        sq += x * x;
    }
    let n = pts.len().max(1) as f64;
    ChanStats { max, min, avg: sum / n, rms: (sq / n).sqrt() }
}

/// 区间统计：能量 Wh = Σ p·Δt / 3600、容量 mAh = Σ i·Δt / 3.6；断线时段不计入积分
pub fn range_stats(pts: &[Point]) -> Option<RangeStats> {
    let (first, last) = (pts.first()?, pts.last()?);
    let (mut e_j, mut q_c, mut gaps, mut gap_s) = (0.0, 0.0, 0usize, 0.0);
    for w in pts.windows(2) {
        let dt = w[1].t - w[0].t;
        if w[1].gap {
            gaps += 1;
            gap_s += dt;
            continue;
        }
        e_j += w[1].v * w[1].i * dt;
        q_c += w[1].i * dt;
    }
    Some(RangeStats {
        n: pts.len(),
        t0: first.t,
        t1: last.t,
        v: chan(pts, |p| p.v),
        i: chan(pts, |p| p.i),
        p: chan(pts, |p| p.v * p.i),
        wh: e_j / 3600.0,
        mah: q_c / 3.6,
        gaps,
        gap_s,
    })
}

/// C 的 `%.6g`
pub fn fmt_g6(x: f64) -> String {
    if x.is_infinite() {
        return if x > 0.0 { "inf".into() } else { "-inf".into() };
    }
    if x == 0.0 || !x.is_finite() {
        return format!("{x}");
    }
    // %g：按四舍五入到 6 位有效数字后的指数决定格式
    let sci = format!("{x:.5e}");
    let (m, ex) = sci.split_once('e').unwrap_or((&sci, "0"));
    let e: i32 = ex.parse().unwrap_or(0);
    if !(-4..6).contains(&e) {
        let m = if m.contains('.') { m.trim_end_matches('0').trim_end_matches('.') } else { m };
        return format!("{m}e{}{:02}", if e < 0 { '-' } else { '+' }, e.abs());
    }
    let d = (5 - e).max(0) as usize;
    let s = format!("{x:.d$}");
    if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_string() } else { s }
}

/// 导出格式（与原版 dump 一致）：`time/s,voltage/V,current/A,power/W,resistance/ohm`
pub fn write_points_csv<'a>(path: &Path, pts: impl IntoIterator<Item = &'a Point>) -> std::io::Result<()> {
    let mut w = std::io::BufWriter::new(std::fs::File::create(path)?);
    writeln!(w, "time/s,voltage/V,current/A,power/W,resistance/ohm")?;
    for p in pts {
        let r = if p.i == 0.0 { f64::INFINITY } else { (p.v / p.i).abs() };
        writeln!(w, "{:.6},{:.6},{:.8},{:.8},{}", p.t, p.v, p.i, p.v * p.i, fmt_g6(r))?;
    }
    w.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOG: &str = "# IOT_Power-Log 1\n# app=IOT_Power 0.2.0\n# device=DC3005U sn=X fw=1\n# poll_interval_ms=50\n\
# columns=t_s\tv_V\ti_A\tp_W\tout\tvset_V\tiset_A\tseq\n\
0.000000\t5.000\t1.00000000\t5.00000000\t1\t5.000\t2.000\t1\n\
1.000000\t5.000\t1.00000000\t5.00000000\t1\t5.000\t2.000\t2\n\
!1.500000\tLINK\tstate=lost\n\
!3.000000\tLINK\tstate=restored\tattempt=1\tout=unconfirmed\n\
3.000000\t5.000\t1.00000000\t5.00000000\t1?\t5.000\t2.000\t3\n\
4.000000\t5.000\t1.00000000\t5.00000000\t1?\t5.000\t2.000\t4\n\
5.000000\t5.0";

    #[test]
    fn parses_events_gaps_and_truncation() {
        let mut d = LogData::default();
        parse_text(LOG, true, &mut d, |_| {}).unwrap();
        assert_eq!(d.points.len(), 4);
        assert!(d.truncated);
        assert!(d.points[2].gap && !d.points[3].gap);
        assert_eq!(d.events.len(), 2);
        assert_eq!(d.events[1].field("attempt"), Some("1"));
        assert_eq!(d.header("poll_interval_ms"), Some("50"));
        let s = range_stats(&d.points).unwrap();
        // 断线 1→3 s 不计入：积分时长 2 s × 5 W
        assert!((s.wh - 10.0 / 3600.0).abs() < 1e-12);
        assert!((s.mah - 2.0 / 3.6).abs() < 1e-12);
        assert_eq!((s.gaps, s.gap_s), (1, 2.0));
        assert_eq!(d.index_range(0.5, 3.5), (1, 3));
    }

    #[test]
    fn rejects_foreign_files_and_orders_parts() {
        let mut d = LogData::default();
        assert!(parse_text("time/s,voltage/V\n", true, &mut d, |_| {}).is_err());
        let mut v = vec![PathBuf::from("a/r_part3.txt"), PathBuf::from("a/r.txt"), PathBuf::from("a/r_part2.txt")];
        order_parts(&mut v);
        assert_eq!(v[0], PathBuf::from("a/r.txt"));
        assert_eq!(v[2], PathBuf::from("a/r_part3.txt"));
    }

    #[test]
    fn g6_matches_c() {
        assert_eq!(fmt_g6(5.0), "5");
        assert_eq!(fmt_g6(1234.5678), "1234.57");
        assert_eq!(fmt_g6(0.000012345), "1.2345e-05");
        assert_eq!(fmt_g6(12345678.0), "1.23457e+07");
        assert_eq!(fmt_g6(f64::INFINITY), "inf");
    }
}
