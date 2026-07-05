//! The `Uncertainty` + `ZonedUncertainty` result pyclasses + the field roll-up builders.

use super::*;

/// The result of an uncertainty run.
#[pyclass(name = "Uncertainty")]
pub struct Uncertainty {
    pub(crate) inner: CoreMc,
    /// The STOIIP P-curve dict, built once at construction (L).
    pub(crate) stoiip: Py<PyDict>,
    /// The GIIP P-curve dict, built once at construction (L).
    pub(crate) giip: Py<PyDict>,
}

#[pymethods]
impl Uncertainty {
    /// The oil in-place (STOIIP) P-curve as a dict (Sm³ + MSm³ + samples). The
    /// dict is built once and this getter returns the same object each access.
    #[getter]
    fn stoiip(&self, py: Python<'_>) -> Py<PyDict> {
        self.stoiip.clone_ref(py)
    }
    /// The free-gas in-place (GIIP) P-curve as a dict (samples empty for a
    /// single-contact model). Built once; the same object is returned each access.
    #[getter]
    fn giip(&self, py: Python<'_>) -> Py<PyDict> {
        self.giip.clone_ref(py)
    }

    /// The ranked tornado bars (oil-in-place swing per input), as dicts.
    fn tornado(&self, py: Python<'_>) -> PyResult<Py<PyList>> {
        let rows = PyList::empty(py);
        for b in self.inner.tornado() {
            let d = PyDict::new(py);
            d.set_item("input", &b.input)?;
            d.set_item("lo_val", b.lo_val)?;
            d.set_item("hi_val", b.hi_val)?;
            d.set_item("out_lo", b.out_lo)?;
            d.set_item("out_hi", b.out_hi)?;
            d.set_item("swing", b.swing)?;
            rows.append(d)?;
        }
        Ok(rows.unbind())
    }

    /// A **tornado** chart bundle from the ranked bars — diverging nested bars
    /// around a base value, in display units. `base` defaults to the STOIIP P50 (the
    /// tornado's all-P50 working point); pass the deterministic value
    /// (`model.summary()["stoiip_msm3"]`) for the base-case anchor. The pivots are
    /// P90/P10-percentile inputs, so the payload carries the **inner** band only (no
    /// outer min/max span). Hand the dict to `model.view(charts=[...])`.
    #[pyo3(signature = (base=None, units="MSm³", fold_count=8))]
    fn tornado_bundle(
        &self,
        py: Python<'_>,
        base: Option<f64>,
        units: &str,
        fold_count: usize,
    ) -> PyResult<Py<PyAny>> {
        let value = self.tornado_value(base, units, Some(fold_count));
        viewer::to_py(py, &value)
    }

    /// A **distribution** chart bundle (histogram + exceedance CDF + P90/P50/P10)
    /// from the kept realization vectors. `gas=True` builds the GIIP panel in `bcm`;
    /// otherwise STOIIP in `MSm³`. Bins are computed here (deterministic 1/2/5·10ⁿ
    /// rule) and ship in the payload — the viewer only draws. `name` labels the
    /// series (default `"STOIIP"`/`"GIIP"`).
    #[pyo3(signature = (gas=false, name=None))]
    fn distribution_bundle(
        &self,
        py: Python<'_>,
        gas: bool,
        name: Option<String>,
    ) -> PyResult<Py<PyAny>> {
        let value = self.distribution_value(gas, name)?;
        viewer::to_py(py, &value)
    }

    /// Open a **pure-analytics** viewer session (no map/volume): the tornado + the
    /// STOIIP distribution by default, or the `charts` you pass. Non-blocking; the
    /// Charts tab renders, the geometry tabs show their empty state.
    #[pyo3(signature = (open_browser=true, port=0, block=false, charts=None))]
    fn view(
        &self,
        py: Python<'_>,
        open_browser: bool,
        port: u16,
        block: bool,
        charts: Option<Vec<Py<PyAny>>>,
    ) -> PyResult<String> {
        let json = self.charts_payload(py, charts)?;
        viewer::serve_charts(py, json, open_browser, port, block)
    }

