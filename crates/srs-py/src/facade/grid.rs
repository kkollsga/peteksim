//! The `Grid` + `Property` pyclasses — property-cube configuration and the model build.

use super::*;

/// A frozen framework accumulating property pipelines.
#[pyclass(name = "Grid")]
pub struct Grid {
    pub(crate) inner: Option<StaticGrid>,
}

impl Grid {
    fn get_mut(&mut self) -> PyResult<&mut StaticGrid> {
        self.inner
            .as_mut()
            .ok_or_else(|| PyValueError::new_err("grid already consumed by model()"))
    }
}

#[pymethods]
impl Grid {
    /// Configure a property cube by name (canonical PORO/NTG/SW drive
    /// volumetrics; others are auxiliary cubes). `PHIE` (the vendor effective-
    /// porosity mnemonic) is aliased to the canonical `PORO` so `property("PHIE")`
    /// populates the cube the volumetrics + MC read, rather than a silent
    /// non-canonical cube that would leave porosity at its prior.
    ///
    /// `zone=Some(name)` scopes the pipeline to a single zone of a multi-zone stack
    /// (petekStatic's `with_zone_property` — a per-zone distribution/variogram over
    /// that zone's k-range); omit it for the whole-model cube.
    #[pyo3(signature = (name, zone=None))]
    fn property(slf: Bound<'_, Self>, name: String, zone: Option<String>) -> Property {
        Property {
            grid: slf.unbind(),
            name: canonical_property_name(&name),
            zone,
        }
    }

    /// The property cubes configured so far.
    fn property_names(&self) -> PyResult<Vec<String>> {
        Ok(self
            .inner
            .as_ref()
            .ok_or_else(|| PyValueError::new_err("grid already consumed"))?
            .property_names())
    }

    /// Build the populated model with the given contacts + fluid/FVF.
    ///
    /// `contacts` is a dict with a lower `fwl` (or `owc`) and optional `goc`; each
    /// value is a float depth or a `TopsPick`. On a **zoned** grid (`set_zonation`)
    /// the per-zone contacts come from the zonation, so `contacts` is optional and
    /// ignored here. Pass `wells=proj.wells()` to attach the bores' positioned
    /// trajectories to the model so the viewer can draw well markers and cut
    /// along-bore sections (`intersection_bundle(well=..)`).
    ///
    /// `memory_budget_bytes` (opt-in) forwards to petekStatic's `MemoryBudget`: when
    /// the built model's live-set estimate exceeds it the engine switches to its
    /// out-of-core (spilled) backing store and emits a loud mode-switch advisory on
    /// stderr (never a silent switch, never an OOM kill). `None` (default) keeps
    /// petekStatic's own default budget (a fraction of physical RAM).
    ///
    /// `sugar_cube` (opt-in, default `False`) selects petekStatic's **sugar-cube**
    /// section rendering: `intersection_bundle` cells render as flat boxes (each
    /// layer's left/right edge depths collapse to the centroid) instead of the
    /// default dip-following trapezoids. It only affects the section view (and is
    /// carried onto the MC templates so realizations render identically).
    #[pyo3(signature = (contacts=None, fluid="oil", fvf=1.25, gas_fvf=None, wells=None, memory_budget_bytes=None, sugar_cube=false))]
    #[allow(clippy::too_many_arguments)]
    fn model(
        &mut self,
        py: Python<'_>,
        contacts: Option<&Bound<'_, PyDict>>,
        fluid: &str,
        fvf: f64,
        gas_fvf: Option<f64>,
        wells: Option<&Wells>,
        memory_budget_bytes: Option<u64>,
        sugar_cube: bool,
    ) -> PyResult<StaticModelPy> {
        let mut goc: Option<f64> = None;
        let mut lower: Option<f64> = None;
        if let Some(contacts) = contacts {
            for (k, v) in contacts.iter() {
                let key = k.extract::<String>()?.to_ascii_lowercase();
                let depth = contact_depth(&v)?;
                match key.as_str() {
                    "goc" => goc = Some(depth),
                    "fwl" | "owc" | "contact" => lower = Some(depth),
                    other => {
                        return Err(PyValueError::new_err(format!(
                            "unknown contact '{other}' (expected fwl/owc or goc)"
                        )))
                    }
                }
            }
        }
        let fl = Fluid::parse(fluid).map_err(err)?;
        let grid = self
            .inner
            .take()
            .ok_or_else(|| PyValueError::new_err("grid already consumed"))?;
        // Capture well tracks (id + positioned [x, y, tvd] path) BEFORE releasing
        // the GIL — reading them needs the Python-held project.
        let mut well_tracks = match wells {
            Some(w) => w.tracks(py),
            None => Vec::new(),
        };
        let stacked = grid.is_stacked();
        // Attach the well-hover surface-tie residuals to each bore's payload (the
        // viewer shows them on well hover / in the layer panel).
        //
        // WIREFRAME path: the FACADE tie residuals (sampled off the facade-gridded
        // surface via `StaticGrid::well_ties_for`, which owns the bore↔parent-well
        // matching). A STACK build skips the facade gridding entirely
        // (`task_suite_scatter_perf`), so `well_ties_for` is empty there — the
        // residuals instead come from the ENGINE `with_well_ties` provenance, attached
        // AFTER the build below (the same residuals the Wells-tab shows, and the ones
        // measured against the surface the stack model actually built).
        if !stacked {
            for wt in &mut well_tracks {
                wt.ties.extend(grid.well_ties_for(&wt.id).into_iter().map(
                    |(horizon, residual_m)| viewer::WellTie {
                        horizon,
                        residual_m,
                    },
                ));
            }
        }
        // S1: building the populated model is pure-Rust compute — release the GIL.
        // `lower` may be None on a zoned grid (per-zone contacts govern); the core
        // requires it only on the wireframe path.
        let model = py
            .detach(|| {
                grid.model(
                    goc,
                    lower,
                    fl,
                    fvf,
                    gas_fvf,
                    memory_budget_bytes,
                    sugar_cube,
                )
            })
            .map_err(err)?;
        // Stack build: route the well-hover ties to the engine tie provenance (the
        // facade grid was skipped). Bore matches its own id or its parent well id.
        if stacked {
            let ties = model.well_tie_residuals();
            for wt in &mut well_tracks {
                let parent = wt.id.split(' ').next().unwrap_or("");
                wt.ties.extend(
                    ties.iter()
                        .filter(|t| t.well_id == wt.id || t.well_id == parent)
                        .map(|t| viewer::WellTie {
                            horizon: t.horizon.clone(),
                            residual_m: t.residual_m,
                        }),
                );
            }
        }
        // Assemble the Wells-tab bundle from the attached bores + the built model
        // (raw + upscaled curves, framework tops/zones, tie residuals). Needs the
        // Python-held project (BoreWell curves), so it runs here under the GIL.
        let wells_logs = wells.and_then(|w| w.log_bundle(py, &model));
        Ok(StaticModelPy {
            inner: model,
            wells: well_tracks,
            wells_logs,
        })
    }
}

