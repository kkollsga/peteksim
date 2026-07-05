//! The `StaticModel` pyclass — bundles, summaries, and the structured-MC entry points.

use super::*;

/// The `ps.hz` structural-uncertainty rows as they cross the FFI: one
/// `(sd_m, (variogram_model, range_m) | None)` per horizon, in order top→base.
type StructuralRows = Vec<(f64, Option<(String, f64)>)>;

/// A populated static model.
#[pyclass(name = "StaticModel")]
pub struct StaticModelPy {
    pub(crate) inner: ProjectModel,
    /// Positioned well tracks attached at build (`grid.model(..., wells=..)`);
    /// empty when none were attached. Drives the viewer's markers + well sections.
    pub(crate) wells: Vec<WellTrack>,
    /// The Wells-tab `wells_logs` bundle, assembled at build from the attached
    /// bores + this model (raw/upscaled curves, tops/zones, ties). `None` when no
    /// bores were attached or none carry a correlatable curve.
    pub(crate) wells_logs: Option<WellLogBundle>,
}

#[pymethods]
impl StaticModelPy {
    /// The populated cube names.
    fn property_names(&self) -> Vec<String> {
        self.inner.property_names()
    }

    /// The areal (plan-view) map bundle as a JSON-ready dict: structural depth
    /// surfaces, per-property zone-average maps (and optional k-slices), the
    /// outline, and contact subcrop masks. `property` selects the cube(s) to map
    /// (a name or list; default = every populated cube).
    #[pyo3(signature = (property=None, k_slice=None))]
    fn map_bundle(
        &self,
        py: Python<'_>,
        property: Option<&Bound<'_, PyAny>>,
        k_slice: Option<usize>,
    ) -> PyResult<Py<PyAny>> {
        let props = self.resolve_properties(property)?;
        let model = self.inner.static_model();
        // Assemble the map bundle + serialize to a Value OFF the GIL (S1); only the
        // final Value→Python walk holds the GIL.
        let mut value = py.detach(|| -> viewer::VResult<Value> {
            viewer::to_value(&viewer::build_map(model, &props, k_slice)?)
        })?;
        // Each zone-average layer is named `"<property>::<zone>"` (petekStatic's
        // ScalarLayer). Add machine-readable `property` + `zone` keys so a consumer
        // need not string-split the composite name (facade payload UX).
        enrich_zone_average_keys(&mut value);
        viewer::json_to_py(py, &value)
    }

    /// A vertical cross-section bundle as a JSON-ready dict. Give either
    /// `line=[[x,y], ...]` (a world polyline) or `well="<id>"` (an attached bore);
    /// `property` colours the section (default = the canonical porosity cube).
    #[pyo3(signature = (line=None, well=None, property=None))]
    fn intersection_bundle(
        &self,
        py: Python<'_>,
        line: Option<Vec<[f64; 2]>>,
        well: Option<String>,
        property: Option<&str>,
    ) -> PyResult<Py<PyAny>> {
        let model = self.inner.static_model();
        let wells = &self.wells;
        // Own the property before releasing the GIL (it borrows a Python string).
        let property = property.map(str::to_owned);
        // Build the section + serialize to a Value OFF the GIL, then one GIL-held
        // Value→Python walk — killing the old string→reparse→to_value→walk quadruple.
        let value =
            py.detach(|| viewer::section_value(model, property.as_deref(), line, well, wells))?;
        viewer::json_to_py(py, &value)
    }

