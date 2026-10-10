//! 曲线环形缓冲（`Doc/详细设计.md` §3.5）：容量 `data_pts`，满了丢最旧的点；
//! 断线后的第一个点标记 `gap`，绘图时在此处断开不连线。绘图与统计直接遍历缓冲区，不复制。

use std::collections::VecDeque;
use std::time::Instant;

use crate::format::Scale;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    /// 相对缓冲起点（连接或清空的时刻）的秒数
    pub t: f64,
    pub v: f64,
    pub i: f64,
    pub gap: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Series {
    Voltage,
    Current,
    Power,
    Resistance,
    None,
}

impl Series {
    pub const ALL: [Series; 5] = [Series::Voltage, Series::Current, Series::Power, Series::Resistance, Series::None];

    pub fn value(self, p: &Point) -> f64 {
        match self {
            Series::Voltage => p.v,
            Series::Current => p.i,
            Series::Power => p.v * p.i,
            Series::Resistance => {
                if p.i == 0.0 {
                    f64::INFINITY
                } else {
                    (p.v / p.i).abs()
                }
            }
            Series::None => f64::NAN,
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Series::Voltage => "qty_voltage",
            Series::Current => "qty_current",
            Series::Power => "qty_power",
            Series::Resistance => "qty_resistance",
            Series::None => "qty_none",
        }
    }

