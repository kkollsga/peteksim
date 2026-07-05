//! The viewer payload — the single JSON document the packaged viewer renders.
//!
//! Strictly **bundle-driven**: every tab reads typed layers/columns/mesh/marks
//! straight off this JSON (names, units and value ranges included), so the viewer
//! JS carries no domain knowledge. This module only *composes* petekStatic's
//! typed bundles ([`VolumeBundle`] / [`MapBundle`] / [`IntersectionBundle`]) plus
//! well tracks and an optional summary into that document, and marshals bundles
//! to Python dicts. All compute lives in the bundles; nothing here renders.

use peteksim::{
    IntersectionBundle, MapBundle, MapSpec, SectionSpec, StaticModel, VolumeBundle, WellLogBundle,
    DEFAULT_VIEW_PROPERTY,
};
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use serde::Serialize;
use serde_json::Value;

/// A viewer payload-assembly error, carried as a plain message string.
///
/// Payload assembly (mesh + bundle build + serialize) runs **off the GIL** via
/// [`Python::detach`], so its error type must be GIL-independent (`Ungil`) — a
/// `PyErr` is not. `ViewerError` holds only the message; it converts to a Python
/// `ValueError` at the boundary (the `?` in a `PyResult` function), so the
/// user-facing messages are unchanged.
#[derive(Debug, Clone)]
pub struct ViewerError(String);

impl ViewerError {
    /// From any `Display` error (a petekStatic `StaticError`, a `serde_json`
    /// error, …) — stringified without the GIL (pure-Rust `Display`).
    fn from_display<E: std::fmt::Display>(e: E) -> Self {
        ViewerError(e.to_string())
    }
    /// From a fixed/validation message.
    fn msg(s: impl Into<String>) -> Self {
        ViewerError(s.into())
    }
}

impl From<ViewerError> for PyErr {
    fn from(e: ViewerError) -> PyErr {
        PyValueError::new_err(e.0)
    }
}

/// Result of an off-GIL payload-assembly step (see [`ViewerError`]).
pub type VResult<T> = Result<T, ViewerError>;

/// The top-level render-payload contract version (petekTools viewer SCHEMA.md).
/// Bumped to **2** when the additive `charts` bundle landed; the per-bundle
/// (volume/map/section) `SCHEMA_VERSION` is petekStatic's cross-repo contract and
/// stays at 1.
pub const PAYLOAD_SCHEMA_VERSION: u32 = 2;

/// One well↔horizon surface-tie residual carried into the wells payload (viewer
/// well hover + the layer panel per-bore entries).
#[derive(Debug, Clone, Serialize)]
pub struct WellTie {
    pub horizon: String,
    /// `pick − surface` mis-tie the hard control corrects \[m, positive-down\].
    pub residual_m: f64,
}

/// A positioned well: id, surface `(x, y)`, its `[x, y, tvd]` path, and any
/// per-horizon surface-tie residuals (the viewer draws the marker + offers a
/// click-to-section along the bore, and shows the ties on hover).
#[derive(Debug, Clone, Serialize)]
pub struct WellTrack {
    pub id: String,
    pub x: f64,
    pub y: f64,
    pub trajectory: Vec<[f64; 3]>,
    /// Per-horizon tie residuals (empty unless the framework tied this well).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub ties: Vec<WellTie>,
}

impl WellTrack {
    /// Build from an id + sampled `[x, y, tvd]` stations (surface = first station).
    pub fn new(id: String, trajectory: Vec<[f64; 3]>) -> Self {
        let (x, y) = trajectory.first().map_or((0.0, 0.0), |p| (p[0], p[1]));
        Self {
            id,
            x,
            y,
            trajectory,
            ties: Vec::new(),
        }
    }
}