    /// The 3-D volume bundle as a JSON-ready dict, coloured by `property`
    /// (default = the porosity cube), in petekStatic's **v3 self-contained wire
    /// envelope** (`encoding:"base64"` + a `blocks` map of the exterior-shell
    /// arrays) — the same shape the viewer payload's `volume` carries and the only
    /// one the petekTools decode kernel accepts.
    #[pyo3(signature = (property=None))]
    fn volume_bundle(&self, py: Python<'_>, property: Option<&str>) -> PyResult<Py<PyAny>> {
        let prop = self.render_property(property);
        let model = self.inner.static_model();
        // The heaviest bundle: corner-point mesh + per-cell property over the whole
        // grid. Assemble + serialize to a Value OFF the GIL; only the final
        // Value→Python walk holds the GIL.
        let value = py.detach(|| -> viewer::VResult<Value> {
            // The v3 block wire envelope (base64 blocks) — same shape the viewer
            // payload carries, not the serde derive's inline arrays (which the
            // petekTools decode kernel crashes on for schema_version>=3).
            viewer::volume_envelope_value(&viewer::build_volume(model, &prop)?)
        })?;
        viewer::json_to_py(py, &value)
    }

    /// Attached well ids (positioned bores; empty unless `grid.model(wells=..)`).
    fn well_ids(&self) -> Vec<String> {
        self.wells.iter().map(|w| w.id.clone()).collect()
    }

    /// The viewer's **Wells** tab bundle (`wells_logs`, schema_version 4) as a
    /// JSON-ready dict — per-bore `md_m`/`tvd_m` lanes, raw + upscaled curves,
    /// framework `tops`/`zones`, and tie residuals. `None` unless bores were
    /// attached (`grid.model(..., wells=proj.wells())`) and at least one carries a
    /// correlatable curve. This is exactly the bundle `view()`/`save_view` emit as
    /// the Wells tab payload.
    fn wells_bundle(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let logs = &self.wells_logs;
        let value = py.detach(|| viewer::to_value(logs))?;
        viewer::json_to_py(py, &value)
    }

    /// Open the viewer in a browser. Non-blocking by default (a background server
    /// prints its URL and returns); pass `block=True` for the old blocking mode.
    /// `lines` pre-computes extra section fences ([[x,y], ...] each); attached
    /// wells are sectioned automatically.
    #[pyo3(signature = (open_browser=true, port=0, block=false, property=None, lines=None, charts=None))]
    #[allow(clippy::too_many_arguments)]
    fn view(
        slf: Bound<'_, Self>,
        py: Python<'_>,
        open_browser: bool,
        port: u16,
        block: bool,
        property: Option<&str>,
        lines: Option<Vec<Vec<[f64; 2]>>>,
        charts: Option<Vec<Py<PyAny>>>,
    ) -> PyResult<String> {
        let chart_vals = charts_to_values(py, charts)?;
        let json = slf.borrow().payload(py, property, lines, chart_vals)?;
        viewer::serve(py, slf.as_any(), json, open_browser, port, block)
    }

    /// Write ONE self-contained HTML file (all JS + data inlined; opens via
    /// `file://`). Attached-well sections and any `lines` are pre-computed; the
    /// live fence-draw control is disabled in this mode (server-only).
    #[pyo3(signature = (path, property=None, lines=None, charts=None))]
    fn save_view(
        &self,
        py: Python<'_>,
        path: &str,
        property: Option<&str>,
        lines: Option<Vec<Vec<[f64; 2]>>>,
        charts: Option<Vec<Py<PyAny>>>,
    ) -> PyResult<()> {
        let chart_vals = charts_to_values(py, charts)?;
        viewer::save_view(py, path, self.payload(py, property, lines, chart_vals)?)
    }

    /// Live-section endpoint the local server calls for a drawn fence or clicked
    /// well (returns one `IntersectionBundle` as JSON).
    #[pyo3(signature = (property=None, line=None, well=None))]
    fn _section_json(
        &self,
        py: Python<'_>,
        property: Option<&str>,
        line: Option<Vec<[f64; 2]>>,
        well: Option<String>,
    ) -> PyResult<String> {
        let model = self.inner.static_model();
        let wells = &self.wells;
        let property = property.map(str::to_owned);
        Ok(py.detach(|| viewer::section_json(model, property.as_deref(), line, well, wells))?)
    }

