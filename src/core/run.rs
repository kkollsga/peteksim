//! Run a [`ModelInputs`]: a deterministic P50 point estimate and a seeded Monte
//! Carlo over the input distributions, both via the analytic box in-place.

use crate::core::inplace::BoxDraw;
use crate::core::model::{ModelInputs, ModelResult};
use crate::units::SrsError;
use petekstatic::uncertainty::{run, Distribution};
use petektools::units::m3_to_mcm;

/// Build a [`BoxDraw`] for the given cumulative probability `u` of every input
/// (deterministic uses `u = 0.5`).
fn draw_at(inputs: &ModelInputs, mut sample: impl FnMut(Distribution) -> f64) -> BoxDraw {
    BoxDraw {
        area_m2: sample(inputs.area_m2),
        gross_height_m: sample(inputs.gross_height_m),
        top_depth_m: inputs.top_depth_m,
        contact_depth_m: inputs.contact_depth_m,
        porosity: sample(inputs.porosity),
        net_to_gross: sample(inputs.net_to_gross),
        water_saturation: sample(inputs.water_saturation),
        fvf: sample(inputs.fvf),
    }
}

/// Run the model: deterministic P50 estimate + `n` Monte Carlo realizations.
/// In-place values are Sm³; the deterministic GRV reports in mcm (10⁶ m³).
///
/// # Errors
/// [`SrsError`] if the median (deterministic) draw is non-physical (H2:
/// φ/NG/Sw ∉ `[0,1]`, non-positive area/height, invalid FVF) or if `n == 0`
/// (H1: no realizations to summarise). Both surface at the FFI as `ValueError`
/// instead of a garbage number / interpreter-aborting panic.
pub fn run_model(inputs: &ModelInputs, n: usize, seed: u64) -> Result<ModelResult, SrsError> {
    // Deterministic: every input at its median. Validate before computing so a
    // constant garbage input (φ=-0.1, fvf=0, …) is a typed error, not -19 MSm³.
    let p50 = draw_at(inputs, |d| d.quantile(0.5));
    p50.validate(inputs.fluid)?;
    let deterministic_in_place = p50.in_place(inputs.fluid);
    let deterministic_grv_mcm = m3_to_mcm(p50.grv_m3());

    // Monte Carlo: independent draws per input (correlation deferred). The loop is
    // byte-identical to before for valid inputs — parity of run_box_model rests on
    // this closure being unchanged.
    let fluid = inputs.fluid;
    let realizations = run(n, seed, |rng| {
        let d = draw_at(inputs, |dist| dist.sample(rng));
        d.in_place(fluid)
    });

    // Summarise, then hand the raw sample out unchanged (W17): the summary is
    // derived from these exact values, so callers can rebuild any percentile.
    let summary = realizations.summary()?;
    Ok(ModelResult {
        fluid,
        deterministic_in_place,
        deterministic_grv_mcm,
        summary,
        realizations: n,
        samples: realizations.values,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::inplace::Fluid;
    use petekstatic::grid::Dims;

    fn oil_model() -> ModelInputs {
        ModelInputs {
            area_m2: Distribution::triangular(320_000.0, 400_000.0, 520_000.0).unwrap(),
            gross_height_m: Distribution::triangular(12.0, 15.0, 20.0).unwrap(),
            porosity: Distribution::Constant(0.25),
            net_to_gross: Distribution::Constant(0.8),
            water_saturation: Distribution::Constant(0.3),
            fvf: Distribution::Constant(1.25),
            fluid: Fluid::Oil,
            dims: Dims::new(10, 10, 5).unwrap(),
            top_depth_m: 1500.0,
            contact_depth_m: 2743.0,
            aspect_ratio: 1.0,
        }
    }

    #[test]
    fn deterministic_matches_p50_draw() {
        let m = oil_model();
        let r = run_model(&m, 10_000, 1).unwrap();
        // With only area & height uncertain (triangular), the deterministic
        // estimate sits near the MC median.
        assert!((r.deterministic_in_place - r.summary.p50).abs() / r.summary.p50 < 0.03);
    }

    #[test]
    fn percentiles_are_ordered_and_reproducible() {
        let m = oil_model();
        let a = run_model(&m, 20_000, 42).unwrap();
        let b = run_model(&m, 20_000, 42).unwrap();
        assert!(a.summary.p90 < a.summary.p50 && a.summary.p50 < a.summary.p10);
        assert_eq!(a.summary.p50, b.summary.p50);
        assert_eq!(a.summary.p10, b.summary.p10);
    }

    #[test]
    fn deterministic_grv_is_positive() {
        let r = run_model(&oil_model(), 100, 7).unwrap();
        assert!(r.deterministic_grv_mcm > 0.0);
        // Sanity band: triangular medians sit near the modes (400 000 m² × 15 m
        // = 6 mcm), so the deterministic GRV lands in the same decade.
        assert!((4.0..9.0).contains(&r.deterministic_grv_mcm));
    }

    #[test]
    fn zero_realizations_is_a_typed_error() {
        // H1: no realizations -> typed error, not a panic (surfaces as ValueError).
        assert!(run_model(&oil_model(), 0, 1).is_err());
    }

    #[test]
    fn non_physical_inputs_are_typed_errors() {
        // H2: the exact garbage the validator drove through the wheel.
        let mut bad_phi = oil_model();
        bad_phi.porosity = Distribution::Constant(-0.1); // was a large negative OOIP
        assert!(run_model(&bad_phi, 1000, 1).is_err());

        let mut bad_sw = oil_model();
        bad_sw.water_saturation = Distribution::Constant(1.2); // was negative
        assert!(run_model(&bad_sw, 1000, 1).is_err());

        let mut bad_fvf = oil_model();
        bad_fvf.fvf = Distribution::Constant(0.0); // was inf
        assert!(run_model(&bad_fvf, 1000, 1).is_err());
    }
}
