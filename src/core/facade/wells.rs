//! `WellLogBundle` — the **Wells** tab payload (viewer `kind: "wells_logs"`,
//! `schema_version` 4), assembled from a LOADED PROJECT + its populated model.
//!
//! This is peteksim's producer slice of the well-correlation seam
//! (`petekSuite/dev-docs/designs/well-log-bundle-seam.md`, codified in
//! `petektools/viewer/SCHEMA.md`). It turns each positioned bore's **raw** logs
//! plus the model's **upscaled** cubes (sampled back along the bore), the model
//! framework's per-bore **tops/zones**, and the engine **tie residuals**
//! (`Provenance.well_ties`, surfaced by [`Model::well_tie_residuals`]) into the
//! documented wire — N wells side-by-side on a shared inverted depth axis.
//!
//! Per the coupling rule the wire is kept **byte-consistent** with the reference
//! fixture: every numeric lane (`md_m` / `tvd_m` / curve `values`) is a v3-style
//! little-endian `f32` block, base64-wrapped, with `NaN` = the canonical
//! `0x7FC00000` — the same block shape the volume decode kernel already reads.

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use serde::Serialize;
use std::collections::BTreeMap;

use crate::core::facade::model::Model;
use crate::core::facade::project::BoreWell;
use crate::core::view::SectionSpec;

/// The wells bundle wire version (the `wells_logs` kind under the family v4).
const WELLS_SCHEMA_VERSION: u32 = 4;
/// Canonical quiet-NaN `f32` bit pattern (matches the engine + viewer decode).
const NAN_F32_BITS: u32 = 0x7FC0_0000;
/// The PHIE net cutoff drawn as the reservoir cutoff line + fill. Producer
/// default (petekStatic net-conditioning convention); caller-overridable.
const DEFAULT_PHIE_CUTOFF: f64 = 0.10;

/// The canonical curves peteksim surfaces on the Wells tab: (source log mnemonic,
/// display name, header hi–lo scale). PORO carries the PHIE cutoff.
const RAW_CURVES: &[(&str, &str, f64, f64)] = &[
    ("PORO", "Effective porosity", 0.0, 0.35),
    ("NTG", "Net-to-gross", 0.0, 1.0),
    ("SW", "Water saturation", 0.0, 1.0),
];

/// One v3-style base64 `f32` lane block `{dtype, shape, data}`.
#[derive(Debug, Clone, Serialize)]
pub struct Lane {
    pub dtype: &'static str,
    pub shape: [usize; 1],
    pub data: String,
}

/// A `{min, max}` fixing a continuous track's hi–lo header scale.
#[derive(Debug, Clone, Serialize)]
pub struct Range {
    pub min: f64,
    pub max: f64,
}

/// One curve track: a continuous polyline or a categorical flag strip.
#[derive(Debug, Clone, Serialize)]
pub struct Curve {
    pub mnemonic: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    pub unit: String,
    /// `"continuous"` (polyline) or `"flag"` (net/facies strip).
    pub kind: &'static str,
    pub values: Lane,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub range: Option<Range>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cutoff: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub codes: Option<BTreeMap<String, String>>,
}

/// A framework horizon pick on a bore (top→down).
#[derive(Debug, Clone, Serialize)]
pub struct Top {
    pub horizon: String,
    pub tvd_m: f64,
}

/// A zone interval on a bore (the band the viewer shades).
#[derive(Debug, Clone, Serialize)]
pub struct Zone {
    pub name: String,
    pub top_tvd_m: f64,
    pub base_tvd_m: f64,
}

/// A per-horizon surface-tie residual on a bore.
#[derive(Debug, Clone, Serialize)]
pub struct Tie {
    pub horizon: String,
    pub residual_m: f64,
}

/// One bore's correlation-view payload.
#[derive(Debug, Clone, Serialize)]
pub struct LogWell {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    pub x: f64,
    pub y: f64,
    pub datum_m: f64,
    pub md_m: Lane,
    pub tvd_m: Lane,
    pub curves: Vec<Curve>,
    pub tops: Vec<Top>,
    pub zones: Vec<Zone>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub ties: Vec<Tie>,
}