    /// Live-volume endpoint the local server calls to RE-CUT the exterior shell at
    /// a property `cutoff` (returns one `VolumeBundle` as the v3 JSON envelope).
    /// `cutoff=None` returns the full-set shell; `keep_above` selects `>=`/`<=`.
    /// The file (`save_view`) export ships only the full-set shell + the client-side
    /// shell filter; true interior exposure needs this live-mode provider.
    #[pyo3(signature = (property=None, cutoff=None, keep_above=true))]
    fn _volume_json(
        &self,
        py: Python<'_>,
        property: Option<&str>,
        cutoff: Option<f64>,
        keep_above: bool,
    ) -> PyResult<String> {
        let model = self.inner.static_model();
        let property = property.map(str::to_owned);
        Ok(py.detach(|| viewer::volume_json(model, property.as_deref(), cutoff, keep_above))?)
    }

    /// Non-blocking build advisories (loud, not swallowed), as dicts. A
    /// `thin_columns_repaired` entry means the opt-in `framework(...,
    /// min_thickness_m=...)` order-repair pulled thin/crossing base columns down to
    /// the minimum thickness below the top (`columns` nodes; `worst_m` = the worst
    /// original base−top separation, negative = a true crossing). Empty on a clean
    /// build.
    fn warnings(&self, py: Python<'_>) -> PyResult<Py<PyList>> {
        let rows = PyList::empty(py);
        for w in self.inner.warnings() {
            let d = PyDict::new(py);
            d.set_item("kind", &w.kind)?;
            d.set_item("message", &w.message)?;
            d.set_item("columns", w.columns)?;
            d.set_item("worst_m", w.worst_m)?;
            rows.append(d)?;
        }
        Ok(rows.unbind())
    }

    /// The deterministic volumetric summary (dict; Sm³ + mcm + MSm³).
    fn summary(&self, py: Python<'_>) -> PyResult<Py<PyDict>> {
        let s: Summary = self.inner.summary().map_err(err)?;
        let d = PyDict::new(py);
        d.set_item("fluid", &s.fluid)?;
        d.set_item("stoiip_sm3", s.stoiip_sm3)?;
        d.set_item("stoiip_msm3", s.stoiip_sm3 / SM3_PER_MSM3)?;
        d.set_item("giip_sm3", s.giip_sm3)?;
        d.set_item("giip_msm3", s.giip_sm3 / SM3_PER_MSM3)?;
        d.set_item("grv_mcm", s.grv_mcm)?;
        d.set_item("two_contact", s.two_contact)?;
        Ok(d.unbind())
    }

    /// Per-zone in-place volumes (multi-zone stack): a dict `{"zones": [...],
    /// "total": {...}}`, each row `{zone, grv_mcm, hcpv_m3, stoiip_sm3, stoiip_msm3,
    /// giip_sm3, giip_bcm, two_contact}`. Each zone is clipped vs its OWN contacts;
    /// a contactless zone reports gross bulk with zero hydrocarbon; the total is the
    /// summed rollup (`total.stoiip_sm3 == Σ zone.stoiip_sm3`).
    fn in_place_by_zone(&self, py: Python<'_>) -> PyResult<Py<PyDict>> {
        let z = self.inner.in_place_by_zone().map_err(err)?;
        let out = PyDict::new(py);
        let rows = PyList::empty(py);
        for zv in &z.zones {
            rows.append(zone_volume_dict(py, zv)?)?;
        }
        out.set_item("zones", rows)?;
        out.set_item("total", zone_volume_dict(py, &z.total)?)?;
        Ok(out.unbind())
    }

    /// Per-zone statistics of a property cube — a list of dicts `{zone, count,
    /// mean, min, max}` (NaN aggregates where a zone has no active cells).
    fn zone_stats(&self, py: Python<'_>, property: &str) -> PyResult<Py<PyList>> {
        let stats = self.inner.zone_stats(property).map_err(err)?;
        let rows = PyList::empty(py);
        for s in &stats {
            let d = PyDict::new(py);
            d.set_item("zone", &s.zone)?;
            d.set_item("count", s.count)?;
            d.set_item("mean", s.mean)?;
            d.set_item("min", s.min)?;
            d.set_item("max", s.max)?;
            rows.append(d)?;
        }
        Ok(rows.unbind())
    }

