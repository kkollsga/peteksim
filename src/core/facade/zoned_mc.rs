//! The **stack-aware** structured Monte Carlo — the multi-zone analog of
//! [`crate::core::facade::uncertainty`]. petekStatic's whole-model driver
//! (`run_structured_mc`) rolls a single total in-place per draw, so per-zone
//! P-curves are unreachable through it; peteksim owns the *sampler* layer, so this
//! module drives petekStatic's per-draw primitive directly: it builds one
//! [`petekstatic::model::RealizationDraw`] per draw carrying a [`petekstatic::model::ZoneDraw`] per
//! zone (per-zone contact draws + per-zone property levels), realizes each on a
//! stack template ([`crate::core::facade::model::Model::stack_template`]), reads
//! [`petekstatic::model::StaticModel::in_place_by_zone`], and rolls the per-draw per-zone
//! volumes up into per-zone **and** total STOIIP/GIIP P-curves.
//!
//! Semantics (design Fix 3/4, extended per-zone): framework + upscaled cells are
//! fixed template state; each draw moves each zone's OWN contacts by their
//! pick-spread and shifts property levels. A **contactless** zone (no GOC/OWC)
//! contributes GRV but **zero** hydrocarbon, so its STOIIP/GIIP legs stay zero.
//! Conservation holds per draw: `total_stoiip[i] == Σ_z zone_stoiip[z][i]`.
//!
//! Reproducibility: every per-zone contact + level input is sampled off ONE seeded
//! stream in a fixed field order (mirroring [`petekstatic::model::McInputs::realize`]), and
//! each draw carries `seed_index = seed + i` — so `(cfg, n, seed)` gives a
//! bit-reproducible draw set. The parallel path shards the realize loop across
//! template clones (`StaticModelTemplate: Send + Clone`) and recombines in
//! draw-index order.

use crate::core::facade::model::{zone_volume, Model};
use crate::core::facade::spec::DistSpec;
use crate::core::facade::uncertainty::PSummary;
use crate::units::SrsError;
use petekstatic::model::{
    GasFvf, OilFvf, PerturbationField, RealizationDraw, StaticModelTemplate, ZoneDraw,
};
use petekstatic::wireframe::{Contact, ContactKind};
use petektools::sampling::{reservoir_summary, seeded_rng};
use rayon::prelude::*;
use std::ops::Range;

/// Per-zone MC overrides (`{zone: name, contacts spread + per-zone property level
/// shifts}`) layered over the run-wide [`ZonedMcConfig`] defaults. Every field is
/// optional; an absent field falls back to the run-wide default (or no draw).
#[derive(Debug, Clone, Default)]
pub struct ZoneMcSpec {
    /// The zone name (matched against the stack's zone names).
    pub zone: String,
    /// Lower-contact (OWC/FWL) pick-spread sd \[m\] for this zone; overrides the
    /// run-wide `contact_sd_m`.
    pub contact_sd_m: Option<f64>,
    /// GOC pick-spread sd \[m\] for this zone; overrides the run-wide `goc_sd_m`.
    pub goc_sd_m: Option<f64>,
    /// A per-zone porosity level shift/override distribution.
    pub porosity: Option<DistSpec>,
    /// A per-zone net-to-gross level shift/override distribution.
    pub net_to_gross: Option<DistSpec>,
    /// A per-zone water-saturation level shift/override distribution.
    pub water_saturation: Option<DistSpec>,
}

