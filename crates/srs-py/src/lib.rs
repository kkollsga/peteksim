//! Thin PyO3 bindings over the `peteksim` crate — the published `peteksim` Python
//! module. Code-first: build a model in Python, then `model.view()` / `save_view`
//! open the bundle-driven viewer (map + intersection + volume). The renderer is
//! petekTools' horizontal `petektools.viewer` unit (owner ruling); the thin
//! `peteksim._serve` / `peteksim._save_view` glue hands it the payload + a live
//! `/section` callback. The view payload (map + volume bundles + summary) is
//! composed in [`viewer`]. Logic stays in the core crates; this file only marshals
//! across the boundary.
//!
//! **Units (SI standard, `decision_si_units_standard`):** the Python surface is
//! metric — area in **km²** (reservoir-scale-natural), lengths/depths in **m**
//! (positive down), results in **Sm³** with mcm/MSm³/bcm reporting conveniences.
//! Imperial is opt-in conversion on the caller's side, never a default.

mod facade;
mod viewer;

use peteksim::{
    box_static_model, km2_to_m2, run_model, scale_area_km2_to_m2, ConstantPriors, Dims,
    Distribution, Fluid, ModelInputs, RefiningModel, StaticModel, SM3_PER_BCM, SM3_PER_MSM3,
};
use pyo3::exceptions::{PyIOError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyTuple};
use std::collections::HashMap;
use std::fs::File;

// --- viewer payload glue -----------------------------------------------------

/// The box/refined headline summary as JSON (deterministic in-place + GRV, plus
/// the P-curve for a Monte Carlo run). Display metadata carried on the payload.
#[allow(clippy::too_many_arguments)]
fn summary_value(
    fluid: &str,
    deterministic_in_place: f64,
    grv_mcm: f64,
    p90: Option<f64>,
    p50: Option<f64>,
    p10: Option<f64>,
    mean: Option<f64>,
) -> serde_json::Value {
    serde_json::json!({
        "fluid": fluid,
        "deterministic_in_place": deterministic_in_place,
        "grv_mcm": grv_mcm,
        "p90": p90, "p50": p50, "p10": p10, "mean": mean,
    })
}

/// Write a viewer payload JSON string to `path` (the `save_json` model.json).
fn write_payload(path: &str, json: &str) -> PyResult<()> {
    use std::io::Write;
    let mut f = File::create(path).map_err(|e| PyIOError::new_err(e.to_string()))?;
    f.write_all(json.as_bytes())
        .map_err(|e| PyIOError::new_err(e.to_string()))
}

// --- input parsing -----------------------------------------------------------

/// Parse a Python volumetric input into a sampler [`Distribution`]. Accepts:
///
/// * a **number** → `Constant` (deterministic);
/// * a **`(min, mode, max)` tuple** → `Triangular` (back-compat shorthand);
/// * a **tagged dict** routing to the matching distribution (W15):
///   * `{"normal": [mean, sd]}`
///   * `{"lognormal": [mu, sigma]}` (log-space parameters)
///   * `{"uniform": [lo, hi]}`
///   * `{"triangular": [lo, mode, hi]}`
///
/// An unknown tag, wrong parameter count, or otherwise-shaped object raises a
/// clear `ValueError`.
fn to_dist(obj: &Bound<'_, PyAny>) -> PyResult<Distribution> {
    if let Ok(v) = obj.extract::<f64>() {
        return Ok(Distribution::Constant(v));
    }
    // A tagged distribution dict, e.g. {"normal": [mean, sd]}.
    if let Ok(d) = obj.cast::<PyDict>() {
        return dist_from_dict(d);
    }
    if let Ok(t) = obj.cast::<PyTuple>() {
        if t.len() == 3 {
            let min = t.get_item(0)?.extract::<f64>()?;
            let mode = t.get_item(1)?.extract::<f64>()?;
            let max = t.get_item(2)?.extract::<f64>()?;
            // Distribution now lives in petekStatic's srs-uncertainty → StaticError;
            // compose into SrsError (#[from]) before mapping to the Python error.
            return Distribution::triangular(min, mode, max).map_err(|e| map_err(e.into()));
        }
    }
    Err(PyValueError::new_err(
        "expected a number (constant); a (min, mode, max) triangular tuple; or a \
         tagged dict {\"normal\":[mean,sd]}, {\"lognormal\":[mu,sigma]}, \
         {\"uniform\":[lo,hi]}, or {\"triangular\":[lo,mode,hi]}",
    ))
}