    /// Whether this model was built from a multi-zone horizon stack.
    fn is_zoned(&self) -> bool {
        self.inner.is_zoned()
    }

    /// Run the structured Monte Carlo. Distributions are `ps.*` objects; contacts
    /// take a `ps.pick_spread(sd_m=..)`; `n`/`seed` drive the run.
    #[pyo3(signature = (
        porosity=None, net_to_gross=None, water_saturation=None,
        contacts=None, fvf=None, gas_fvf=None, shifts=None,
        n=10_000, seed=42, lo_pct=10.0, hi_pct=90.0
    ))]
    #[allow(clippy::too_many_arguments)]
    fn uncertainty(
        &self,
        py: Python<'_>,
        porosity: Option<Dist>,
        net_to_gross: Option<Dist>,
        water_saturation: Option<Dist>,
        contacts: Option<&Bound<'_, PyAny>>,
        fvf: Option<Dist>,
        gas_fvf: Option<Dist>,
        shifts: Option<&Bound<'_, PyDict>>,
        n: usize,
        seed: u64,
        lo_pct: f64,
        hi_pct: f64,
    ) -> PyResult<Uncertainty> {
        let (contact_sd, goc_sd) = parse_pick_spread(contacts)?;
        let mut extra_shifts = Vec::new();
        if let Some(map) = shifts {
            for (k, v) in map.iter() {
                extra_shifts.push((k.extract::<String>()?, v.extract::<Dist>()?.spec));
            }
        }
        let cfg = McConfig {
            porosity: porosity.map(|d| d.spec),
            net_to_gross: net_to_gross.map(|d| d.spec),
            water_saturation: water_saturation.map(|d| d.spec),
            extra_shifts,
            contact_sd_m: contact_sd,
            goc_sd_m: goc_sd,
            boi: fvf.map(|d| d.spec),
            bgi: gas_fvf.map(|d| d.spec),
            n,
            seed,
            lo_pct,
            hi_pct,
        };
        // S1: the structured Monte Carlo (`n` realizations) is the heavy pure-Rust
        // path — release the GIL so concurrent Python threads keep running. `cfg`
        // is fully GIL-independent (its `Py` inputs were extracted above).
        let outcome = py.detach(|| self.inner.uncertainty(cfg)).map_err(err)?;
        // L: build each P-curve dict ONCE here (not per `.stoiip`/`.giip` access),
        // reading samples straight from the outcome's kept result (no double copy).
        let stoiip = psummary_dict(py, &outcome.stoiip, outcome.stoiip_samples())?;
        let giip = psummary_dict(py, &outcome.giip, outcome.giip_samples())?;
        Ok(Uncertainty {
            inner: outcome,
            stoiip,
            giip,
        })
    }

    /// The per-horizon well-tie residuals from the engine tie seam
    /// (`fw.set_well_ties(..)`), as dicts `{well, horizon, measured_depth_m,
    /// model_depth_m, residual_m}` (depths positive-down; `residual_m = measured −
    /// untied model surface`). Empty when no tie table was supplied.
    fn well_tie_residuals(&self, py: Python<'_>) -> PyResult<Py<PyList>> {
        let rows = PyList::empty(py);
        for r in self.inner.well_tie_residuals() {
            let d = PyDict::new(py);
            d.set_item("well", &r.well_id)?;
            d.set_item("horizon", &r.horizon)?;
            d.set_item("measured_depth_m", r.measured_depth_m)?;
            d.set_item("model_depth_m", r.model_depth_m)?;
            d.set_item("residual_m", r.residual_m)?;
            rows.append(d)?;
        }
        Ok(rows.unbind())
    }

    /// Run the **stack-aware** Monte Carlo over a multi-zone model: per-zone contact
    /// draws + per-zone property level shifts, rolled up into per-zone AND total
    /// STOIIP/GIIP P-curves (a contactless zone contributes GRV, zero hydrocarbon).
    ///
    /// `contacts`/`goc` take a `ps.pick_spread(sd_m=..)` (or float sd) — the
    /// run-wide lower-contact / GOC pick-spread applied to every zone that carries
    /// that contact. `porosity`/`net_to_gross`/`water_saturation` are base level
    /// shifts (`ps.level_shift(sd=..)`). `zones` is a per-zone override dict
    /// `{zone: {"contacts": .., "goc": .., "porosity": ps.*, "net_to_gross": ps.*,
    /// "water_saturation": ps.*}}`. `workers>1` shards the realize loop. Returns a
    /// `ZonedUncertainty` (`.total`, `.zones`).
    /// `structural` carries the `ps.hz` per-row structural uncertainty (`sd`/`vgm`),
    /// in horizon order top→base: entry `0` is the TOP-surface depth field; entry
    /// `k` (`k >= 1`) is zone `k-1`'s isochore (thickness) field. Each entry is
    /// `(sd_m, (variogram_model, range_m) | None)`; `sd_m <= 0` is a no-op.
    #[pyo3(signature = (
        porosity=None, net_to_gross=None, water_saturation=None,
        contacts=None, goc=None, fvf=None, gas_fvf=None, zones=None,
        structural=None, n=10_000, seed=42, workers=0
    ))]
    #[allow(clippy::too_many_arguments)]
    fn zoned_uncertainty(
        &self,
        py: Python<'_>,
        porosity: Option<Dist>,
        net_to_gross: Option<Dist>,
        water_saturation: Option<Dist>,
        contacts: Option<&Bound<'_, PyAny>>,
        goc: Option<&Bound<'_, PyAny>>,
        fvf: Option<Dist>,
        gas_fvf: Option<Dist>,
        zones: Option<&Bound<'_, PyDict>>,
        structural: Option<StructuralRows>,
        n: usize,
        seed: u64,
        workers: usize,
    ) -> PyResult<ZonedUncertainty> {
        let (contact_sd, _) = parse_pick_spread(contacts)?;
        let goc_sd = match goc {
            Some(o) => {
                let (sd, _) = parse_pick_spread(Some(o))?;
                sd
            }
            None => None,
        };
        let per_zone = match zones {
            Some(map) => parse_zone_mc(map)?,
            None => Vec::new(),
        };
        let (top_structural, zone_isochore_structural) = build_structural(structural)?;
        let cfg = ZonedMcConfig {
            porosity: porosity.map(|d| d.spec),
            net_to_gross: net_to_gross.map(|d| d.spec),
            water_saturation: water_saturation.map(|d| d.spec),
            contact_sd_m: contact_sd,
            goc_sd_m: goc_sd,
            boi: fvf.map(|d| d.spec),
            bgi: gas_fvf.map(|d| d.spec),
            per_zone,
            top_structural,
            zone_isochore_structural,
            n,
            seed,
            workers,
        };
        // S1: the zoned MC (`n` per-zone realizations) is the heavy pure-Rust path —
        // release the GIL (the cfg is fully GIL-independent).
        let outcome = py
            .detach(|| self.inner.zoned_uncertainty(cfg))
            .map_err(err)?;
        // L: build the `total`/`zones` dict trees ONCE here (not per getter access),
        // reading the samples straight from the kept outcome (no rebuild per read).
        let total = build_zoned_total_dict(py, &outcome)?;
        let zones = build_zoned_zones_list(py, &outcome)?;
        Ok(ZonedUncertainty {
            inner: outcome,
            total,
            zones,
        })
    }
}