/// The full viewer document: metadata + the three bundles + well tracks + an
/// optional headline summary. Serialized to `model.json` (served) or inlined
/// (single-file export).
#[derive(Debug, Clone, Serialize)]
pub struct ViewerPayload {
    pub schema_version: u32,
    /// Provenance origin (`box` / `refined` / `static`) — display metadata only.
    pub kind: String,
    /// The active render property (colours the volume; the map's default layer).
    pub property: String,
    /// Every populated cube the viewer's property picker can offer.
    pub properties: Vec<String>,
    /// A free-form headline summary (volumetrics / P-curve) — display only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<Value>,
    /// The 3-D volume as petekStatic's **v3 self-contained wire envelope**
    /// (`{schema_version, kind:"volume", …, encoding:"base64", blocks:{…}}`), NOT
    /// the raw [`VolumeBundle`]. The petekTools viewer routes any
    /// `schema_version>=3` volume to its binary-block decode kernel (`env.blocks`);
    /// the serde derive's inline `positions`/`indices` arrays carry no `blocks`, so
    /// they crash the decoder. Built via [`volume_envelope_value`]; every peteksim
    /// emission path (save/serve/`/volume`/dict) routes through that wire.
    pub volume: Value,
    pub map: MapBundle,
    /// Pre-computed sections (well bores + any user-supplied lines).
    pub sections: Vec<IntersectionBundle>,
    /// A display label per `sections` entry (a well id for a bore, `"Line N"` for
    /// a user line) — viewer metadata, so the section picker and file-mode
    /// click-a-well can name and resolve a pre-computed section.
    pub section_labels: Vec<String>,
    pub wells: Vec<WellTrack>,
    /// The **Wells** tab bundle (`wells_logs`, v4): per-bore raw + upscaled log
    /// tracks, framework tops/zones, and tie residuals — present only in a model
    /// context with attached, curve-carrying bores (absent otherwise).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wells_logs: Option<WellLogBundle>,
    /// Analytics chart marks (tornado / scatter / distribution). Attached when MC
    /// results or logs are handed to `view()` / `save_view(charts=...)`; each is a
    /// render-only bundle per the viewer's `charts` contract.
    pub charts: Vec<Value>,
}

/// A charts-only payload — a pure-analytics session (`mc.view()`), no map/volume.
/// The viewer renders its Charts tab and shows the empty state on the others.
#[derive(Debug, Clone, Serialize)]
pub struct ChartsPayload {
    pub schema_version: u32,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<Value>,
    pub charts: Vec<Value>,
}

/// A section request: a world polyline, or a named well's bore.
pub enum SectionRequest {
    Line(Vec<[f64; 2]>),
    Well(String),
}

/// Build the areal [`MapBundle`], carrying zone-average maps (and optional
/// k-slices) for `properties`.
pub fn build_map(
    model: &StaticModel,
    properties: &[String],
    k_slice: Option<usize>,
) -> VResult<MapBundle> {
    let mut spec = MapSpec::new();
    for p in properties {
        spec = spec.property(p.clone());
    }
    if let Some(k) = k_slice {
        spec = spec.k_slice(k);
    }
    let mut map = model.map_bundle(&spec).map_err(ViewerError::from_display)?;
    // A zoned model can carry several contacts of the same petekStatic kind — a
    // two-contact zone's lower contact collapses to `OWC` (ContactKind has no FWL),
    // so a stack with more than one hydrocarbon zone emits e.g. [OWC, GOC, OWC].
    // Qualify the repeats so the viewer legend shows one row per distinct contact
    // instead of duplicate "OWC" entries.
    qualify_duplicate_contacts(map.contacts.iter_mut().map(|c| (&mut c.kind, c.depth_m)));
    Ok(map)
}