/// Route a single-tag distribution dict to the matching srs-uncertainty
/// constructor. The tag is the distribution name; the value is its parameter
/// list (2 for normal/lognormal/uniform, 3 for triangular).
fn dist_from_dict(d: &Bound<'_, PyDict>) -> PyResult<Distribution> {
    if d.len() != 1 {
        return Err(PyValueError::new_err(
            "a distribution dict must have exactly one tag, e.g. {\"normal\": [mean, sd]}",
        ));
    }
    let (key, val) = d.iter().next().expect("len checked == 1");
    let tag: String = key.extract().map_err(|_| {
        PyValueError::new_err("a distribution dict tag must be a string, e.g. \"normal\"")
    })?;
    let p: Vec<f64> = val.extract().map_err(|_| {
        PyValueError::new_err(format!(
            "distribution '{tag}' expects a list of numbers, e.g. {{\"{tag}\": [a, b]}}"
        ))
    })?;
    // `?` on the arity check early-returns a clear ValueError; each constructor
    // maps its StaticError through SrsError (#[from]) to the Python error.
    match tag.to_ascii_lowercase().as_str() {
        "normal" => {
            dist_arity(&tag, &p, 2)?;
            Distribution::normal(p[0], p[1]).map_err(|e| map_err(e.into()))
        }
        "lognormal" => {
            dist_arity(&tag, &p, 2)?;
            Distribution::lognormal(p[0], p[1]).map_err(|e| map_err(e.into()))
        }
        "uniform" => {
            dist_arity(&tag, &p, 2)?;
            Distribution::uniform(p[0], p[1]).map_err(|e| map_err(e.into()))
        }
        "triangular" => {
            dist_arity(&tag, &p, 3)?;
            Distribution::triangular(p[0], p[1], p[2]).map_err(|e| map_err(e.into()))
        }
        other => Err(PyValueError::new_err(format!(
            "unknown distribution tag '{other}'; expected one of: \
             normal, lognormal, uniform, triangular"
        ))),
    }
}

/// Enforce a distribution's parameter count, with a message naming the tag.
fn dist_arity(tag: &str, params: &[f64], n: usize) -> PyResult<()> {
    if params.len() == n {
        Ok(())
    } else {
        Err(PyValueError::new_err(format!(
            "distribution '{tag}' expects {n} parameters, got {}",
            params.len()
        )))
    }
}

fn parse_fluid(fluid: &str) -> PyResult<Fluid> {
    match fluid.to_ascii_lowercase().as_str() {
        "oil" => Ok(Fluid::Oil),
        "gas" => Ok(Fluid::Gas),
        other => Err(PyValueError::new_err(format!(
            "fluid must be 'oil' or 'gas', got '{other}'"
        ))),
    }
}

fn map_err(e: peteksim::SrsError) -> PyErr {
    PyValueError::new_err(e.to_string())
}

// --- box Monte Carlo result --------------------------------------------------

/// The result of `run_box_model`: a deterministic + Monte Carlo summary plus the
/// render mesh of the deterministic grid. All volumes are SI: in-place values in
/// **Sm³**, GRV in **mcm** (10⁶ m³).
#[pyclass(name = "ModelResult", frozen)]
struct PyModelResult {
    /// Deterministic in-place \[Sm³\].
    #[pyo3(get)]
    deterministic_in_place: f64,
    /// Deterministic GRV \[mcm\].
    #[pyo3(get)]
    grv_mcm: f64,
    #[pyo3(get)]
    p90: f64,
    #[pyo3(get)]
    p50: f64,
    #[pyo3(get)]
    p10: f64,
    #[pyo3(get)]
    mean: f64,
    #[pyo3(get)]
    realizations: usize,
    #[pyo3(get)]
    fluid: String,
    /// The per-realization in-place values (draw order); exposed via `samples`.
    samples: Vec<f64>,
    /// The deterministic (P50) static model behind the view bundles.
    model: StaticModel,
}

#[pymethods]
impl PyModelResult {
    /// The per-realization in-place values \[Sm³\], in draw order — the full
    /// Monte Carlo sample behind the `p90`/`p50`/`p10`/`mean` summary (length ==
    /// `realizations`). Use it for compositional workflows (EUR bridging, custom
    /// percentiles) without re-running or reimplementing the in-place
    /// computation. The summary fields are unchanged.
    #[getter]
    fn samples(&self) -> Vec<f64> {
        self.samples.clone()
    }

