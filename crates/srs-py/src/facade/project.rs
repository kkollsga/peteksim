//! The ingest front-door pyclasses — `Project`, `Inventory`, `Wells`, `Surface`, `Tops`, `TopsPick`.

use super::*;

/// A loaded Petrel-export project — the ingest front-door.
#[pyclass(name = "Project")]
pub struct Project {
    pub(crate) inner: CoreProject,
}

#[pymethods]
impl Project {
    /// Walk `path`, load every recognised file, and return the project. `crs` is
    /// a provenance label; `aliases` canonicalise log mnemonics at load.
    #[staticmethod]
    #[pyo3(signature = (path, crs=None, aliases=None))]
    fn load(
        path: &str,
        crs: Option<String>,
        aliases: Option<&Bound<'_, PyDict>>,
    ) -> PyResult<Project> {
        let alias_pairs = aliases
            .map(|d| {
                d.iter()
                    .map(|(k, v)| Ok((k.extract::<String>()?, v.extract::<String>()?)))
                    .collect::<PyResult<Vec<_>>>()
            })
            .transpose()?;
        Ok(Project {
            inner: CoreProject::load(path, crs, alias_pairs).map_err(err)?,
        })
    }

    /// The load inventory (loaded artifacts + skipped-with-reason).
    fn inventory(&self) -> Inventory {
        let inv = self.inner.inventory();
        Inventory {
            surfaces: inv.surfaces.clone(),
            polygons: inv.polygons.clone(),
            points: inv.points.clone(),
            wells: inv.wells.clone(),
            tops: inv.tops.clone(),
            merged: inv.merged.clone(),
            skipped: inv.skipped.clone(),
        }
    }

    /// A handle over the loaded wells (pass to `property.upscale`).
    fn wells(slf: Bound<'_, Self>) -> Wells {
        Wells {
            project: slf.unbind(),
        }
    }

    /// A named surface handle (pass to `ps.collocated`).
    #[pyo3(name = "surface")]
    fn surface(slf: Bound<'_, Self>, name: String) -> PyResult<Surface> {
        {
            let this = slf.borrow();
            if this.inner.surface(&name).is_none() {
                return Err(PyValueError::new_err(format!(
                    "surface '{name}' not loaded"
                )));
            }
        }
        Ok(Surface {
            project: slf.unbind(),
            name,
        })
    }

    /// The tops accessor (`proj.tops.pick("GOC")`).
    #[getter]
    fn tops(slf: Bound<'_, Self>) -> Tops {
        Tops {
            project: slf.unbind(),
        }
    }

    /// Declare the structural framework.
    ///
    /// `min_thickness_m` (opt-in) sets the post-gridding order-repair floor: raw
    /// point-horizon builds with thin crossing margins (the real case — 100–300
    /// crossing nodes per structure) build cleanly by pulling the base down to
    /// `top + min_thickness_m` at the offending columns (top preserved) and
    /// reporting a `thin_columns_repaired` warning on `model.warnings()`. Left
    /// unset, a crossed base is a loud build error (the crossing guard).
    ///
    /// `cell_size_m` sets the lattice resolution when a horizon is a scattered
    /// **point-set** (real seismic picks) that must be gridded: the build lattice
    /// is `ceil(extent / cell_size_m) + 1` nodes per axis (capped loudly at 2000
    /// cells/axis). Left unset, a fixed 100×100 lattice is used regardless of
    /// extent — too coarse for a regional grid, so pass e.g. `cell_size_m=100.0`
    /// for a several-hundred-node regional framework. Ignored when every horizon
    /// is a loaded grid surface.
    /// `collapse_below_m` (opt-in, multi-zone stack only) is the Petrel-style
    /// cell-collapse floor: sub-threshold cells merge volume-conservingly into a
    /// thicker zone-interior neighbour, reported as a `cells_collapsed` warning.
    #[pyo3(signature = (horizons, outline=None, tie_to_tops=true, gross_m=50.0, min_thickness_m=None, cell_size_m=None, collapse_below_m=None))]
    #[allow(clippy::too_many_arguments)]
    fn framework(
        slf: Bound<'_, Self>,
        horizons: Vec<String>,
        outline: Option<String>,
        tie_to_tops: bool,
        gross_m: f64,
        min_thickness_m: Option<f64>,
        cell_size_m: Option<f64>,
        collapse_below_m: Option<f64>,
    ) -> PyResult<Framework> {
        // `build` is now CHEAP — it resolves the stack sources + lattice without
        // gridding the wireframe (that is deferred; the held project re-grids it lazily
        // only for a wireframe consumer — `task_suite_scatter_perf`).
        let fw = {
            let this = slf.borrow();
            CoreFramework::build(
                &this.inner,
                &horizons,
                outline.as_deref(),
                tie_to_tops,
                gross_m,
                min_thickness_m,
                cell_size_m,
                collapse_below_m,
            )
            .map_err(err)?
        };
        Ok(Framework {
            inner: Some(fw),
            project: slf.unbind(),
        })
    }

