//! 电池放电曲线（`Doc/详细设计.md` §4.5）：CSV `SOC/%,Voltage/V`，线性插值（同 np.interp，两端外推为端点值）。

pub const BUILTIN_NAME: &str = "Li-ion";
pub const BUILTIN_CSV: &str = include_str!("../../assets/Li-ion.csv");

#[derive(Debug, Clone, PartialEq)]
pub struct Curve {
    pub name: String,
    /// 按 SOC 升序
    pub points: Vec<(f64, f64)>,
}

impl Curve {
    pub fn builtin() -> Self {
        Self::parse(BUILTIN_NAME, BUILTIN_CSV).expect("built-in curve")
    }

    pub fn parse(name: &str, text: &str) -> Result<Self, String> {
        let mut pts = Vec::new();
        for (k, line) in text.lines().enumerate() {
            let l = line.trim();
            if l.is_empty() {
                continue;
            }
            let mut it = l.split(',').map(str::trim);
            let (Some(a), Some(b)) = (it.next(), it.next()) else { return Err(format!("line {}: {l}", k + 1)) };
            match (a.parse::<f64>(), b.parse::<f64>()) {
                (Ok(soc), Ok(v)) if soc.is_finite() && v.is_finite() => pts.push((soc, v)),
                // 表头
                _ if pts.is_empty() && k == 0 => {}
                _ => return Err(format!("line {}: {l}", k + 1)),
            }
        }
        if pts.len() < 2 {
            return Err("need at least 2 points".into());
        }
        if pts.iter().any(|&(s, v)| !(0.0..=100.0).contains(&s) || v <= 0.0) {
            return Err("SOC must be 0~100 and voltage > 0".into());
        }
        let inc = pts.windows(2).all(|w| w[1].0 > w[0].0);
        let dec = pts.windows(2).all(|w| w[1].0 < w[0].0);
        if !(inc || dec) {
            return Err("SOC must be monotonic".into());
        }
        if dec {
            pts.reverse();
        }
        Ok(Self { name: name.into(), points: pts })
    }

    pub fn ocv(&self, soc: f64) -> f64 {
        let p = &self.points;
        if soc <= p[0].0 {
            return p[0].1;
        }
        if soc >= p[p.len() - 1].0 {
            return p[p.len() - 1].1;
        }
        let k = p.partition_point(|&(s, _)| s <= soc);
        let ((s0, v0), (s1, v1)) = (p[k - 1], p[k]);
        v0 + (v1 - v0) * (soc - s0) / (s1 - s0)
    }

    pub fn max_v(&self) -> f64 {
        self.points.iter().map(|p| p.1).fold(f64::MIN, f64::max)
    }

    pub fn min_v(&self) -> f64 {
        self.points.iter().map(|p| p.1).fold(f64::MAX, f64::min)
    }

    /// 查看曲线：按 0.1% 重采样
    pub fn resampled(&self) -> Vec<[f64; 2]> {
        (0..=1000).map(|k| f64::from(k) / 10.0).map(|s| [s, self.ocv(s)]).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_parses_and_interpolates() {
        let c = Curve::builtin();
        assert!(c.points.len() > 10);
        assert_eq!(c.ocv(100.0), c.max_v());
        assert_eq!(c.ocv(150.0), c.max_v());
        let c = Curve::parse("t", "SOC/%,Voltage/V\n0,3.0\n100,4.0\n").unwrap();
        assert!((c.ocv(50.0) - 3.5).abs() < 1e-12);
        assert!(Curve::parse("t", "0,3\n50,3.5\n40,3.6").is_err());
    }
}
