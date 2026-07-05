//! `Model` — a populated `StaticModel` plus the template ingredients (wireframe,
//! options, MC-mode property pipelines) and the fluid/FVF surface. `summary()`
//! reads the model's own volumetrics; `uncertainty(...)` builds the structured
//! MC over a regenerated template (see [`crate::core::facade::uncertainty`]).

use crate::core::facade::uncertainty::{run_uncertainty, McConfig, McOutcome};
use crate::core::facade::zoned_mc::{run_zoned_uncertainty, ZonedMcConfig, ZonedMcOutcome};
use crate::core::inplace::Fluid;
use crate::core::view::{IntersectionBundle, MapBundle, MapSpec, SectionSpec, VolumeBundle};
use crate::units::SrsError;
use petekio::GridGeometry;
use petekstatic::model::{
    BuildOpts, BuildWarning, ConstantPriors, GasFvf, HorizonStack, InPlace, McMode, OilFvf,
    PropertyPipeline, RealizationDraw, StaticModel, StaticModelTemplate, WellTie,
};
use petekstatic::wireframe::Wireframe;

/// The deterministic volumetric summary (SI): in-place in Sm³, GRV in mcm.
#[derive(Debug, Clone)]
pub struct Summary {
    pub fluid: String,
    /// Oil in-place \[Sm³\] (the oil leg for a two-contact model).
    pub stoiip_sm3: f64,
    /// Free-gas in-place \[Sm³\] (gas cap of a two-contact model; 0 otherwise).
    pub giip_sm3: f64,
    /// GRV of the hydrocarbon column \[mcm = 10⁶ m³\].
    pub grv_mcm: f64,
    /// Whether the model resolved a gas-cap + oil-rim split.
    pub two_contact: bool,
}

/// A non-blocking build advisory surfaced through the Python result — most
/// notably [`BuildWarning::ThinColumnsRepaired`], raised (not swallowed) when the
/// opt-in `min_thickness_m` order-repair pulled thin/crossing base columns down to
/// a minimum thickness below the top at a crossing margin.
#[derive(Debug, Clone)]
pub struct BuildWarn {
    /// A machine-readable tag (`"thin_columns_repaired"` / `"unused_horizon"`).
    pub kind: String,
    /// The human-readable advisory.
    pub message: String,
    /// Repaired-node count for a `ThinColumnsRepaired` advisory (else 0).
    pub columns: usize,
    /// The worst (most negative) original `base − top` separation among repaired
    /// nodes for a `ThinColumnsRepaired` advisory — negative = a true crossing
    /// (else 0.0).
    pub worst_m: f64,
}

