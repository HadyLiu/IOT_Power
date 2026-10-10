//! 结果窗的多项式拟合（`Doc/详细设计.md` §4.6）：Householder QR 解 Vandermonde 最小二乘，
//! 输出格式与 RMSE 定义同原版 `np.polyfit` + `polyval`。

/// 系数从最高次到常数项（同 np.polyfit）
#[allow(clippy::needless_range_loop)] // 矩阵按行列下标写更清楚
pub fn polyfit(x: &[f64], y: &[f64], order: usize) -> Option<Vec<f64>> {
    let n = x.len().min(y.len());
    let m = order + 1;
    if order == 0 || n <= order {
        return None;
    }
    // 先把 x 缩放到 [-1, 1] 附近，改善条件数
    let (lo, hi) = x.iter().fold((f64::MAX, f64::MIN), |(a, b), &v| (a.min(v), b.max(v)));
    let c = (hi + lo) / 2.0;
    let s = if hi > lo { (hi - lo) / 2.0 } else { 1.0 };
    let mut a: Vec<Vec<f64>> = (0..n)
        .map(|r| {
            let u = (x[r] - c) / s;
            (0..m).map(|j| u.powi((m - 1 - j) as i32)).collect()
        })
        .collect();
    let mut b: Vec<f64> = y[..n].to_vec();
    for k in 0..m {
        let norm = (k..n).map(|r| a[r][k] * a[r][k]).sum::<f64>().sqrt();
        if norm < 1e-300 {
            return None;
        }
        let alpha = if a[k][k] > 0.0 { -norm } else { norm };
        let mut v: Vec<f64> = (k..n).map(|r| a[r][k]).collect();
        v[0] -= alpha;
        let vn = v.iter().map(|t| t * t).sum::<f64>();
        if vn < 1e-300 {
            continue;
        }
        for j in k..m {
            let d = (k..n).map(|r| v[r - k] * a[r][j]).sum::<f64>() * 2.0 / vn;
            for r in k..n {
                a[r][j] -= d * v[r - k];
            }
        }
        let d = (k..n).map(|r| v[r - k] * b[r]).sum::<f64>() * 2.0 / vn;
        for r in k..n {
            b[r] -= d * v[r - k];
        }
    }
    let mut p = vec![0.0; m];
    for k in (0..m).rev() {
        let sum: f64 = (k + 1..m).map(|j| a[k][j] * p[j]).sum();
        if a[k][k].abs() < 1e-300 {
            return None;
        }
        p[k] = (b[k] - sum) / a[k][k];
    }
    // 把 u = (x - c)/s 的系数换回 x 的系数
    let mut out = vec![0.0; m];
    for (j, &pj) in p.iter().enumerate() {
        let deg = m - 1 - j;
        // pj * ((x - c)/s)^deg 展开
        let mut binom = 1.0;
        for t in 0..=deg {
            if t > 0 {
                binom = binom * (deg - t + 1) as f64 / t as f64;
            }
            let coef = pj * binom * (-c).powi((deg - t) as i32) / s.powi(deg as i32);
            out[m - 1 - t] += coef;
        }
    }
    Some(out)
}

pub fn polyval(p: &[f64], x: f64) -> f64 {
    p.iter().fold(0.0, |acc, c| acc * x + c)
}

pub fn rmse(p: &[f64], x: &[f64], y: &[f64]) -> f64 {
    let n = x.len().min(y.len());
    if n == 0 {
        return f64::NAN;
    }
    (x.iter().zip(y).map(|(&a, &b)| (b - polyval(p, a)).powi(2)).sum::<f64>() / n as f64).sqrt()
}

fn g4(c: f64) -> String {
    if c == 0.0 {
        return "0".into();
    }
    let e = c.abs().log10().floor() as i32;
    if !(-4..4).contains(&e) {
        let s = format!("{c:.3e}");
        // 1.000e5 → 1e+05 风格不重要，去掉多余的 0
        return s;
    }
    let d = (3 - e).max(0) as usize;
    let s = format!("{c:.d$}");
    if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_string() } else { s }
}

/// `rmse=%.4f y = (c0x^n) + (c1x^(n-1)) + … + (cn)`；|c| < 1e-9 视为 0
pub fn describe(p: &[f64], rmse: f64) -> String {
    let n = p.len() - 1;
    let terms: Vec<String> = p
        .iter()
        .enumerate()
        .map(|(j, &c)| {
            let c = if c.abs() < 1e-9 { 0.0 } else { c };
            let deg = n - j;
            match deg {
                0 => format!("({})", g4(c)),
                1 => format!("({}x)", g4(c)),
                _ => format!("({}x^{deg})", g4(c)),
            }
        })
        .collect();
    format!("rmse={rmse:.4} y = {}", terms.join(" + "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_quadratic() {
        let x: Vec<f64> = (0..20).map(|k| f64::from(k) * 0.5 + 3.0).collect();
        let y: Vec<f64> = x.iter().map(|v| 2.0 * v * v - 3.0 * v + 1.5).collect();
        let p = polyfit(&x, &y, 2).unwrap();
        assert!((p[0] - 2.0).abs() < 1e-8 && (p[1] + 3.0).abs() < 1e-7 && (p[2] - 1.5).abs() < 1e-6, "{p:?}");
        assert!(rmse(&p, &x, &y) < 1e-6);
        assert!(describe(&p, 0.0).starts_with("rmse=0.0000 y = (2x^2) + (-3x) + (1.5)"));
    }

    #[test]
    fn not_enough_points() {
        assert!(polyfit(&[1.0, 2.0], &[1.0, 2.0], 2).is_none());
        assert!(polyfit(&[1.0, 2.0], &[1.0, 2.0], 0).is_none());
    }
}