    /// The percentile summary in **MSm³** (10⁶ Sm³) — the oil reporting scale:
    /// `{"p90": .., "p50": .., "p10": .., "mean": ..}`. For gas prefer
    /// `summary_bcm`.
    #[getter]
    fn summary_msm3(&self) -> HashMap<String, f64> {
        self.scaled_summary(SM3_PER_MSM3)
    }

    /// The percentile summary in **bcm** (10⁹ Sm³) — the gas reporting scale:
    /// `{"p90": .., "p50": .., "p10": .., "mean": ..}`. For oil prefer
    /// `summary_msm3`.
    #[getter]
    fn summary_bcm(&self) -> HashMap<String, f64> {
        self.scaled_summary(SM3_PER_BCM)
    }

    fn __repr__(&self) -> String {
        format!(
            "ModelResult(fluid='{}', P90={:.3e}, P50={:.3e}, P10={:.3e}, mean={:.3e}, det={:.3e} Sm3)",
            self.fluid, self.p90, self.p50, self.p10, self.mean, self.deterministic_in_place
        )
    }

    /// Write the viewer payload (`model.json`: map + volume + summary) to `path`.
    #[pyo3(signature = (path, property=None))]
    fn save_json(&self, path: &str, property: Option<&str>) -> PyResult<()> {
        write_payload(path, &self.payload(property)?)
    }

    /// Open the viewer in a browser. Non-blocking by default (a background server
    /// prints its URL and returns); pass `block=True` for the old blocking mode.
    #[pyo3(signature = (open_browser=true, port=0, block=false, property=None))]
    fn view(
        slf: Bound<'_, Self>,
        py: Python<'_>,
        open_browser: bool,
        port: u16,
        block: bool,
        property: Option<&str>,
    ) -> PyResult<String> {
        let json = slf.borrow().payload(property)?;
        viewer::serve(py, slf.as_any(), json, open_browser, port, block)
    }

    /// Write ONE self-contained HTML file (all JS + data inlined; opens via
    /// `file://`). Sections are pre-computed; live fence-draw is server-only.
    #[pyo3(signature = (path, property=None))]
    fn save_view(&self, py: Python<'_>, path: &str, property: Option<&str>) -> PyResult<()> {
        viewer::save_view(py, path, self.payload(property)?)
    }

    /// Live-section endpoint the local server calls for a drawn fence (box path
    /// has no wells, so `well=` is unsupported here).
    #[pyo3(signature = (property=None, line=None, well=None))]
    fn _section_json(
        &self,
        property: Option<&str>,
        line: Option<Vec<[f64; 2]>>,
        well: Option<String>,
    ) -> PyResult<String> {
        Ok(viewer::section_json(
            &self.model,
            property,
            line,
            well,
            &[],
        )?)
    }
}

impl PyModelResult {
    fn scaled_summary(&self, per: f64) -> HashMap<String, f64> {
        HashMap::from([
            ("p90".to_string(), self.p90 / per),
            ("p50".to_string(), self.p50 / per),
            ("p10".to_string(), self.p10 / per),
            ("mean".to_string(), self.mean / per),
        ])
    }

    fn payload(&self, property: Option<&str>) -> PyResult<String> {
        let summary = summary_value(
            &self.fluid,
            self.deterministic_in_place,
            self.grv_mcm,
            Some(self.p90),
            Some(self.p50),
            Some(self.p10),
            Some(self.mean),
        );
        Ok(viewer::payload_json(
            &self.model,
            "box",
            property,
            &[],
            &[],
            false,
            Some(summary),
            Vec::new(),
            None,
        )?)
    }
}

