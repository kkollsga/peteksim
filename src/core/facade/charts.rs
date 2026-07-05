//! Crossplot extraction — pair two positioned well-log curves into a scatter
//! payload. This is the DATA-plumbing half (petekio per-bore log access lives in
//! [`Project`](super::project::Project)); the presentation half (regression, the
//! payload shape) is [`crate::core::charts`]. Render-only: samples are read, the trend is
//! fit deterministically in Rust, and only the line + coefficients ship.

use crate::core::charts::{crossplot_chart, Axis, ColorBy, Range, ScatterChart, ScatterInput};
use crate::core::facade::project::Project;
use crate::units::SrsError;
use serde_json::Value;

/// A crossplot request over a project's positioned logs.
#[derive(Debug, Clone)]
pub struct CrossplotOpts {
    /// x-axis log mnemonic (e.g. `"PHIE"`).
    pub x: String,
    /// y-axis log mnemonic (e.g. `"PERM"`).
    pub y: String,
    /// Restrict to these bore/well ids (a plain well id selects all its bores);
    /// `None` = every positioned bore.
    pub wells: Option<Vec<String>>,
    /// `"well"` / `"zone"` (categorical) or any log mnemonic (continuous ramp).
    pub color_by: String,
    pub x_log: bool,
    pub y_log: bool,
    /// Fit a per-group (categorical) or global trend line.
    pub regression: bool,
}

/// Build a [`ScatterChart`] from paired `(x, y)` log samples, coloured per
/// `opts.color_by`. Samples are paired on the x-curve's MDs (`y` resolved by
/// `at_md`); a non-finite either side is dropped.
pub fn crossplot(proj: &Project, opts: &CrossplotOpts) -> Result<ScatterChart, SrsError> {
    let cb = opts.color_by.to_ascii_lowercase();
    let categorical = cb == "well" || cb == "zone";

    let mut points: Vec<(f64, f64, Value)> = Vec::new();
    let mut groups: Vec<String> = Vec::new(); // categorical identities, encounter order
    let (mut cmin, mut cmax) = (f64::INFINITY, f64::NEG_INFINITY);

    for bw in proj.wells() {
        if let Some(sel) = &opts.wells {
            let parent = bw.id.split(' ').next().unwrap_or(&bw.id);
            if !sel.iter().any(|s| s == &bw.id || s == parent) {
                continue;
            }
        }
        let (Some(xv), Some(yv)) = (bw.log(&opts.x), bw.log(&opts.y)) else {
            continue; // this bore lacks one of the curves
        };
        // colour sources
        let zones = if cb == "zone" { bw.zones() } else { Vec::new() };
        let cv = if !categorical {
            bw.log(&opts.color_by)
        } else {
            None
        };

        let mds = xv.md();
        let vals = xv.values();
        for (md, &xval) in mds.iter().zip(vals.iter()) {
            if !xval.is_finite() {
                continue;
            }
            let Some(yval) = yv.at_md(*md) else { continue };
            if !yval.is_finite() {
                continue;
            }
            let c = if cb == "well" {
                cat_value(&bw.id, &mut groups)
            } else if cb == "zone" {
                let z = zone_at(*md, &zones);
                cat_value(&z, &mut groups)
            } else {
                match cv.as_ref().and_then(|v| v.at_md(*md)) {
                    Some(cval) if cval.is_finite() => {
                        cmin = cmin.min(cval);
                        cmax = cmax.max(cval);
                        Value::from(cval)
                    }
                    _ => continue, // no colour value at this depth on a continuous colour
                }
            };
            points.push((xval, yval, c));
        }
    }

    if points.is_empty() {
        return Err(SrsError::InvalidInput(format!(
            "crossplot: no paired ({}, {}) log samples found on the selected bores",
            opts.x, opts.y
        )));
    }

    let color_by = if categorical {
        Some(ColorBy {
            name: opts.color_by.clone(),
            kind: "categorical",
            range: None,
            units: None,
        })
    } else {
        Some(ColorBy {
            name: opts.color_by.clone(),
            kind: "continuous",
            range: Some(Range {
                min: cmin,
                max: cmax,
            }),
            units: None,
        })
    };

    Ok(crossplot_chart(ScatterInput {
        title: format!("{} vs {}", opts.x, opts.y),
        x: Axis {
            name: opts.x.clone(),
            units: String::new(),
            log: opts.x_log,
        },
        y: Axis {
            name: opts.y.clone(),
            units: String::new(),
            log: opts.y_log,
        },
        color_by,
        groups,
        points,
        regression: opts.regression,
    }))
}

/// Intern a category into `groups` (encounter order) and return it as a JSON string.
fn cat_value(name: &str, groups: &mut Vec<String>) -> Value {
    if !groups.iter().any(|g| g == name) {
        groups.push(name.to_string());
    }
    Value::String(name.to_string())
}

/// The name of the zone `md` falls in (`[top_md, base_md)`), or `"unzoned"`.
fn zone_at(md: f64, zones: &[petekio::Interval<'_>]) -> String {
    zones
        .iter()
        .find(|z| md >= z.top_md && md < z.base_md)
        .map(|z| z.name.clone())
        .unwrap_or_else(|| "unzoned".to_string())
}
