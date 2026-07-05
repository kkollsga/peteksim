//! Declarative value-holder specs (`ps.normal`, `ps.spherical`, `ps.layers`, …) and their module-level builders.

use super::*;

/// A named appraisal distribution (optionally clamped) — `ps.normal(...)`, etc.
#[pyclass(name = "Dist", from_py_object)]
#[derive(Clone)]
pub struct Dist {
    pub(crate) spec: DistSpec,
}

#[pymethods]
impl Dist {
    /// Hard-clamp draws onto `[lo, hi]`.
    fn clamped(&self, lo: f64, hi: f64) -> PyResult<Dist> {
        Ok(Dist {
            spec: self.spec.clone().clamped(lo, hi).map_err(err)?,
        })
    }
    fn __repr__(&self) -> String {
        "Dist(...)".into()
    }
}

/// A contact pick-spread (`ps.pick_spread(sd_m=..)`) applied to the model's
/// contacts under MC.
#[pyclass(name = "PickSpread", from_py_object)]
#[derive(Clone)]
pub struct PickSpread {
    #[pyo3(get)]
    pub(crate) sd_m: f64,
}

/// A variogram model spec (`ps.spherical/exponential/gaussian_vgm/fit_variogram`).
#[pyclass(name = "Vgm", from_py_object)]
#[derive(Clone)]
pub struct Vgm {
    pub(crate) spec: VgmSpec,
}

/// An SGS propagation spec (`ps.gaussian(variogram=.., seed=..)`).
#[pyclass(name = "GaussianSpec", skip_from_py_object)]
#[derive(Clone)]
pub struct GaussianSpec {
    pub(crate) vgm: Vgm,
    pub(crate) seed: u64,
    pub(crate) search: Option<(usize, f64)>,
}

/// A collocated-cokriging trend (`ps.collocated(surface, corr=..)`).
#[pyclass(name = "Trend", skip_from_py_object)]
#[derive(Clone)]
pub struct Trend {
    pub(crate) spec: TrendSpec,
}

/// A layer allocation (`ps.layers(n=..)` / `ps.layers(dz_m=..)`) + conformity style.
#[pyclass(name = "Layers", from_py_object)]
#[derive(Clone)]
pub struct Layers {
    pub(crate) spec: LayersSpec,
    pub(crate) conformity: Conformity,
}
/// `ps.normal(mean, sd)`.
#[pyfunction]
pub fn normal(mean: f64, sd: f64) -> PyResult<Dist> {
    Ok(Dist {
        spec: DistSpec::normal(mean, sd).map_err(err)?,
    })
}

/// `ps.lognormal(mean, sd)` (log-space parameters).
#[pyfunction]
pub fn lognormal(mean: f64, sd: f64) -> PyResult<Dist> {
    Ok(Dist {
        spec: DistSpec::lognormal(mean, sd).map_err(err)?,
    })
}

/// `ps.uniform(lo, hi)`.
#[pyfunction]
pub fn uniform(lo: f64, hi: f64) -> PyResult<Dist> {
    Ok(Dist {
        spec: DistSpec::uniform(lo, hi).map_err(err)?,
    })
}

/// `ps.triangular(lo, mode, hi)`.
#[pyfunction]
pub fn triangular(lo: f64, mode: f64, hi: f64) -> PyResult<Dist> {
    Ok(Dist {
        spec: DistSpec::triangular(lo, mode, hi).map_err(err)?,
    })
}

/// `ps.truncated_normal(mean, sd, lo, hi)`.
#[pyfunction]
pub fn truncated_normal(mean: f64, sd: f64, lo: f64, hi: f64) -> PyResult<Dist> {
    Ok(Dist {
        spec: DistSpec::truncated_normal(mean, sd, lo, hi).map_err(err)?,
    })
}

