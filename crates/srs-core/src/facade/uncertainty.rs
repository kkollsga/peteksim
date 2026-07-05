//! The structured Monte Carlo over a regenerated template — the thin assembly of
//! `srs_model::McInputs` + `run_mc` + `tornado` + `aggregate_field`.
//!
//! Semantics (design Fix 3/4): the framework + upscaled cells are fixed template
//! state (area/gross pinned); each draw re-realizes the model, shifting modelled
//! property cubes by their per-draw level shift and moving the contacts by their
//! pick-spread. A property modelled by a pipeline (a cube in the template) takes
//! its uncertainty as a **level shift**; an un-modelled prior cube (PORO/NTG/SW)
//! takes it as the scalar input directly.

use crate::facade::model::Model;
use crate::facade::spec::DistSpec;
use petektools::sampling::reservoir_summary;
use srs_model::{
    aggregate_field, run_mc, tornado, Correlation, Input, McInputs, McResult, McSettings,
    TornadoBar,
};
use srs_units::SrsError;

/// The uncertainty run configuration (built from the Python kwargs).
#[derive(Debug, Clone, Default)]
pub struct McConfig {
    /// Porosity uncertainty (level shift on PORO if modelled, else scalar prior).
    pub porosity: Option<DistSpec>,
    pub net_to_gross: Option<DistSpec>,
    pub water_saturation: Option<DistSpec>,
    /// Explicit per-cube level shifts by name (e.g. a non-canonical "PHIE").
    pub extra_shifts: Vec<(String, DistSpec)>,
    /// Pick-spread (sd, m) on the lower contact (FWL/OWC).
    pub contact_sd_m: Option<f64>,
    /// Pick-spread (sd, m) on the GOC (two-contact models).
    pub goc_sd_m: Option<f64>,
    /// Oil FVF distribution (defaults to the model's fixed Boi).
    pub boi: Option<DistSpec>,
    /// Gas FVF distribution (defaults to the model's fixed Bgi).
    pub bgi: Option<DistSpec>,
    pub n: usize,
    pub seed: u64,
    /// Tornado pivot percentiles (statistical, `[0,100]`); default 10/90.
    pub lo_pct: f64,
    pub hi_pct: f64,
}

/// A P90/P50/P10/mean digest. The underlying realization samples are NOT stored
/// here — they live once on the [`McOutcome`]'s kept [`McResult`] and are read
/// back via [`McOutcome::stoiip_samples`] / [`McOutcome::giip_samples`] (no
/// second copy per leg).
#[derive(Debug, Clone)]
pub struct PSummary {
    pub p90: f64,
    pub p50: f64,
    pub p10: f64,
    pub mean: f64,
}

impl PSummary {
    /// The all-zero digest — a contactless zone / gas-less leg (no hydrocarbon).
    pub fn zero() -> Self {
        Self {
            p90: 0.0,
            p50: 0.0,
            p10: 0.0,
            mean: 0.0,
        }
    }
}

/// The outcome of an uncertainty run.
pub struct McOutcome {
    pub stoiip: PSummary,
    pub giip: PSummary,
    pub two_contact: bool,
    tornado: Vec<TornadoBar>,
    result: McResult,
}

impl McOutcome {
    /// The ranked tornado bars (oil in-place swing per input, descending).
    pub fn tornado(&self) -> &[TornadoBar] {
        &self.tornado
    }
    /// The per-realization STOIIP samples (draw order) — a view onto the kept
    /// result, not a copy.
    pub fn stoiip_samples(&self) -> &[f64] {
        &self.result.oil_sm3
    }
    /// The per-realization GIIP samples (draw order). Empty for a single-contact
    /// model (no gas leg) — the caller need not materialise a zero vector.
    pub fn giip_samples(&self) -> &[f64] {
        if self.two_contact {
            &self.result.gas_sm3
        } else {
            &[]
        }
    }
}