    /// Write a **pure-analytics** self-contained HTML file (charts only).
    #[pyo3(signature = (path, charts=None))]
    fn save_view(
        &self,
        py: Python<'_>,
        path: &str,
        charts: Option<Vec<Py<PyAny>>>,
    ) -> PyResult<()> {
        let json = self.charts_payload(py, charts)?;
        viewer::save_view(py, path, json)
    }

    fn __repr__(&self) -> String {
        let s = &self.inner.stoiip;
        format!(
            "Uncertainty(STOIIP P90/P50/P10 = {:.3}/{:.3}/{:.3} MSm3)",
            s.p90 / SM3_PER_MSM3,
            s.p50 / SM3_PER_MSM3,
            s.p10 / SM3_PER_MSM3
        )
    }
}

impl Uncertainty {
    /// The tornado bundle as a `serde_json::Value` (shared by `tornado_bundle` +
    /// the default `view()`). `base` is in display units; `None` → STOIIP P50.
    fn tornado_value(&self, base: Option<f64>, units: &str, fold: Option<usize>) -> Value {
        let base = base.unwrap_or(self.inner.stoiip.p50 / SM3_PER_MSM3);
        let chart = tornado_chart(self.inner.tornado(), base, SM3_PER_MSM3, units, fold);
        serde_json::to_value(chart).unwrap_or(Value::Null)
    }

    /// The distribution bundle as a `serde_json::Value`. `gas` selects the GIIP leg
    /// (`bcm`) over STOIIP (`MSm³`).
    fn distribution_value(&self, gas: bool, name: Option<String>) -> PyResult<Value> {
        let (samples, ps, scale, units, default): (&[f64], _, f64, &str, &str) = if gas {
            (
                self.inner.giip_samples(),
                &self.inner.giip,
                SM3_PER_BCM,
                "bcm",
                "GIIP",
            )
        } else {
            (
                self.inner.stoiip_samples(),
                &self.inner.stoiip,
                SM3_PER_MSM3,
                "MSm³",
                "STOIIP",
            )
        };
        let series_name = name.unwrap_or_else(|| default.to_string());
        let mk = DistMarkers {
            p90: ps.p90,
            p50: ps.p50,
            p10: ps.p10,
        };
        let title = format!("{default} distribution");
        let panel = distribution_panel(&title, units, &[(series_name, samples, mk)], scale, 24);
        serde_json::to_value(panel).map_err(|e| PyValueError::new_err(e.to_string()))
    }

    /// Build a charts-only payload JSON: the given `charts`, else the default
    /// tornado + STOIIP distribution.
    fn charts_payload(&self, py: Python<'_>, charts: Option<Vec<Py<PyAny>>>) -> PyResult<String> {
        let values = match charts {
            Some(list) => charts_to_values(py, Some(list))?,
            None => vec![
                self.tornado_value(None, "MSm³", Some(8)),
                self.distribution_value(false, None)?,
            ],
        };
        Ok(viewer::charts_payload_json("mc", values, None)?)
    }
}
/// The result of a zoned uncertainty run — per-zone + total STOIIP/GIIP P-curves.
#[pyclass(name = "ZonedUncertainty")]
pub struct ZonedUncertainty {
    pub(crate) inner: ZonedMcOutcome,
    /// The TOTAL P-curve dict, built once at construction (L).
    pub(crate) total: Py<PyDict>,
    /// The per-zone P-curve list, built once at construction (L).
    pub(crate) zones: Py<PyList>,
}

#[pymethods]
impl ZonedUncertainty {
    /// The field TOTAL P-curves: `{"stoiip": {..}, "giip": {..}, "two_contact": bool}`.
    /// `total.stoiip == Σ zone.stoiip` per draw (conservation). Built once; the
    /// same object is returned each access.
    #[getter]
    fn total(&self, py: Python<'_>) -> Py<PyDict> {
        self.total.clone_ref(py)
    }

