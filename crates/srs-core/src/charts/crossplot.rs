//! Build a [`ScatterChart`](crate::charts::ScatterChart) from positioned-log
//! samples the facade extracts. Render-only: the points arrive here, the optional
//! regression is fit **here** (least squares, in the axes' own log/linear space) and
//! only the line endpoints + coefficients ship — the viewer never fits.

use crate::charts::regression::{self, Fit};
use crate::charts::{Axis, ColorBy, ScatterChart, ScatterPoint, Trend};
use serde_json::Value;

/// The facade's extracted crossplot: paired samples with a colour value each, plus
/// axis metadata and whether to fit a trend. `color_by` is pre-built by the facade
/// (categorical `groups` in encounter order, or a continuous range); `points` are
/// `(x, y, c)` with `c` a category name (categorical) or a number (continuous).
pub struct ScatterInput {
    pub title: String,
    pub x: Axis,
    pub y: Axis,
    pub color_by: Option<ColorBy>,
    pub groups: Vec<String>,
    pub points: Vec<(f64, f64, Value)>,
    pub regression: bool,
}

/// Assemble the scatter payload, fitting per-group trends when the colour is
/// categorical (else one global trend) if `regression` is set.
pub fn crossplot_chart(input: ScatterInput) -> ScatterChart {
    let ScatterInput {
        title,
        x,
        y,
        color_by,
        groups,
        points,
        regression,
    } = input;

    let trends = if regression {
        build_trends(&points, &groups, &x, &y)
    } else {
        Vec::new()
    };

    let pts = points
        .into_iter()
        .map(|(px, py, c)| ScatterPoint { x: px, y: py, c })
        .collect();

    ScatterChart {
        mark: "scatter",
        title,
        x,
        y,
        color_by,
        groups,
        points: pts,
        trends,
    }
}

/// One trend per categorical group (or a single global trend when there are no
/// groups), each spanning its own x-range.
fn build_trends(points: &[(f64, f64, Value)], groups: &[String], x: &Axis, y: &Axis) -> Vec<Trend> {
    if groups.is_empty() {
        let xy: Vec<(f64, f64)> = points.iter().map(|(a, b, _)| (*a, *b)).collect();
        return trend_for(None, &xy, x, y).into_iter().collect();
    }
    groups
        .iter()
        .filter_map(|g| {
            let xy: Vec<(f64, f64)> = points
                .iter()
                .filter(|(_, _, c)| c.as_str() == Some(g.as_str()))
                .map(|(a, b, _)| (*a, *b))
                .collect();
            trend_for(Some(g.clone()), &xy, x, y)
        })
        .collect()
}

/// Fit one line and turn it into a drawable [`Trend`] (endpoints at the x-range
/// extremes, in data space).
fn trend_for(group: Option<String>, xy: &[(f64, f64)], x: &Axis, y: &Axis) -> Option<Trend> {
    let fit = regression::fit(xy, x.log, y.log)?;
    // x-range in data space (drop non-positive on a log x).
    let (mut x0, mut x1) = (f64::INFINITY, f64::NEG_INFINITY);
    for &(px, _) in xy {
        if x.log && px <= 0.0 {
            continue;
        }
        x0 = x0.min(px);
        x1 = x1.max(px);
    }
    if x1 <= x0 {
        return None; // x0/x1 are finite (filtered above); a flat x can't anchor a line
    }
    let y_at = |px: f64| eval(&fit, px, x.log, y.log);
    Some(Trend {
        group,
        kind: kind_str(x.log, y.log),
        x0,
        y0: y_at(x0),
        x1,
        y1: y_at(x1),
        slope: fit.slope,
        intercept: fit.intercept,
        r2: fit.r2,
        equation: equation(&fit, x, y),
    })
}

/// Evaluate the fitted line at a data-space `px`, undoing the axis log transforms.
fn eval(fit: &Fit, px: f64, x_log: bool, y_log: bool) -> f64 {
    let xp = if x_log { px.log10() } else { px };
    let yp = fit.slope * xp + fit.intercept;
    if y_log {
        10f64.powf(yp)
    } else {
        yp
    }
}

fn kind_str(x_log: bool, y_log: bool) -> &'static str {
    match (x_log, y_log) {
        (false, false) => "linear",
        (false, true) => "loglinear",
        (true, false) => "linear-logx",
        (true, true) => "loglog",
    }
}

/// A human-readable fit expression in the axes' names (log-wrapped per axis).
fn equation(fit: &Fit, x: &Axis, y: &Axis) -> String {
    let xt = if x.log {
        format!("log10({})", x.name)
    } else {
        x.name.clone()
    };
    let yt = if y.log {
        format!("log10({})", y.name)
    } else {
        y.name.clone()
    };
    let sign = if fit.intercept >= 0.0 { "+" } else { "−" };
    format!(
        "{yt} = {:.3}·{xt} {sign} {:.3}  (R²={:.3})",
        fit.slope,
        fit.intercept.abs(),
        fit.r2
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn axis(name: &str, log: bool) -> Axis {
        Axis {
            name: name.into(),
            units: String::new(),
            log,
        }
    }

    #[test]
    fn per_group_trends_on_log_y() {
        let mut points = Vec::new();
        for i in 1..30 {
            let phi = i as f64 * 0.01;
            points.push((phi, 10f64.powf(2.0 * phi - 1.0), json!("A")));
        }
        let input = ScatterInput {
            title: "PHIE vs PERM".into(),
            x: axis("PHIE", false),
            y: axis("PERM", true),
            color_by: None,
            groups: vec!["A".to_string()],
            points,
            regression: true,
        };
        let ch = crossplot_chart(input);
        assert_eq!(ch.mark, "scatter");
        assert_eq!(ch.points.len(), 29);
        assert_eq!(ch.trends.len(), 1);
        let t = &ch.trends[0];
        assert_eq!(t.group.as_deref(), Some("A"));
        assert_eq!(t.kind, "loglinear");
        assert!((t.slope - 2.0).abs() < 1e-6);
        assert!(t.r2 > 0.999);
        assert!(t.y1 > t.y0); // positive perm-porosity trend, drawn in data space
    }

    #[test]
    fn no_regression_ships_no_trends() {
        let input = ScatterInput {
            title: "t".into(),
            x: axis("a", false),
            y: axis("b", false),
            color_by: None,
            groups: vec![],
            points: vec![(1.0, 2.0, json!("g")), (2.0, 4.0, json!("g"))],
            regression: false,
        };
        assert!(crossplot_chart(input).trends.is_empty());
    }
}
