//! PyO3 marshalling for the staged model-build facade (`Project.load` → …
//! `model.uncertainty`). Every class here holds an `peteksim::facade` type and
//! only marshals across the boundary; the orchestration + all seam calls live in
//! the `peteksim` crate. Units are SI (area m²/km², depths m positive-down, results Sm³).
//!
//! Split by domain into submodules; this hub re-exports the shared imports (so a
//! submodule needs only `use super::*`) plus the small `err` helper, and wires
//! every class + function onto the `_core` module in [`register`].

use pyo3::prelude::*;

// --- shared imports, re-exported to every submodule via `use super::*` --------
pub(crate) use crate::viewer::{self, WellTrack};
pub(crate) use peteksim::{
    aggregate as core_aggregate, build_well_log_bundle, crossplot as core_crossplot,
    distribution_panel, tornado_chart, Conformity, Correlation, CrossplotOpts, DistMarkers,
    DistSpec, Fluid, Framework as CoreFramework, LayersSpec, McConfig, McOutcome as CoreMc,
    Project as CoreProject, ProjectModel, StaticGrid, Summary, TopsPick as CoreTopsPick, TrendSpec,
    UpscaleMethod, VariogramModel, VgmSpec, WellLogBundle, ZoneMcSpec, ZoneSpec, ZonedMcConfig,
    ZonedMcOutcome, SM3_PER_BCM, SM3_PER_MSM3,
};
pub(crate) use pyo3::exceptions::PyValueError;
pub(crate) use pyo3::types::{PyDict, PyList};
pub(crate) use serde_json::Value;

mod framework;
mod grid;
mod model;
mod project;
mod specs;
mod uncertainty;

pub(crate) use framework::*;
pub(crate) use grid::*;
pub(crate) use model::*;
pub(crate) use project::*;
pub(crate) use specs::*;
pub(crate) use uncertainty::*;

/// Map an [`peteksim::SrsError`] to a Python `ValueError`.
pub(crate) fn err(e: peteksim::SrsError) -> PyErr {
    PyValueError::new_err(e.to_string())
}

/// Register the facade classes + functions on the `_core` module.
pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Project>()?;
    m.add_class::<Inventory>()?;
    m.add_class::<Wells>()?;
    m.add_class::<Surface>()?;
    m.add_class::<Tops>()?;
    m.add_class::<TopsPick>()?;
    m.add_class::<Framework>()?;
    m.add_class::<Grid>()?;
    m.add_class::<Property>()?;
    m.add_class::<StaticModelPy>()?;
    m.add_class::<Uncertainty>()?;
    m.add_class::<ZonedUncertainty>()?;
    m.add_class::<Dist>()?;
    m.add_class::<PickSpread>()?;
    m.add_class::<Vgm>()?;
    m.add_class::<GaussianSpec>()?;
    m.add_class::<Trend>()?;
    m.add_class::<Layers>()?;
    m.add_function(wrap_pyfunction!(normal, m)?)?;
    m.add_function(wrap_pyfunction!(lognormal, m)?)?;
    m.add_function(wrap_pyfunction!(uniform, m)?)?;
    m.add_function(wrap_pyfunction!(triangular, m)?)?;
    m.add_function(wrap_pyfunction!(truncated_normal, m)?)?;
    m.add_function(wrap_pyfunction!(level_shift, m)?)?;
    m.add_function(wrap_pyfunction!(pick_spread, m)?)?;
    m.add_function(wrap_pyfunction!(spherical, m)?)?;
    m.add_function(wrap_pyfunction!(exponential, m)?)?;
    m.add_function(wrap_pyfunction!(gaussian_vgm, m)?)?;
    m.add_function(wrap_pyfunction!(fit_variogram, m)?)?;
    m.add_function(wrap_pyfunction!(gaussian, m)?)?;
    m.add_function(wrap_pyfunction!(collocated, m)?)?;
    m.add_function(wrap_pyfunction!(resimulate, m)?)?;
    m.add_function(wrap_pyfunction!(layers, m)?)?;
    m.add_function(wrap_pyfunction!(aggregate, m)?)?;
    m.add_function(wrap_pyfunction!(distribution_bundle, m)?)?;
    Ok(())
}