    /// Per-zone P-curves: a list of `{"zone", "stoiip": {..}, "giip": {..},
    /// "two_contact"}` (top→base). A contactless zone's STOIIP/GIIP stay zero.
    /// Built once; the same object is returned each access.
    #[getter]
    fn zones(&self, py: Python<'_>) -> Py<PyList> {
        self.zones.clone_ref(py)
    }

    /// A **distribution** chart bundle for the field total (or a named `zone`) —
    /// histogram + exceedance CDF + P90/P50/P10 markers from the kept realization
    /// vectors, in `MSm³` (oil) / `bcm` (gas). Hand it to `model.view(charts=[..])`.
    #[pyo3(signature = (gas=false, zone=None, name=None))]
    fn distribution_bundle(
        &self,
        py: Python<'_>,
        gas: bool,
        zone: Option<String>,
        name: Option<String>,
    ) -> PyResult<Py<PyAny>> {
        let (samples, ps): (&[f64], &peteksim::PSummary) = match &zone {
            Some(zn) => {
                let z = self
                    .inner
                    .zones
                    .iter()
                    .find(|z| &z.zone == zn)
                    .ok_or_else(|| {
                        PyValueError::new_err(format!("no zone '{zn}' in the zoned result"))
                    })?;
                if gas {
                    (&z.giip_samples, &z.giip)
                } else {
                    (&z.stoiip_samples, &z.stoiip)
                }
            }
            None => {
                if gas {
                    (&self.inner.total_giip_samples, &self.inner.total_giip)
                } else {
                    (&self.inner.total_stoiip_samples, &self.inner.total_stoiip)
                }
            }
        };
        let (scale, units, default) = if gas {
            (SM3_PER_BCM, "bcm", "GIIP")
        } else {
            (SM3_PER_MSM3, "MSm³", "STOIIP")
        };
        let series_name = name.unwrap_or_else(|| {
            zone.map(|z| format!("{z} {default}"))
                .unwrap_or_else(|| default.to_string())
        });
        let mk = DistMarkers {
            p90: ps.p90,
            p50: ps.p50,
            p10: ps.p10,
        };
        let title = format!("{default} distribution");
        let panel = distribution_panel(&title, units, &[(series_name, samples, mk)], scale, 24);
        let value =
            serde_json::to_value(panel).map_err(|e| PyValueError::new_err(e.to_string()))?;
        viewer::to_py(py, &value)
    }

    fn __repr__(&self) -> String {
        let t = &self.inner.total_stoiip;
        format!(
            "ZonedUncertainty({} zones; total STOIIP P90/P50/P10 = {:.3}/{:.3}/{:.3} MSm3)",
            self.inner.zones.len(),
            t.p90 / SM3_PER_MSM3,
            t.p50 / SM3_PER_MSM3,
            t.p10 / SM3_PER_MSM3
        )
    }
}
/// `ps.aggregate([mc, ...], correlation="independent")` — a field roll-up.
#[pyfunction]
#[pyo3(signature = (segments, correlation="independent"))]
pub fn aggregate(
    py: Python<'_>,
    segments: Vec<Py<Uncertainty>>,
    correlation: &str,
) -> PyResult<Py<PyDict>> {
    let corr = match correlation.to_ascii_lowercase().as_str() {
        "independent" => Correlation::Independent,
        "comonotonic" | "perfect" => Correlation::Comonotonic,
        other => {
            return Err(PyValueError::new_err(format!(
                "correlation must be independent/comonotonic, got '{other}'"
            )))
        }
    };
    let borrows: Vec<PyRef<'_, Uncertainty>> =
        segments.iter().map(|s| s.bind(py).borrow()).collect();
    let refs: Vec<&CoreMc> = borrows.iter().map(|b| &b.inner).collect();
    let field = core_aggregate(&refs, corr).map_err(err)?;
    let d = PyDict::new(py);
    d.set_item("p90", field.p90)?;
    d.set_item("p50", field.p50)?;
    d.set_item("p10", field.p10)?;
    d.set_item("mean", field.mean)?;
    d.set_item("p90_msm3", field.p90 / SM3_PER_MSM3)?;
    d.set_item("p50_msm3", field.p50 / SM3_PER_MSM3)?;
    d.set_item("p10_msm3", field.p10 / SM3_PER_MSM3)?;
    d.set_item("samples", field.samples)?;
    Ok(d.unbind())
}