/// Disambiguate contacts that share a `kind` within one bundle by appending a
/// 1-based ordinal (in depth order) to each repeated kind — `OWC` → `OWC (1)` /
/// `OWC (2)`. A kind that appears once is left untouched (the common single-contact
/// case stays a clean `OWC`). Section and map bundles both read the same
/// `model.contacts()` set, so applying the identical scheme to each keeps the
/// legend labels + identity colours consistent across the map and section tabs.
fn qualify_duplicate_contacts<'a, I>(contacts: I)
where
    I: Iterator<Item = (&'a mut String, f64)>,
{
    use std::collections::HashMap;
    // Collect (kind, depth) with a stable index so we can rewrite in place.
    let mut items: Vec<(&'a mut String, f64)> = contacts.collect();
    let mut counts: HashMap<String, usize> = HashMap::new();
    for (kind, _) in &items {
        *counts.entry((*kind).clone()).or_default() += 1;
    }
    // Order the duplicated kinds by depth so the ordinal is deterministic.
    let mut order: Vec<usize> = (0..items.len()).collect();
    order.sort_by(|&a, &b| {
        items[a]
            .1
            .partial_cmp(&items[b].1)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut seen: HashMap<String, usize> = HashMap::new();
    for idx in order {
        let base = items[idx].0.clone();
        if counts.get(&base).copied().unwrap_or(0) > 1 {
            let n = seen.entry(base.clone()).or_insert(0);
            *n += 1;
            *items[idx].0 = format!("{base} ({n})");
        }
    }
}

/// Build one section [`IntersectionBundle`] for `req`, coloured by `property`.
/// A `Well` request resolves its trajectory from `wells`.
pub fn build_section(
    model: &StaticModel,
    req: &SectionRequest,
    property: Option<&str>,
    wells: &[WellTrack],
) -> VResult<IntersectionBundle> {
    let spec = match req {
        SectionRequest::Line(pts) => SectionSpec::Polyline(pts.clone()),
        SectionRequest::Well(id) => {
            let w = wells.iter().find(|w| &w.id == id).ok_or_else(|| {
                ViewerError::msg(format!(
                    "intersection_bundle: no well '{id}' attached to the model \
                     (attach wells via grid.model(..., wells=proj.wells()))"
                ))
            })?;
            if w.trajectory.len() < 2 {
                return Err(ViewerError::msg(format!(
                    "intersection_bundle: well '{id}' has no positioned trajectory"
                )));
            }
            SectionSpec::AlongBore {
                trajectory: w.trajectory.clone(),
            }
        }
    };
    // Colour by the request property, else the canonical porosity cube, else the
    // first populated cube — so a fence/well section is never geometry-only.
    let resolved = resolve_property(model, property);
    let mut section = model
        .intersection_bundle(&spec, resolved.as_deref())
        .map_err(ViewerError::from_display)?;
    // Same duplicate-kind qualification the map bundle applies, over the same
    // `model.contacts()` set, so the section legend + contact colours match the map.
    qualify_duplicate_contacts(
        section
            .contacts
            .iter_mut()
            .map(|c| (&mut c.kind, c.depth_m)),
    );
    Ok(section)
}

/// The render property for a section/volume: the request if populated, else the
/// canonical porosity cube, else the first populated cube.
pub fn resolve_property(model: &StaticModel, property: Option<&str>) -> Option<String> {
    let names = model.property_names();
    property
        .filter(|p| names.iter().any(|q| q == p))
        .map(String::from)
        .or_else(|| {
            names
                .iter()
                .find(|p| **p == DEFAULT_VIEW_PROPERTY)
                .map(|s| s.to_string())
        })
        .or_else(|| names.first().map(|s| s.to_string()))
}

/// Build the 3-D [`VolumeBundle`] coloured by `property`.
pub fn build_volume(model: &StaticModel, property: &str) -> VResult<VolumeBundle> {
    model
        .volume_bundle(property)
        .map_err(ViewerError::from_display)
}

/// Encode a [`VolumeBundle`] as petekStatic's **v3 self-contained** wire envelope
/// (`{schema_version, kind:"volume", …, encoding:"base64", blocks:{…}}`) as a
/// `serde_json::Value` — the ONLY shape the petekTools viewer decode kernel
/// accepts. The `serde` derive on `VolumeBundle` is a Rust-side round-trip
/// convenience (inline `positions`/`indices`, no `blocks`); the viewer routes any
/// `schema_version>=3` volume to its binary-block decoder (`env.blocks`), so the
/// derive crashes it (`Cannot read properties of undefined (reading 'positions')`).
/// **Every** peteksim volume-emission path serializes through here (the payload's
/// `volume` field, the live `/volume` re-cut endpoint, and the dict-returning
/// `volume_bundle`). Self-contained base64 (not sidecar): the payload is one
/// `model.json` with the volume inlined — no companion `model.bin` is served.
pub fn volume_envelope_value(bundle: &VolumeBundle) -> VResult<Value> {
    let mut buf = Vec::new();
    bundle
        .write_self_contained(&mut buf)
        .map_err(ViewerError::from_display)?;
    serde_json::from_slice(&buf).map_err(ViewerError::from_display)
}

/// The [`volume_envelope_value`] envelope as a JSON string — the live `/volume`
/// endpoint form (mirrors [`section_json`]; the writer emits UTF-8 JSON).
pub fn volume_envelope_json(bundle: &VolumeBundle) -> VResult<String> {
    let mut buf = Vec::new();
    bundle
        .write_self_contained(&mut buf)
        .map_err(ViewerError::from_display)?;
    String::from_utf8(buf).map_err(ViewerError::from_display)
}

/// One thresholded volume re-cut as a JSON string, for the live `/volume`
/// endpoint (mirrors [`section_json`]). `cutoff = None` returns the full-set shell
/// (identical to the payload's volume); `Some(c)` re-cuts the exterior shell to
/// the cells whose value is `>= c` (`keep_above`) or `<= c` — so newly exposed
/// interior faces appear. Same v3 exterior-shell bundle shape the payload carries.
pub fn volume_json(
    model: &StaticModel,
    property: Option<&str>,
    cutoff: Option<f64>,
    keep_above: bool,
) -> VResult<String> {
    let prop = resolve_property(model, property)
        .ok_or_else(|| ViewerError::msg("model carries no populated property cubes"))?;
    let bundle = match cutoff {
        Some(c) => model
            .volume_bundle_thresholded(&prop, c, keep_above)
            .map_err(ViewerError::from_display)?,
        None => build_volume(model, &prop)?,
    };
    // The v3 block envelope — NOT the serde derive: the viewer feeds this straight
    // into `App.payload.volume` and decodes it through the same `env.blocks` kernel.
    volume_envelope_json(&bundle)
}

/// Compose the full [`ViewerPayload`]: volume + all-property map + the requested
/// sections (well bores + user lines) + well tracks + an optional summary.
#[allow(clippy::too_many_arguments)]
pub fn build_payload(
    model: &StaticModel,
    kind: &str,
    property: Option<&str>,
    wells: &[WellTrack],
    line_sections: &[Vec<[f64; 2]>],
    include_well_sections: bool,
    summary: Option<Value>,
    charts: Vec<Value>,
    wells_logs: Option<WellLogBundle>,
) -> VResult<ViewerPayload> {
    let properties: Vec<String> = model
        .property_names()
        .iter()
        .map(|s| s.to_string())
        .collect();
    // Resolve the render property: the request if present + populated, else the
    // canonical porosity cube, else the first populated cube.
    let property = property
        .filter(|p| properties.iter().any(|q| q == p))
        .map(String::from)
        .or_else(|| {
            properties
                .iter()
                .find(|p| p.as_str() == DEFAULT_VIEW_PROPERTY)
                .cloned()
        })
        .or_else(|| properties.first().cloned())
        .ok_or_else(|| ViewerError::msg("model carries no populated property cubes"))?;

    // Encode the volume as the v3 block wire envelope (base64 blocks) — the shape
    // the petekTools viewer decode kernel requires; the serde derive's inline
    // arrays crash it (`env.blocks` undefined) on `schema_version>=3`.
    let volume = volume_envelope_value(&build_volume(model, &property)?)?;
    let map = build_map(model, &properties, None)?;

    let mut sections = Vec::new();
    let mut section_labels = Vec::new();
    if include_well_sections {
        for w in wells {
            if w.trajectory.len() >= 2 {
                sections.push(build_section(
                    model,
                    &SectionRequest::Well(w.id.clone()),
                    Some(&property),
                    wells,
                )?);
                section_labels.push(w.id.clone());
            }
        }
    }
    for (n, line) in line_sections.iter().enumerate() {
        sections.push(build_section(
            model,
            &SectionRequest::Line(line.clone()),
            Some(&property),
            wells,
        )?);
        section_labels.push(format!("Line {}", n + 1));
    }

    Ok(ViewerPayload {
        schema_version: PAYLOAD_SCHEMA_VERSION,
        kind: kind.to_string(),
        property,
        properties,
        summary,
        volume,
        map,
        sections,
        section_labels,
        wells: wells.to_vec(),
        wells_logs,
        charts,
    })
}

/// Serialize any `Serialize` value to a compact JSON string. Non-finite floats
/// serialize to `null` (serde_json's convention) — the inactive-cell contract
/// the viewer + bundle consumers rely on.
pub fn to_json<T: Serialize>(v: &T) -> VResult<String> {
    serde_json::to_string(v).map_err(ViewerError::from_display)
}

/// Build the full viewer document as a JSON string (for `view` / `save_view`).
#[allow(clippy::too_many_arguments)]
pub fn payload_json(
    model: &StaticModel,
    kind: &str,
    property: Option<&str>,
    wells: &[WellTrack],
    lines: &[Vec<[f64; 2]>],
    include_well_sections: bool,
    summary: Option<Value>,
    charts: Vec<Value>,
    wells_logs: Option<WellLogBundle>,
) -> VResult<String> {
    to_json(&build_payload(
        model,
        kind,
        property,
        wells,
        lines,
        include_well_sections,
        summary,
        charts,
        wells_logs,
    )?)
}

/// A charts-only payload as JSON (`mc.view()` / `mc.save_view()`), no model.
pub fn charts_payload_json(
    kind: &str,
    charts: Vec<Value>,
    summary: Option<Value>,
) -> VResult<String> {
    to_json(&ChartsPayload {
        schema_version: PAYLOAD_SCHEMA_VERSION,
        kind: kind.to_string(),
        summary,
        charts,
    })
}

/// Convert a Python object (a chart-bundle dict from `*_bundle()`) into a
/// `serde_json::Value` via the stdlib `json` module — robust for arbitrary nested
/// dict/list/scalar payloads the user composed.
pub fn py_to_json(py: Python<'_>, obj: &Bound<'_, PyAny>) -> PyResult<Value> {
    let json = py.import("json")?;
    let s: String = json.call_method1("dumps", (obj,))?.extract()?;
    serde_json::from_str(&s).map_err(|e| PyValueError::new_err(e.to_string()))
}

/// Serve a charts-only payload (no `/section` provider) via the Python helper.
pub fn serve_charts(
    py: Python<'_>,
    payload_json: String,
    open_browser: bool,
    port: u16,
    block: bool,
) -> PyResult<String> {
    let pkg = py.import("peteksim")?;
    let url = pkg.call_method1("_serve_charts", (payload_json, open_browser, port, block))?;
    url.extract::<String>()
}

/// Build one section as a JSON string for a live fence/well request.
pub fn section_json(
    model: &StaticModel,
    property: Option<&str>,
    line: Option<Vec<[f64; 2]>>,
    well: Option<String>,
    wells: &[WellTrack],
) -> VResult<String> {
    let req = section_request(line, well)?;
    to_json(&build_section(model, &req, property, wells)?)
}

/// Build one section as a `serde_json::Value` for a live fence/well request —
/// the dict-returning `intersection_bundle` path (serialized off the GIL, then
/// [`json_to_py`] under the GIL). Mirrors [`section_json`] but skips the JSON
/// string, so no reparse is needed to hand back a Python dict.
pub fn section_value(
    model: &StaticModel,
    property: Option<&str>,
    line: Option<Vec<[f64; 2]>>,
    well: Option<String>,
    wells: &[WellTrack],
) -> VResult<Value> {
    let req = section_request(line, well)?;
    to_value(&build_section(model, &req, property, wells)?)
}

/// Resolve a `(line, well)` pair into a [`SectionRequest`] (exactly one required).
fn section_request(line: Option<Vec<[f64; 2]>>, well: Option<String>) -> VResult<SectionRequest> {
    match (line, well) {
        (Some(l), _) => Ok(SectionRequest::Line(l)),
        (None, Some(w)) => Ok(SectionRequest::Well(w)),
        (None, None) => Err(ViewerError::msg(
            "a section needs either a `line=[[x,y],..]` or a `well=<id>`",
        )),
    }
}

/// Hand the payload to the Python `peteksim._serve` helper, passing `model_obj`
/// so the local server can compute live sections on demand. Non-blocking unless
/// `block`.
pub fn serve(
    py: Python<'_>,
    model_obj: &Bound<'_, PyAny>,
    payload_json: String,
    open_browser: bool,
    port: u16,
    block: bool,
) -> PyResult<String> {
    let pkg = py.import("peteksim")?;
    let url = pkg.call_method1(
        "_serve",
        (model_obj, payload_json, open_browser, port, block),
    )?;
    url.extract::<String>()
}

/// Hand the payload to the Python `peteksim._save_view` helper — one
/// self-contained HTML file with everything inlined.
pub fn save_view(py: Python<'_>, path: &str, payload_json: String) -> PyResult<()> {
    let pkg = py.import("peteksim")?;
    pkg.call_method1("_save_view", (path, payload_json))?;
    Ok(())
}

/// Serialize any `Serialize` value to a `serde_json::Value` — the GIL-free half
/// of the bundle→Python conversion. Non-finite floats become `null` (serde_json's
/// convention) — the inactive-cell contract [`json_to_py`] then maps to `None`.
///
/// Building the Python object tree ([`json_to_py`]) fundamentally needs the GIL;
/// this step (which does the heavy walk of the bundle) does **not**, so the big
/// map/volume/section methods run it inside [`Python::detach`] and only hold the
/// GIL for the final [`json_to_py`].
pub fn to_value<T: Serialize>(v: &T) -> VResult<Value> {
    serde_json::to_value(v).map_err(ViewerError::from_display)
}

/// Serialize any `Serialize` value straight to a Python object (dict/list/scalar).
/// The one-shot form for the small chart/well bundles (both halves under the GIL);
/// the heavy bundles split it — [`to_value`] off the GIL, then [`json_to_py`].
pub fn to_py<T: Serialize>(py: Python<'_>, v: &T) -> PyResult<Py<PyAny>> {
    json_to_py(py, &to_value(v)?)
}

/// Recursively convert a `serde_json::Value` into the matching Python object.
/// `null` → `None`, numbers → int/float, arrays → list, objects → dict. Building
/// Python objects requires the GIL, so this is the GIL-held tail of the
/// conversion (the fastest converter measured — a direct `Value`→`Py` walk beats
/// re-parsing a JSON string through `json.loads` for the structured map bundle).
pub fn json_to_py(py: Python<'_>, v: &Value) -> PyResult<Py<PyAny>> {
    Ok(match v {
        Value::Null => py.None(),
        Value::Bool(b) => b.into_pyobject(py)?.to_owned().into_any().unbind(),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                i.into_pyobject(py)?.into_any().unbind()
            } else if let Some(u) = n.as_u64() {
                u.into_pyobject(py)?.into_any().unbind()
            } else {
                n.as_f64()
                    .unwrap_or(f64::NAN)
                    .into_pyobject(py)?
                    .into_any()
                    .unbind()
            }
        }
        Value::String(s) => s.into_pyobject(py)?.into_any().unbind(),
        Value::Array(a) => {
            let list = pyo3::types::PyList::empty(py);
            for item in a {
                list.append(json_to_py(py, item)?)?;
            }
            list.into_any().unbind()
        }
        Value::Object(o) => {
            let dict = pyo3::types::PyDict::new(py);
            for (k, val) in o {
                dict.set_item(k, json_to_py(py, val)?)?;
            }
            dict.into_any().unbind()
        }
    })
}
