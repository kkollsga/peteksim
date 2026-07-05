//! The `core` module — orchestration tying grid + volumetrics + uncertainty together.
//!
//! [`run_model`] turns a [`ModelInputs`] (inputs-as-distributions) into a
//! [`ModelResult`]: a deterministic P50 estimate plus a seeded Monte Carlo
//! P90/P50/P10. The [`view`] module re-exports petekStatic's typed inspection
//! bundles (map / intersection / volume) the packaged viewer renders, plus the
//! [`view::box_static_model`] glue that lets the analytic box path export them.
//! This is the compute spine of the model-first walking skeleton; the live
//! refinement loop (add data -> re-converge) builds on it.

pub mod charts;
mod data_adapter;
pub mod facade;
mod inplace;
mod model;
mod refine;
mod run;
pub mod view;

pub use charts::{
    crossplot_chart, distribution_panel, tornado_chart, Axis, ColorBy, DistMarkers,
    DistributionPanel, Range, ScatterChart, ScatterInput, TornadoChart,
};
pub use data_adapter::{distribution_of, scale_area_km2_to_m2};
pub use facade::{
    aggregate, build_well_log_bundle, crossplot, perturbation_field, BuildWarn, CrossplotOpts,
    DistSpec, FieldSummary, Framework, Inventory, LayersSpec, McConfig, McOutcome,
    Model as ProjectModel, PSummary, Project, StaticGrid, Summary, TieResidual, TopsPick,
    TrendSpec, VgmSpec, WellLogBundle, WellTieResidual, ZoneMcSpec, ZonePCurves, ZoneSpec,
    ZoneStat, ZoneVolume, ZonedMcConfig, ZonedMcOutcome, ZonedVolumes,
};
pub use inplace::{BoxDraw, Fluid};
pub use model::{ModelInputs, ModelResult};
pub use refine::{PetroSample, Refined, RefiningModel, WireframeSeed};
pub use run::run_model;
pub use view::{
    box_static_model, ContactMask, GridFrame, IntersectionBundle, MapBundle, MapSpec, ScalarLayer,
    SectionColumn, SectionContact, SectionSpec, ValueRange, VolumeBundle, WellMarker,
    DEFAULT_VIEW_PROPERTY, SCHEMA_VERSION,
};

// Re-export seam types the Python binding marshals but does not otherwise name.
pub use petektools::VariogramModel;
// Re-export petekTools' canonical volume-reporting scales (S3) + the km²→m² area
// scale so the Python binding (which deps only srs-core) consumes the one source
// of truth instead of re-declaring bare `1e6`/`1e9` literals. `SM3_PER_MSM3` = 1e6
// (oil MSm³ report), `SM3_PER_BCM` = 1e9 (gas bcm report), `M2_PER_KM2` = 1e6 +
// `km2_to_m2` (the area seam — the box binding scales user-facing km² to core m²).
pub use petekstatic::model::{
    live_set_bytes, Correlation, PerturbationField, StaticModel, TornadoBar, UpscaleMethod,
    UpscaleQc,
};
pub use petektools::units::{km2_to_m2, M2_PER_KM2, SM3_PER_BCM, SM3_PER_MSM3};

// petekio owns the canonical mnemonic-alias table (the family's single home for
// log/property-name normalisation). Re-export it so the binding (which deps only
// srs-core) routes facade name-normalisation through that one table instead of a
// private per-site lookup.
pub use petekio::analysis::normalize::canonical_mnemonic;

// Re-export the input vocabulary so binding crates need only depend on srs-core.
pub use crate::units::SrsError;
pub use petekstatic::grid::{build_box, BoxSpec, Dims, Grid};
pub use petekstatic::gridder::Conformity;
pub use petekstatic::uncertainty::{Distribution, PercentileSummary};
pub use petekstatic::volumetrics::ConstantPriors;
