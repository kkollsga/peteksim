//! End-to-end integration over the DATA→MC/refine seam, from petekSim's side.
//!
//! These tests were lifted out of srs-data (now in petekStatic) because they need
//! petekSim-side pieces srs-data must not depend on: the `srs-uncertainty` sampler and
//! srs-core's refine loop. They drive a real petekio `GeoData` through
//! srs-data's adapter/wireframe, then apply the lifted [`srs_core::distribution_of`]
//! (DTO→sampler) mapping and the refine loop here.
//!
//! Fixtures (copied from petekio's repo) live in `tests/fixtures/`.
//!
//! SEAM NOTE: petekio's `SummaryInputs` is now SI (`area_m2`, `net_pay_m`), so
//! these consume the metric fields directly. Surface z follows petekio's
//! documented convention — **negative-down subsea elevation** (`simple.irap`
//! carries negative z) — and srs-data's `assemble_wireframe` negates it at its
//! ingest boundary onto the Wireframe's **positive-down `depth_m`** datum (the
//! Z1 datum unification, 2026-07-04). The end-to-end test asserts that
//! documented conversion, not value-pass-through.

use srs_core::{distribution_of, ConstantPriors, Fluid, RefiningModel, WireframeSeed};
use srs_data::logs::petro_samples;
use srs_data::petekio::{Distribution, GeoData, Unit};
use srs_data::wireframe::assemble_wireframe;
use srs_uncertainty::SplitMix64;
use srs_wireframe::{Contact, ContactKind, Hardness};

const SURFACE: &str = "tests/fixtures/simple.irap";
const WELL_DIR: &str = "tests/fixtures/wells/15_9-A1";

/// A real GeoData loaded from the shipped fixtures (a surface + a well).
fn loaded_geo() -> GeoData {
    let mut geo = GeoData::new(Unit::Feet);
    geo.load_surface("top", SURFACE).expect("load_surface");
    geo.load_well("15/9-A1", (1200.0, 1500.0), 0.0, WELL_DIR)
        .expect("load_well");
    geo
}

// Uncertainty path — the lifted DTO→sampler mapping maps + samples every variant.
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
    for d in variants {
        let mapped = distribution_of(d, 3.0).expect("map distribution");
        let sample = mapped.sample(&mut rng);
        assert!(sample.is_finite(), "{d:?} sampled non-finite");
    }
    // and the real summary distributions actually delivered, mapped at the seam
    let mi = loaded_geo().model_inputs().unwrap();
    for u in [
        mi.summary.area_m2,
        mi.summary.net_pay_m,
        mi.summary.porosity_frac,
        mi.summary.water_saturation_frac,
        mi.summary.net_to_gross_frac,
    ] {
        let mapped = distribution_of(u.distribution, u.value).expect("map real dist");
        assert!(mapped.sample(&mut rng).is_finite());
    }
}

// End-to-end golden — real GeoData -> model_inputs() -> data_to_wireframe -> grid.
#[test]
fn end_to_end_real_geodata_to_grid() {
    let geo = loaded_geo();
    let mi = geo.model_inputs().unwrap();

    let mut wf = assemble_wireframe(&mi).expect("assemble wireframe");
    assert!(!wf.horizons.is_empty());

    // KNOWN VALUE CONVERTS AT THE SEAM (Z1 datum unification): petekio delivers
    // surface z as NEGATIVE-down subsea elevation (the fixture's mean is
    // -1015.4545…); srs-data's assemble_wireframe negates it at its ingest
    // boundary, so the Wireframe horizon carries POSITIVE-down depth_m
    // (+1015.4545…). Assert the documented conversion: wf_mean == -surf_mean.
    let surf_mean = geo.surface("top").unwrap().stats().mean;
    assert!(
        surf_mean < 0.0,
        "fixture must carry petekio's negative-down elevation, got {surf_mean}"
    );
    let defined: Vec<f64> = wf.horizons[0]
        .surface
        .depth_m
        .iter()
        .copied()
        .filter(|d| !d.is_nan())
        .collect();
    let wf_mean = defined.iter().sum::<f64>() / defined.len() as f64;
    assert!(
        (wf_mean - (-surf_mean)).abs() < 1e-6,
        "the seam must negate elevation onto positive-down depth_m: {wf_mean} vs -({surf_mean})"
    );
    assert!(
        wf_mean > 0.0,
        "Wireframe depth_m must be positive-down, got {wf_mean}"
    );

    // The fixtures carry no OWC (None today), so supply a user contact below the top to
    // form a hydrocarbon column — a legitimate appraisal input, not a contract workaround.
    let top_max = wf.horizons[0]
        .surface
        .depth_m
        .iter()
        .copied()
        .filter(|d| !d.is_nan())
        .fold(f64::NEG_INFINITY, f64::max);
    wf.contacts.push(Contact {
        kind: ContactKind::Owc,
        depth_m: top_max + 100.0,
        hardness: Hardness::Assumed,
    });

    let seed = WireframeSeed {
        // Seam shim: petekio delivers acres; our seed takes m².
        area_m2: mi.summary.area_m2.value.max(1.0),
        gross_height_m: 15.0,
        nk: 3,
        priors: ConstantPriors {
            porosity: 0.25,
            net_to_gross: 0.8,
            water_saturation: 0.3,
        },
        fluid: Fluid::Oil,
        fvf: 1.25,
    };
    let refined = RefiningModel::from_wireframe(&wf, seed)
        .expect("seed from wireframe")
        .solve()
        .expect("converge + volumes");
    assert!(refined.in_place > 0.0, "in-place oil should be positive");
    assert!(refined.grv_mcm > 0.0, "GRV should be positive");
}

// Log -> cell upscaling: extract positioned (tvd,φ,Sw) samples from the real well
// curves and populate the grid through RefiningModel::with_logs.
#[test]
fn real_logs_extract_and_populate() {
    let geo = loaded_geo();
    let mi = geo.model_inputs().unwrap();

    let samples = petro_samples(&mi.spatial.well_curves);
    assert!(
        !samples.is_empty(),
        "real PHIE/SW curves yield paired (tvd,φ,Sw) samples"
    );
    for (_, p, s) in &samples {
        assert!((0.0..=1.0).contains(p), "porosity fraction {p}");
        assert!((0.0..=1.0).contains(s), "Sw fraction {s}");
    }

    // they wire into the refine loop (depth overlap with the grid is geometry-dependent,
    // so we assert the seam runs + produces a positive in-place, not specific cells).
    let mut wf = assemble_wireframe(&mi).unwrap();
    let top_max = wf.horizons[0]
        .surface
        .depth_m
        .iter()
        .copied()
        .filter(|d| !d.is_nan())
        .fold(f64::NEG_INFINITY, f64::max);
    wf.contacts.push(Contact {
        kind: ContactKind::Owc,
        depth_m: top_max + 100.0,
        hardness: Hardness::Assumed,
    });
    let seed = WireframeSeed {
        // Seam shim: petekio delivers acres; our seed takes m².
        area_m2: mi.summary.area_m2.value.max(1.0),
        gross_height_m: 15.0,
        nk: 3,
        priors: ConstantPriors {
            porosity: 0.25,
            net_to_gross: 0.8,
            water_saturation: 0.3,
        },
        fluid: Fluid::Oil,
        fvf: 1.25,
    };
    let refined = RefiningModel::from_wireframe(&wf, seed)
        .unwrap()
        .with_logs(samples)
        .solve()
        .unwrap();
    assert!(refined.in_place > 0.0);
}