    /// A crossplot (scatter) chart bundle from positioned well logs — e.g.
    /// `proj.crossplot_bundle(x="PHIE", y="PERM", wells=[...], color_by="well",
    /// y_log=True, regression=True)`. Samples are paired on the x-curve's MDs; the
    /// colour is a well/zone identity (categorical) or any log mnemonic (a
    /// continuous ramp). The optional regression is fit **here** (least squares, in
    /// the axes' log/linear space) and only the line + coefficients ship — the
    /// viewer never fits. Hand the returned dict to `model.view(charts=[...])` or
    /// `save_view`.
    #[pyo3(signature = (x, y, wells=None, color_by="well", x_log=false, y_log=false, regression=false))]
    #[allow(clippy::too_many_arguments)]
    fn crossplot_bundle(
        &self,
        py: Python<'_>,
        x: String,
        y: String,
        wells: Option<Vec<String>>,
        color_by: &str,
        x_log: bool,
        y_log: bool,
        regression: bool,
    ) -> PyResult<Py<PyAny>> {
        let opts = CrossplotOpts {
            x,
            y,
            wells,
            color_by: color_by.to_string(),
            x_log,
            y_log,
            regression,
        };
        let chart = core_crossplot(&self.inner, &opts).map_err(err)?;
        viewer::to_py(py, &chart)
    }
}

/// The load inventory.
#[pyclass(name = "Inventory")]
pub struct Inventory {
    #[pyo3(get)]
    surfaces: Vec<String>,
    #[pyo3(get)]
    polygons: Vec<String>,
    #[pyo3(get)]
    points: Vec<String>,
    #[pyo3(get)]
    wells: Vec<String>,
    #[pyo3(get)]
    tops: Vec<String>,
    /// `(filename, bore_id)` for each LAS 3.0 core file merged into a bore (F8).
    #[pyo3(get)]
    merged: Vec<(String, String)>,
    /// `(path, reason)` skips.
    #[pyo3(get)]
    skipped: Vec<(String, String)>,
}

#[pymethods]
impl Inventory {
    fn __repr__(&self) -> String {
        format!(
            "Inventory(surfaces={}, polygons={}, points={}, wells={}, tops={}, merged={}, skipped={})",
            self.surfaces.len(),
            self.polygons.len(),
            self.points.len(),
            self.wells.len(),
            self.tops.len(),
            self.merged.len(),
            self.skipped.len()
        )
    }
}

/// A handle over the project's wells.
#[pyclass(name = "Wells")]
pub struct Wells {
    pub(crate) project: Py<Project>,
}

#[pymethods]
impl Wells {
    fn __len__(&self, py: Python<'_>) -> usize {
        self.project.bind(py).borrow().inner.wells().len()
    }
    fn ids(&self, py: Python<'_>) -> Vec<String> {
        self.project
            .bind(py)
            .borrow()
            .inner
            .wells()
            .iter()
            .map(|w| w.id.clone())
            .collect()
    }

    /// The positioned bores' surface heads as `(id, x, y)` — the wellhead world
    /// coordinate (first trajectory station) per bore. Framework-independent (no
    /// model build needed), so a caller can seed a `set_well_ties` table with each
    /// bore's own head. Unpositioned bores are omitted (they carry no head).
    fn heads(&self, py: Python<'_>) -> Vec<(String, f64, f64)> {
        self.project
            .bind(py)
            .borrow()
            .inner
            .wells()
            .iter()
            .filter_map(|w| w.trajectory().first().map(|p| (w.id.clone(), p[0], p[1])))
            .collect()
    }
}