/// Run a box appraisal model end-to-end: area + height (+ properties) ->
/// deterministic in-place and a Monte Carlo P90/P50/P10, plus a render mesh.
///
/// **Units:** `area_km2` in km² (reservoir-scale-natural; converted to m²
/// internally), heights/depths in m (positive down), FVF in Rm³/Sm³. Results
/// are Sm³ (`p90/p50/p10/mean/deterministic_in_place/samples`), GRV in mcm;
/// `summary_msm3` / `summary_bcm` give the MSm³ / bcm reporting scales.
///
/// Each volumetric input accepts:
///   * a number → deterministic constant;
///   * a `(min, mode, max)` tuple → triangular (shorthand);
///   * a tagged dict → the matching distribution:
///     `{"normal": [mean, sd]}`, `{"lognormal": [mu, sigma]}`,
///     `{"uniform": [lo, hi]}`, `{"triangular": [lo, mode, hi]}`.
///   (Area distributions are parameterised in km²; lognormal `mu`/`sigma` are
///   log-space km² parameters.)
/// An unknown tag or wrong parameter count raises `ValueError`.
///
/// `fluid` is `"oil"` or `"gas"` (both report Sm³). `contact_m` is **required**
/// (a finite depth in metres) — it defines the base of the hydrocarbon column.
///
/// The result's `samples` attribute holds every per-realization in-place value
/// (draw order) behind the summary, for compositional workflows.
#[pyfunction]
#[pyo3(signature = (
    area_km2, gross_height_m, porosity, net_to_gross, water_saturation, fvf,
    *, fluid="oil", top_m=0.0, contact_m=f64::INFINITY,
    ni=10, nj=10, nk=5, realizations=10_000, seed=1
))]
#[allow(clippy::too_many_arguments)]
fn run_box_model(
    py: Python<'_>,
    area_km2: &Bound<'_, PyAny>,
    gross_height_m: &Bound<'_, PyAny>,
    porosity: &Bound<'_, PyAny>,
    net_to_gross: &Bound<'_, PyAny>,
    water_saturation: &Bound<'_, PyAny>,
    fvf: &Bound<'_, PyAny>,
    fluid: &str,
    top_m: f64,
    contact_m: f64,
    ni: usize,
    nj: usize,
    nk: usize,
    realizations: usize,
    seed: u64,
) -> PyResult<PyModelResult> {
    let fluid_enum = parse_fluid(fluid)?;
    // W16: fail fast on a non-finite contact (including the missing INFINITY
    // default) — a silent `contact_m=∞` can't define a hydrocarbon column
    // and previously slipped through to produce a whole-box volume. Mirror
    // `Model.__new__`'s guard and message: the contact is required, not optional.
    if !contact_m.is_finite() {
        return Err(PyValueError::new_err(
            "contact_m must be a finite depth in metres \
             (pass e.g. contact_m=2743); it is required, not optional",
        ));
    }
    // Dims::new now comes from petekStatic's srs-grid → StaticError; compose it
    // into SrsError (via #[from]) before mapping to the Python error.
    let dims = Dims::new(ni, nj, nk).map_err(|e| map_err(e.into()))?;
    let inputs = ModelInputs {
        // The user speaks km²; the core is m². Scaling the *distribution* keeps
        // the km² parameters natural and is exact for every family (see
        // `scale_area_km2_to_m2`).
        area_m2: scale_area_km2_to_m2(to_dist(area_km2)?),
        gross_height_m: to_dist(gross_height_m)?,
        porosity: to_dist(porosity)?,
        net_to_gross: to_dist(net_to_gross)?,
        water_saturation: to_dist(water_saturation)?,
        fvf: to_dist(fvf)?,
        fluid: fluid_enum,
        dims,
        top_depth_m: top_m,
        contact_depth_m: contact_m,
        aspect_ratio: 1.0,
    };
    // H1/H2: run_model now rejects zero realizations and non-physical inputs with
    // a typed error; surface it as a clean ValueError instead of a panic / garbage.
    // S1: the Monte Carlo is pure-Rust compute — release the GIL while it runs.
    // S1: the Monte Carlo + the deterministic (P50) static-model build are both
    // pure-Rust compute — release the GIL while they run.
    let (r, model) = py
        .detach(|| {
            let r = run_model(&inputs, realizations, seed)?;
            let model = box_static_model(&inputs)?;
            Ok::<_, peteksim::SrsError>((r, model))
        })
        .map_err(map_err)?;
    Ok(PyModelResult {
        deterministic_in_place: r.deterministic_in_place,
        grv_mcm: r.deterministic_grv_mcm,
        p90: r.summary.p90,
        p50: r.summary.p50,
        p10: r.summary.p10,
        mean: r.summary.mean,
        realizations: r.realizations,
        fluid: fluid.to_string(),
        samples: r.samples,
        model,
    })
}

// --- structured model builder (the refine loop) ------------------------------

/// A solved structured model: deterministic volumes + render mesh. In-place in
/// Sm³, GRV in mcm.
#[pyclass(name = "Refined", frozen)]
struct PyRefined {
    /// In-place \[Sm³\].
    #[pyo3(get)]
    in_place: f64,
    /// GRV \[mcm\].
    #[pyo3(get)]
    grv_mcm: f64,
    #[pyo3(get)]
    controls: usize,
    #[pyo3(get)]
    fluid: String,
    /// The converged static model behind the view bundles.
    model: StaticModel,
}

