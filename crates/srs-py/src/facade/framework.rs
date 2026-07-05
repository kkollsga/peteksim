//! The `Framework` pyclass (zonation / layering / well-tie declaration) + its parse helpers.

use super::*;

/// A declared framework, pre-`build_grid`.
///
/// Holds a handle to the loaded [`Project`] (`project`) so the facade wireframe + tie
/// report can be gridded **lazily** — only when a wireframe consumer needs them
/// (`tie_report`/`tie_ok`, or a single-`ZoneTable` `build_grid`). A stack build
/// (`set_zonation`) never touches the project again: it conditions the raw scatter in
/// the engine, so the second facade-side gridding of every point-set horizon is
/// skipped (`task_suite_scatter_perf`).
#[pyclass(name = "Framework")]
pub struct Framework {
    pub(crate) inner: Option<CoreFramework>,
    /// The loaded project (a ref-counted Python handle), re-borrowed for the deferred
    /// wireframe/tie gridding. Same holding pattern as `Wells`/`Surface`.
    pub(crate) project: Py<Project>,
}

impl Framework {
    fn get(&self) -> PyResult<&CoreFramework> {
        self.inner
            .as_ref()
            .ok_or_else(|| PyValueError::new_err("framework already consumed by build_grid()"))
    }
    fn get_mut(&mut self) -> PyResult<&mut CoreFramework> {
        self.inner
            .as_mut()
            .ok_or_else(|| PyValueError::new_err("framework already consumed by build_grid()"))
    }
    /// Materialize the deferred facade wireframe + tie report from the held project
    /// (idempotent). Drives the lazy gridding that `build` skipped so a stack build
    /// never pays for it.
    fn materialize(&mut self, py: Python<'_>) -> PyResult<()> {
        let proj = self.project.borrow(py);
        let fw = self
            .inner
            .as_mut()
            .ok_or_else(|| PyValueError::new_err("framework already consumed by build_grid()"))?;
        fw.materialize_wireframe(&proj.inner).map_err(err)
    }
}

#[pymethods]
impl Framework {
    /// The per-horizon-per-well tie residuals, as dicts. Loud: rows with
    /// `ok=False` failed to tie (missing pick / off-grid). Triggers the deferred
    /// facade wireframe gridding (the residuals are sampled off the gridded surface).
    fn tie_report(&mut self, py: Python<'_>) -> PyResult<Py<PyList>> {
        self.materialize(py)?;
        let rows = PyList::empty(py);
        for t in self.get()?.tie_report() {
            let d = PyDict::new(py);
            d.set_item("horizon", &t.horizon)?;
            d.set_item("well", &t.well_id)?;
            d.set_item("surface_m", t.surface_m)?;
            d.set_item("pick_m", t.pick_m)?;
            d.set_item("residual_m", t.residual_m)?;
            d.set_item("ok", t.ok)?;
            d.set_item("note", &t.note)?;
            rows.append(d)?;
        }
        Ok(rows.unbind())
    }

    /// Whether every tie succeeded. Triggers the deferred facade wireframe gridding.
    fn tie_ok(&mut self, py: Python<'_>) -> PyResult<bool> {
        self.materialize(py)?;
        Ok(self.get()?.tie_ok())
    }

    /// Constant priors used where no pipeline/log conditions a cube.
    #[pyo3(signature = (porosity=0.25, net_to_gross=0.8, water_saturation=0.3))]
    fn set_priors(
        &mut self,
        porosity: f64,
        net_to_gross: f64,
        water_saturation: f64,
    ) -> PyResult<()> {
        self.get_mut()?
            .set_priors(porosity, net_to_gross, water_saturation);
        Ok(())
    }

    /// Declare zones between horizon pairs (`{name: (top, base)}`). **Documented
    /// limitation:** the current stack is a single ZoneTable, so zones inform the
    /// total layer allocation but do not yet grant per-zone property
    /// independence (the P5 zones task).
    fn set_zones(&mut self, zones: &Bound<'_, PyDict>) -> PyResult<()> {
        let n = zones.len();
        self.get_mut()?.set_zones(n);
        Ok(())
    }