impl Wells {
    /// The bores as viewer well tracks (id + positioned `[x, y, tvd]` path).
    /// Unpositioned bores contribute an empty trajectory (marker-less).
    pub(crate) fn tracks(&self, py: Python<'_>) -> Vec<WellTrack> {
        self.project
            .bind(py)
            .borrow()
            .inner
            .wells()
            .iter()
            .map(|w| WellTrack::new(w.id.clone(), w.trajectory()))
            .collect()
    }

    /// The `wells_logs` bundle for the viewer's **Wells** tab — per-bore raw +
    /// upscaled log tracks, framework tops/zones, and tie residuals — assembled
    /// from these bores plus the just-built `model`. `None` when no bore carries a
    /// correlatable curve.
    pub(crate) fn log_bundle(&self, py: Python<'_>, model: &ProjectModel) -> Option<WellLogBundle> {
        let proj = self.project.bind(py).borrow();
        let bores = proj.inner.wells();
        build_well_log_bundle(model, &bores)
    }
}

/// A named surface handle.
#[pyclass(name = "Surface")]
pub struct Surface {
    pub(crate) project: Py<Project>,
    pub(crate) name: String,
}

#[pymethods]
impl Surface {
    /// The surface value at world `(x, y)` — nearest node on the surface's own
    /// lattice (`NaN` outside/undefined). The raw stored value: petekio's
    /// negative-down elevation for a depth surface, or the value itself for a
    /// value grid (e.g. a depositional-trend map). Uses the same node→world map
    /// `collocated` reads, so a trend sampled here matches what steers a
    /// collocated population.
    fn value_at(&self, py: Python<'_>, x: f64, y: f64) -> PyResult<f64> {
        let proj = self.project.bind(py).borrow();
        proj.inner.surface_value_at(&self.name, x, y).map_err(err)
    }

    /// Batched [`value_at`](Self::value_at): sample the surface at every `(x, y)`
    /// in `points` with a single lookup — the vectorised form for a caller
    /// seeding many trend samples at once (avoids the per-point call overhead).
    fn values_at(&self, py: Python<'_>, points: Vec<[f64; 2]>) -> PyResult<Vec<f64>> {
        let proj = self.project.bind(py).borrow();
        proj.inner
            .surface_values_at(&self.name, &points)
            .map_err(err)
    }
}

/// The tops accessor.
#[pyclass(name = "Tops")]
pub struct Tops {
    project: Py<Project>,
}

#[pymethods]
impl Tops {
    /// Per-well picks of a named tops surface + a representative level/spread.
    ///
    /// `wells` optionally restricts the aggregation to a subset of bores/wells (a
    /// listed well id like `"99/9-1"` selects all its bores); omit it to aggregate
    /// across every well.
    #[pyo3(signature = (name, wells=None))]
    fn pick(&self, py: Python<'_>, name: String, wells: Option<Vec<String>>) -> PyResult<TopsPick> {
        let proj = self.project.bind(py).borrow();
        match proj.inner.pick(&name, wells.as_deref()) {
            Some(p) => Ok(TopsPick::from_core(p)),
            None => Err(PyValueError::new_err(match &wells {
                Some(ws) if !ws.is_empty() => {
                    format!("no well in {ws:?} carries a '{name}' pick")
                }
                _ => format!("no well carries a '{name}' pick"),
            })),
        }
    }
}

/// A named tops surface's per-well picks and representative level.
#[pyclass(name = "TopsPick", from_py_object)]
#[derive(Clone)]
pub struct TopsPick {
    #[pyo3(get)]
    name: String,
    /// `(well_id, depth_m)` picks.
    #[pyo3(get)]
    picks: Vec<(String, f64)>,
    /// Representative (mean) level \[m, positive-down\].
    #[pyo3(get)]
    pub(crate) level_m: f64,
    /// Standard deviation of the picks \[m\].
    #[pyo3(get)]
    spread_m: f64,
}

impl TopsPick {
    fn from_core(p: CoreTopsPick) -> Self {
        Self {
            name: p.name,
            picks: p.picks,
            level_m: p.level_m,
            spread_m: p.spread_m,
        }
    }
}

#[pymethods]
impl TopsPick {
    fn __repr__(&self) -> String {
        format!(
            "TopsPick('{}', level_m={:.2}, spread_m={:.2}, n={})",
            self.name,
            self.level_m,
            self.spread_m,
            self.picks.len()
        )
    }
}