/// `ps.level_shift(sd)` — a zero-mean additive shift for a modelled cube.
#[pyfunction]
pub fn level_shift(sd: f64) -> PyResult<Dist> {
    Ok(Dist {
        spec: DistSpec::level_shift(sd).map_err(err)?,
    })
}

/// `ps.pick_spread(sd_m)` — a contact spread.
#[pyfunction]
pub fn pick_spread(sd_m: f64) -> PickSpread {
    PickSpread { sd_m }
}

/// `ps.spherical(range_m, sill=1, nugget=0)`.
#[pyfunction]
#[pyo3(signature = (range_m, sill=1.0, nugget=0.0))]
pub fn spherical(range_m: f64, sill: f64, nugget: f64) -> PyResult<Vgm> {
    Ok(Vgm {
        spec: VgmSpec::spherical(range_m, sill, nugget).map_err(err)?,
    })
}

/// `ps.exponential(range_m, sill=1, nugget=0)`.
#[pyfunction]
#[pyo3(signature = (range_m, sill=1.0, nugget=0.0))]
pub fn exponential(range_m: f64, sill: f64, nugget: f64) -> PyResult<Vgm> {
    Ok(Vgm {
        spec: VgmSpec::exponential(range_m, sill, nugget).map_err(err)?,
    })
}

/// `ps.gaussian_vgm(range_m, sill=1, nugget=0)`.
#[pyfunction]
#[pyo3(signature = (range_m, sill=1.0, nugget=0.0))]
pub fn gaussian_vgm(range_m: f64, sill: f64, nugget: f64) -> PyResult<Vgm> {
    Ok(Vgm {
        spec: VgmSpec::gaussian(range_m, sill, nugget).map_err(err)?,
    })
}

/// `ps.fit_variogram(coords, model="spherical", lag=.., n_lags=..)` — fit a
/// model to an experimental variogram over `[x, y, value]` rows.
#[pyfunction]
#[pyo3(signature = (coords, model="spherical", lag=1.0, n_lags=10))]
pub fn fit_variogram(coords: Vec<[f64; 3]>, model: &str, lag: f64, n_lags: usize) -> PyResult<Vgm> {
    let vm = parse_vgm_model(model)?;
    Ok(Vgm {
        spec: VgmSpec::fit(vm, &coords, lag, n_lags).map_err(err)?,
    })
}

pub(crate) fn parse_vgm_model(model: &str) -> PyResult<VariogramModel> {
    match model.to_ascii_lowercase().as_str() {
        "spherical" => Ok(VariogramModel::Spherical),
        "exponential" => Ok(VariogramModel::Exponential),
        "gaussian" => Ok(VariogramModel::Gaussian),
        "nugget" => Ok(VariogramModel::Nugget),
        other => Err(PyValueError::new_err(format!(
            "variogram model must be spherical/exponential/gaussian/nugget, got '{other}'"
        ))),
    }
}

/// `ps.gaussian(variogram, seed, max_neighbours=None, radius_m=None)` — an SGS spec.
#[pyfunction]
#[pyo3(signature = (variogram, seed=1, max_neighbours=None, radius_m=None))]
pub fn gaussian(
    variogram: Vgm,
    seed: u64,
    max_neighbours: Option<usize>,
    radius_m: Option<f64>,
) -> GaussianSpec {
    let search = match (max_neighbours, radius_m) {
        (Some(n), Some(r)) => Some((n, r)),
        _ => None,
    };
    GaussianSpec {
        vgm: variogram,
        seed,
        search,
    }
}