/// A populated static model + its regeneration ingredients.
pub struct Model {
    model: StaticModel,
    wireframe: Wireframe,
    top_geom: GridGeometry,
    opts: BuildOpts,
    pipes: Vec<(PropertyPipeline, McMode)>,
    fluid: Fluid,
    boi: f64,
    bgi: Option<f64>,
    goc_m: Option<f64>,
    fwl_m: f64,
    /// The opt-in post-gridding order-repair floor, carried onto the MC template
    /// so realizations repair thin/crossing margins too (`None` = crossing guard).
    min_thickness_m: Option<f64>,
    /// Whether this model was built from a multi-zone horizon stack — then
    /// `summary()` reads the zoned rollup (whole-model `in_place()` mixes contacts
    /// across zones) and `in_place_by_zone`/`zone_stats`/`zoned_uncertainty` are
    /// available.
    zoned: bool,
    /// The multi-zone horizon stack (`Some` on a zoned build) — retained so
    /// `zoned_uncertainty` can regenerate a **stack-aware** MC template
    /// (`StaticModelTemplate::from_horizon_stack`) that varies per-zone contacts +
    /// per-zone property levels per draw. `None` on the single-framework path.
    stack: Option<HorizonStack>,
    /// Per-zone base priors set via `fw.set_zone_priors(zone, ..)` (`with_zone_priors`
    /// at build), by zone name. The zoned MC uses these as the per-zone base level a
    /// draw's per-zone level shift moves; a zone absent here uses `opts.priors`.
    zone_priors: Vec<(String, ConstantPriors)>,
    /// The opt-in Petrel-style cell-collapse floor, carried onto the stack MC
    /// template so realizations collapse sub-threshold cells exactly as the build did.
    collapse_below_m: Option<f64>,
    /// Per-zone property pipelines `(zone, pipeline, mode)` staged via
    /// `grid.property(name, zone=..)` — carried onto the zoned-MC stack template
    /// (`StaticModelTemplate::with_zone_property_mode`) so a zone piped through a
    /// zone-scoped upscale+SGS cube realizes from that ACTUAL cube (level shift on
    /// top of its per-zone priors), not from the zone priors alone. Empty on the
    /// single-framework path. (`question_zoned_mc_zone_pipe_parity`)
    zone_pipes: Vec<(String, PropertyPipeline, McMode)>,
    /// Per-horizon well ties (`fw.set_well_ties`) — carried onto the zoned-MC stack
    /// template (`StaticModelTemplate::with_well_ties`, draw-invariant, applied once
    /// at construction) so every realization inherits the tied geometry. Empty when
    /// no tie table was supplied.
    well_ties: Vec<WellTie>,
    /// Opt-in **sugar-cube** section rendering (`grid.model(sugar_cube=True)`):
    /// section cells render as flat boxes (edge arrays collapse to the centroid)
    /// instead of dip-following trapezoids. Carried onto the MC templates so every
    /// realization's section bundle renders identically to the base model.
    sugar_cube: bool,
    /// A **once-built, primed** stack-aware MC template, shared across every
    /// `zoned_uncertainty` call on this model (the zero-spread parity QC and the
    /// spread run each used to rebuild the ~1.4s template from scratch — resolve +
    /// LevelShift SGS baseline). Built lazily on the first `stack_template()` call
    /// and **primed** (one throwaway realize populates the draw-invariant LevelShift
    /// pattern caches), so subsequent callers clone a template that skips BOTH the
    /// surface re-resolve AND the SGS re-propagate. A pure function of the model's
    /// immutable build state, so a cached clone is byte-identical to a fresh build.
    /// `None` on a non-zoned model (only the stack path builds it).
    stack_tmpl: std::sync::OnceLock<StaticModelTemplate>,
}

/// One horizon's tie residual on one well, surfaced from the built model's
/// [`petekstatic::model::Provenance::well_ties`] (the engine `with_well_ties` seam).
#[derive(Debug, Clone)]
pub struct WellTieResidual {
    pub well_id: String,
    /// The framework horizon this tie is on.
    pub horizon: String,
    /// The measured formation-top depth \[m, positive-down\].
    pub measured_depth_m: f64,
    /// The untied model-surface depth at the well node \[m, positive-down\].
    pub model_depth_m: f64,
    /// `measured_depth_m − model_depth_m` (positive ⇒ the well is deeper than the
    /// untied model surface).
    pub residual_m: f64,
}

/// Per-zone in-place volumes off a multi-zone [`Model`] (`in_place_by_zone`).
#[derive(Debug, Clone)]
pub struct ZoneVolume {
    pub zone: String,
    /// Zone gross rock volume \[mcm = 10⁶ m³\].
    pub grv_mcm: f64,
    /// Zone hydrocarbon pore volume \[m³\] (0 for a contactless zone).
    pub hcpv_m3: f64,
    /// Oil in-place \[Sm³\] (the oil leg for a two-contact zone, else the zone HCPV).
    pub stoiip_sm3: f64,
    /// Free-gas in-place \[Sm³\] (gas cap of a two-contact zone; 0 otherwise).
    pub giip_sm3: f64,
    /// Whether the zone resolved a gas-cap + oil-rim split.
    pub two_contact: bool,
}

/// The full per-zone rollup: one [`ZoneVolume`] per zone (top→base) + the total
/// (the summed rollup; `total.stoiip_sm3 == Σ zone.stoiip_sm3`).
#[derive(Debug, Clone)]
pub struct ZonedVolumes {
    pub zones: Vec<ZoneVolume>,
    pub total: ZoneVolume,
}