    /// 统计栏前缀（原版 `Vavg/Vmax/Vmin/Vpp`）
    pub fn letter(self) -> &'static str {
        match self {
            Series::Voltage => "V",
            Series::Current => "I",
            Series::Power => "P",
            Series::Resistance => "R",
            Series::None => "",
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Series::Voltage => "voltage",
            Series::Current => "current",
            Series::Power => "power",
            Series::Resistance => "resistance",
            Series::None => "none",
        }
    }

    pub fn parse(s: &str) -> Self {
        Self::ALL.into_iter().find(|x| x.as_str() == s).unwrap_or(Series::None)
    }

    pub fn base_unit(self) -> &'static str {
        match self {
            Series::Voltage | Series::None => "V",
            Series::Current => "A",
            Series::Power => "W",
            Series::Resistance => "Ω",
        }
    }

    /// 按显示区间最大绝对值选单位：返回 (乘数, 单位)
    pub fn unit_for(self, max_abs: f64) -> (f64, &'static str) {
        match self {
            Series::Voltage | Series::None => (1.0, "V"),
            Series::Current | Series::Power => {
                let base = if self == Series::Current { ["A", "mA", "µA"] } else { ["W", "mW", "µW"] };
                match crate::format::auto_scale(Scale::Unit, max_abs) {
                    Scale::Unit => (1.0, base[0]),
                    Scale::Milli => (1e3, base[1]),
                    Scale::Micro => (1e6, base[2]),
                }
            }
            Series::Resistance => {
                if max_abs >= 1e6 {
                    (1e-6, "MΩ")
                } else if max_abs >= 1e3 {
                    (1e-3, "kΩ")
                } else {
                    (1.0, "Ω")
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stats {
    pub avg: f64,
    pub max: f64,
    pub min: f64,
}

impl Stats {
    pub fn pp(&self) -> f64 {
        self.max - self.min
    }
}

/// 只统计有限值（阻值在电流为 0 时是 +inf，跳过）
pub fn stats<'a>(points: impl IntoIterator<Item = &'a Point>, s: Series) -> Option<Stats> {
    let mut n = 0usize;
    let (mut sum, mut max, mut min) = (0.0, f64::NEG_INFINITY, f64::INFINITY);
    for p in points {
        let x = s.value(p);
        if x.is_finite() {
            n += 1;
            sum += x;
            max = max.max(x);
            min = min.min(x);
        }
    }
    (n > 0).then(|| Stats { avg: sum / n as f64, max, min })
}

/// 切成连续段（在 gap 或非有限值处断开）；`n` 为点数，超过 `budget` 时按桶取 min/max 降采样。
pub fn segments<'a>(
    points: impl IntoIterator<Item = &'a Point>,
    n: usize,
    s: Series,
    scale: f64,
    budget: usize,
) -> Vec<Vec<[f64; 2]>> {
    let budget = budget.max(4);
    let bucket = if n <= budget { 1 } else { n.div_ceil(budget / 2) };
    let mut segs: Vec<Vec<[f64; 2]>> = Vec::new();
    let mut cur: Vec<[f64; 2]> = Vec::new();
    let mut chunk: Vec<[f64; 2]> = Vec::with_capacity(bucket);
    let flush = |chunk: &mut Vec<[f64; 2]>, cur: &mut Vec<[f64; 2]>| {
        if chunk.len() <= 2 {
            cur.append(chunk);
            return;
        }
        let (mut lo, mut hi) = (0, 0);
        for (k, q) in chunk.iter().enumerate() {
            if q[1] < chunk[lo][1] {
                lo = k;
            }
            if q[1] > chunk[hi][1] {
                hi = k;
            }
        }
        let (a, b) = if lo <= hi { (lo, hi) } else { (hi, lo) };
        cur.push(chunk[a]);
        if b != a {
            cur.push(chunk[b]);
        }
        chunk.clear();
    };
    for p in points {
        let y = s.value(p);
        if p.gap || !y.is_finite() {
            flush(&mut chunk, &mut cur);
            if !cur.is_empty() {
                segs.push(std::mem::take(&mut cur));
            }
        }
        if y.is_finite() {
            chunk.push([p.t, y * scale]);
            if chunk.len() >= bucket {
                flush(&mut chunk, &mut cur);
            }
        }
    }
    flush(&mut chunk, &mut cur);
    if !cur.is_empty() {
        segs.push(cur);
    }
    segs
}

pub struct SampleBuffer {
    points: VecDeque<Point>,
    cap: usize,
    t0: Option<Instant>,
    gap_next: bool,
    /// 累计写入点数（用于固定区间时跟着平移）
    total: u64,
}

impl SampleBuffer {
    pub fn new(cap: usize) -> Self {
        Self { points: VecDeque::new(), cap: cap.max(1), t0: None, gap_next: false, total: 0 }
    }

    pub fn push(&mut self, t: Instant, v: f64, i: f64) {
        let t0 = *self.t0.get_or_insert(t);
        let gap = std::mem::take(&mut self.gap_next) && !self.points.is_empty();
        self.points.push_back(Point { t: t.saturating_duration_since(t0).as_secs_f64(), v, i, gap });
        self.total += 1;
        while self.points.len() > self.cap {
            self.points.pop_front();
        }
    }

    pub fn mark_gap(&mut self) {
        self.gap_next = true;
    }

    pub fn clear(&mut self) {
        self.points.clear();
        self.t0 = None;
        self.gap_next = false;
    }

    /// 修改容量会清空（原版行为）
    pub fn set_cap(&mut self, cap: usize) {
        if cap.max(1) != self.cap {
            self.cap = cap.max(1);
            self.clear();
        }
    }

    pub fn len(&self) -> usize {
        self.points.len()
    }

    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    pub fn total(&self) -> u64 {
        self.total
    }

    pub fn usage(&self) -> f64 {
        self.points.len() as f64 / self.cap as f64
    }

    /// 下标 `lo..hi`（0 = 最旧）
    pub fn range(&self, lo: usize, hi: usize) -> impl Iterator<Item = &Point> + Clone {
        let hi = hi.min(self.points.len());
        self.points.range(lo.min(hi)..hi)
    }

    /// 最新的 `n` 个点；`None` 为全部
    #[cfg(test)]
    pub fn latest(&self, n: Option<usize>) -> Vec<Point> {
        let len = self.points.len();
        let start = n.map_or(0, |n| len.saturating_sub(n));
        self.points.range(start..).copied().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn ring_drops_oldest_and_marks_gap() {
        let t0 = Instant::now();
        let mut b = SampleBuffer::new(3);
        for k in 0..4 {
            b.push(t0 + Duration::from_millis(50 * k), 5.0, 0.1);
        }
        assert_eq!(b.len(), 3);
        assert_eq!(b.latest(None)[0].t, 0.05);
        b.mark_gap();
        b.push(t0 + Duration::from_secs(2), 5.0, 0.1);
        assert!(b.latest(Some(1))[0].gap);
        assert_eq!(b.total(), 5);
        assert_eq!(b.range(1, 3).count(), 2);
    }

    #[test]
    fn segments_split_at_gap_and_infinite() {
        let p = |t, i, gap| Point { t, v: 5.0, i, gap };
        let pts = [p(0.0, 1.0, false), p(1.0, 1.0, false), p(5.0, 1.0, true), p(6.0, 0.0, false), p(7.0, 1.0, false)];
        assert_eq!(segments(&pts, pts.len(), Series::Voltage, 1.0, 1000).len(), 2);
        // 阻值在 I=0 处是 inf，再断一次
        assert_eq!(segments(&pts, pts.len(), Series::Resistance, 1.0, 1000).len(), 3);
    }

    #[test]
    fn decimation_keeps_extremes() {
        let pts: Vec<Point> = (0..10_000)
            .map(|k| Point { t: k as f64, v: if k == 5000 { 9.0 } else { 5.0 }, i: 0.0, gap: false })
            .collect();
        let segs = segments(&pts, pts.len(), Series::Voltage, 1.0, 200);
        let n: usize = segs.iter().map(Vec::len).sum();
        assert!(n <= 220, "{n}");
        assert!(segs[0].iter().any(|q| q[1] == 9.0));
    }

    #[test]
    fn stats_skip_infinite() {
        let pts = [Point { t: 0.0, v: 5.0, i: 0.0, gap: false }, Point { t: 1.0, v: 5.0, i: 1.0, gap: false }];
        let s = stats(&pts, Series::Resistance).unwrap();
        assert_eq!((s.avg, s.pp()), (5.0, 0.0));
    }
}