/// The zoned uncertainty run configuration (built from the Python kwargs).
#[derive(Debug, Clone, Default)]
pub struct ZonedMcConfig {
    /// Base porosity level uncertainty (a level shift on a modelled PORO cube, or a
    /// per-zone prior shift for a prior-populated PORO).
    pub porosity: Option<DistSpec>,
    pub net_to_gross: Option<DistSpec>,
    pub water_saturation: Option<DistSpec>,
    /// Run-wide lower-contact (OWC/FWL) pick-spread sd \[m\] (per-zone can override).
    pub contact_sd_m: Option<f64>,
    /// Run-wide GOC pick-spread sd \[m\] (per-zone can override).
    pub goc_sd_m: Option<f64>,
    /// Oil FVF distribution (defaults to the model's fixed Boi).
    pub boi: Option<DistSpec>,
    /// Gas FVF distribution (defaults to the model's fixed Bgi).
    pub bgi: Option<DistSpec>,
    /// Per-zone overrides, by zone name.
    pub per_zone: Vec<ZoneMcSpec>,
    /// Structural uncertainty on the **top** surface (`ps.hz` top row `sd`/`vgm`) —
    /// a correlated depth field stamped on every draw's [`RealizationDraw`]
    /// (`decision_structural_uncertainty_isochore`). `None` (or `sd_m <= 0`) is a no-op.
    pub top_structural: Option<PerturbationField>,
    /// Structural (isochore/thickness) uncertainty per zone, indexed by zone
    /// position (top→base) — `ps.hz` row `k` (`k >= 1`) drives zone `k-1`. A `None`
    /// entry (or a short vector) leaves that zone's thickness unperturbed.
    pub zone_isochore_structural: Vec<Option<PerturbationField>>,
    pub n: usize,
    pub seed: u64,
    /// Worker count for the sharded realize loop; `0`/`1` = serial.
    pub workers: usize,
}

/// One zone's MC P-curves (STOIIP + GIIP), plus the kept per-draw samples.
#[derive(Debug, Clone)]
pub struct ZonePCurves {
    pub zone: String,
    pub stoiip: PSummary,
    pub giip: PSummary,
    /// Whether the zone resolved a gas-cap + oil-rim split (has a gas leg).
    pub two_contact: bool,
    pub stoiip_samples: Vec<f64>,
    /// Empty for a single-contact / contactless zone (no gas leg).
    pub giip_samples: Vec<f64>,
}

/// The outcome of a zoned uncertainty run: per-zone P-curves + the summed total.
pub struct ZonedMcOutcome {
    pub zones: Vec<ZonePCurves>,
    pub total_stoiip: PSummary,
    pub total_giip: PSummary,
    pub total_stoiip_samples: Vec<f64>,
    pub total_giip_samples: Vec<f64>,
    /// Whether any zone has a gas leg.
    pub two_contact: bool,
}

/// The clamp keeping a drawn property level inside the engine's valid `[0, 1]`
/// (petekStatic's `realize` rejects a fraction outside it, H2).
const LEVEL_LO: f64 = 1.0e-4;
const LEVEL_HI: f64 = 1.0 - 1.0e-4;
/// The minimum GOC-below-OWC separation kept when both are drawn (a two-contact
/// draw requires the gas cap strictly shallower than the lower contact).
const CONTACT_GAP_M: f64 = 1.0;

/// One draw's per-zone `(stoiip, giip)` outputs (indexed by zone position,
/// top→base) — the realize loop's unit of output, transposed into per-zone vectors.
type ZoneRow = Vec<(f64, f64)>;

/// A per-zone pre-sampled draw plan (all `n` draws), assembled once off the seeded
/// stream and then indexed per draw when building the [`RealizationDraw`]s.
struct ZonePlan {
    idx: usize,
    /// This zone's lower-contact draws (empty ⇒ the zone has no lower contact).
    owc: Vec<f64>,
    /// This zone's GOC draws (empty ⇒ no gas cap).
    goc: Vec<f64>,
    /// Per-zone porosity level override draws (`None` ⇒ leave the cube / base prior).
    poro: Option<Vec<f64>>,
    ntg: Option<Vec<f64>>,
    sw: Option<Vec<f64>>,
}