/// Per-zone statistics of one property cube (`zone_stats`) — count/mean/min/max
/// over the zone's active (non-collapsed, finite) cells. `count == 0` ⇒ NaN
/// aggregates.
#[derive(Debug, Clone)]
pub struct ZoneStat {
    pub zone: String,
    pub count: usize,
    pub mean: f64,
    pub min: f64,
    pub max: f64,
}

impl Model {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        model: StaticModel,
        wireframe: Wireframe,
        top_geom: GridGeometry,
        opts: BuildOpts,
        pipes: Vec<(PropertyPipeline, McMode)>,
        fluid: Fluid,
        boi: f64,
        bgi: Option<f64>,
        goc_m: Option<f64>,
        fwl_m: f64,
        min_thickness_m: Option<f64>,
        zoned: bool,
        stack: Option<HorizonStack>,
        zone_priors: Vec<(String, ConstantPriors)>,
        collapse_below_m: Option<f64>,
        zone_pipes: Vec<(String, PropertyPipeline, McMode)>,
        well_ties: Vec<WellTie>,
        sugar_cube: bool,
    ) -> Self {
        Self {
            model,
            wireframe,
            top_geom,
            opts,
            pipes,
            fluid,
            boi,
            bgi,
            goc_m,
            fwl_m,
            min_thickness_m,
            zoned,
            stack,
            zone_priors,
            collapse_below_m,
            zone_pipes,
            well_ties,
            sugar_cube,
            stack_tmpl: std::sync::OnceLock::new(),
        }
    }

    /// Whether this model was built from a multi-zone horizon stack.
    pub fn is_zoned(&self) -> bool {
        self.zoned
    }

    /// Non-blocking advisories the build raised (loud, not swallowed) — e.g. a
    /// [`BuildWarning::ThinColumnsRepaired`] from an opt-in `min_thickness_m`
    /// order-repair, an unused supplied horizon, or (on the multi-zone stack path)
    /// `CellsCollapsed` and per-interface order repairs. Empty on a clean build.
    pub fn warnings(&self) -> Vec<BuildWarn> {
        let prov = self.model.provenance();
        let mut out: Vec<BuildWarn> = prov.warnings.iter().map(build_warn).collect();
        // The multi-zone stack records interface order-repairs in provenance (not
        // as BuildWarnings); surface them here so `model.warnings()` stays the one
        // loud channel for the zoned build too.
        if let Some(stack) = &prov.stack {
            for r in &stack.interface_repairs {
                out.push(BuildWarn {
                    kind: "interface_repaired".to_string(),
                    message: format!(
                        "zone interface {} pulled {} column(s) to the minimum thickness \
                         (worst original separation {:.3} m)",
                        r.interface, r.columns, r.worst_m
                    ),
                    columns: r.columns,
                    worst_m: r.worst_m,
                });
            }
        }
        out
    }

    /// The populated cube names carried by the model.
    pub fn property_names(&self) -> Vec<String> {
        self.model
            .property_names()
            .iter()
            .map(|s| s.to_string())
            .collect()
    }

    /// The deterministic volumetric summary. For a **zoned** model this is the
    /// per-zone rollup (each zone clipped vs its OWN contacts) — whole-model
    /// `in_place()` mixes contacts across zones and is not physically meaningful.
    pub fn summary(&self) -> Result<Summary, SrsError> {
        if self.zoned {
            let z = self.in_place_by_zone()?;
            return Ok(Summary {
                fluid: self.fluid.label().to_string(),
                stoiip_sm3: z.total.stoiip_sm3,
                giip_sm3: z.total.giip_sm3,
                grv_mcm: z.total.grv_mcm,
                two_contact: z.zones.iter().any(|zz| zz.two_contact),
            });
        }
        let ip = self.model.in_place().map_err(SrsError::from)?;
        let two_contact = ip.gas.is_some() && ip.oil.is_some();
        let boi = OilFvf::new(self.boi).map_err(SrsError::from)?;
        let stoiip = if two_contact {
            ip.oil_zone_ooip_sm3(boi)
        } else {
            ip.ooip_sm3(boi)
        };
        let giip = match (two_contact, self.bgi) {
            (true, Some(bgi)) => ip.gas_zone_ogip_sm3(GasFvf::new(bgi).map_err(SrsError::from)?),
            _ => 0.0,
        };
        Ok(Summary {
            fluid: self.fluid.label().to_string(),
            stoiip_sm3: stoiip,
            giip_sm3: giip,
            grv_mcm: ip.grv_mcm(),
            two_contact,
        })
    }

    /// Per-zone in-place volumes (multi-zone stack): each zone clipped vs its OWN
    /// contacts, plus the summed total (`total.stoiip_sm3 == Σ zone.stoiip_sm3`).
    /// A contactless zone contributes gross bulk with zero hydrocarbon.
    ///
    /// # Errors
    /// [`SrsError`] if the model carries no zones with contacts (a non-zoned model
    /// returns a single whole-column "zone"), or the FVF inputs are invalid.
    pub fn in_place_by_zone(&self) -> Result<ZonedVolumes, SrsError> {
        let z = self.model.in_place_by_zone().map_err(SrsError::from)?;
        let boi = OilFvf::new(self.boi).map_err(SrsError::from)?;
        let bgi = match self.bgi {
            Some(b) => Some(GasFvf::new(b).map_err(SrsError::from)?),
            None => None,
        };
        let zones: Vec<ZoneVolume> = z
            .zones
            .iter()
            .map(|zi| zone_volume(&zi.zone, &zi.in_place, boi, bgi))
            .collect();
        // Sum the per-zone legs for the total rather than reading the engine's
        // rolled-up InPlace: its `oil`/`gas` ZoneVolumes accumulate only from
        // TWO-contact zones, so a single-contact oil zone (Z2) would be dropped
        // from the total's oil leg. The engine total's grv/hcpv ARE full sums.
        let stoiip_sm3 = zones.iter().map(|zz| zz.stoiip_sm3).sum();
        let giip_sm3 = zones.iter().map(|zz| zz.giip_sm3).sum();
        let total = ZoneVolume {
            zone: "Total".to_string(),
            grv_mcm: z.total.grv_mcm(),
            hcpv_m3: z.total.hcpv_m3,
            stoiip_sm3,
            giip_sm3,
            two_contact: zones.iter().any(|zz| zz.two_contact),
        };
        Ok(ZonedVolumes { zones, total })
    }

    /// Per-zone statistics of a property cube — count/mean/min/max over each zone's
    /// active cells (`count == 0` ⇒ NaN aggregates).
    ///
    /// # Errors
    /// [`SrsError`] if the named cube is not populated.
    pub fn zone_stats(&self, property: &str) -> Result<Vec<ZoneStat>, SrsError> {
        let stats = self.model.zone_stats(property).map_err(SrsError::from)?;
        Ok(stats
            .iter()
            .map(|s| ZoneStat {
                zone: s.zone.clone(),
                count: s.count,
                mean: s.mean,
                min: s.min,
                max: s.max,
            })
            .collect())
    }

    /// The per-horizon-per-well tie residuals from the engine `with_well_ties` seam
    /// (`fw.set_well_ties(..)`), read off the built model's
    /// [`petekstatic::model::Provenance::well_ties`]. Empty when no tie table was supplied.
    /// Each mapped horizon's residual is `measured − untied model surface` at the
    /// well node; a tops-only horizon carries a QC-only residual.
    pub fn well_tie_residuals(&self) -> Vec<WellTieResidual> {
        let mut out = Vec::new();
        for rec in &self.model.provenance().well_ties {
            for r in &rec.residuals {
                out.push(WellTieResidual {
                    well_id: rec.id.clone(),
                    horizon: r.horizon.clone(),
                    measured_depth_m: r.measured_depth_m,
                    model_depth_m: r.model_depth_m,
                    residual_m: r.residual_m,
                });
            }
        }
        out
    }

    /// The multi-zone stack (zoned build only) — the source of the per-zone base
    /// contacts + zone order the zoned MC draws over.
    pub(crate) fn stack(&self) -> Option<&HorizonStack> {
        self.stack.as_ref()
    }

    /// The per-zone base priors (`with_zone_priors`), by zone name.
    pub(crate) fn zone_priors(&self) -> &[(String, ConstantPriors)] {
        &self.zone_priors
    }

    /// A **stack-aware** regeneration template (`from_horizon_stack` + the
    /// whole-model property pipelines in their MC modes), **built + primed once**
    /// and shared across every `zoned_uncertainty` call on this model. The template
    /// is a pure function of the model's immutable build state, so a cached clone is
    /// byte-identical to a fresh build; priming (one throwaway realize) populates the
    /// draw-invariant LevelShift SGS pattern caches so a clone skips both the surface
    /// re-resolve and the SGS re-propagate. Callers clone the returned template (one
    /// clone per shard). Errors on a non-zoned model.
    pub(crate) fn stack_template(&self) -> Result<StaticModelTemplate, SrsError> {
        if let Some(t) = self.stack_tmpl.get() {
            return Ok(t.clone());
        }
        let mut t = self.build_stack_template()?;
        self.prime_stack_template(&mut t)?;
        // A concurrent init would only lose the race harmlessly (identical template);
        // `zoned_uncertainty` calls are sequential in practice.
        let _ = self.stack_tmpl.set(t);
        Ok(self
            .stack_tmpl
            .get()
            .expect("just initialized above")
            .clone())
    }

    /// Build a fresh **stack-aware** template (`from_horizon_stack` + the whole-model
    /// property pipelines in their MC modes) — the resolve half of the cached
    /// `stack_template`. Errors on a non-zoned model.
    fn build_stack_template(&self) -> Result<StaticModelTemplate, SrsError> {
        let stack = self.stack.clone().ok_or_else(|| {
            SrsError::InvalidInput("stack_template() needs a multi-zone stack model".into())
        })?;
        // `self.stack` was conditioned ONCE at `build_grid` (all horizons `Mapped`),
        // so build the MC template via `from_horizon_stack` + `with_georef` — NO
        // re-conditioning (the canonical scatter is gridded once per model lifecycle,
        // `task_suite_scatter_perf`). Byte-for-byte the deterministic builder twin, so
        // `realize` reproduces the built geometry.
        let (gx, gy, gsx, gsy) = super::grid::georef_args(&self.top_geom);
        let mut t = StaticModelTemplate::from_horizon_stack(stack, self.opts)?
            .with_georef(gx, gy, gsx, gsy);
        t = t.with_sugar_cube(self.sugar_cube);
        t = t.with_min_thickness_m(self.min_thickness_m.unwrap_or(0.0));
        if let Some(cb) = self.collapse_below_m {
            t = t.with_collapse_below_m(cb);
        }
        for (pl, mode) in &self.pipes {
            t = t.with_property_mode(pl.clone(), *mode);
        }
        // Zone-scoped pipelines: realize honours the with_zone_property upscale+SGS
        // cube per draw over the per-zone priors (level shift on top), so a
        // zone-piped zone's zero-spread zoned MC reproduces in_place_by_zone rather
        // than realizing from the zone priors alone.
        for (zone, pl, mode) in &self.zone_pipes {
            t = t.with_zone_property_mode(zone.clone(), pl.clone(), *mode);
        }
        // Well ties: draw-invariant, applied once at template construction
        // (re-solve + repair); every draw inherits the tied geometry.
        if !self.well_ties.is_empty() {
            t = t.with_well_ties(self.well_ties.clone())?;
        }
        Ok(t)
    }

    /// **Prime** a freshly-built stack template: one throwaway realize over a base
    /// draw (global priors, every zone on its static contacts — no per-zone override)
    /// so the draw-invariant `McMode::LevelShift` SGS pattern caches populate on the
    /// SHARED template. Callers then clone a warm template, skipping the ~0.9 s
    /// re-propagate every clone otherwise pays on its own first realize. The realized
    /// model is discarded — priming only warms the caches (the pattern is a pure
    /// function of the pipeline + framework, independent of the draw, so this is
    /// byte-identical to a cold clone's first realize). The stack realize path never
    /// touches the warm-start seed, and `LayerScratch` clones empty, so a primed clone
    /// differs from a cold build only by the (identical) pre-filled caches.
    fn prime_stack_template(&self, t: &mut StaticModelTemplate) -> Result<(), SrsError> {
        let priors = self.opts.priors;
        let draw = RealizationDraw::new(
            self.opts.area_m2,
            self.opts.gross_height_m,
            // The top-level contact is unused on the stack path (per-zone contacts
            // govern); a finite sentinel satisfies `RealizationDraw::new`.
            1.0e6,
            priors.porosity,
            priors.net_to_gross,
            priors.water_saturation,
            0,
        );
        let mut model = t.reusable_model();
        t.realize_into(&draw, &mut model).map_err(SrsError::from)?;
        Ok(())
    }

    /// A fresh regeneration template (framework + property pipelines in their MC
    /// modes) — one per MC run (the seam shards one template per worker).
    pub(crate) fn template(&self) -> Result<StaticModelTemplate, SrsError> {
        let mut t = StaticModelTemplate::new(&self.wireframe, self.opts)?;
        // Same world georeference as the builder path (grid::model) so every MC
        // realization's view bundles emit the WORLD frame (column-centroid).
        let (gx, gy, gsx, gsy) = super::grid::georef_args(&self.top_geom);
        t = t.with_georef(gx, gy, gsx, gsy);
        t = t.with_sugar_cube(self.sugar_cube);
        if let Some(mt) = self.min_thickness_m {
            t = t.with_min_thickness_m(mt);
        }
        for (pl, mode) in &self.pipes {
            t = t.with_property_mode(pl.clone(), *mode);
        }
        Ok(t)
    }

    pub(crate) fn opts(&self) -> &BuildOpts {
        &self.opts
    }
    pub(crate) fn boi(&self) -> f64 {
        self.boi
    }
    pub(crate) fn bgi(&self) -> Option<f64> {
        self.bgi
    }
    pub(crate) fn contacts(&self) -> (Option<f64>, f64) {
        (self.goc_m, self.fwl_m)
    }
    pub(crate) fn pipeline_names(&self) -> Vec<String> {
        self.pipes
            .iter()
            .map(|(p, _)| p.name().to_string())
            .collect()
    }
    /// The shared areal lattice.
    pub fn geom(&self) -> &GridGeometry {
        &self.top_geom
    }

    /// The populated static model this facade wraps — the source of the view
    /// bundles below.
    pub fn static_model(&self) -> &StaticModel {
        &self.model
    }

    /// The areal (plan-view) [`MapBundle`] for the viewer.
    ///
    /// # Errors
    /// [`SrsError`] if the lattice is degenerate, a requested property is absent,
    /// or a `k_slice` index is out of range.
    pub fn map_bundle(&self, spec: &MapSpec) -> Result<MapBundle, SrsError> {
        self.model.map_bundle(spec).map_err(SrsError::from)
    }

    /// The vertical cross-section [`IntersectionBundle`] along `spec`, carrying an
    /// optional per-layer `property`.
    ///
    /// # Errors
    /// [`SrsError`] if the lattice is degenerate, the trace has fewer than two
    /// vertices, or the named property is absent.
    pub fn intersection_bundle(
        &self,
        spec: &SectionSpec,
        property: Option<&str>,
    ) -> Result<IntersectionBundle, SrsError> {
        self.model
            .intersection_bundle(spec, property)
            .map_err(SrsError::from)
    }

    /// The 3-D [`VolumeBundle`] (corner-point mesh) coloured by `property`.
    ///
    /// # Errors
    /// [`SrsError`] if `property` is absent.
    pub fn volume_bundle(&self, property: &str) -> Result<VolumeBundle, SrsError> {
        self.model.volume_bundle(property).map_err(SrsError::from)
    }

    /// Run the whole-model structured Monte Carlo over a regenerated template.
    ///
    /// # Errors
    /// [`SrsError::InvalidInput`] for a **zoned** model — a multi-zone stack has
    /// per-zone contacts, so its P-curve is per-zone (whole-model MC would mix
    /// contacts across zones). Use [`Self::zoned_uncertainty`] instead.
    pub fn uncertainty(&self, cfg: McConfig) -> Result<McOutcome, SrsError> {
        if self.zoned {
            return Err(SrsError::InvalidInput(
                "this is a multi-zone stack model — use zoned_uncertainty() for per-zone + \
                 total P-curves (whole-model uncertainty() would mix contacts across zones)"
                    .into(),
            ));
        }
        run_uncertainty(self, cfg)
    }

    /// Run the **stack-aware** structured Monte Carlo over a multi-zone model: each
    /// draw re-realizes the stack template with per-zone contact draws + per-zone
    /// property level shifts, and the per-draw per-zone volumes roll up into per-zone
    /// AND total STOIIP/GIIP P-curves. A contactless zone contributes GRV, zero
    /// hydrocarbon (its STOIIP/GIIP legs stay zero).
    ///
    /// # Errors
    /// [`SrsError::InvalidInput`] for a non-zoned model (use [`Self::uncertainty`]),
    /// or `n == 0`; [`SrsError`] on a failed realization/volumetrics draw.
    pub fn zoned_uncertainty(&self, cfg: ZonedMcConfig) -> Result<ZonedMcOutcome, SrsError> {
        if !self.zoned {
            return Err(SrsError::InvalidInput(
                "zoned_uncertainty() needs a multi-zone stack model — use uncertainty() for a \
                 single-framework model"
                    .into(),
            ));
        }
        run_zoned_uncertainty(self, cfg)
    }
}