impl StaticModelPy {
    /// Resolve a `property` argument (a `str`, a list of `str`, or `None`) into the
    /// cubes to map. `None` = every populated cube.
    fn resolve_properties(&self, property: Option<&Bound<'_, PyAny>>) -> PyResult<Vec<String>> {
        let all = self.inner.property_names();
        let Some(obj) = property else {
            return Ok(all);
        };
        if let Ok(name) = obj.extract::<String>() {
            return Ok(vec![name]);
        }
        if let Ok(names) = obj.extract::<Vec<String>>() {
            return Ok(names);
        }
        Err(PyValueError::new_err(
            "property must be a cube name, a list of names, or None",
        ))
    }

    /// The render property: the request if populated, else the canonical porosity
    /// cube, else the first populated cube (else `"PORO"`).
    fn render_property(&self, property: Option<&str>) -> String {
        let all = self.inner.property_names();
        property
            .filter(|p| all.iter().any(|q| q == p))
            .map(String::from)
            .or_else(|| all.iter().find(|p| p.as_str() == "PORO").cloned())
            .or_else(|| all.first().cloned())
            .unwrap_or_else(|| "PORO".to_string())
    }

    /// The deterministic volumetric summary as JSON (display metadata). For a
    /// zoned model this additively appends one per-zone STOIIP row so the viewer's
    /// summary panel shows the per-zone volumes table (payload-additive — the panel
    /// renders any flat key→value it is handed, no viewer change needed).
    fn summary_value(&self) -> Option<serde_json::Value> {
        let s = self.inner.summary().ok()?;
        let mut m = serde_json::Map::new();
        m.insert("fluid".into(), Value::from(s.fluid));
        m.insert("stoiip_sm3".into(), Value::from(s.stoiip_sm3));
        m.insert("giip_sm3".into(), Value::from(s.giip_sm3));
        m.insert("grv_mcm".into(), Value::from(s.grv_mcm));
        m.insert("two_contact".into(), Value::from(s.two_contact));
        if self.inner.is_zoned() {
            if let Ok(z) = self.inner.in_place_by_zone() {
                for zv in &z.zones {
                    m.insert(
                        format!("{} STOIIP (MSm³)", zv.zone),
                        Value::from(zv.stoiip_sm3 / SM3_PER_MSM3),
                    );
                }
            }
        }
        Some(Value::Object(m))
    }