/// Run the zoned structured MC for `model` under `cfg`.
pub fn run_zoned_uncertainty(
    model: &Model,
    cfg: ZonedMcConfig,
) -> Result<ZonedMcOutcome, SrsError> {
    if cfg.n == 0 {
        return Err(SrsError::InvalidInput(
            "zoned uncertainty needs n >= 1".into(),
        ));
    }
    let n = cfg.n;
    let stack = model
        .stack()
        .ok_or_else(|| SrsError::InvalidInput("zoned MC needs a stack model".into()))?;
    // Zone names now live on each `StackZone` (the old `HorizonStack::zone_names`
    // was folded into `StackZone::name`).
    let zone_names: Vec<String> = stack.zone_layers.iter().map(|z| z.name.clone()).collect();
    let n_zones = zone_names.len();
    let modelled = model.pipeline_names();
    let is_modelled = |p: &str| modelled.iter().any(|q| q == p);
    let global = model.opts().priors;
    let area = model.opts().area_m2;
    let gross = model.opts().gross_height_m;

    let mut rng = seeded_rng(cfg.seed);

    // --- fixed-order sampling (the reproducibility contract) --------------------
    // 1) FVFs.
    let boi = match &cfg.boi {
        Some(d) => d.sample_vec(n, &mut rng),
        None => vec![model.boi(); n],
    };
    let bgi: Option<Vec<f64>> = match model.bgi() {
        Some(b) => Some(match &cfg.bgi {
            Some(d) => d.sample_vec(n, &mut rng),
            None => vec![b; n],
        }),
        None => None,
    };
    // 2) whole-model base level shifts (only used for a MODELLED cube — a level
    // shift on the once-propagated pattern).
    let base_poro = cfg.porosity.as_ref().map(|d| d.sample_vec(n, &mut rng));
    let base_ntg = cfg.net_to_gross.as_ref().map(|d| d.sample_vec(n, &mut rng));
    let base_sw = cfg
        .water_saturation
        .as_ref()
        .map(|d| d.sample_vec(n, &mut rng));

    // 3) per-zone contacts + per-zone property levels (in stack order).
    let mut plans = Vec::with_capacity(n_zones);
    for (zi, name) in zone_names.iter().enumerate() {
        let per = cfg.per_zone.iter().find(|z| &z.zone == name);
        let (base_goc, base_owc) = zone_base_contacts(&stack.zone_layers[zi].contacts);
        let owc_sd = per.and_then(|p| p.contact_sd_m).or(cfg.contact_sd_m);
        let goc_sd = per.and_then(|p| p.goc_sd_m).or(cfg.goc_sd_m);
        let owc = draw_contact(base_owc, owc_sd, n, &mut rng)?;
        let mut goc = draw_contact(base_goc, goc_sd, n, &mut rng)?;
        // Keep the gas cap strictly above the lower contact (else realize rejects it).
        if !goc.is_empty() && !owc.is_empty() {
            for i in 0..n {
                goc[i] = goc[i].min(owc[i] - CONTACT_GAP_M);
            }
        }
        let zbase = zone_base_priors(model.zone_priors(), global, name);
        let has_explicit = model.zone_priors().iter().any(|(z, _)| z == name);
        let poro = zone_level(
            "PORO",
            is_modelled("PORO"),
            zbase.porosity,
            base_poro.as_deref(),
            per.and_then(|p| p.porosity.as_ref()),
            has_explicit,
            n,
            &mut rng,
        );
        let ntg = zone_level(
            "NTG",
            is_modelled("NTG"),
            zbase.net_to_gross,
            base_ntg.as_deref(),
            per.and_then(|p| p.net_to_gross.as_ref()),
            has_explicit,
            n,
            &mut rng,
        );
        let sw = zone_level(
            "SW",
            is_modelled("SW"),
            zbase.water_saturation,
            base_sw.as_deref(),
            per.and_then(|p| p.water_saturation.as_ref()),
            has_explicit,
            n,
            &mut rng,
        );
        plans.push(ZonePlan {
            idx: zi,
            owc,
            goc,
            poro,
            ntg,
            sw,
        });
    }

    // --- build the per-draw RealizationDraws -----------------------------------
    let deep = deepest_contact(&plans).unwrap_or(1.0e6);
    let mut draws = Vec::with_capacity(n);
    for i in 0..n {
        let mut d = RealizationDraw::new(
            area,
            gross,
            deep, // top-level contact is unused on the stack path (per-zone govern)
            global.porosity,
            global.net_to_gross,
            global.water_saturation,
            cfg.seed.wrapping_add(i as u64),
        );
        // Structural uncertainty: the SAME correlated top-depth field is stamped on
        // every draw (only `seed_index` varies — the engine seeds each field off it,
        // salted by horizon index). `sd_m <= 0` is a no-op at the engine.
        if let Some(field) = cfg.top_structural {
            d = d.with_top_structural(field);
        }
        // A modelled cube's base level moves via a whole-model level shift.
        if is_modelled("PORO") {
            if let Some(v) = &base_poro {
                d = d.with_property_shift("PORO", v[i]);
            }
        }
        if is_modelled("NTG") {
            if let Some(v) = &base_ntg {
                d = d.with_property_shift("NTG", v[i]);
            }
        }
        if is_modelled("SW") {
            if let Some(v) = &base_sw {
                d = d.with_property_shift("SW", v[i]);
            }
        }
        for p in &plans {
            let mut zd = ZoneDraw::new(p.idx);
            // Per-zone isochore (thickness) structural field — `ps.hz` row `p.idx + 1`.
            if let Some(Some(field)) = cfg.zone_isochore_structural.get(p.idx).copied() {
                zd = zd.with_isochore_structural(field);
            }
            if !p.owc.is_empty() {
                zd = zd.with_owc(p.owc[i]);
            }
            if !p.goc.is_empty() {
                zd = zd.with_goc(p.goc[i]);
            }
            if let Some(v) = &p.poro {
                zd.porosity = Some(v[i]);
            }
            if let Some(v) = &p.ntg {
                zd.net_to_gross = Some(v[i]);
            }
            if let Some(v) = &p.sw {
                zd.water_saturation = Some(v[i]);
            }
            d = d.with_zone_draw(zd);
        }
        draws.push(d);
    }

    // --- realize (serial or sharded) -------------------------------------------
    let base_tmpl = model.stack_template()?;
    let workers = if cfg.workers <= 1 {
        1
    } else {
        cfg.workers.min(n)
    };
    let rows: Vec<ZoneRow> = if workers == 1 {
        run_draws_range(&base_tmpl, &draws, &boi, bgi.as_deref(), 0..n, n_zones)?
    } else {
        let ranges = shard_ranges(n, workers);
        let (bt, dr, br) = (&base_tmpl, &draws, &boi);
        let bg = bgi.as_deref();
        let shards: Result<Vec<Vec<ZoneRow>>, SrsError> = ranges
            .into_par_iter()
            .map(|r| run_draws_range(bt, dr, br, bg, r, n_zones))
            .collect();
        shards?.into_iter().flatten().collect()
    };

    // --- roll up per-zone + total P-curves -------------------------------------
    let mut zones_out = Vec::with_capacity(n_zones);
    let mut total_oil = vec![0.0; n];
    let mut total_gas = vec![0.0; n];
    for zi in 0..n_zones {
        let mut oil = Vec::with_capacity(n);
        let mut gas = Vec::with_capacity(n);
        for (i, row) in rows.iter().enumerate() {
            let (o, g) = row[zi];
            oil.push(o);
            gas.push(g);
            total_oil[i] += o;
            total_gas[i] += g;
        }
        let two_contact = gas.iter().any(|&x| x > 0.0);
        zones_out.push(ZonePCurves {
            zone: zone_names[zi].clone(),
            stoiip: psummary(&oil)?,
            giip: if two_contact {
                psummary(&gas)?
            } else {
                PSummary::zero()
            },
            two_contact,
            stoiip_samples: oil,
            giip_samples: if two_contact { gas } else { Vec::new() },
        });
    }
    let any_two = zones_out.iter().any(|z| z.two_contact);
    Ok(ZonedMcOutcome {
        total_stoiip: psummary(&total_oil)?,
        total_giip: if any_two {
            psummary(&total_gas)?
        } else {
            PSummary::zero()
        },
        total_stoiip_samples: total_oil,
        total_giip_samples: if any_two { total_gas } else { Vec::new() },
        zones: zones_out,
        two_contact: any_two,
    })
}

