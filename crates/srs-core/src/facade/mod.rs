//! The `peteksim` product facade — the staged model-build API distilled from the
//! design contract (`petekSuite/dev-docs/designs/python-model-build-api.md`).
//! Everything here is a **thin** orchestration over the owning layers: petekio
//! ingest (`Project`), the petekStatic framework/property/model/MC seam
//! (`Framework`/`StaticGrid`/`Model`), and petekTools kernels (via the seam).
//! No algorithm logic lives here — only assembly and the small type conversions
//! at the seams.

pub mod charts;
pub mod framework;
pub mod grid;
pub mod model;
pub mod project;
pub mod spec;
pub mod uncertainty;
pub mod wells;
pub mod zonation;
pub mod zoned_mc;

pub use charts::{crossplot, CrossplotOpts};
pub use framework::{Framework, TieResidual};
pub use grid::StaticGrid;
pub use model::{BuildWarn, Model, Summary, WellTieResidual, ZoneStat, ZoneVolume, ZonedVolumes};
pub use project::{Inventory, Project, TopsPick};
pub use spec::{perturbation_field, DistSpec, LayersSpec, TrendSpec, VgmSpec};
pub use uncertainty::{aggregate, FieldSummary, McConfig, McOutcome, PSummary};
pub use wells::{build_well_log_bundle, WellLogBundle};
pub use zonation::ZoneSpec;
pub use zoned_mc::{ZoneMcSpec, ZonePCurves, ZonedMcConfig, ZonedMcOutcome};