    /// Allocate layering. Under the single-ZoneTable limitation a mapping of
    /// per-zone specs collapses to the summed whole-column layer count; a single
    /// `ps.layers(...)` sets it directly.
    fn set_layering(&mut self, layering: &Bound<'_, PyAny>) -> PyResult<()> {
        let (spec, conformity) = layering_spec(layering)?;
        self.get_mut()?.set_layering(spec, conformity);
        Ok(())
    }

    /// Whether the requested zonation exceeds the single-ZoneTable capability.
    fn zones_limited(&self) -> PyResult<bool> {
        Ok(self.get()?.zones_limited())
    }

    /// Declare a **multi-zone horizon stack**: a list of per-zone dicts, one per
    /// horizon gap (top→down). Each dict is
    /// `{"zone": name, "below_horizon": h, "conformity": "proportional"|"follow_top"|
    /// "follow_base", "nk": int | "dz_m": float, "contacts": {"goc": d, "fwl"/"owc": d}
    /// | None}`. This selects petekStatic's `from_horizon_stack` build — real
    /// per-zone layering + per-zone volumetrics (`model.in_place_by_zone()`,
    /// `model.zone_stats(prop)`). Horizons may be mapped surfaces OR tops-only (a
    /// horizon with well picks but no surface; the top must be a mapped surface).
    ///
    /// Distinct from `set_zones` (the single-ZoneTable provenance-only path).
    fn set_zonation(&mut self, zones: &Bound<'_, PyList>) -> PyResult<()> {
        let specs = parse_zonation(zones)?;
        self.get_mut()?.set_zonation(specs).map_err(err)
    }

    /// Set the opt-in cell-collapse floor (Petrel-style) for the stack build —
    /// equivalent to `framework(..., collapse_below_m=..)`.
    fn set_collapse_below_m(&mut self, collapse_below_m: f64) -> PyResult<()> {
        self.get_mut()?.set_collapse_below_m(collapse_below_m);
        Ok(())
    }

    /// Give a **named zone** its own base priors (PORO/NTG/SW level) on the
    /// multi-zone stack build (petekStatic's `with_zone_priors`): a sand vs a shale
    /// zone sits at a different level over its own k-range. Also the base level the
    /// zoned MC's per-zone level shift moves. Replaces any prior for the same zone.
    #[pyo3(signature = (zone, porosity=0.25, net_to_gross=0.8, water_saturation=0.3))]
    fn set_zone_priors(
        &mut self,
        zone: &str,
        porosity: f64,
        net_to_gross: f64,
        water_saturation: f64,
    ) -> PyResult<()> {
        self.get_mut()?
            .set_zone_priors(zone, porosity, net_to_gross, water_saturation);
        Ok(())
    }

    /// Accept a **well-tie table** and thread it to the stack build's engine tie
    /// seam (petekStatic's `with_well_ties`): each mapped horizon is re-solved with
    /// the measured formation top as a hard control at the well node, and the
    /// residuals surface on `model.well_tie_residuals()`. Each entry is a dict
    /// `{"id": str, "x": float, "y": float, "tops": {horizon: depth_m}}` — depths
    /// positive-down subsea (SI, metres). Off-grid wells are dropped. Multi-zone
    /// stack only.
    fn set_well_ties(&mut self, ties: &Bound<'_, PyList>) -> PyResult<()> {
        let parsed = parse_well_ties(ties)?;
        self.get_mut()?.set_well_ties(parsed);
        Ok(())
    }

    /// Whether a multi-zone stack was declared (`set_zonation`).
    fn is_zoned(&self) -> PyResult<bool> {
        Ok(self.get()?.is_zoned())
    }