    /// Compose the full viewer payload JSON (map + volume + well/user sections +
    /// any attached analytics chart bundles).
    fn payload(
        &self,
        py: Python<'_>,
        property: Option<&str>,
        lines: Option<Vec<Vec<[f64; 2]>>>,
        charts: Vec<Value>,
    ) -> PyResult<String> {
        let lines = lines.unwrap_or_default();
        let model = self.inner.static_model();
        let wells = &self.wells;
        let summary = self.summary_value();
        let wells_logs = self.wells_logs.clone();
        let property = property.map(str::to_owned);
        // Compose the whole viewer document (map + volume + all sections) + serialize
        // OFF the GIL — the heaviest single GIL-holding path at 1M cells (S1).
        Ok(py.detach(|| {
            viewer::payload_json(
                model,
                "static",
                property.as_deref(),
                wells,
                &lines,
                true,
                summary,
                charts,
                wells_logs,
            )
        })?)
    }
}

/// Add machine-readable `property` + `zone` keys to each `zone_averages` entry of
/// a map bundle Value. petekStatic names each zone-average layer
/// `"<property>::<zone>"`; splitting it here (on the last `::`) lets a Python
/// consumer key on the zone directly instead of parsing the composite name. The
/// original `name` is preserved (additive). No-op if the shape is unexpected.
fn enrich_zone_average_keys(value: &mut Value) {
    let Some(layers) = value.get_mut("zone_averages").and_then(Value::as_array_mut) else {
        return;
    };
    for layer in layers {
        let Some(name) = layer.get("name").and_then(Value::as_str) else {
            continue;
        };
        let (property, zone) = match name.rsplit_once("::") {
            Some((p, z)) => (p.to_string(), z.to_string()),
            None => continue,
        };
        if let Some(obj) = layer.as_object_mut() {
            obj.insert("property".to_string(), Value::from(property));
            obj.insert("zone".to_string(), Value::from(zone));
        }
    }
}

/// Convert an optional list of chart-bundle dicts (from `*_bundle()`) into
/// `serde_json::Value`s for the payload.
pub(crate) fn charts_to_values(
    py: Python<'_>,
    charts: Option<Vec<Py<PyAny>>>,
) -> PyResult<Vec<Value>> {
    charts
        .unwrap_or_default()
        .iter()
        .map(|obj| viewer::py_to_json(py, obj.bind(py)))
        .collect()
}