/// Realize a contiguous draw `range` on a **clone** of `base_tmpl` (warm-start
/// chain serial within a shard) and return, per draw, the `(stoiip, giip)` per
/// zone (top→base, indexed by zone position). The shared body of the serial and
/// sharded drivers.
fn run_draws_range(
    base_tmpl: &StaticModelTemplate,
    draws: &[RealizationDraw],
    boi: &[f64],
    bgi: Option<&[f64]>,
    range: Range<usize>,
    n_zones: usize,
) -> Result<Vec<ZoneRow>, SrsError> {
    let mut tmpl = base_tmpl.clone();
    let mut model = tmpl.reusable_model();
    let mut out = Vec::with_capacity(range.len());
    for i in range {
        tmpl.realize_into(&draws[i], &mut model)
            .map_err(SrsError::from)?;
        let zoned = model.in_place_by_zone().map_err(SrsError::from)?;
        let oil_fvf = OilFvf::new(boi[i]).map_err(SrsError::from)?;
        let gas_fvf = match bgi {
            Some(b) => Some(GasFvf::new(b[i]).map_err(SrsError::from)?),
            None => None,
        };
        let mut row = vec![(0.0, 0.0); n_zones];
        for (zi, z) in zoned.zones.iter().enumerate().take(n_zones) {
            let zv = zone_volume(&z.zone, &z.in_place, oil_fvf, gas_fvf);
            row[zi] = (zv.stoiip_sm3, zv.giip_sm3);
        }
        out.push(row);
    }
    Ok(out)
}