/// Run the structured MC for `model` under `cfg`.
pub fn run_uncertainty(model: &Model, cfg: McConfig) -> Result<McOutcome, SrsError> {
    if cfg.n == 0 {
        return Err(SrsError::InvalidInput("uncertainty needs n >= 1".into()));
    }
    let opts = model.opts();
    let (goc_m, fwl_m) = model.contacts();
    let modelled = model.pipeline_names();
    let has = |cube: &str| modelled.iter().any(|p| p == cube);

    // Fixed template geometry.
    let area = DistSpec::constant(opts.area_m2)?.to_input()?;
    let gross = DistSpec::constant(opts.gross_height_m)?.to_input()?;

    // Lower contact: pick-spread or fixed. When a GOC is present, clamp the
    // lower-contact draws to stay below it (a two-contact draw requires GOC above
    // OWC — the seam is fail-fast on a crossing).
    let contact = match cfg.contact_sd_m {
        Some(sd) if sd > 0.0 => {
            let d = DistSpec::normal(fwl_m, sd)?;
            match goc_m {
                Some(goc) => d.clamped(goc + 1.0, fwl_m + 1000.0)?.to_input()?,
                None => d.to_input()?,
            }
        }
        _ => DistSpec::constant(fwl_m)?.to_input()?,
    };

    // Prior scalars: the given dist drives the scalar input only when the cube is
    // NOT modelled by a pipeline; otherwise the scalar is fixed at the prior and
    // the dist becomes a level shift.
    let mut shifts: Vec<(String, Input)> = Vec::new();
    let porosity = scalar_or_shift(
        "PORO",
        cfg.porosity.as_ref(),
        opts.priors.porosity,
        has("PORO"),
        &mut shifts,
    )?;
    let ntg = scalar_or_shift(
        "NTG",
        cfg.net_to_gross.as_ref(),
        opts.priors.net_to_gross,
        has("NTG"),
        &mut shifts,
    )?;
    let sw = scalar_or_shift(
        "SW",
        cfg.water_saturation.as_ref(),
        opts.priors.water_saturation,
        has("SW"),
        &mut shifts,
    )?;
    for (name, dist) in &cfg.extra_shifts {
        shifts.push((name.clone(), dist.to_input()?));
    }

    let boi = match &cfg.boi {
        Some(d) => d.to_input()?,
        None => DistSpec::constant(model.boi())?.to_input()?,
    };

    let mut inputs = McInputs::new(area, gross, contact, porosity, ntg, sw, boi);
    if let Some(goc) = goc_m {
        let goc_in = match cfg.goc_sd_m {
            Some(sd) if sd > 0.0 => DistSpec::normal(goc, sd)?.to_input()?,
            _ => DistSpec::constant(goc)?.to_input()?,
        };
        inputs = inputs.with_goc(goc_in);
    }
    if let Some(bgi_val) = model.bgi() {
        let bgi_in = match &cfg.bgi {
            Some(d) => d.to_input()?,
            None => DistSpec::constant(bgi_val)?.to_input()?,
        };
        inputs = inputs.with_bgi(bgi_in);
    }
    for (name, input) in shifts {
        inputs = inputs.with_property_shift(name, input);
    }

    // Run + tornado on fresh templates.
    let mut tmpl = model.template()?;
    let result =
        run_mc(&mut tmpl, &inputs, &McSettings::new(cfg.n, cfg.seed)).map_err(SrsError::from)?;

    let (lo, hi) = (
        if cfg.lo_pct > 0.0 { cfg.lo_pct } else { 10.0 },
        if cfg.hi_pct > 0.0 { cfg.hi_pct } else { 90.0 },
    );
    let mut tmpl_t = model.template()?;
    let bars = tornado(&mut tmpl_t, &inputs, cfg.n, cfg.seed, lo, hi).map_err(SrsError::from)?;

    let two_contact = goc_m.is_some() && model.bgi().is_some();
    let oil = result.summary().map_err(SrsError::from)?;
    let stoiip = PSummary {
        p90: oil.p90,
        p50: oil.p50,
        p10: oil.p10,
        mean: oil.mean,
    };
    // Samples are NOT copied into the summary — they stay once on `result` and
    // are read via `McOutcome::{stoiip,giip}_samples`. A single-contact model has
    // no gas leg, so its GIIP digest is zero (and its samples view is empty).
    let giip = if two_contact {
        let g = result.gas_summary().map_err(SrsError::from)?;
        PSummary {
            p90: g.p90,
            p50: g.p50,
            p10: g.p10,
            mean: g.mean,
        }
    } else {
        PSummary {
            p90: 0.0,
            p50: 0.0,
            p10: 0.0,
            mean: 0.0,
        }
    };

    Ok(McOutcome {
        stoiip,
        giip,
        two_contact,
        tornado: bars,
        result,
    })
}

/// Route a prior scalar: fixed-at-prior + a level shift when `modelled`, else the
/// dist (or a fixed prior) as the scalar input.
fn scalar_or_shift(
    cube: &str,
    dist: Option<&DistSpec>,
    prior: f64,
    modelled: bool,
    shifts: &mut Vec<(String, Input)>,
) -> Result<Input, SrsError> {
    if modelled {
        if let Some(d) = dist {
            shifts.push((cube.to_string(), d.to_input()?));
        }
        DistSpec::constant(prior)?.to_input()
    } else {
        match dist {
            Some(d) => d.to_input(),
            None => DistSpec::constant(prior)?.to_input(),
        }
    }
}

/// The field-total digest from [`aggregate`]: the P-curve (via the same
/// `reservoir_summary` as a single segment, so a one-segment field matches it)
/// plus the aggregated samples.
#[derive(Debug, Clone)]
pub struct FieldSummary {
    pub p90: f64,
    pub p50: f64,
    pub p10: f64,
    pub mean: f64,
    pub samples: Vec<f64>,
}

/// `ps.aggregate([...], correlation=...)` — sum several segments' oil
/// realizations into a field total under the dependence assumption, summarised
/// with petekTools' oil-industry `reservoir_summary`.
pub fn aggregate(outcomes: &[&McOutcome], corr: Correlation) -> Result<FieldSummary, SrsError> {
    let results: Vec<&McResult> = outcomes.iter().map(|o| &o.result).collect();
    let samples = aggregate_field(&results, corr);
    if samples.is_empty() {
        return Ok(FieldSummary {
            p90: 0.0,
            p50: 0.0,
            p10: 0.0,
            mean: 0.0,
            samples,
        });
    }
    let s = reservoir_summary(&samples)?;
    Ok(FieldSummary {
        p90: s.p90,
        p50: s.p50,
        p10: s.p10,
        mean: s.mean,
        samples,
    })
}