    /// Freeze into a `Grid`.
    fn build_grid(&mut self, py: Python<'_>) -> PyResult<Grid> {
        let zoned = self.get()?.is_zoned();
        let fw = self
            .inner
            .take()
            .ok_or_else(|| PyValueError::new_err("framework already consumed"))?;
        let grid = if zoned {
            // Stack build: the engine builds its framework from the conditioned raw
            // scatter — no loaded project, and the facade wireframe is NEVER gridded
            // (the second facade-side gridding is skipped, `task_suite_scatter_perf`).
            // S1: release the GIL for the pure-Rust conditioning + build.
            py.detach(|| fw.build_grid_stack()).map_err(err)?
        } else {
            // Wireframe build: materializes the facade wireframe + ties by gridding the
            // horizons from the GIL-held project, so it runs under the GIL (wireframe
            // builds are single-`ZoneTable`, small; the scatter cost lives on the stack
            // path above).
            let proj = self.project.borrow(py);
            fw.build_grid_wireframe(&proj.inner).map_err(err)?
        };
        Ok(Grid { inner: Some(grid) })
    }
}

/// Parse a `set_zonation([...])` list of per-zone dicts into [`ZoneSpec`]s. Each
/// dict: `zone`, `below_horizon`, `conformity` (default "proportional"), one of
/// `nk` / `dz_m` for the layer allocation, and optional `contacts` (`goc` +
/// `fwl`/`owc`; absent/None ⇒ contactless).
fn parse_zonation(zones: &Bound<'_, PyList>) -> PyResult<Vec<ZoneSpec>> {
    let mut out = Vec::with_capacity(zones.len());
    for item in zones.iter() {
        let d = item.cast::<PyDict>().map_err(|_| {
            PyValueError::new_err("each zonation entry must be a dict {zone, below_horizon, ..}")
        })?;
        let get = |k: &str| -> PyResult<Option<Bound<'_, PyAny>>> { d.get_item(k) };
        let name: String = get("zone")?
            .ok_or_else(|| PyValueError::new_err("zonation entry needs a 'zone' name"))?
            .extract()?;
        let below_horizon: String = get("below_horizon")?
            .ok_or_else(|| PyValueError::new_err(format!("zone '{name}' needs a 'below_horizon'")))?
            .extract()?;
        let style: String = match get("conformity")? {
            Some(v) => v.extract()?,
            None => "proportional".to_string(),
        };
        let dz_m: Option<f64> = match get("dz_m")? {
            Some(v) => Some(v.extract()?),
            None => None,
        };
        let nk: Option<usize> = match get("nk")? {
            Some(v) => Some(v.extract()?),
            None => None,
        };
        let (conformity, layers) = zone_layering(&name, &style, nk, dz_m)?;
        let (goc_m, owc_m) = match get("contacts")? {
            Some(c) if !c.is_none() => parse_zone_contacts(&name, &c)?,
            _ => (None, None),
        };
        out.push(ZoneSpec {
            name,
            below_horizon,
            conformity,
            layers,
            goc_m,
            owc_m,
        });
    }
    Ok(out)
}

/// Resolve a zone's `conformity` + `nk`/`dz_m` into `(Conformity, LayersSpec)`.
fn zone_layering(
    zone: &str,
    style: &str,
    nk: Option<usize>,
    dz_m: Option<f64>,
) -> PyResult<(Conformity, LayersSpec)> {
    match style.to_ascii_lowercase().as_str() {
        "proportional" | "prop" => match (nk, dz_m) {
            (Some(n), None) => Ok((Conformity::Proportional, LayersSpec::Count(n))),
            (None, Some(dz)) => Ok((Conformity::Proportional, LayersSpec::Thickness(dz))),
            _ => Err(PyValueError::new_err(format!(
                "zone '{zone}' (proportional) needs exactly one of nk= or dz_m="
            ))),
        },
        "follow_top" | "followtop" | "erosional" | "truncation" => {
            let dz = dz_m.ok_or_else(|| {
                PyValueError::new_err(format!("zone '{zone}' (follow_top) needs dz_m="))
            })?;
            Ok((
                Conformity::FollowTop { dz_m: dz },
                LayersSpec::Thickness(dz),
            ))
        }
        "follow_base" | "followbase" | "onlap" => {
            let dz = dz_m.ok_or_else(|| {
                PyValueError::new_err(format!("zone '{zone}' (follow_base) needs dz_m="))
            })?;
            Ok((
                Conformity::FollowBase { dz_m: dz },
                LayersSpec::Thickness(dz),
            ))
        }
        other => Err(PyValueError::new_err(format!(
            "zone '{zone}': unknown conformity '{other}' (proportional / follow_top / follow_base)"
        ))),
    }
}