fn parse_pick_spread(obj: Option<&Bound<'_, PyAny>>) -> PyResult<(Option<f64>, Option<f64>)> {
    // A bare pick-spread spreads the *lower* contact (FWL/OWC); the GOC stays at
    // its pick unless a per-contact dict gives it its own spread. This keeps the
    // two contacts from crossing under independent draws (a two-contact model
    // requires the GOC shallower than the OWC).
    let Some(o) = obj else {
        return Ok((None, None));
    };
    if let Ok(ps) = o.extract::<PickSpread>() {
        return Ok((Some(ps.sd_m), None));
    }
    if let Ok(sd) = o.extract::<f64>() {
        return Ok((Some(sd), None));
    }
    if let Ok(map) = o.cast::<PyDict>() {
        let mut lower = None;
        let mut goc = None;
        for (k, v) in map.iter() {
            let key = k.extract::<String>()?.to_ascii_lowercase();
            let sd = pick_sd(&v)?;
            match key.as_str() {
                "goc" => goc = Some(sd),
                _ => lower = Some(sd),
            }
        }
        return Ok((lower, goc));
    }
    Err(PyValueError::new_err(
        "contacts expects ps.pick_spread(sd_m=..), a float sd, or a dict",
    ))
}

fn pick_sd(v: &Bound<'_, PyAny>) -> PyResult<f64> {
    if let Ok(ps) = v.extract::<PickSpread>() {
        return Ok(ps.sd_m);
    }
    v.extract::<f64>()
        .map_err(|_| PyValueError::new_err("a contact spread must be a float or ps.pick_spread"))
}

/// Resolve the `ps.hz` per-row structural uncertainty (`(sd_m, (model, range) |
/// None)` in horizon order top→base) into `(top_field, per_zone_isochore_fields)`
/// for [`ZonedMcConfig`]. Row 0 → the top-surface depth field; row `k` (`k >= 1`)
/// → zone `k-1`'s isochore field. A `sd_m <= 0` row yields no field; a positive
/// `sd_m` requires a variogram (`ps.hz(sd=.., vgm=(model, range))`).
#[allow(clippy::type_complexity)]
fn build_structural(
    rows: Option<StructuralRows>,
) -> PyResult<(
    Option<peteksim::PerturbationField>,
    Vec<Option<peteksim::PerturbationField>>,
)> {
    let Some(rows) = rows else {
        return Ok((None, Vec::new()));
    };
    let field_of = |sd: f64,
                    vgm: &Option<(String, f64)>,
                    what: &str|
     -> PyResult<Option<peteksim::PerturbationField>> {
        if sd <= 0.0 {
            return Ok(None);
        }
        let (model, range) = vgm.as_ref().ok_or_else(|| {
            PyValueError::new_err(format!(
                "{what} structural uncertainty (sd={sd}) needs a variogram — \
                 ps.hz(sd=.., vgm=(model, range))"
            ))
        })?;
        let vm = parse_vgm_model(model)?;
        Ok(Some(
            peteksim::perturbation_field(sd, vm, *range).map_err(err)?,
        ))
    };
    let mut iter = rows.iter();
    let top = match iter.next() {
        Some((sd, vgm)) => field_of(*sd, vgm, "top-horizon")?,
        None => None,
    };
    let mut zones = Vec::new();
    for (sd, vgm) in iter {
        zones.push(field_of(*sd, vgm, "zone-isochore")?);
    }
    Ok((top, zones))
}

