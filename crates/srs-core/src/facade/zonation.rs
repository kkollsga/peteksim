//! `set_zonation` — the facade half of the multi-zone horizon stack. A list of
//! per-zone specs (`ZoneSpec`) + the framework's already-resolved horizons
//! ([`srs_model::StackHorizon`], `Mapped` surfaces or `TopsOnly` picks) assemble
//! into petekStatic's [`srs_model::HorizonStack`], which
//! [`srs_model::StaticModelBuilder::from_horizon_stack`] turns into a zoned
//! `StaticModel`. This module owns only the marshalling — the drape, the per-zone
//! layering + contacts, and the volumetrics all live down in the engine.
//!
//! `N` horizons (top→down) → `N-1` zones. Each `ZoneSpec` names the zone, its
//! **base** horizon (`below_horizon` — must be the next horizon down), a
//! conformity + layering allocation, and its OWN contacts (a GOC and/or a lower
//! contact; both absent = a contactless zone that carries gross bulk but zero
//! hydrocarbon).

use crate::facade::spec::LayersSpec;
use srs_gridder::Conformity;
use srs_model::{HorizonSource, HorizonStack, StackHorizon, StackZone};
use srs_units::SrsError;
use srs_wireframe::{Contact, ContactKind, Hardness};

/// One zone of a declared zonation (`fw.set_zonation([...])`).
#[derive(Debug, Clone)]
pub struct ZoneSpec {
    /// The zone name (labels the per-zone volumes + stats).
    pub name: String,
    /// The horizon that bounds this zone below — must equal the next horizon down
    /// in the framework's declared order (validated at assembly).
    pub below_horizon: String,
    /// Proportional (equal-fraction) or a Follow style (constant-dz drape).
    pub conformity: Conformity,
    /// The layer allocation (`ps.layers(n=..)` / `ps.layers(dz_m=..)`); the zone's
    /// layer count is derived from this against the zone's own gross thickness.
    pub layers: LayersSpec,
    /// A gas-oil contact inside this zone (positive-down depth), or `None`.
    pub goc_m: Option<f64>,
    /// The zone's lower contact — OWC/FWL/GWC (positive-down depth), or `None`.
    /// Both `goc_m` and `owc_m` absent ⇒ a **contactless** zone (zero hydrocarbon).
    pub owc_m: Option<f64>,
}

/// Validate a zonation against the framework's horizons **without** assembling the
/// stack (cheap, loud) — for `set_zonation` to reject a bad zonation eagerly.
pub fn validate(horizon_names: &[String], zones: &[ZoneSpec]) -> Result<(), SrsError> {
    let n = horizon_names.len();
    if n < 2 {
        return Err(SrsError::InvalidInput(
            "a zoned framework needs at least 2 horizons".into(),
        ));
    }
    if zones.len() != n - 1 {
        return Err(SrsError::InvalidInput(format!(
            "{n} horizons define {} zone(s), but {} zone spec(s) were given",
            n - 1,
            zones.len()
        )));
    }
    for (i, z) in zones.iter().enumerate() {
        let expected = &horizon_names[i + 1];
        if &z.below_horizon != expected {
            return Err(SrsError::InvalidInput(format!(
                "zone '{}' has below_horizon '{}', but interval {i} is bounded below by \
                 horizon '{expected}' (zones must be declared top→down, one per horizon gap)",
                z.name, z.below_horizon
            )));
        }
    }
    Ok(())
}