/// `ps.distribution_bundle([mc, ...], aggregate=field, names=[...], gas=False)` —
/// a **multi-series** distribution panel: one overlaid series per segment plus, if
/// an `aggregate` dict (from `ps.aggregate`) is given, a `"Field"` series from its
/// realization samples. Per-structure + field-aggregate in one histogram/CDF, in
/// `MSm³` (oil) / `bcm` (gas). Binning is deterministic (the plumbing's nice-bin
/// rule); the viewer only draws.
#[pyfunction]
#[pyo3(signature = (segments, aggregate=None, names=None, gas=false, title=None))]
pub fn distribution_bundle(
    py: Python<'_>,
    segments: Vec<Py<Uncertainty>>,
    aggregate: Option<&Bound<'_, PyDict>>,
    names: Option<Vec<String>>,
    gas: bool,
    title: Option<String>,
) -> PyResult<Py<PyAny>> {
    let (scale, units, metric) = if gas {
        (SM3_PER_BCM, "bcm", "GIIP")
    } else {
        (SM3_PER_MSM3, "MSm³", "STOIIP")
    };
    // Own each segment's samples so the panel builder can borrow slices of them.
    let borrows: Vec<PyRef<'_, Uncertainty>> =
        segments.iter().map(|s| s.bind(py).borrow()).collect();
    let mut owned: Vec<(String, Vec<f64>, DistMarkers)> = Vec::new();
    for (i, seg) in borrows.iter().enumerate() {
        let name = names
            .as_ref()
            .and_then(|n| n.get(i).cloned())
            .unwrap_or_else(|| format!("Segment {}", i + 1));
        let (samples, ps) = if gas {
            (seg.inner.giip_samples(), &seg.inner.giip)
        } else {
            (seg.inner.stoiip_samples(), &seg.inner.stoiip)
        };
        owned.push((
            name,
            samples.to_vec(),
            DistMarkers {
                p90: ps.p90,
                p50: ps.p50,
                p10: ps.p10,
            },
        ));
    }
    // The optional field-aggregate overlay (its own kept samples + P-markers).
    if let Some(agg) = aggregate {
        let get = |k: &str| -> PyResult<f64> {
            agg.get_item(k)?
                .ok_or_else(|| PyValueError::new_err(format!("aggregate dict missing '{k}'")))?
                .extract::<f64>()
        };
        let samples: Vec<f64> = agg
            .get_item("samples")?
            .ok_or_else(|| PyValueError::new_err("aggregate dict missing 'samples'"))?
            .extract()?;
        owned.push((
            "Field".to_string(),
            samples,
            DistMarkers {
                p90: get("p90")?,
                p50: get("p50")?,
                p10: get("p10")?,
            },
        ));
    }
    let series: Vec<(String, &[f64], DistMarkers)> = owned
        .iter()
        .map(|(n, s, m)| (n.clone(), s.as_slice(), *m))
        .collect();
    let title = title.unwrap_or_else(|| format!("{metric} distribution"));
    let panel = distribution_panel(&title, units, &series, scale, 24);
    let value = serde_json::to_value(panel).map_err(|e| PyValueError::new_err(e.to_string()))?;
    viewer::to_py(py, &value)
}
