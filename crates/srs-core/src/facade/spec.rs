//! Value-holder specs the facade passes across the seam — distributions,
//! variograms, trends, layering. These carry validated parameters and convert
//! into the owning-layer types (`srs_model::Sampler`/`Input`,
//! `petektools::Variogram`, `srs_model::TrendSurface`) at the point of use. No
//! algorithm logic lives here; construction only validates and forwards.

use petektools::geostat::experimental_variogram;
use petektools::{Variogram, VariogramModel};
use srs_model::{Input, PerturbationField, Sampler, TrendSurface};
use srs_units::SrsError;

/// Build a [`srs_model::PerturbationField`] for a structural-uncertainty row —
/// a magnitude `sd_m` \[m\] with the spatial continuity of a `model`/`range_m`
/// variogram. Nugget is 0 and sill is 1 (the field is rescaled to `sd_m`, so only
/// the shape + range matter — the seam's contract). Feeds
/// [`RealizationDraw::with_top_structural`](srs_model::RealizationDraw) (top row)
/// and [`ZoneDraw::with_isochore_structural`](srs_model::ZoneDraw) (deeper rows).
pub fn perturbation_field(
    sd_m: f64,
    model: VariogramModel,
    range_m: f64,
) -> Result<PerturbationField, SrsError> {
    let vgm = Variogram::new(model, 0.0, 1.0, range_m)?;
    Ok(PerturbationField::new(sd_m, vgm))
}

/// A named appraisal distribution with an optional hard clamp — the facade's
/// `ps.normal(...)`, `ps.lognormal(...)`, … `.clamped(lo, hi)`. Converts into a
/// structured-MC [`Input`] (the seam's per-quantity sampler).
#[derive(Debug, Clone)]
pub struct DistSpec {
    sampler: Sampler,
    clamp: Option<(f64, f64)>,
}

impl DistSpec {
    fn wrap(sampler: Result<Sampler, petektools::AlgoError>) -> Result<Self, SrsError> {
        Ok(Self {
            sampler: sampler?,
            clamp: None,
        })
    }

    /// `ps.normal(mean, sd)`.
    pub fn normal(mean: f64, sd: f64) -> Result<Self, SrsError> {
        Self::wrap(Sampler::new_normal(mean, sd))
    }

    /// `ps.lognormal(mean, sd)` — parameters of the underlying log-space normal.
    pub fn lognormal(mean: f64, sd: f64) -> Result<Self, SrsError> {
        Self::wrap(Sampler::new_lognormal(mean, sd))
    }

    /// `ps.uniform(lo, hi)`.
    pub fn uniform(lo: f64, hi: f64) -> Result<Self, SrsError> {
        Self::wrap(Sampler::new_uniform(lo, hi))
    }

    /// `ps.triangular(lo, mode, hi)`.
    pub fn triangular(lo: f64, mode: f64, hi: f64) -> Result<Self, SrsError> {
        Self::wrap(Sampler::new_triangular(lo, mode, hi))
    }

    /// `ps.truncated_normal(mean, sd, lo, hi)` — the density reshaped onto
    /// `[lo, hi]` (distinct from a hard `.clamped()` limiter).
    pub fn truncated_normal(mean: f64, sd: f64, lo: f64, hi: f64) -> Result<Self, SrsError> {
        Self::wrap(Sampler::new_truncated_normal(mean, sd, lo, hi))
    }

    /// `ps.level_shift(sd)` — a zero-mean normal additive shift for a modelled
    /// property cube under MC (Fix 4's shape-vs-level split: the pattern is
    /// fixed, its level moves).
    pub fn level_shift(sd: f64) -> Result<Self, SrsError> {
        Self::normal(0.0, sd)
    }

    /// A (numerically) fixed input — used to pin the template's area/gross,
    /// which the design holds constant through the MC. The structured-MC seam
    /// has no point-mass sampler, so this is a uniform of negligible width
    /// (`|v|·1e-9`); the induced variance (`width²/12`) is far below any
    /// modelled spread. Documented deviation from a true constant.
    pub fn constant(value: f64) -> Result<Self, SrsError> {
        if !value.is_finite() {
            return Err(SrsError::InvalidInput(format!(
                "constant needs a finite value, got {value}"
            )));
        }
        let width = (value.abs() * 1e-9).max(1e-9);
        Self::uniform(value, value + width)
    }

    /// Hard-clamp the sampler onto `[lo, hi]` (tail mass piles on the bounds).
    pub fn clamped(mut self, lo: f64, hi: f64) -> Result<Self, SrsError> {
        if lo >= hi {
            return Err(SrsError::InvalidInput(format!(
                "clamp needs lo < hi, got [{lo}, {hi}]"
            )));
        }
        self.clamp = Some((lo, hi));
        Ok(self)
    }

    /// The structured-MC [`Input`] for this spec.
    pub fn to_input(&self) -> Result<Input, SrsError> {
        match self.clamp {
            None => Ok(Input::plain(self.sampler)),
            Some((lo, hi)) => Input::clamped(self.sampler, lo, hi).map_err(SrsError::from),
        }
    }