/// One zone's (or the total's) volumes from an engine [`InPlace`] + FVFs. A
/// two-contact zone splits into oil/gas legs; a single-contact or contactless
/// zone reports its whole HCPV as oil (0 when contactless).
pub(crate) fn zone_volume(
    name: &str,
    ip: &InPlace,
    boi: OilFvf,
    bgi: Option<GasFvf>,
) -> ZoneVolume {
    let two_contact = ip.gas.is_some() && ip.oil.is_some();
    let stoiip_sm3 = if two_contact {
        ip.oil_zone_ooip_sm3(boi)
    } else {
        ip.ooip_sm3(boi)
    };
    let giip_sm3 = match (two_contact, bgi) {
        (true, Some(b)) => ip.gas_zone_ogip_sm3(b),
        _ => 0.0,
    };
    ZoneVolume {
        zone: name.to_string(),
        grv_mcm: ip.grv_mcm(),
        hcpv_m3: ip.hcpv_m3,
        stoiip_sm3,
        giip_sm3,
        two_contact,
    }
}

/// Map a petekStatic [`BuildWarning`] onto the facade's [`BuildWarn`] surface.
fn build_warn(w: &BuildWarning) -> BuildWarn {
    match w {
        BuildWarning::ThinColumnsRepaired { columns, worst_m } => BuildWarn {
            kind: "thin_columns_repaired".to_string(),
            message: format!(
                "post-gridding order-repair pulled {columns} thin/crossing base node(s) down to \
                 the minimum thickness below the top (worst original separation {worst_m:.3} m)"
            ),
            columns: *columns,
            worst_m: *worst_m,
        },
        BuildWarning::UnusedHorizon { name, role, reason } => BuildWarn {
            kind: "unused_horizon".to_string(),
            message: format!("horizon '{name}' ({role:?}) was not consumed: {reason}"),
            columns: 0,
            worst_m: 0.0,
        },
        BuildWarning::LayersTruncated { cells } => BuildWarn {
            kind: "layers_truncated".to_string(),
            // The engine warning is generic (any conformity truncates at a
            // pinch-out / merged envelope — a Proportional build over a zero-gross
            // interval collapses too), so report it honestly rather than pinning it
            // to Follow conformity.
            message: format!(
                "{cells} cell(s) collapsed to zero thickness (pinch-out / merged envelope) \
                 (zero volume — excluded from volumetrics, NaN-marked in the view bundles)"
            ),
            columns: *cells,
            worst_m: 0.0,
        },
        BuildWarning::LayerCountCapped { nk } => BuildWarn {
            kind: "layer_count_capped".to_string(),
            message: format!(
                "dz-derived layer count hit the {nk}-layer cap (dz finer than the cap can span \
                 over the thickest column); coarsen dz or accept the cap"
            ),
            columns: *nk,
            worst_m: 0.0,
        },
        BuildWarning::CellsCollapsed { cells } => BuildWarn {
            kind: "cells_collapsed".to_string(),
            message: format!(
                "{cells} sub-threshold cell(s) merged volume-conservingly into a thicker \
                 zone-interior neighbour (opt-in collapse_below_m; the multi-zone stack path)"
            ),
            columns: *cells,
            worst_m: 0.0,
        },
    }
}