/// Parse a zoned-MC per-zone override map `{zone: {contacts, goc, porosity,
/// net_to_gross, water_saturation}}` into [`ZoneMcSpec`]s.
fn parse_zone_mc(map: &Bound<'_, PyDict>) -> PyResult<Vec<ZoneMcSpec>> {
    let mut out = Vec::with_capacity(map.len());
    for (k, v) in map.iter() {
        let zone: String = k.extract()?;
        let d = v.cast::<PyDict>().map_err(|_| {
            PyValueError::new_err(format!("zone '{zone}' MC override must be a dict"))
        })?;
        let sd = |key: &str| -> PyResult<Option<f64>> {
            match d.get_item(key)? {
                Some(o) if !o.is_none() => Ok(parse_pick_spread(Some(&o))?.0),
                _ => Ok(None),
            }
        };
        let dist = |key: &str| -> PyResult<Option<DistSpec>> {
            match d.get_item(key)? {
                Some(o) if !o.is_none() => Ok(Some(o.extract::<Dist>()?.spec)),
                _ => Ok(None),
            }
        };
        out.push(ZoneMcSpec {
            zone,
            contact_sd_m: sd("contacts")?,
            goc_sd_m: sd("goc")?,
            porosity: dist("porosity")?,
            net_to_gross: dist("net_to_gross")?,
            water_saturation: dist("water_saturation")?,
        });
    }
    Ok(out)
}
fn psummary_dict(py: Python<'_>, s: &peteksim::PSummary, samples: &[f64]) -> PyResult<Py<PyDict>> {
    let d = PyDict::new(py);
    d.set_item("p90", s.p90)?;
    d.set_item("p50", s.p50)?;
    d.set_item("p10", s.p10)?;
    d.set_item("mean", s.mean)?;
    d.set_item("p90_msm3", s.p90 / SM3_PER_MSM3)?;
    d.set_item("p50_msm3", s.p50 / SM3_PER_MSM3)?;
    d.set_item("p10_msm3", s.p10 / SM3_PER_MSM3)?;
    d.set_item("mean_msm3", s.mean / SM3_PER_MSM3)?;
    d.set_item("samples", samples.to_vec())?;
    Ok(d.unbind())
}

/// Build the `total` P-curve dict once (`{stoiip, giip, two_contact}`).
fn build_zoned_total_dict(py: Python<'_>, o: &ZonedMcOutcome) -> PyResult<Py<PyDict>> {
    let d = PyDict::new(py);
    d.set_item(
        "stoiip",
        psummary_dict(py, &o.total_stoiip, &o.total_stoiip_samples)?,
    )?;
    d.set_item(
        "giip",
        psummary_dict(py, &o.total_giip, &o.total_giip_samples)?,
    )?;
    d.set_item("two_contact", o.two_contact)?;
    Ok(d.unbind())
}

/// Build the per-zone P-curve list once (`[{zone, stoiip, giip, two_contact}]`).
fn build_zoned_zones_list(py: Python<'_>, o: &ZonedMcOutcome) -> PyResult<Py<PyList>> {
    let rows = PyList::empty(py);
    for z in &o.zones {
        let d = PyDict::new(py);
        d.set_item("zone", &z.zone)?;
        d.set_item("stoiip", psummary_dict(py, &z.stoiip, &z.stoiip_samples)?)?;
        d.set_item("giip", psummary_dict(py, &z.giip, &z.giip_samples)?)?;
        d.set_item("two_contact", z.two_contact)?;
        rows.append(d)?;
    }
    Ok(rows.unbind())
}
/// One per-zone (or total) volumes row as a dict (SI + report units).
fn zone_volume_dict(py: Python<'_>, zv: &peteksim::ZoneVolume) -> PyResult<Py<PyDict>> {
    let d = PyDict::new(py);
    d.set_item("zone", &zv.zone)?;
    d.set_item("grv_mcm", zv.grv_mcm)?;
    d.set_item("hcpv_m3", zv.hcpv_m3)?;
    d.set_item("stoiip_sm3", zv.stoiip_sm3)?;
    d.set_item("stoiip_msm3", zv.stoiip_sm3 / SM3_PER_MSM3)?;
    d.set_item("giip_sm3", zv.giip_sm3)?;
    d.set_item("giip_bcm", zv.giip_sm3 / SM3_PER_BCM)?;
    d.set_item("two_contact", zv.two_contact)?;
    Ok(d.unbind())
}