    /// Draw `n` samples off `rng` (applying the optional hard clamp). The zoned-MC
    /// driver ([`crate::facade::zoned_mc`]) samples every per-zone contact/level
    /// input off ONE seeded stream in a fixed field order — the same reproducibility
    /// contract [`srs_model::McInputs::realize`] gives the whole-model path, but here
    /// petekSim owns the sampler (the per-zone `ZoneDraw` fields are not expressible
    /// through `McInputs`). The whole-model path still uses [`Self::to_input`] +
    /// petekStatic's driver.
    pub fn sample_vec<R: rand::Rng>(&self, n: usize, rng: &mut R) -> Vec<f64> {
        let mut v = self.sampler.sample_n(n, rng);
        if let Some((lo, hi)) = self.clamp {
            for x in &mut v {
                *x = x.clamp(lo, hi);
            }
        }
        v
    }
}

/// A variogram spec (`ps.spherical/exponential/gaussian_vgm(range_m, sill, nugget)`)
/// or a fitted one (`ps.fit_variogram(...)`) — resolves to a `petektools::Variogram`.
#[derive(Debug, Clone)]
pub struct VgmSpec {
    variogram: Variogram,
}

impl VgmSpec {
    fn build(
        model: VariogramModel,
        range_m: f64,
        sill: f64,
        nugget: f64,
    ) -> Result<Self, SrsError> {
        Ok(Self {
            variogram: Variogram::new(model, nugget, sill, range_m)?,
        })
    }

    /// `ps.spherical(range_m, sill=1, nugget=0)`.
    pub fn spherical(range_m: f64, sill: f64, nugget: f64) -> Result<Self, SrsError> {
        Self::build(VariogramModel::Spherical, range_m, sill, nugget)
    }

    /// `ps.exponential(range_m, sill=1, nugget=0)`.
    pub fn exponential(range_m: f64, sill: f64, nugget: f64) -> Result<Self, SrsError> {
        Self::build(VariogramModel::Exponential, range_m, sill, nugget)
    }

    /// `ps.gaussian_vgm(range_m, sill=1, nugget=0)`.
    pub fn gaussian(range_m: f64, sill: f64, nugget: f64) -> Result<Self, SrsError> {
        Self::build(VariogramModel::Gaussian, range_m, sill, nugget)
    }

    /// `ps.fit_variogram(coords, model, lag, n_lags)` — fit a model to an
    /// experimental variogram over `[x, y, value]` rows (petekTools inference).
    pub fn fit(
        model: VariogramModel,
        coords: &[[f64; 3]],
        lag: f64,
        n_lags: usize,
    ) -> Result<Self, SrsError> {
        let exp = experimental_variogram(coords, lag, n_lags)?;
        Ok(Self {
            variogram: Variogram::fit(model, &exp)?,
        })
    }

    pub fn variogram(&self) -> &Variogram {
        &self.variogram
    }
}

/// A layering allocation for a zone (`ps.layers(n=..)` or `ps.layers(dz_m=..)`).
/// The current single-`ZoneTable` stack maps this to a whole-column layer count
/// (see [`crate::facade::framework::Framework::set_layering`] for the documented
/// limitation).
#[derive(Debug, Clone, Copy)]
pub enum LayersSpec {
    /// A fixed number of layers.
    Count(usize),
    /// A target layer thickness \[m\]; the layer count is derived from the
    /// gross column at build time.
    Thickness(f64),
}

impl LayersSpec {
    /// The layer count this spec implies over a gross column of `gross_m`.
    pub fn nk(&self, gross_m: f64) -> usize {
        match *self {
            LayersSpec::Count(n) => n.max(1),
            LayersSpec::Thickness(dz) => {
                if dz <= 0.0 || !gross_m.is_finite() || gross_m <= 0.0 {
                    1
                } else {
                    ((gross_m / dz).round() as usize).max(1)
                }
            }
        }
    }
}

/// A collocated-cokriging secondary (`ps.collocated(surface, corr)`): a named
/// areal trend surface + the Markov-1 correlation folded into SGS.
#[derive(Debug, Clone)]
pub struct TrendSpec {
    trend: TrendSurface,
    corr: f64,
}

impl TrendSpec {
    /// Build from an areal lattice (`ncol × nrow` node values, row-major) with a
    /// georeference so the seam can resample it to the model lattice.
    #[allow(clippy::too_many_arguments)]
    pub fn collocated(
        ncol: usize,
        nrow: usize,
        values: Vec<f64>,
        origin_x: f64,
        origin_y: f64,
        node_dx: f64,
        node_dy: f64,
        corr: f64,
    ) -> Result<Self, SrsError> {
        let trend = TrendSurface::new(ncol, nrow, values)
            .map_err(SrsError::from)?
            .with_georef(origin_x, origin_y, node_dx, node_dy);
        Ok(Self { trend, corr })
    }

    pub fn parts(&self) -> (TrendSurface, f64) {
        (self.trend.clone(), self.corr)
    }
}