/// `ps.collocated(surface, corr, as_depth=False)` — a collocated-cokriging trend
/// from a loaded surface.
///
/// **F7 — a trend is not a depth.** By default (`as_depth=False`) the surface is
/// read as a **trend in its own units** (a net-sand fraction, an amplitude, …),
/// so `corr` reads against the surface value: a property that tracks the trend
/// takes a positive `corr`. This is what a depo-trend / net-sand grid needs — the
/// facade previously *always* negated the surface (elevation→depth), so a
/// `[0, 1]` trend became negative and the seam rejected it ("trend multiplier
/// must be non-negative").
///
/// Pass `as_depth=True` for a **structural** surface you want steered by *depth*:
/// petekio's negative-down elevation is flipped to positive-down depth (the same
/// Z1-family flip the framework applies), so `corr>0` = the property increases
/// with depth.
///
/// Either way the field is shifted to be non-negative if needed (the seam's
/// trend is a multiplier domain); because the collocated secondary is
/// standardized internally (normal scores), an additive offset does not change
/// the steering — it only satisfies the constructor.
#[pyfunction]
#[pyo3(signature = (surface, corr, as_depth=false))]
pub fn collocated(py: Python<'_>, surface: &Surface, corr: f64, as_depth: bool) -> PyResult<Trend> {
    let proj = surface.project.bind(py).borrow();
    let spec = proj
        .inner
        .collocated_trend(&surface.name, corr, as_depth)
        .map_err(err)?;
    Ok(Trend { spec })
}

/// `ps.resimulate()` — select the **resimulate** MC mode for
/// `property.propagate(..., resimulate=...)`: a fresh SGS pattern per realization
/// (captures heterogeneity uncertainty) instead of the default level-shift.
/// Equivalent to `resimulate=True`; exposed because the symbol was expected to
/// exist (`propagate(resimulate=ps.resimulate())`).
#[pyfunction]
pub fn resimulate() -> bool {
    true
}

/// `ps.layers(n=None, dz_m=None, style="proportional")`.
///
/// `style` selects the conformity:
/// - `"proportional"` (default) — equal-fraction layers; the count is `n=` (or
///   `dz_m=` → `ceil(gross/dz)`).
/// - `"follow_top"` / `"follow_base"` — constant-`dz_m` drape parallel to the top
///   (truncation/erosional) or base (onlap); `nk` is dz-derived at the build seam
///   (capped at `MAX_NK=200`), deep/shallow layers truncate at pinch-outs. `dz_m`
///   is required; `n` is ignored.
#[pyfunction]
#[pyo3(signature = (n=None, dz_m=None, style="proportional"))]
pub fn layers(n: Option<usize>, dz_m: Option<f64>, style: &str) -> PyResult<Layers> {
    let conformity = match style.to_ascii_lowercase().as_str() {
        "proportional" | "prop" => Conformity::Proportional,
        "follow_top" | "followtop" | "erosional" | "truncation" => {
            let dz = dz_m.ok_or_else(|| {
                PyValueError::new_err("ps.layers(style=\"follow_top\") needs dz_m=")
            })?;
            Conformity::FollowTop { dz_m: dz }
        }
        "follow_base" | "followbase" | "onlap" => {
            let dz = dz_m.ok_or_else(|| {
                PyValueError::new_err("ps.layers(style=\"follow_base\") needs dz_m=")
            })?;
            Conformity::FollowBase { dz_m: dz }
        }
        other => {
            return Err(PyValueError::new_err(format!(
            "unknown layering style '{other}' (expected proportional / follow_top / follow_base)"
        )))
        }
    };
    // The allocation the framework derives its nk from. Proportional takes exactly
    // one of n=/dz_m=; a Follow style is dz-driven (n= is not meaningful there).
    let spec = match &conformity {
        Conformity::Proportional => match (n, dz_m) {
            (Some(n), None) => LayersSpec::Count(n),
            (None, Some(dz)) => LayersSpec::Thickness(dz),
            (Some(_), Some(_)) => {
                return Err(PyValueError::new_err(
                    "ps.layers (proportional) needs exactly one of n= or dz_m=",
                ))
            }
            (None, None) => return Err(PyValueError::new_err("ps.layers needs n= or dz_m=")),
        },
        Conformity::FollowTop { dz_m } | Conformity::FollowBase { dz_m } => {
            LayersSpec::Thickness(*dz_m)
        }
    };
    Ok(Layers { spec, conformity })
}