/// Even contiguous split of `0..n` into `workers` ranges (remainder on the first
/// shards) — a deterministic function of `(n, workers)`.
fn shard_ranges(n: usize, workers: usize) -> Vec<Range<usize>> {
    let base = n / workers;
    let rem = n % workers;
    let mut ranges = Vec::with_capacity(workers);
    let mut start = 0;
    for w in 0..workers {
        let len = base + usize::from(w < rem);
        ranges.push(start..start + len);
        start += len;
    }
    ranges
}

/// The GOC (upper) and lower (OWC/FWL/GWC) base contact depths of a zone's static
/// contacts — `None` for a contact that is not present.
fn zone_base_contacts(contacts: &[Contact]) -> (Option<f64>, Option<f64>) {
    let mut goc = None;
    let mut lower = None;
    for c in contacts {
        match c.kind {
            ContactKind::Goc => goc = Some(c.depth_m),
            _ => lower = Some(c.depth_m),
        }
    }
    (goc, lower)
}

/// Draw a contact's `n` per-realization depths: `Normal(base, sd)` when a spread is
/// given, else the fixed base repeated. Empty when the zone has no such contact.
fn draw_contact(
    base: Option<f64>,
    sd: Option<f64>,
    n: usize,
    rng: &mut impl rand::Rng,
) -> Result<Vec<f64>, SrsError> {
    match base {
        None => Ok(Vec::new()),
        Some(b) => match sd {
            Some(s) if s > 0.0 => {
                let shifts = DistSpec::normal(0.0, s)?.sample_vec(n, rng);
                Ok(shifts.into_iter().map(|d| b + d).collect())
            }
            _ => Ok(vec![b; n]),
        },
    }
}

/// The effective per-draw property level for one zone/property, or `None` to leave
/// the cube / base prior untouched. A **modelled** cube keeps `None` unless a
/// per-zone override is asked (which then forces a per-zone level); a **prior**
/// property is set whenever there is any per-zone reason (an explicit zone prior, a
/// base level dist, or a per-zone override) so the per-zone base level is honoured.
#[allow(clippy::too_many_arguments)]
fn zone_level(
    _property: &str,
    modelled: bool,
    zone_base: f64,
    base_shift: Option<&[f64]>,
    zone_dist: Option<&DistSpec>,
    has_explicit_prior: bool,
    n: usize,
    rng: &mut impl rand::Rng,
) -> Option<Vec<f64>> {
    let zone_shift = zone_dist.map(|d| d.sample_vec(n, rng));
    if modelled {
        // A modelled cube's base level moves via the whole-model level shift; only a
        // per-zone override forces a per-zone constant here.
        zone_shift.map(|zs| {
            (0..n)
                .map(|i| clamp_level(zone_base + zs[i]))
                .collect::<Vec<_>>()
        })
    } else if has_explicit_prior || base_shift.is_some() || zone_shift.is_some() {
        Some(
            (0..n)
                .map(|i| {
                    let b = base_shift.map_or(0.0, |v| v[i]);
                    let z = zone_shift.as_ref().map_or(0.0, |v| v[i]);
                    clamp_level(zone_base + b + z)
                })
                .collect(),
        )
    } else {
        None
    }
}