/// The Wells tab bundle (viewer `kind: "wells_logs"`, `schema_version` 4).
#[derive(Debug, Clone, Serialize)]
pub struct WellLogBundle {
    pub kind: &'static str,
    pub schema_version: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub flatten_default: Option<String>,
    pub wells: Vec<LogWell>,
}

/// Encode one lane as a v3-style base64 `f32` block. A non-finite sample packs as
/// the canonical quiet-NaN `0x7FC00000` (the viewer reads `NaN` as null → the
/// curve breaks) — byte-identical to the reference fixture's `encode_lane` for
/// the same input.
pub fn encode_lane(values: &[f64]) -> Lane {
    let mut bytes = Vec::with_capacity(values.len() * 4);
    for &v in values {
        let f = v as f32;
        let bits = if f.is_finite() {
            f.to_bits()
        } else {
            NAN_F32_BITS
        };
        bytes.extend_from_slice(&bits.to_le_bytes());
    }
    Lane {
        dtype: "f32",
        shape: [values.len()],
        data: STANDARD.encode(&bytes),
    }
}

/// Assemble the [`WellLogBundle`] from the loaded project's positioned `bores`
/// plus the populated `model` (upscaled cubes + framework tops/zones + tie
/// residuals). A bore with no curves is skipped (nothing to correlate); an empty
/// result yields `None` so the payload omits the Wells tab.
pub fn build_well_log_bundle(model: &Model, bores: &[BoreWell<'_>]) -> Option<WellLogBundle> {
    // Which of the canonical cubes the model actually populated (an upscaled track
    // is emitted only for a cube that exists).
    let populated = model.property_names();
    let has_cube = |name: &str| populated.iter().any(|p| p.eq_ignore_ascii_case(name));

    // Tie residuals keyed by well id (a bore matches its own id or its parent).
    let ties = model.well_tie_residuals();

    let mut wells: Vec<LogWell> = Vec::new();
    for bore in bores {
        if let Some(w) = build_log_well(model, bore, &has_cube, &ties) {
            wells.push(w);
        }
    }
    if wells.is_empty() {
        return None;
    }

    // Flatten default: the first interior framework horizon (a mid pick moves
    // wells visibly), else the shallowest top.
    let flatten_default = wells
        .iter()
        .flat_map(|w| w.tops.iter())
        .nth(1)
        .or_else(|| wells.iter().flat_map(|w| w.tops.iter()).next())
        .map(|t| t.horizon.clone());

    Some(WellLogBundle {
        kind: "wells_logs",
        schema_version: WELLS_SCHEMA_VERSION,
        flatten_default,
        wells,
    })
}

/// Build one [`LogWell`], or `None` when the bore carries no correlatable curves.
fn build_log_well(
    model: &Model,
    bore: &BoreWell<'_>,
    has_cube: &impl Fn(&str) -> bool,
    ties: &[crate::core::facade::WellTieResidual],
) -> Option<LogWell> {
    // The lane grid is the primary raw log's MD (PORO, else NTG, else SW). No raw
    // log → nothing to correlate.
    let primary = ["PORO", "NTG", "SW"]
        .iter()
        .find_map(|m| bore.log(m).map(|v| (*m, v)))?;
    let md: Vec<f64> = primary.1.md().to_vec();
    if md.len() < 2 {
        return None;
    }

    // TVD-SS (positive-down) at each MD on the bore's own trajectory; datum (KB)
    // from the first positioned station (`md = tvd + datum`, family z negative-down).
    let mut tvd: Vec<f64> = Vec::with_capacity(md.len());
    let mut datum_m = 0.0;
    let mut have_datum = false;
    for &m in &md {
        match bore.xyz(m) {
            Some(p) => {
                tvd.push(-p.z);
                if !have_datum {
                    datum_m = m + p.z; // md - tvd = md + z
                    have_datum = true;
                }
            }
            None => tvd.push(f64::NAN),
        }
    }
    let (head_x, head_y) = bore
        .trajectory()
        .first()
        .map_or((0.0, 0.0), |p| (p[0], p[1]));

    let mut curves: Vec<Curve> = Vec::new();

    // FACIES flag strip: the net flag (PHIE ≥ cutoff), derived from the raw
    // porosity so the strip reads net vs non-net rock coupled to the PHIE cutoff.
    if let Some(por) = bore.log("PORO") {
        let facies: Vec<f64> = md
            .iter()
            .map(|&m| match por.at_md(m) {
                Some(v) if v.is_finite() => {
                    if v >= DEFAULT_PHIE_CUTOFF {
                        1.0
                    } else {
                        0.0
                    }
                }
                _ => f64::NAN,
            })
            .collect();
        let mut codes = BTreeMap::new();
        codes.insert("0".to_string(), "non-net".to_string());
        codes.insert("1".to_string(), "net sand".to_string());
        curves.push(Curve {
            mnemonic: "FACIES".to_string(),
            display_name: Some("Facies (net)".to_string()),
            unit: String::new(),
            kind: "flag",
            values: encode_lane(&facies),
            range: None,
            cutoff: None,
            codes: Some(codes),
        });
    }

    // Raw continuous curves, each resampled onto the MD grid.
    for (mnem, display, lo, hi) in RAW_CURVES {
        let Some(log) = bore.log(mnem) else { continue };
        let vals: Vec<f64> = md
            .iter()
            .map(|&m| log.at_md(m).unwrap_or(f64::NAN))
            .collect();
        let phie = mnem.eq_ignore_ascii_case("PORO");
        curves.push(Curve {
            mnemonic: if phie {
                "PHIE".to_string()
            } else {
                (*mnem).to_string()
            },
            display_name: Some((*display).to_string()),
            unit: "v/v".to_string(),
            kind: "continuous",
            values: encode_lane(&vals),
            range: Some(Range { min: *lo, max: *hi }),
            cutoff: if phie {
                Some(DEFAULT_PHIE_CUTOFF)
            } else {
                None
            },
            codes: None,
        });
    }

    // Upscaled cubes sampled back along the bore (the blocky cell curve beside the
    // raw log). One along-bore section per populated cube; the first also yields
    // the model tops/zones for this bore.
    let traj = bore.trajectory();
    let mut model_tops: Vec<Top> = Vec::new();
    let mut model_zones: Vec<Zone> = Vec::new();
    let mut tops_taken = false;
    if traj.len() >= 2 {
        for (mnem, display, lo, hi) in RAW_CURVES {
            if !has_cube(mnem) {
                continue;
            }
            let spec = SectionSpec::AlongBore {
                trajectory: traj.clone(),
            };
            let Ok(section) = model.intersection_bundle(&spec, Some(mnem)) else {
                continue;
            };
            // The representative column (nearest the wellhead) carries the layer
            // geometry for the upscaled curve + the tops/zones.
            if let Some(col) = nearest_column(&section.columns, head_x, head_y) {
                let intervals = cell_intervals(&col.layer_tops, &col.layer_bases, &col.values);
                let up: Vec<f64> = tvd.iter().map(|&d| sample_step(&intervals, d)).collect();
                let phie = mnem.eq_ignore_ascii_case("PORO");
                curves.push(Curve {
                    mnemonic: format!("{}_UP", if phie { "PHIE" } else { mnem }),
                    display_name: Some(format!("{display} (upscaled)")),
                    unit: "v/v".to_string(),
                    kind: "continuous",
                    values: encode_lane(&up),
                    range: Some(Range { min: *lo, max: *hi }),
                    cutoff: if phie {
                        Some(DEFAULT_PHIE_CUTOFF)
                    } else {
                        None
                    },
                    codes: None,
                });
                if !tops_taken {
                    (model_tops, model_zones) = framework_picks(model, &section, col);
                    tops_taken = true;
                }
            }
        }
    }

    if curves.is_empty() {
        return None;
    }

    // Tie residuals for this bore (own id or its parent well id).
    let parent = bore.id.split(' ').next().unwrap_or("");
    let bore_ties: Vec<Tie> = ties
        .iter()
        .filter(|t| t.well_id == bore.id || t.well_id == parent)
        .map(|t| Tie {
            horizon: t.horizon.clone(),
            residual_m: t.residual_m,
        })
        .collect();

    Some(LogWell {
        id: bore.id.clone(),
        display_name: Some(bore.id.clone()),
        x: head_x,
        y: head_y,
        datum_m,
        md_m: encode_lane(&md),
        tvd_m: encode_lane(&tvd),
        curves,
        tops: model_tops,
        zones: model_zones,
        ties: bore_ties,
    })
}

/// The section column nearest the wellhead `(x, y)` — for a vertical bore the one
/// (and only) column; for a deviated bore the shallowest penetration column.
fn nearest_column(
    columns: &[crate::core::view::SectionColumn],
    x: f64,
    y: f64,
) -> Option<&crate::core::view::SectionColumn> {
    columns.iter().min_by(|a, b| {
        let da = (a.x - x).hypot(a.y - y);
        let db = (b.x - x).hypot(b.y - y);
        da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
    })
}

/// The finite cell intervals `(top, base, value)` of one section column, top→base.
fn cell_intervals(tops: &[f64], bases: &[f64], values: &[f64]) -> Vec<(f64, f64, f64)> {
    let mut out = Vec::new();
    for k in 0..tops.len() {
        let (t, b) = (tops[k], bases[k]);
        let v = values.get(k).copied().unwrap_or(f64::NAN);
        if t.is_finite() && b.is_finite() && b > t {
            out.push((t, b, v));
        }
    }
    out
}

/// The upscaled value at depth `d`: the cube value of the cell whose `[top, base)`
/// window contains `d`, else `NaN` (above the shallowest cell or below the deepest).
fn sample_step(intervals: &[(f64, f64, f64)], d: f64) -> f64 {
    if !d.is_finite() {
        return f64::NAN;
    }
    for &(t, b, v) in intervals {
        if d >= t && d < b {
            return v;
        }
    }
    f64::NAN
}

/// The model's per-bore framework tops (top→down) + zone bands, read off the
/// representative section `col` (structural top/base + interior horizon traces)
/// and the model's zone table (per-zone `k_range` → cell top/base at this column).
fn framework_picks(
    model: &Model,
    section: &crate::core::view::IntersectionBundle,
    col: &crate::core::view::SectionColumn,
) -> (Vec<Top>, Vec<Zone>) {
    let mut tops: Vec<Top> = Vec::new();
    // Structural top = first active layer top; base = last active layer base.
    if let Some(&t) = col.layer_tops.iter().find(|v| v.is_finite()) {
        tops.push(Top {
            horizon: section.top_name.clone(),
            tvd_m: t,
        });
    }
    for tr in &section.horizon_traces {
        if let Some(&d) = tr.depths.first() {
            if d.is_finite() {
                tops.push(Top {
                    horizon: tr.name.clone(),
                    tvd_m: d,
                });
            }
        }
    }
    if let Some(&b) = col.layer_bases.iter().rev().find(|v| v.is_finite()) {
        tops.push(Top {
            horizon: section.base_name.clone(),
            tvd_m: b,
        });
    }
    tops.sort_by(|a, b| {
        a.tvd_m
            .partial_cmp(&b.tvd_m)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    tops.dedup_by(|a, b| a.horizon == b.horizon);

    // Zone bands: each model zone's `k_range` → the cell top/base at this column.
    let mut zones: Vec<Zone> = Vec::new();
    for z in model.static_model().zones().zones() {
        let start = z.k_range.start;
        let end = z.k_range.end;
        if end == 0 || end > col.layer_bases.len() {
            continue;
        }
        let top = col.layer_tops.get(start).copied().unwrap_or(f64::NAN);
        let base = col.layer_bases.get(end - 1).copied().unwrap_or(f64::NAN);
        if top.is_finite() && base.is_finite() && base > top {
            zones.push(Zone {
                name: z.name.clone(),
                top_tvd_m: top,
                base_tvd_m: base,
            });
        }
    }
    (tops, zones)
}
