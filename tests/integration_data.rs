//! Integration over petekSim's owned DATA→SIM distribution seam.
//!
//! petekStatic owns and tests its optional petekIO→wireframe compatibility
//! adapter. Repeating those ingest, wireframe, and log-upscaling assertions here
//! made the release train retest an upstream implementation. This suite keeps the
//! one seam petekSim owns: mapping every neutral petekIO distribution DTO into a
//! petekStatic sampler.

use petekio::Distribution;
use peteksim::distribution_of;
use petekstatic::uncertainty::SplitMix64;

#[test]
fn uncertainty_path_handles_every_variant() {
    let mut rng = SplitMix64::new(42);
    let variants = [
        Distribution::Deterministic,
        Distribution::Uniform { lo: 1.0, hi: 5.0 },
        Distribution::Triangular {
            lo: 1.0,
            mode: 2.0,
            hi: 5.0,
        },
        Distribution::Normal {
            mean: 0.2,
            std: 0.02,
        },
        Distribution::LogNormal {
            mu: 0.0,
            sigma: 0.25,
        },
    ];

    for dto in variants {
        let mapped = distribution_of(dto, 3.0).expect("map distribution");
        assert!(mapped.sample(&mut rng).is_finite());
    }
}