pub(crate) fn contact_depth(v: &Bound<'_, PyAny>) -> PyResult<f64> {
    if let Ok(p) = v.extract::<TopsPick>() {
        return Ok(p.level_m);
    }
    v.extract::<f64>()
        .map_err(|_| PyValueError::new_err("a contact must be a float depth (m) or a TopsPick"))
}
/// A property pipeline handle (`grid.property("PORO")`).
#[pyclass(name = "Property")]
pub struct Property {
    grid: Py<Grid>,
    name: String,
    /// `Some(zone)` scopes this pipeline to one zone of a multi-zone stack
    /// (`with_zone_property`); `None` is the whole-model cube.
    zone: Option<String>,
}

#[pymethods]
impl Property {
    /// Upscale the property's log from the wells into the cells (the visible,
    /// QC-able step). Returns the count of positioned samples found.
    ///
    /// `net_only=True` masks the conditioning samples to **net rock** before
    /// upscaling: only samples where the well's own `NTG` curve (a net-to-gross /
    /// pay flag) exceeds `net_cutoff` (default 0.5) are kept, from the positioned
    /// NTG curve where present. So a conditioned cube reflects net rock rather than
    /// the full log (e.g. a conditioned SW ~ net-rock Sw, not diluted by an aquifer
    /// interval). Facade-side sample filtering only — no engine change.
    #[pyo3(signature = (wells, method="arithmetic", net_only=false, net_cutoff=0.5))]
    fn upscale(
        &self,
        py: Python<'_>,
        wells: &Wells,
        method: &str,
        net_only: bool,
        net_cutoff: f64,
    ) -> PyResult<usize> {
        let m = parse_upscale(method)?;
        let net = if net_only { Some(net_cutoff) } else { None };
        let proj = wells.project.bind(py).borrow();
        let ws = proj.inner.wells();
        let mut grid = self.grid.bind(py).borrow_mut();
        let g = grid.get_mut()?;
        let name = &self.name;
        let zone = self.zone.as_deref();
        // S1: log positioning + upscaling is pure Rust — release the GIL. The
        // borrowed inputs (`&ws`, `&mut StaticGrid`) hold no Python state.
        py.detach(|| g.set_upscale(name, &ws, m, net, zone))
            .map_err(err)
    }