fn clamp_level(v: f64) -> f64 {
    v.clamp(LEVEL_LO, LEVEL_HI)
}

/// The zone's base priors: an explicit `with_zone_priors` entry, else the global.
fn zone_base_priors(
    zone_priors: &[(String, petekstatic::model::ConstantPriors)],
    global: petekstatic::model::ConstantPriors,
    name: &str,
) -> petekstatic::model::ConstantPriors {
    zone_priors
        .iter()
        .find(|(z, _)| z == name)
        .map(|(_, p)| *p)
        .unwrap_or(global)
}

/// The deepest base lower-contact across the plans (a finite sentinel for the
/// unused top-level `contact_depth_m` on the stack path).
fn deepest_contact(plans: &[ZonePlan]) -> Option<f64> {
    plans
        .iter()
        .flat_map(|p| p.owc.first().copied())
        .filter(|d| d.is_finite())
        .fold(None, |acc, d| Some(acc.map_or(d, |a: f64| a.max(d))))
        .map(|d| d + 100.0)
}

fn psummary(v: &[f64]) -> Result<PSummary, SrsError> {
    let s = reservoir_summary(v)?;
    Ok(PSummary {
        p90: s.p90,
        p50: s.p50,
        p10: s.p10,
        mean: s.mean,
    })
}

#[cfg(test)]
mod tests {
    //! The zoned-MC driver on a hand-built 2-zone stack (Z0 contactless, Z1 single
    //! OWC): per-zone + total P-curves, contactless zones contribute zero HC, the
    //! total conserves the per-zone legs per draw, and a **zero-spread** MC
    //! reproduces the deterministic per-zone volumes (the cross-language parity
    //! identity the Python `test_synth_asset` asserts on the synthetic asset).
    use super::*;
    use crate::core::facade::grid::{Frame, StaticGrid};
    use crate::core::inplace::Fluid;
    use petekio::GridGeometry;
    use petekstatic::gridder::{Conformity, SolveOpts};
    use petekstatic::model::{
        BuildOpts, ConstantPriors, HorizonSource, HorizonStack, StackHorizon, StackZone,
    };
    use petekstatic::wireframe::{Boundary, Contact, ContactKind, GriddedDepth, Hardness};

    const N: usize = 11;
    const INC: f64 = 100.0;
    const SIDE: f64 = INC * (N - 1) as f64;

    fn geom() -> GridGeometry {
        GridGeometry {
            xori: 0.0,
            yori: 0.0,
            xinc: INC,
            yinc: INC,
            ncol: N,
            nrow: N,
            rotation_deg: 0.0,
            yflip: false,
        }
    }

    fn flat(depth: f64) -> GriddedDepth {
        GriddedDepth {
            ncol: N,
            nrow: N,
            depth_m: vec![depth; N * N],
            is_control: vec![true; N * N],
        }
    }

    fn mapped(name: &str, depth: f64) -> StackHorizon {
        StackHorizon {
            name: name.into(),
            source: HorizonSource::Mapped(flat(depth)),
        }
    }

    fn owc(depth_m: f64) -> Vec<Contact> {
        vec![Contact {
            kind: ContactKind::Owc,
            depth_m,
            hardness: Hardness::Hard,
        }]
    }

