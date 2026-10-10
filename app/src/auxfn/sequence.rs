//! 序列文件（`Doc/详细设计.md` §3.4）：每行一个动作，与原版列表文本一致。

use chrono::NaiveDateTime;
use dc3005u_proto::{Field, SafetyLimits};

pub const WAIT_FORMAT: &str = "%Y-%m-%d %H:%M:%S";
pub const DELAY_MAX_MS: u32 = 100_000;

#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    Delay(u32),
    Wait(NaiveDateTime),
    /// 0 = 关输出
    SetV(f64),
    SetI(f64),
}

impl Step {
    pub fn to_line(&self) -> String {
        match self {
            Step::Delay(ms) => format!("DELAY {ms} ms"),
            Step::Wait(t) => format!("WAIT  {}", t.format(WAIT_FORMAT)),
            Step::SetV(v) => format!("SET-V {v:.3} V"),
            Step::SetI(i) => format!("SET-I {i:.3} A"),
        }
    }

    /// 硬件极限内（0 或 0.8~24 V、0.5~5.0 A）；用于载入校验
    pub fn within_hw(&self) -> bool {
        let hw = SafetyLimits::hardware();
        match *self {
            Step::Delay(ms) => ms <= DELAY_MAX_MS,
            Step::Wait(_) => true,
            Step::SetV(v) => v == 0.0 || hw.check(Field::Voltage, v).is_ok(),
            Step::SetI(i) => hw.check(Field::Current, i).is_ok(),
        }
    }

    /// 超出安全边界（`SET-V 0` 表示关输出，不受边界影响）
    pub fn outside(&self, lim: &SafetyLimits) -> bool {
        match *self {
            Step::SetV(v) => v != 0.0 && lim.check(Field::Voltage, v).is_err(),
            Step::SetI(i) => lim.check(Field::Current, i).is_err(),
            _ => false,
        }
    }

    pub fn is_timed(&self) -> bool {
        matches!(self, Step::Delay(ms) if *ms > 0) || matches!(self, Step::Wait(_))
    }
}

/// 按空白切分（修复原版 `WAIT  日期 时间` 无法载入的问题）
pub fn parse_line(line: &str) -> Option<Step> {
    let parts: Vec<&str> = line.split_whitespace().collect();
    match parts.as_slice() {
        ["DELAY", n, "ms"] => n.parse::<u32>().ok().map(Step::Delay),
        ["WAIT", d, t] => NaiveDateTime::parse_from_str(&format!("{d} {t}"), WAIT_FORMAT).ok().map(Step::Wait),
        ["SET-V", x, "V"] => x.parse::<f64>().ok().filter(|v| v.is_finite()).map(Step::SetV),
        ["SET-I", x, "A"] => x.parse::<f64>().ok().filter(|v| v.is_finite()).map(Step::SetI),
        _ => None,
    }
    .filter(Step::within_hw)
}

/// 整份校验，失败返回出错的那一行（整份不载入）。`#` 开头的行和空行跳过。
pub fn parse_text(text: &str) -> Result<Vec<Step>, String> {
    let mut out = Vec::new();
    for line in text.lines() {
        let l = line.trim();
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        out.push(parse_line(l).ok_or_else(|| l.to_string())?);
    }
    Ok(out)
}

pub fn to_text(steps: &[Step]) -> String {
    let mut s: String = steps.iter().map(|x| x.to_line() + "\n").collect();
    if s.is_empty() {
        s.push('\n');
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_including_wait_with_two_spaces() {
        let text = "DELAY 500 ms\nWAIT  2026-10-10 14:00:00\nSET-V 5.000 V\nSET-I 1.500 A\nSET-V 0.000 V\n";
        let steps = parse_text(text).unwrap();
        assert_eq!(steps.len(), 5);
        assert_eq!(to_text(&steps), text);
    }

    #[test]
    fn comments_skipped_bad_line_reported() {
        assert_eq!(parse_text("# hi\n\nDELAY 1 ms\n").unwrap().len(), 1);
        assert_eq!(parse_text("DELAY 1 ms\nSET-V 30 V\n").unwrap_err(), "SET-V 30 V");
        assert!(parse_text("SET-I 0.2 A").is_err());
        assert!(parse_text("SET-V 0.5 V").is_err());
    }

    #[test]
    fn safety_flags() {
        let lim = SafetyLimits { v_min: 3.0, v_max: 12.0, i_min: 0.5, i_max: 2.0 };
        assert!(Step::SetV(20.0).outside(&lim));
        assert!(!Step::SetV(0.0).outside(&lim));
        assert!(Step::SetI(3.0).outside(&lim));
    }
}