/// Parse a zone's `contacts` dict into `(goc, lower)` depths — a float depth or a
/// `TopsPick` each. Missing keys ⇒ `None` (a contactless or single-contact zone).
fn parse_zone_contacts(
    zone: &str,
    contacts: &Bound<'_, PyAny>,
) -> PyResult<(Option<f64>, Option<f64>)> {
    let map = contacts.cast::<PyDict>().map_err(|_| {
        PyValueError::new_err(format!("zone '{zone}' contacts must be a dict or None"))
    })?;
    let mut goc = None;
    let mut lower = None;
    for (k, v) in map.iter() {
        let key = k.extract::<String>()?.to_ascii_lowercase();
        let depth = contact_depth(&v)?;
        match key.as_str() {
            "goc" => goc = Some(depth),
            "fwl" | "owc" | "gwc" | "contact" => lower = Some(depth),
            other => {
                return Err(PyValueError::new_err(format!(
                    "zone '{zone}': unknown contact '{other}' (expected goc / fwl / owc)"
                )))
            }
        }
    }
    Ok((goc, lower))
}

/// A raw well-tie row: `(well_id, x, y, [(horizon, measured_depth_m)])`.
type WellTieRow = (String, f64, f64, Vec<(String, f64)>);

/// Parse a well-tie table (list of `{"id", "x", "y", "tops": {horizon: depth_m}}`)
/// into `(id, x, y, [(horizon, depth_m)])` tuples for `Framework::set_well_ties`.
fn parse_well_ties(ties: &Bound<'_, PyList>) -> PyResult<Vec<WellTieRow>> {
    let mut out = Vec::with_capacity(ties.len());
    for item in ties.iter() {
        let d = item.cast::<PyDict>().map_err(|_| {
            PyValueError::new_err("each well-tie entry must be a dict {id, x, y, tops}")
        })?;
        let get = |k: &str| -> PyResult<Option<Bound<'_, PyAny>>> { d.get_item(k) };
        let id: String = get("id")?
            .ok_or_else(|| PyValueError::new_err("well-tie entry needs an 'id'"))?
            .extract()?;
        let x: f64 = get("x")?
            .ok_or_else(|| PyValueError::new_err(format!("well tie '{id}' needs 'x'")))?
            .extract()?;
        let y: f64 = get("y")?
            .ok_or_else(|| PyValueError::new_err(format!("well tie '{id}' needs 'y'")))?
            .extract()?;
        let tops_obj = get("tops")?
            .ok_or_else(|| PyValueError::new_err(format!("well tie '{id}' needs a 'tops' dict")))?;
        let tops_map = tops_obj.cast::<PyDict>().map_err(|_| {
            PyValueError::new_err(format!("well tie '{id}': 'tops' must be a dict"))
        })?;
        let mut tops = Vec::with_capacity(tops_map.len());
        for (h, depth) in tops_map.iter() {
            tops.push((h.extract::<String>()?, depth.extract::<f64>()?));
        }
        out.push((id, x, y, tops));
    }
    Ok(out)
}

/// Resolve a `ps.layers(...)` spec (or a `{zone: ps.layers(...)}` mapping,
/// single-ZoneTable collapse) into `(LayersSpec, Conformity)`. A single `Layers`
/// carries its own conformity; a multi-zone mapping collapses to the summed
/// proportional count (Follow styles are single-zone only for now).
fn layering_spec(obj: &Bound<'_, PyAny>) -> PyResult<(LayersSpec, Conformity)> {
    if let Ok(layers) = obj.extract::<Layers>() {
        return Ok((layers.spec, layers.conformity));
    }
    if let Ok(map) = obj.cast::<PyDict>() {
        let mut total = 0usize;
        for (_k, v) in map.iter() {
            let l = v.extract::<Layers>()?;
            total += l.spec.nk(50.0);
        }
        return Ok((LayersSpec::Count(total.max(1)), Conformity::Proportional));
    }
    Err(PyValueError::new_err(
        "set_layering expects ps.layers(...) or a {zone: ps.layers(...)} dict",
    ))
}