#[pymethods]
impl PyRefined {
    fn __repr__(&self) -> String {
        format!(
            "Refined(fluid='{}', in_place={:.3e} Sm3, grv_mcm={:.3}, controls={})",
            self.fluid, self.in_place, self.grv_mcm, self.controls
        )
    }

    /// Write the viewer payload (`model.json`: map + volume + summary) to `path`.
    #[pyo3(signature = (path, property=None))]
    fn save_json(&self, path: &str, property: Option<&str>) -> PyResult<()> {
        write_payload(path, &self.payload(property)?)
    }

    /// Open the viewer in a browser. Non-blocking by default (background server,
    /// prints its URL, returns); `block=True` for the old blocking mode.
    #[pyo3(signature = (open_browser=true, port=0, block=false, property=None))]
    fn view(
        slf: Bound<'_, Self>,
        py: Python<'_>,
        open_browser: bool,
        port: u16,
        block: bool,
        property: Option<&str>,
    ) -> PyResult<String> {
        let json = slf.borrow().payload(property)?;
        viewer::serve(py, slf.as_any(), json, open_browser, port, block)
    }

    /// Write ONE self-contained HTML file (all JS + data inlined; opens via
    /// `file://`).
    #[pyo3(signature = (path, property=None))]
    fn save_view(&self, py: Python<'_>, path: &str, property: Option<&str>) -> PyResult<()> {
        viewer::save_view(py, path, self.payload(property)?)
    }

    /// Live-section endpoint the local server calls for a drawn fence.
    #[pyo3(signature = (property=None, line=None, well=None))]
    fn _section_json(
        &self,
        property: Option<&str>,
        line: Option<Vec<[f64; 2]>>,
        well: Option<String>,
    ) -> PyResult<String> {
        Ok(viewer::section_json(
            &self.model,
            property,
            line,
            well,
            &[],
        )?)
    }
}

impl PyRefined {
    fn payload(&self, property: Option<&str>) -> PyResult<String> {
        let summary = summary_value(
            &self.fluid,
            self.in_place,
            self.grv_mcm,
            None,
            None,
            None,
            None,
        );
        Ok(viewer::payload_json(
            &self.model,
            "refined",
            property,
            &[],
            &[],
            false,
            Some(summary),
            Vec::new(),
            None,
        )?)
    }
}

/// A structural model you build in code: start from a flat box, add top-surface
/// depth control points, then `solve()` to converge a structured grid and its
/// volumes (the model-first refinement loop). Metric surface: `area_km2` in
/// km², heights/depths in m (positive down), results in Sm³ / mcm.
#[pyclass(name = "Model")]
struct PyModel {
    inner: RefiningModel,
    fluid: String,
    // Retained for `__repr__` (RefiningModel does not expose these back out).
    ni: usize,
    nj: usize,
    nk: usize,
    top_m: f64,
    contact_m: f64,
    controls: usize,
}

