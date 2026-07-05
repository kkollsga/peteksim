//! Analytics chart bundles — the render-only mapping of MC results + logs onto
//! petekTools' generic **`charts`** payload (viewer SCHEMA.md § ChartBundle).
//!
//! Strictly presentation plumbing: every number the viewer draws (tornado pivots,
//! histogram bins, exceedance points, regression coefficients) is computed **here**,
//! deterministically, in Rust and shipped in the payload. The viewer fits and bins
//! nothing. No new reservoir computation lives here — the tornado bars, the kept
//! realization vectors and the positioned logs all arrive from upstream surfaces.
//!
//! One concept per file: [`bins`] (the deterministic histogram rule), [`regression`]
//! (least squares for the crossplot trend), and the three builders
//! ([`tornado`], [`distribution`], [`crossplot`]).

mod bins;
mod crossplot;
mod distribution;
mod regression;
mod tornado;

pub use crossplot::{crossplot_chart, ScatterInput};
pub use distribution::{distribution_panel, DistMarkers};
pub use tornado::tornado_chart;

use serde::Serialize;
use serde_json::Value;

/// A tornado mark: ranked bars swinging around a base value (viewer draws the
/// nested inner/outer bands + the symmetric axis).
#[derive(Debug, Clone, Serialize)]
pub struct TornadoChart {
    pub mark: &'static str,
    pub title: String,
    pub units: String,
    pub base: f64,
    pub bars: Vec<TornadoBarOut>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fold_count: Option<usize>,
}

/// One tornado bar in display units. `out_lo`/`out_hi` are the inner (P90→P10)
/// band; `out_min`/`out_max` (the faint outer full-span) are `None` when only the
/// pivot band is available (the tornado's lo/hi percentiles) — inner-only, and the
/// payload says so by omitting them.
#[derive(Debug, Clone, Serialize)]
pub struct TornadoBarOut {
    pub param: String,
    /// A human display label disambiguating the raw input dimension (e.g. a
    /// property level shift `"PORO level shift"` vs the box draw `"porosity
    /// (draw)"`). Optional/additive — the viewer falls back to `param` when absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub in_lo: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub in_hi: Option<f64>,
    pub out_lo: f64,
    pub out_hi: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub out_min: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub out_max: Option<f64>,
    pub swing: f64,
}

/// A crossplot (scatter) mark.
#[derive(Debug, Clone, Serialize)]
pub struct ScatterChart {
    pub mark: &'static str,
    pub title: String,
    pub x: Axis,
    pub y: Axis,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color_by: Option<ColorBy>,
    pub groups: Vec<String>,
    pub points: Vec<ScatterPoint>,
    pub trends: Vec<Trend>,
}

/// A crossplot axis: name, units, and whether it renders on a log scale.
#[derive(Debug, Clone, Serialize)]
pub struct Axis {
    pub name: String,
    pub units: String,
    pub log: bool,
}

/// The colour-by-third encoding: categorical (identity palette) or continuous
/// (sequential ramp + colorbar over `range`).
#[derive(Debug, Clone, Serialize)]
pub struct ColorBy {
    pub name: String,
    pub kind: &'static str, // "categorical" | "continuous"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub range: Option<Range>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub units: Option<String>,
}

/// One scatter point; `c` is the colour value — a category name (categorical) or a
/// number (continuous).
#[derive(Debug, Clone, Serialize)]
pub struct ScatterPoint {
    pub x: f64,
    pub y: f64,
    pub c: Value,
}

/// A render-only regression line: the endpoints (in data space) the viewer draws,
/// plus the coefficients for the hover/legend. Fitting happens here, not in the JS.
#[derive(Debug, Clone, Serialize)]
pub struct Trend {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    pub kind: &'static str, // "linear" | "loglinear" | "linear-logx" | "loglog"
    pub x0: f64,
    pub y0: f64,
    pub x1: f64,
    pub y1: f64,
    pub slope: f64,
    pub intercept: f64,
    pub r2: f64,
    pub equation: String,
}

/// A volume-distribution mark: histogram + exceedance-CDF (two stacked panels).
#[derive(Debug, Clone, Serialize)]
pub struct DistributionPanel {
    pub mark: &'static str,
    pub title: String,
    pub units: String,
    pub series: Vec<DistSeries>,
}

/// One distribution overlay: pre-computed bins, exceedance points and P-markers.
#[derive(Debug, Clone, Serialize)]
pub struct DistSeries {
    pub name: String,
    pub bins: Vec<Bin>,
    pub cdf: Vec<CdfPoint>,
    pub markers: Markers,
}

/// A histogram bin: half-open `[lo, hi)` with its realization count.
#[derive(Debug, Clone, Serialize)]
pub struct Bin {
    pub lo: f64,
    pub hi: f64,
    pub count: u32,
}

/// An exceedance point: the fraction of realizations `≥ x` (reservoir sense).
#[derive(Debug, Clone, Serialize)]
pub struct CdfPoint {
    pub x: f64,
    pub exceedance: f64,
}

/// P-curve markers in the reservoir convention (P90 = low, P10 = high).
#[derive(Debug, Clone, Serialize)]
pub struct Markers {
    pub p90: f64,
    pub p50: f64,
    pub p10: f64,
}

/// A shared `{min, max}` range for an axis/colorbar domain.
#[derive(Debug, Clone, Serialize)]
pub struct Range {
    pub min: f64,
    pub max: f64,
}