    /// A 3-horizon / 2-zone flat stack: Z0 (2000→2020) contactless, Z1 (2020→2040)
    /// with an OWC at 2030 (upper half oil).
    fn stack_grid() -> StaticGrid {
        let stack = HorizonStack {
            horizons: vec![
                mapped("H0", 2000.0),
                mapped("H1", 2020.0),
                mapped("H2", 2040.0),
            ],
            zone_layers: vec![
                StackZone::new("Z0", Conformity::Proportional, 4, Vec::new()),
                StackZone::new("Z1", Conformity::Proportional, 4, owc(2030.0)),
            ],
        };
        let opts = BuildOpts {
            area_m2: SIDE * SIDE,
            gross_height_m: 40.0,
            nk: 8,
            conformity: Conformity::Proportional,
            solve_opts: SolveOpts::default(),
            priors: ConstantPriors {
                porosity: 0.25,
                net_to_gross: 0.8,
                water_saturation: 0.3,
            },
        };
        let boundary = Boundary {
            ring: vec![
                [0.0, 0.0],
                [SIDE, 0.0],
                [SIDE, SIDE],
                [0.0, SIDE],
                [0.0, 0.0],
            ],
            hardness: Hardness::Interpolated,
        };
        let _ = boundary; // boundary lives on the wireframe path; the stack carries its own extent
        StaticGrid::new(
            Frame::Stack(stack),
            geom(),
            opts,
            Some(0.0),
            None,
            Vec::new(),
            Vec::new(),
            Vec::new(),
        )
    }

    fn cfg(n: usize, contact_sd_m: Option<f64>) -> ZonedMcConfig {
        ZonedMcConfig {
            contact_sd_m,
            boi: None,
            n,
            seed: 7,
            workers: 1,
            ..Default::default()
        }
    }

    #[test]
    fn zoned_mc_rolls_up_per_zone_and_conserves() {
        let model = stack_grid()
            .model(None, None, Fluid::Oil, 1.25, None, None, false)
            .unwrap();
        assert!(model.is_zoned());
        let out = model.zoned_uncertainty(cfg(48, Some(3.0))).unwrap();

        assert_eq!(out.zones.len(), 2);
        assert_eq!(out.zones[0].zone, "Z0");
        assert_eq!(out.zones[1].zone, "Z1");

        // Z0 is contactless -> zero hydrocarbon on every draw.
        assert!(out.zones[0].stoiip_samples.iter().all(|&v| v == 0.0));
        assert_eq!(out.zones[0].stoiip.mean, 0.0);
        // Z1 (single OWC) holds oil.
        assert!(out.zones[1].stoiip.mean > 0.0);

        // Conservation: the total conserves the per-zone legs on EVERY draw.
        for i in 0..out.total_stoiip_samples.len() {
            let sum: f64 = out.zones.iter().map(|z| z.stoiip_samples[i]).sum();
            assert!((out.total_stoiip_samples[i] - sum).abs() < 1e-6);
        }
        // Ordered P-curve.
        let t = &out.total_stoiip;
        assert!(t.p90 <= t.p50 && t.p50 <= t.p10);
    }

    #[test]
    fn zoned_mc_zero_spread_matches_deterministic() {
        // With no contact spread + no property dists, every draw reproduces the
        // deterministic build, so the MC per-zone mean == in_place_by_zone (the
        // parity identity; the Python test asserts the same on the synth asset).
        let model = stack_grid()
            .model(None, None, Fluid::Oil, 1.25, None, None, false)
            .unwrap();
        let det = model.in_place_by_zone().unwrap();
        let out = model.zoned_uncertainty(cfg(8, None)).unwrap();
        for (zi, z) in out.zones.iter().enumerate() {
            let d = det.zones[zi].stoiip_sm3;
            assert!(
                (z.stoiip.mean - d).abs() <= 1e-6 * d.abs().max(1.0),
                "zone {} MC mean {} != deterministic {}",
                z.zone,
                z.stoiip.mean,
                d
            );
        }
        assert!(
            (out.total_stoiip.mean - det.total.stoiip_sm3).abs()
                <= 1e-6 * det.total.stoiip_sm3.abs().max(1.0)
        );
    }
}