/// Assemble the [`HorizonStack`] from the framework's resolved horizons + the
/// zonation. `resolved` is parallel to `horizon_names` (top→down); the first is a
/// `Mapped` surface or a `Scatter` point-set (either fixes the node lattice — a
/// `Scatter` top is conditioned to `Mapped` inside `from_scatter_stack` before the
/// engine's mapped-top requirement is checked).
pub fn assemble_stack(
    horizon_names: &[String],
    resolved: &[StackHorizon],
    zones: &[ZoneSpec],
) -> Result<HorizonStack, SrsError> {
    validate(horizon_names, zones)?;
    let mut zone_layers = Vec::with_capacity(zones.len());
    for (i, z) in zones.iter().enumerate() {
        // The zone's gross thickness (for the Proportional layer count) is the mean
        // node separation of its bounding horizons; Follow styles ignore nk (the
        // engine derives it from dz), but a sane value keeps provenance honest.
        let gross = zone_gross(&resolved[i].source, &resolved[i + 1].source);
        let nk = z.layers.nk(gross).max(1);
        // The zone name folds into StackZone (the old `HorizonStack::zone_names`
        // was removed). Colour stays `None` — viewer colour exposure arrives with
        // api_v2; `StackZone::with_color` is the seam when it does.
        zone_layers.push(StackZone::new(
            z.name.clone(),
            z.conformity,
            nk,
            build_contacts(z.goc_m, z.owc_m),
        ));
    }
    Ok(HorizonStack {
        horizons: resolved.to_vec(),
        zone_layers,
    })
}

/// The zone's per-zone contacts as hard [`Contact`]s (GOC then lower). An empty
/// vec is a contactless zone (gross bulk, zero hydrocarbon).
fn build_contacts(goc_m: Option<f64>, owc_m: Option<f64>) -> Vec<Contact> {
    let mut cs = Vec::new();
    if let Some(g) = goc_m {
        cs.push(Contact {
            kind: ContactKind::Goc,
            depth_m: g,
            hardness: Hardness::Hard,
        });
    }
    if let Some(w) = owc_m {
        cs.push(Contact {
            kind: ContactKind::Owc,
            depth_m: w,
            hardness: Hardness::Hard,
        });
    }
    cs
}

/// Mean (below − above) separation across the two bounding horizons — the zone's
/// representative gross thickness for the (Proportional) layer count. Two `Mapped`
/// surfaces on the same lattice give the exact per-node mean; otherwise (a
/// `Scatter` point-set or a mixed pair) the difference of each horizon's mean
/// depth is a robust estimate without co-locating the scatter. Falls back to a
/// nominal 20 m only when neither horizon offers a finite depth (e.g. a
/// `TopsOnly` sparse-pick horizon on both sides).
fn zone_gross(above: &HorizonSource, below: &HorizonSource) -> f64 {
    if let (HorizonSource::Mapped(a), HorizonSource::Mapped(b)) = (above, below) {
        if a.depth_m.len() == b.depth_m.len() {
            let mut sum = 0.0;
            let mut n = 0usize;
            for (za, zb) in a.depth_m.iter().zip(&b.depth_m) {
                if za.is_finite() && zb.is_finite() {
                    sum += zb - za;
                    n += 1;
                }
            }
            if n > 0 {
                return (sum / n as f64).abs().max(1e-3);
            }
        }
    }
    match (source_mean_depth(above), source_mean_depth(below)) {
        (Some(a), Some(b)) => (b - a).abs().max(1e-3),
        _ => 20.0,
    }
}

/// A horizon source's mean finite depth (positive-down): the per-node mean of a
/// `Mapped` surface or the per-point mean of a `Scatter` set; `None` for a
/// `TopsOnly` horizon (its picks are node indices, not a depth field here) or a
/// source with no finite depth.
fn source_mean_depth(source: &HorizonSource) -> Option<f64> {
    let (mut sum, mut n) = (0.0, 0usize);
    match source {
        HorizonSource::Mapped(g) => {
            for &z in &g.depth_m {
                if z.is_finite() {
                    sum += z;
                    n += 1;
                }
            }
        }
        HorizonSource::Scatter(points) => {
            for p in points {
                if p.depth_m.is_finite() {
                    sum += p.depth_m;
                    n += 1;
                }
            }
        }
        HorizonSource::TopsOnly(_) => return None,
    }
    (n > 0).then(|| sum / n as f64)
}
