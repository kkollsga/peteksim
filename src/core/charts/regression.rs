//! Ordinary least squares for the crossplot trend — trivial, deterministic, done
//! in the plumbing so the viewer ships only the line (endpoints + coefficients),
//! never a fitter. Fits in the axis' own space: a log axis fits `log10` of that
//! variable (perm-vs-porosity reads straight on the log scale).

/// A fitted line `y' = slope·x' + intercept` (primes = the possibly-log-transformed
/// axis space) with its coefficient of determination.
#[derive(Debug, Clone, Copy)]
pub struct Fit {
    pub slope: f64,
    pub intercept: f64,
    pub r2: f64,
}

/// Least-squares fit of `points` after applying the per-axis log transform. Points
/// with a non-positive value on a log axis (or any non-finite) are dropped. Returns
/// `None` for fewer than two usable points or a degenerate (zero-variance) x.
pub fn fit(points: &[(f64, f64)], x_log: bool, y_log: bool) -> Option<Fit> {
    let tx = |v: f64| if x_log { v.log10() } else { v };
    let ty = |v: f64| if y_log { v.log10() } else { v };
    let mut xs = Vec::new();
    let mut ys = Vec::new();
    for &(x, y) in points {
        if (x_log && x <= 0.0) || (y_log && y <= 0.0) {
            continue;
        }
        let (a, b) = (tx(x), ty(y));
        if a.is_finite() && b.is_finite() {
            xs.push(a);
            ys.push(b);
        }
    }
    let n = xs.len();
    if n < 2 {
        return None;
    }
    let nf = n as f64;
    let mx = xs.iter().sum::<f64>() / nf;
    let my = ys.iter().sum::<f64>() / nf;
    let mut sxx = 0.0;
    let mut sxy = 0.0;
    let mut syy = 0.0;
    for i in 0..n {
        let dx = xs[i] - mx;
        let dy = ys[i] - my;
        sxx += dx * dx;
        sxy += dx * dy;
        syy += dy * dy;
    }
    if sxx <= 0.0 {
        return None;
    }
    let slope = sxy / sxx;
    let intercept = my - slope * mx;
    let r2 = if syy > 0.0 {
        (sxy * sxy) / (sxx * syy)
    } else {
        1.0
    };
    Some(Fit {
        slope,
        intercept,
        r2,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linear_fit_recovers_slope() {
        let pts: Vec<(f64, f64)> = (0..10).map(|i| (i as f64, 3.0 * i as f64 + 2.0)).collect();
        let f = fit(&pts, false, false).unwrap();
        assert!((f.slope - 3.0).abs() < 1e-9);
        assert!((f.intercept - 2.0).abs() < 1e-9);
        assert!((f.r2 - 1.0).abs() < 1e-9);
    }

    #[test]
    fn loglinear_fit_on_perm() {
        // perm = 10^(2*phi - 1): fitting log10(perm) vs phi recovers slope 2.
        let pts: Vec<(f64, f64)> = (1..20)
            .map(|i| {
                let phi = i as f64 * 0.02;
                (phi, 10f64.powf(2.0 * phi - 1.0))
            })
            .collect();
        let f = fit(&pts, false, true).unwrap();
        assert!((f.slope - 2.0).abs() < 1e-6);
    }

    #[test]
    fn degenerate_x_is_none() {
        assert!(fit(&[(1.0, 2.0), (1.0, 3.0)], false, false).is_none());
    }
}
