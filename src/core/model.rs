//! The appraisal model: inputs-as-distributions plus fixed structural settings,
//! and the result (deterministic point estimate + probabilistic summary).

use crate::core::inplace::Fluid;
use petekstatic::grid::Dims;
use petekstatic::uncertainty::{Distribution, PercentileSummary};

/// A model: every uncertain volumetric input is a [`Distribution`]; structural
/// settings (resolution, depths) are fixed. SI units throughout
/// (`decision_si_units_standard`): area m², lengths/depths m (positive down).
#[derive(Debug, Clone)]
pub struct ModelInputs {
    /// Reservoir area \[m²\].
    pub area_m2: Distribution,
    /// Gross column height \[m\].
    pub gross_height_m: Distribution,
    pub porosity: Distribution,
    pub net_to_gross: Distribution,
    pub water_saturation: Distribution,
    /// Boi (oil) or Bgi (gas) \[Rm³/Sm³\], as a distribution.
    pub fvf: Distribution,
    pub fluid: Fluid,
    /// Grid resolution for the rendered model.
    pub dims: Dims,
    /// Top depth \[m, positive down\].
    pub top_depth_m: f64,
    /// Contact depth \[m, positive down\].
    pub contact_depth_m: f64,
    pub aspect_ratio: f64,
}

/// The model answer.
#[derive(Debug, Clone)]
pub struct ModelResult {
    /// Which fluid was modelled.
    pub fluid: Fluid,
    /// Deterministic in-place at the P50 of each input \[Sm³\] (oil or gas).
    pub deterministic_in_place: f64,
    /// Deterministic GRV of the hydrocarbon column \[mcm = 10⁶ m³\].
    pub deterministic_grv_mcm: f64,
    /// P90/P50/P10 + mean of in-place \[Sm³\] over the Monte Carlo realizations.
    pub summary: PercentileSummary,
    /// Number of Monte Carlo realizations.
    pub realizations: usize,
    /// The per-realization in-place values \[Sm³\], in draw order (the full MC
    /// sample behind `summary`). Exposed so compositional workflows (EUR
    /// bridging, custom percentiles) can reuse the sample instead of
    /// reimplementing it.
    pub samples: Vec<f64>,
}
