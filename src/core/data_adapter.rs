//! The DTO→sampler mapping, lifted UP from the (now-downstream) srs-data adapter.
//!
//! srs-data (in petekStatic) carries petekio's **neutral** [`petekio::Distribution`]
//! DTO on every `InputScalar` — it does no Monte-Carlo sampling and must not depend on
//! petekSim's sampler. The conversion into the sampler type ([`Distribution`]) is the
//! simulation layer's job, so it lives here in srs-core and is applied where petekSim
//! consumes the data for the MC loop (per realization).

use crate::units::SrsError;
use petekstatic::uncertainty::Distribution;

/// petikio `Distribution` DTO → srs-uncertainty [`Distribution`] (the sampler's type).
///
/// `value` supplies the deterministic point for the `Deterministic` DTO variant.
///
/// # Errors
/// [`SrsError::InvalidInput`] if the parameters fail srs-uncertainty's validation
/// (e.g. `min >= max`, `sd <= 0`). petikio is expected to deliver valid parameters;
/// surfacing the error keeps a bad bundle from silently producing a garbage P-curve.
pub fn distribution_of(d: petekio::Distribution, value: f64) -> Result<Distribution, SrsError> {
    Ok(match d {
        petekio::Distribution::Deterministic => Distribution::Constant(value),
        petekio::Distribution::Uniform { lo, hi } => Distribution::uniform(lo, hi)?,
        petekio::Distribution::Triangular { lo, mode, hi } => {
            Distribution::triangular(lo, mode, hi)?
        }
        petekio::Distribution::Normal { mean, std } => Distribution::normal(mean, std)?,
        petekio::Distribution::LogNormal { mu, sigma } => Distribution::lognormal(mu, sigma)?,
    })
}

/// Rescale an **area** distribution expressed in km² into the core's m² (× 10⁶).
/// Exact for every supported family: constant/uniform/triangular/normal are
/// location-scale (parameters scale linearly); a lognormal scales by shifting
/// `mu` by `ln(10⁶)` (`k·exp(N(mu, σ)) = exp(N(mu + ln k, σ))`). This keeps the
/// user-facing area parameters in natural km² units while the volumetric core
/// works in m². Lives here (not in the binding) so the unit convention is one
/// tested seam, not marshalling glue.
pub fn scale_area_km2_to_m2(d: Distribution) -> Distribution {
    // petekTools owns the km²→m² scale (its `units` module is the single home);
    // consume it rather than re-declaring the `1e6` literal here.
    let k = petektools::units::M2_PER_KM2;
    match d {
        Distribution::Constant(v) => Distribution::Constant(v * k),
        Distribution::Uniform { min, max } => Distribution::Uniform {
            min: min * k,
            max: max * k,
        },
        Distribution::Triangular { min, mode, max } => Distribution::Triangular {
            min: min * k,
            mode: mode * k,
            max: max * k,
        },
        Distribution::Normal { mean, sd } => Distribution::Normal {
            mean: mean * k,
            sd: sd * k,
        },
        Distribution::Lognormal { mu, sigma } => Distribution::Lognormal {
            mu: mu + k.ln(),
            sigma,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distribution_variants_map_across() {
        assert_eq!(
            distribution_of(petekio::Distribution::Deterministic, 7.0).unwrap(),
            Distribution::Constant(7.0)
        );
        assert_eq!(
            distribution_of(
                petekio::Distribution::Triangular {
                    lo: 1.0,
                    mode: 2.0,
                    hi: 3.0
                },
                2.0
            )
            .unwrap(),
            Distribution::Triangular {
                min: 1.0,
                mode: 2.0,
                max: 3.0
            }
        );
        assert_eq!(
            distribution_of(
                petekio::Distribution::Normal {
                    mean: 0.2,
                    std: 0.02
                },
                0.2
            )
            .unwrap(),
            Distribution::Normal {
                mean: 0.2,
                sd: 0.02
            }
        );
    }

    #[test]
    fn invalid_distribution_params_surface_an_error() {
        // min >= max must propagate, not be silently swallowed.
        assert!(distribution_of(petekio::Distribution::Uniform { lo: 5.0, hi: 5.0 }, 5.0).is_err());
    }
}
