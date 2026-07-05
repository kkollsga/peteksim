//! View-bundle seam — the render exports the packaged viewer consumes.
//!
//! The typed, JSON-stable inspection bundles ([`MapBundle`] / [`IntersectionBundle`]
//! / [`VolumeBundle`]) live **down** in petekStatic's `srs-model` (the mesh belongs
//! beside the grid it meshes; the DAG flows downward). petekSim re-exports them here
//! and adds only the thin glue the product facade needs:
//!
//! - [`box_static_model`] — build the deterministic (P50) [`StaticModel`] for the
//!   analytic box path so it too exports bundles (the box viewer keeps working
//!   through the new seam, no local mesh builder).
//! - the `*_bundle` accessors on [`crate::facade::Model`] (delegating to the inner
//!   [`StaticModel`]).
//!
//! The viewer renders whatever the JSON declares; it never computes.

use crate::model::ModelInputs;
use srs_gridder::{Conformity, SolveOpts};
use srs_model::{BuildOpts, StaticModel, StaticModelBuilder};
use srs_units::SrsError;
use srs_volumetrics::ConstantPriors;

// The bundle vocabulary (re-exported from the geomodel layer so binding crates
// depend only on srs-core).
pub use srs_model::{
    ContactMask, GridFrame, IntersectionBundle, MapBundle, MapSpec, ScalarLayer, SectionColumn,
    SectionContact, SectionSpec, ValueRange, VolumeBundle, WellMarker, SCHEMA_VERSION,
};

/// The canonical porosity cube — the box/refined paths' default view property.
pub const DEFAULT_VIEW_PROPERTY: &str = "PORO";

/// Build the deterministic (P50) [`StaticModel`] behind an analytic box model: a
/// flat box sized at the median area/height, populated with the median priors.
/// This is the model the box viewer exports its bundles from — the replacement
/// for the retired `mesh.rs` deterministic-grid path.
///
/// # Errors
/// [`SrsError`] if the median dimensions are degenerate or the build fails.
pub fn box_static_model(inputs: &ModelInputs) -> Result<StaticModel, SrsError> {
    let p50 = |d: srs_uncertainty::Distribution| d.quantile(0.5);
    let dims = inputs.dims;
    let opts = BuildOpts {
        area_m2: p50(inputs.area_m2).max(f64::MIN_POSITIVE),
        gross_height_m: p50(inputs.gross_height_m).max(f64::MIN_POSITIVE),
        nk: dims.nk,
        conformity: Conformity::Proportional,
        solve_opts: SolveOpts::default(),
        priors: ConstantPriors {
            porosity: p50(inputs.porosity).clamp(0.0, 1.0),
            net_to_gross: p50(inputs.net_to_gross).clamp(0.0, 1.0),
            water_saturation: p50(inputs.water_saturation).clamp(0.0, 1.0),
        },
    };
    // A finite contact keeps the flat builder happy; the box column base is the
    // model contact (or, when unbounded, the box base — never affects the mesh).
    let contact = if inputs.contact_depth_m.is_finite() {
        inputs.contact_depth_m
    } else {
        inputs.top_depth_m + opts.gross_height_m
    };
    let model =
        StaticModelBuilder::flat(dims.ni, dims.nj, inputs.top_depth_m, contact, opts)?.build()?;
    Ok(model)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inplace::Fluid;
    use srs_uncertainty::Distribution;

    fn inputs() -> ModelInputs {
        ModelInputs {
            area_m2: Distribution::Constant(400_000.0),
            gross_height_m: Distribution::Constant(15.0),
            porosity: Distribution::Constant(0.25),
            net_to_gross: Distribution::Constant(0.8),
            water_saturation: Distribution::Constant(0.3),
            fvf: Distribution::Constant(1.25),
            fluid: Fluid::Oil,
            dims: srs_grid::Dims::new(6, 5, 4).unwrap(),
            top_depth_m: 1500.0,
            contact_depth_m: 1510.0,
            aspect_ratio: 1.0,
        }
    }

    #[test]
    fn box_model_exports_a_volume_bundle() {
        let m = box_static_model(&inputs()).unwrap();
        let vb = m.volume_bundle(DEFAULT_VIEW_PROPERTY).unwrap();
        assert_eq!(vb.cell_count, 6 * 5 * 4);
        // Shell-mesh bundle (petekStatic view redesign): deduplicated shell
        // vertices + triangle indices, one compact value per shell cell.
        assert!(!vb.positions.is_empty() && vb.positions.len() % 3 == 0);
        assert!(!vb.indices.is_empty() && vb.indices.len() % 3 == 0);
        // The P50 porosity populates every cell → a finite, non-empty legend.
        assert!(vb.value_range.min.is_finite() && vb.value_range.max.is_finite());
        assert!(!vb.cell_values.is_empty() && vb.cell_values.iter().all(|v| v.is_finite()));
    }

    #[test]
    fn box_model_exports_a_map_bundle() {
        let m = box_static_model(&inputs()).unwrap();
        let mb = m
            .map_bundle(&MapSpec::new().property(DEFAULT_VIEW_PROPERTY))
            .unwrap();
        assert_eq!(mb.frame.ncol, 6);
        assert_eq!(mb.frame.nrow, 5);
        assert_eq!(mb.horizons.len(), 2);
        assert!(!mb.zone_averages.is_empty());
    }
}