#[pymethods]
impl PyModel {
    #[new]
    #[pyo3(signature = (
        area_km2, gross_height_m, *, ni=20, nj=20, nk=8,
        top_m=1500.0, contact_m=f64::INFINITY,
        porosity=0.25, net_to_gross=0.8, water_saturation=0.3, fvf=1.25, fluid="oil"
    ))]
    #[allow(clippy::too_many_arguments)]
    fn new(
        area_km2: f64,
        gross_height_m: f64,
        ni: usize,
        nj: usize,
        nk: usize,
        top_m: f64,
        contact_m: f64,
        porosity: f64,
        net_to_gross: f64,
        water_saturation: f64,
        fvf: f64,
        fluid: &str,
    ) -> PyResult<Self> {
        let fluid_enum = parse_fluid(fluid)?;
        // Fail fast at construction: a non-finite contact (including the missing
        // default) can't define a hydrocarbon column, and `solve()` would only
        // reject it later. Require an explicit, finite contact depth up front.
        if !contact_m.is_finite() {
            return Err(PyValueError::new_err(
                "contact_m must be a finite depth in metres \
                 (pass e.g. contact_m=2743); it is required, not optional",
            ));
        }
        let inner = RefiningModel::new(
            km2_to_m2(area_km2),
            gross_height_m,
            ni,
            nj,
            nk,
            top_m,
            contact_m,
            ConstantPriors {
                porosity,
                net_to_gross,
                water_saturation,
            },
            fluid_enum,
            fvf,
        )
        .map_err(map_err)?;
        Ok(Self {
            inner,
            fluid: fluid.to_string(),
            ni,
            nj,
            nk,
            top_m,
            contact_m,
            // RefiningModel::new seeds the four corner controls.
            controls: 4,
        })
    }

    fn __repr__(&self) -> String {
        format!(
            "Model(fluid='{}', dims={}x{}x{}, top_m={:.0}, contact_m={:.0}, controls={})",
            self.fluid, self.ni, self.nj, self.nk, self.top_m, self.contact_m, self.controls
        )
    }

    /// Add a top-surface depth control point on the `(ni+1) x (nj+1)` node
    /// lattice — the new datum (m, positive down) the grid re-converges to honour.
    fn add_control(&mut self, ip: usize, jp: usize, depth_m: f64) {
        self.inner.add_top_control(ip, jp, depth_m);
        self.controls += 1;
    }

    /// Converge the grid at the current controls and return the result.
    fn solve(&self, py: Python<'_>) -> PyResult<PyRefined> {
        self.solve_refined(py)
    }

    /// Convenience: solve and write the viewer payload (`model.json`) to `path`.
    #[pyo3(signature = (path, property=None))]
    fn save_json(&self, py: Python<'_>, path: &str, property: Option<&str>) -> PyResult<()> {
        self.solve_refined(py)?.save_json(path, property)
    }

    /// Solve and open the viewer in a browser. Non-blocking by default;
    /// `block=True` for the old blocking mode.
    #[pyo3(signature = (open_browser=true, port=0, block=false, property=None))]
    fn view(
        &self,
        py: Python<'_>,
        open_browser: bool,
        port: u16,
        block: bool,
        property: Option<&str>,
    ) -> PyResult<String> {
        let refined = Bound::new(py, self.solve_refined(py)?)?;
        let json = refined.borrow().payload(property)?;
        viewer::serve(py, refined.as_any(), json, open_browser, port, block)
    }

    /// Solve and write ONE self-contained HTML file (all JS + data inlined).
    #[pyo3(signature = (path, property=None))]
    fn save_view(&self, py: Python<'_>, path: &str, property: Option<&str>) -> PyResult<()> {
        self.solve_refined(py)?.save_view(py, path, property)
    }
}

impl PyModel {
    /// Converge the grid off the GIL (the grid re-convergence is pure-Rust
    /// compute, S1); the result carries the static model for the view bundles.
    fn solve_refined(&self, py: Python<'_>) -> PyResult<PyRefined> {
        let r = py.detach(|| self.inner.solve()).map_err(map_err)?;
        Ok(PyRefined {
            in_place: r.in_place,
            grv_mcm: r.grv_mcm,
            controls: r.controls,
            fluid: self.fluid.clone(),
            model: r.model,
        })
    }
}

/// Return the installed petekSim version.
#[pyfunction]
fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// petekStatic's in-core **live-set estimate** (bytes) for an `ni×nj×nk` grid with
/// `n_cubes` property cubes — the single home of the memory-budget formula
/// (`peteksim::live_set_bytes`: ZCORN + cubes, scaled by petekStatic's
/// `WARM_FACTOR`). Exposed so the Python spill-forcing recipe reads the formula
/// **across the seam** instead of re-deriving petekStatic's `WARM_FACTOR`/
/// `ELEM_BYTES` constants (which would drift silently if petekStatic changed them).
#[pyfunction]
fn live_set_bytes(ni: usize, nj: usize, nk: usize, n_cubes: usize) -> PyResult<u64> {
    let dims = Dims::new(ni, nj, nk).map_err(|e| PyValueError::new_err(e.to_string()))?;
    Ok(peteksim::live_set_bytes(dims, n_cubes))
}

#[pymodule]
fn _core(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(version, m)?)?;
    m.add_function(wrap_pyfunction!(run_box_model, m)?)?;
    m.add_function(wrap_pyfunction!(live_set_bytes, m)?)?;
    m.add_class::<PyModelResult>()?;
    m.add_class::<PyModel>()?;
    m.add_class::<PyRefined>()?;
    // The staged model-build facade (Project.load → … → model.uncertainty).
    facade::register(m)?;
    Ok(())
}