    /// The upscale QC digest (before/after levels, conditioned cell count). For a
    /// zone-scoped pipe (`grid.property(name, zone=..)`) this reports that zone's
    /// own conditioning; the payload adds a `zone` key so the digest is
    /// self-describing (`None` for a whole-model pipe).
    fn qc(&self, py: Python<'_>) -> PyResult<Py<PyDict>> {
        let grid = self.grid.bind(py).borrow();
        let g = grid
            .inner
            .as_ref()
            .ok_or_else(|| PyValueError::new_err("grid already consumed"))?;
        let qc = g
            .upscale_qc(&self.name, self.zone.as_deref())
            .map_err(err)?;
        let d = PyDict::new(py);
        d.set_item("property", &qc.property)?;
        d.set_item("zone", self.zone.as_deref())?;
        d.set_item("conditioned_cells", qc.conditioned_cells)?;
        d.set_item("log_samples", qc.log_samples)?;
        d.set_item("log_mean", qc.log_mean)?;
        d.set_item("upscaled_mean", qc.upscaled_mean)?;
        d.set_item("upscaled_min", qc.upscaled_min)?;
        d.set_item("upscaled_max", qc.upscaled_max)?;
        Ok(d.unbind())
    }

    /// Propagate the cube by SGS (conditioned on the upscaled cells), optionally
    /// steered by a collocated trend, and choose the MC behaviour.
    ///
    /// `allow_mean_fill=False` (the default) makes a simulated layer that carries
    /// **no conditioning data** a loud, named error at build/propagate time — the
    /// petekStatic default names the property (and, on the zone-scoped path, the
    /// zone). Pass `allow_mean_fill=True` to instead fill such a data-less layer
    /// with the conditioned mean — a **structureless** flat fill (any collocated
    /// trend is lost for those layers), acceptable only where a data-less layer is
    /// expected.
    #[pyo3(signature = (gaussian, trend=None, resimulate=false, allow_mean_fill=false))]
    fn propagate(
        &self,
        py: Python<'_>,
        gaussian: &GaussianSpec,
        trend: Option<&Trend>,
        resimulate: bool,
        allow_mean_fill: bool,
    ) -> PyResult<()> {
        // Pull the GIL-independent inputs out before releasing the GIL.
        let variogram = *gaussian.vgm.spec.variogram();
        let seed = gaussian.seed;
        let search = gaussian.search;
        let trend_spec = trend.map(|t| t.spec.clone());
        let name = &self.name;
        let zone = self.zone.as_deref();
        let mut grid = self.grid.bind(py).borrow_mut();
        let g = grid.get_mut()?;
        // S1: recording the propagation spec is cheap but pure Rust — keep the
        // whole property-configuration path off the GIL for consistency.
        py.detach(|| {
            g.set_propagate(
                name,
                variogram,
                seed,
                search,
                trend_spec.as_ref(),
                resimulate,
                allow_mean_fill,
                zone,
            )
        })
        .map_err(err)
    }
}

/// Canonicalise a facade property name. The raw name is first resolved through
/// petekio's **shared** mnemonic-alias table (`petekio::canonical_mnemonic`, the
/// family's single home — it folds `PHIE`/`PHI`/`EFFPHI`/… and strips vendor
/// vintage tags like `_2025`), then the canonical effective-porosity mnemonic
/// `PHIE` is renamed to the `PORO` cube the volumetrics + MC read. That rename is
/// a facade-local vocabulary mapping (cube name, not a mnemonic alias), so it stays
/// here; every other canonical mnemonic passes through unchanged.
fn canonical_property_name(name: &str) -> String {
    match srs_core::canonical_mnemonic(name).as_str() {
        "PHIE" => "PORO".to_string(),
        other => other.to_string(),
    }
}

fn parse_upscale(method: &str) -> PyResult<UpscaleMethod> {
    match method.to_ascii_lowercase().as_str() {
        "arithmetic" => Ok(UpscaleMethod::Arithmetic),
        "harmonic" => Ok(UpscaleMethod::Harmonic),
        "geometric" => Ok(UpscaleMethod::Geometric),
        other => Err(PyValueError::new_err(format!(
            "upscale method must be arithmetic/harmonic/geometric, got '{other}'"
        ))),
    }
}
