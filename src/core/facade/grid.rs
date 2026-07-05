//! `StaticGrid` — a frozen framework accumulating per-property pipelines
//! (upscale → QC → propagate), then `model(contacts)` to build the populated
//! `StaticModel`. Every property step is a thin call into
//! `petekstatic::model::PropertyPipeline`; positioned well logs are extracted from the
//! petekio wells here (the "positioned logs" wrapper).

use crate::core::facade::model::Model;
use crate::core::facade::project::BoreWell;
use crate::core::facade::spec::TrendSpec;
use crate::core::inplace::Fluid;
use crate::units::SrsError;
use petekio::GridGeometry;
use petekstatic::model::{
    BuildOpts, ConstantPriors, Gaussian, Georef, HorizonStack, McMode, MemoryBudget,
    PropertyPipeline, StackFrame, StaticModel, StaticModelBuilder, UpscaleMethod, UpscaleQc,
    WellLog, WellTie,
};
use petekstatic::wireframe::{Contact, ContactKind, Hardness, Wireframe};
use petektools::Variogram;
use std::collections::BTreeMap;

/// The frozen structural framework a [`StaticGrid`] builds from: either the
/// single-`ZoneTable` wireframe (`from_wireframe`) or a multi-zone horizon stack
/// (per-zone conformity + contacts).
///
/// **Invariant:** a `Stack` is **pre-conditioned** — [`Framework::build_grid`]
/// grids every raw-scatter horizon onto the frame lattice ONCE
/// ([`StaticModelBuilder::condition_scatter_stack`]) before it lands here, so the
/// stack is all-[`petekstatic::model::HorizonSource::Mapped`]. Every consumer
/// (`model`, `scratch_grid`, and the built [`Model`]'s MC template) therefore
/// reuses it via `from_horizon_stack` with **no** re-conditioning — the canonical
/// model's expensive per-horizon bilinear solve runs exactly once per model
/// lifecycle (`task_suite_scatter_perf`), not once per build/QC/MC path.
pub(crate) enum Frame {
    Wireframe(Wireframe),
    Stack(HorizonStack),
}

/// A property pipeline under construction on a [`StaticGrid`].
#[derive(Clone)]
struct PendingPipe {
    wells: Vec<WellLog>,
    method: UpscaleMethod,
    gaussian: Option<Gaussian>,
    resimulate: bool,
}

impl Default for PendingPipe {
    fn default() -> Self {
        Self {
            wells: Vec::new(),
            method: UpscaleMethod::Arithmetic,
            gaussian: None,
            resimulate: false,
        }
    }
}

/// A frozen framework + build options + pending property pipelines.
pub struct StaticGrid {
    frame: Frame,
    top_geom: GridGeometry,
    opts: BuildOpts,
    pipes: BTreeMap<String, PendingPipe>,
    /// Per-zone property pipelines (`grid.property(name, zone=..)`), keyed
    /// `(zone, mnemonic)` — threaded to the stack build's
    /// [`StaticModelBuilder::with_zone_property`] (a per-zone distribution/variogram
    /// over that zone's `k`-range). Ignored on the single-framework path.
    zone_pipes: BTreeMap<(String, String), PendingPipe>,
    /// Per-zone base priors (`fw.set_zone_priors`) → `with_zone_priors` on the stack.
    zone_priors: Vec<(String, ConstantPriors)>,
    /// Per-horizon well ties (`fw.set_well_ties`) → `with_well_ties` on the stack.
    well_ties: Vec<WellTie>,
    /// Per-horizon-per-well tie residuals from the framework (carried so the viewer
    /// payload can surface them on the wells).
    ties: Vec<crate::core::facade::framework::TieResidual>,
    /// Post-gridding order-repair floor applied at the build seam
    /// (`StaticModelBuilder::with_min_thickness_m`); carried onto the built
    /// [`Model`] so its MC template repairs per realization too. `None` = the
    /// crossing guard errors on a crossed base.
    min_thickness_m: Option<f64>,
    /// Opt-in Petrel-style cell-collapse floor for the multi-zone stack build
    /// (`StaticModelBuilder::with_collapse_below_m`); `None` = no collapse.
    collapse_below_m: Option<f64>,
}

impl StaticGrid {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        frame: Frame,
        top_geom: GridGeometry,
        opts: BuildOpts,
        min_thickness_m: Option<f64>,
        collapse_below_m: Option<f64>,
        ties: Vec<crate::core::facade::framework::TieResidual>,
        zone_priors: Vec<(String, ConstantPriors)>,
        well_ties: Vec<WellTie>,
    ) -> Self {
        Self {
            frame,
            top_geom,
            opts,
            pipes: BTreeMap::new(),
            zone_pipes: BTreeMap::new(),
            zone_priors,
            well_ties,
            min_thickness_m,
            collapse_below_m,
            ties,
        }
    }

    /// Whether this grid is a multi-zone stack (per-zone contacts drive the build,
    /// so `model(...)` does not need whole-model contacts).
    pub fn is_stacked(&self) -> bool {
        matches!(self.frame, Frame::Stack(_))
    }

    /// The shared areal lattice (for building collocated trends externally).
    pub fn geom(&self) -> &GridGeometry {
        &self.top_geom
    }

    /// The per-horizon-per-well tie residuals carried from the framework.
    pub fn ties(&self) -> &[crate::core::facade::framework::TieResidual] {
        &self.ties
    }

    /// The successful (`ok`) surface-tie residuals that belong to bore `well_id`,
    /// as `(horizon, residual_m)` — the viewer attaches these to the bore's
    /// payload. A tie row keyed on a well matches its own bores: a bore id like
    /// `"99_9-1 A"` matches a tie keyed on that exact id **or** on its parent well
    /// `"99_9-1"` (the space-delimited head). This bore↔parent-well matching rule
    /// lives here so the binding stays a thin marshaller.
    pub fn well_ties_for(&self, well_id: &str) -> Vec<(String, f64)> {
        let parent = well_id.split(' ').next().unwrap_or("");
        self.ties
            .iter()
            .filter(|t| t.ok && (t.well_id == well_id || t.well_id == parent))
            .map(|t| (t.horizon.clone(), t.residual_m))
            .collect()
    }

    /// Set the upscale step for property `mnemonic` from the given wells: reads
    /// each well's `mnemonic` log, positions its samples by the trajectory,
    /// **registers each well's world (x, y) onto the model's areal frame** (F5),
    /// and upscales by `method`. Returns the count of positioned samples found.
    ///
    /// `net_cutoff = Some(cutoff)` masks the conditioning samples to **net rock**
    /// before upscaling: a sample is kept only where the bore's positioned `NTG`
    /// curve (a net-to-gross / pay flag) exceeds `cutoff` at that depth (bores
    /// carrying no `NTG` curve keep every sample — "where present"). This is a
    /// facade-side sample filter (no engine change): a conditioned cube then
    /// reflects net rock rather than the full log incl. any non-net (e.g. aquifer)
    /// interval. `None` upscales every finite sample.
    ///
    /// # Errors
    /// [`SrsError::InvalidInput`] if samples were positioned but **no** well maps
    /// onto the framework's areal extent (the loud "0 cells conditioned" guard:
    /// real UTM well coordinates that fall outside the horizons' georeference —
    /// previously this slipped through to an opaque downstream `propagate`
    /// failure).
    ///
    /// `zone = Some(name)` scopes the upscale to a **per-zone** pipeline (a
    /// multi-zone stack only) threaded to `with_zone_property`; `None` is the
    /// whole-model pipeline.
    pub fn set_upscale(
        &mut self,
        mnemonic: &str,
        wells: &[BoreWell<'_>],
        method: UpscaleMethod,
        net_cutoff: Option<f64>,
        zone: Option<&str>,
    ) -> Result<usize, SrsError> {
        let logs = positioned_logs(wells, mnemonic, net_cutoff);
        let n: usize = logs.iter().map(|w| w.samples.len()).sum();
        let registered = self.register_logs(&logs);
        if n > 0 && registered.is_empty() {
            return Err(SrsError::InvalidInput(format!(
                "property '{mnemonic}': {n} log samples positioned, but no well maps onto the \
                 model grid — the well (x, y) fall outside the framework's areal extent. Check \
                 that the wells and horizons share a georeference/CRS (the model grid is \
                 area-scaled onto the horizon lattice; wells at a foreign CRS condition 0 cells)."
            )));
        }
        let pipe = self.pipe_entry(mnemonic, zone);
        pipe.wells = registered;
        pipe.method = method;
        Ok(n)
    }

    /// The pending pipe for `mnemonic`, whole-model (`zone = None`) or per-zone.
    fn pipe_entry(&mut self, mnemonic: &str, zone: Option<&str>) -> &mut PendingPipe {
        match zone {
            Some(z) => self
                .zone_pipes
                .entry((z.to_string(), mnemonic.to_string()))
                .or_default(),
            None => self.pipes.entry(mnemonic.to_string()).or_default(),
        }
    }

    /// Register world-XY well logs onto the model's **local areal frame** (F5 —
    /// the R4 georeference gap on the conditioning seam).
    ///
    /// The petekStatic grid seam builds an **area-scaled square at local origin**
    /// (`layer_grid` places pillar node `(ip, jp)` at `(ip·dx, jp·dy)`, `dx =
    /// √area / ni`; the horizons' world origin + spacing are not carried onto the
    /// pillars — a documented petekStatic limitation). So a well carrying real
    /// UTM `(x, y)` snaps to zero cells unless it is first registered.
    ///
    /// We keep the GRV-defining area scaling untouched (volumetrics fidelity) and
    /// instead map each well through the **real** top-surface geometry
    /// ([`GridGeometry::xy_to_ij`], which absorbs any rotation/`yflip`) to its
    /// fractional column, then place it at that column's centre in the local
    /// frame — so `PropertyPipeline`'s reconstructed areal lattice snaps it back
    /// to the intended cell. Wells outside the horizon extent are dropped (they
    /// carry no conditioning). The spacing formula mirrors `srs_model`'s
    /// `spacing(area_m2, ni, nj)` at this seam.
    fn register_logs(&self, logs: &[WellLog]) -> Vec<WellLog> {
        let g = &self.top_geom;
        let ni = g.ncol.saturating_sub(1).max(1);
        let nj = g.nrow.saturating_sub(1).max(1);
        let side = self.opts.area_m2.max(0.0).sqrt();
        let dx = side / ni as f64;
        let dy = side / nj as f64;
        logs.iter()
            .filter_map(|w| {
                let (fi, fj) = g.xy_to_ij(w.x, w.y)?;
                if !(fi.is_finite() && fj.is_finite()) {
                    return None;
                }
                // Drop a well that maps outside the cell lattice by more than half
                // a cell — it is off the framework, not conditioning it.
                if fi < -0.5 || fj < -0.5 || fi > ni as f64 - 0.5 || fj > nj as f64 - 0.5 {
                    return None;
                }
                let i = fi.round().clamp(0.0, (ni - 1) as f64);
                let j = fj.round().clamp(0.0, (nj - 1) as f64);
                Some(WellLog::new(
                    (i + 0.5) * dx,
                    (j + 0.5) * dy,
                    w.samples.clone(),
                ))
            })
            .collect()
    }

    /// Run the upscale QC for `mnemonic` against a scratch build of the current
    /// framework (a deep temporary contact) — the visible before/after digest.
    ///
    /// `zone = Some(name)` reads the **zone-scoped** pipeline set via
    /// `grid.property(name, zone=..)` (the per-zone upscale wells + method) instead
    /// of the whole-model pipe, so a zone-scoped `Property.qc()` reflects its own
    /// conditioning rather than erroring or reporting the whole-model pipe. `None`
    /// is the whole-model pipe.
    pub fn upscale_qc(&self, mnemonic: &str, zone: Option<&str>) -> Result<UpscaleQc, SrsError> {
        let pipe = match zone {
            Some(z) => self
                .zone_pipes
                .get(&(z.to_string(), mnemonic.to_string()))
                .ok_or_else(|| {
                    SrsError::InvalidInput(format!(
                        "property '{mnemonic}' has no upscale step in zone '{z}'"
                    ))
                })?,
            None => self.pipes.get(mnemonic).ok_or_else(|| {
                SrsError::InvalidInput(format!("property '{mnemonic}' has no upscale step"))
            })?,
        };
        let grid = self.scratch_grid()?;
        let pl = PropertyPipeline::new(mnemonic).upscale(pipe.wells.clone(), pipe.method);
        let (_cells, qc) = pl.upscale_cells(grid.grid()).map_err(SrsError::from)?;
        Ok(qc)
    }

    /// Set the SGS propagation for `mnemonic` — variogram + seed, optional
    /// moving-neighbourhood search, optional collocated trend, and MC mode.
    #[allow(clippy::too_many_arguments)]
    pub fn set_propagate(
        &mut self,
        mnemonic: &str,
        variogram: Variogram,
        seed: u64,
        search: Option<(usize, f64)>,
        trend: Option<&TrendSpec>,
        resimulate: bool,
        allow_mean_fill: bool,
        zone: Option<&str>,
    ) -> Result<(), SrsError> {
        let mut g = Gaussian::new(variogram, seed);
        if let Some((max_n, radius)) = search {
            g = g.with_search(max_n, radius);
        }
        if let Some(t) = trend {
            let (surface, corr) = t.parts();
            g = g.with_trend(surface, corr);
        }
        // Opt into the (structureless) constant mean-fill for a data-less simulated
        // layer of this property; the petekStatic default is a loud, named
        // `InvalidInput` (naming the property, and the zone via the zone-scoped
        // caller) rather than a silent flat fill.
        if allow_mean_fill {
            g = g.allow_mean_fill();
        }
        let pipe = self.pipe_entry(mnemonic, zone);
        pipe.gaussian = Some(g);
        pipe.resimulate = resimulate;
        Ok(())
    }

    /// The finished per-zone pipelines `(zone, pipeline, mode)` for the stack
    /// build's `with_zone_property` and the zoned-MC template's
    /// `with_zone_property_mode`.
    fn zone_pipelines(&self) -> Vec<(String, PropertyPipeline, McMode)> {
        self.zone_pipes
            .iter()
            .map(|((zone, name), p)| {
                let mut pl = PropertyPipeline::new(name).upscale(p.wells.clone(), p.method);
                if let Some(g) = &p.gaussian {
                    pl = pl.propagate(g.clone());
                }
                let mode = if p.resimulate {
                    McMode::Resimulate
                } else {
                    McMode::LevelShift
                };
                (zone.clone(), pl, mode)
            })
            .collect()
    }

    /// The finished pipelines (name → pipeline) for build/template.
    fn pipelines(&self) -> Vec<(PropertyPipeline, McMode)> {
        self.pipes
            .iter()
            .map(|(name, p)| {
                let mut pl = PropertyPipeline::new(name).upscale(p.wells.clone(), p.method);
                if let Some(g) = &p.gaussian {
                    pl = pl.propagate(g.clone());
                }
                let mode = if p.resimulate {
                    McMode::Resimulate
                } else {
                    McMode::LevelShift
                };
                (pl, mode)
            })
            .collect()
    }

    /// The pipeline property names (which cubes carry a model).
    pub fn property_names(&self) -> Vec<String> {
        self.pipes.keys().cloned().collect()
    }

    fn scratch_grid(&self) -> Result<StaticModel, SrsError> {
        match &self.frame {
            Frame::Wireframe(wireframe) => {
                // A deep temporary OWC just to satisfy the builder's contact requirement.
                let deep = deepest(wireframe) + 1.0e5;
                let wf = with_contacts(wireframe, None, deep);
                let mut builder =
                    StaticModelBuilder::from_wireframe(&wf, self.opts).map_err(SrsError::from)?;
                if let Some(mt) = self.min_thickness_m {
                    builder = builder.with_min_thickness_m(mt);
                }
                builder.build().map_err(SrsError::from)
            }
            Frame::Stack(stack) => {
                // The stack carries its own per-zone contacts — no scratch contact
                // needed. Build a plain (property-less) stacked model for the QC.
                // The stack was conditioned ONCE at `build_grid` (all horizons
                // `Mapped`), so reuse it via `from_horizon_stack` + `with_georef` —
                // NO re-conditioning. Byte-for-byte the `from_scatter_stack` twin, so
                // the QC build matches the real build's geometry (`task_suite_scatter_perf`).
                let (gx, gy, gsx, gsy) = georef_args(&self.top_geom);
                let mut builder = StaticModelBuilder::from_horizon_stack(stack.clone(), self.opts)
                    .map_err(SrsError::from)?
                    .with_georef(gx, gy, gsx, gsy)
                    .with_min_thickness_m(self.min_thickness_m.unwrap_or(0.0));
                if let Some(cb) = self.collapse_below_m {
                    builder = builder.with_collapse_below_m(cb);
                }
                builder.build().map_err(SrsError::from)
            }
        }
    }

    /// Build the populated [`Model`] + a fluid/FVF for the volumetric surface.
    ///
    /// On the **wireframe** path `fwl_m` is required (the lower FWL/OWC; `goc_m`
    /// optional). On the **stack** path the per-zone contacts come from the
    /// zonation, so `goc_m`/`fwl_m` are ignored (pass `None`) — the model is zoned
    /// (`in_place_by_zone` / `zone_stats`), and whole-model `in_place()` is not
    /// physically meaningful, so `summary()` reads the zoned rollup instead.
    /// `memory_budget_bytes` (opt-in, thin passthrough to petekStatic's
    /// [`StaticModelBuilder::with_memory_budget`]): when the built model's live-set
    /// estimate exceeds this many bytes the engine switches to its out-of-core
    /// (spilled) backing store and emits a loud mode-switch advisory. `None` keeps
    /// petekStatic's default budget (a fraction of physical RAM). Applied on both
    /// the wireframe and the multi-zone stack build paths.
    #[allow(clippy::too_many_arguments)]
    pub fn model(
        self,
        goc_m: Option<f64>,
        fwl_m: Option<f64>,
        fluid: Fluid,
        boi: f64,
        bgi: Option<f64>,
        memory_budget_bytes: Option<u64>,
        sugar_cube: bool,
    ) -> Result<Model, SrsError> {
        let pipes = self.pipelines();
        let zone_pipes = self.zone_pipelines();
        let zone_priors = self.zone_priors.clone();
        let well_ties = self.well_ties.clone();
        let opts = self.opts;
        let top_geom = self.top_geom.clone();
        let min_thickness_m = self.min_thickness_m;
        let collapse_below_m = self.collapse_below_m;
        // The world georeference so the view bundles emit the WORLD frame
        // (column-centroid: origin = node origin + half a node spacing).
        let (gx, gy, gsx, gsy) = georef_args(&top_geom);

        match self.frame {
            Frame::Wireframe(wireframe) => {
                let fwl = fwl_m.filter(|v| v.is_finite()).ok_or_else(|| {
                    SrsError::InvalidInput(
                        "model needs a finite lower contact (fwl/owc) depth".into(),
                    )
                })?;
                let wf = with_contacts(&wireframe, goc_m, fwl);
                let mut builder = StaticModelBuilder::from_wireframe(&wf, opts)?;
                builder = builder.with_georef(gx, gy, gsx, gsy);
                builder = builder.with_sugar_cube(sugar_cube);
                if let Some(b) = memory_budget_bytes {
                    builder = builder.with_memory_budget(MemoryBudget::bytes(b));
                }
                if let Some(mt) = min_thickness_m {
                    builder = builder.with_min_thickness_m(mt);
                }
                for (pl, _mode) in &pipes {
                    builder = builder.with_property(pl.clone());
                }
                let model = builder.build().map_err(SrsError::from)?;
                Ok(Model::new(
                    model,
                    wf,
                    top_geom,
                    opts,
                    pipes,
                    fluid,
                    boi,
                    bgi,
                    goc_m,
                    fwl,
                    min_thickness_m,
                    false,
                    None,
                    Vec::new(),
                    None,
                    Vec::new(),
                    Vec::new(),
                    sugar_cube,
                ))
            }
            Frame::Stack(stack) => {
                // The stack was conditioned ONCE at `build_grid` (all horizons
                // `Mapped`); keep a clone so the built `Model` hands the SAME
                // conditioned handle to its stack-aware MC template
                // (zoned_uncertainty) with no re-conditioning.
                let stack_for_mc = stack.clone();
                // Per-zone contacts live in the stack; a small min-thickness keeps
                // any tops-only drape ordered inside its zone (the engine default).
                // Reuse the conditioned stack via `from_horizon_stack` + `with_georef`
                // (NO re-conditioning) — byte-identical to `from_scatter_stack` on the
                // raw scatter (petekStatic's proven dedup seam, `task_suite_scatter_perf`).
                let mut builder = StaticModelBuilder::from_horizon_stack(stack, opts)?
                    .with_georef(gx, gy, gsx, gsy)
                    .with_sugar_cube(sugar_cube)
                    .with_min_thickness_m(min_thickness_m.unwrap_or(0.0));
                if let Some(b) = memory_budget_bytes {
                    builder = builder.with_memory_budget(MemoryBudget::bytes(b));
                }
                if let Some(cb) = collapse_below_m {
                    builder = builder.with_collapse_below_m(cb);
                }
                // Per-zone base priors (a sand vs shale level) over each zone k-range.
                for (name, priors) in &zone_priors {
                    builder = builder.with_zone_priors(name.clone(), *priors);
                }
                // Whole-model property pipelines, then per-zone pipelines (each
                // simulated over the per-zone-priors baseline in its own k-range).
                for (pl, _mode) in &pipes {
                    builder = builder.with_property(pl.clone());
                }
                for (zone, pl, _mode) in &zone_pipes {
                    builder = builder.with_zone_property(zone.clone(), pl.clone());
                }
                // Per-horizon well ties: re-solve each mapped horizon with the
                // measured top as a hard control; residuals land in provenance.
                if !well_ties.is_empty() {
                    builder = builder.with_well_ties(well_ties.clone());
                }
                let model = builder.build().map_err(SrsError::from)?;
                // The stack model owns its framework (flattened per-zone contacts);
                // keep a clone so `Model` has a wireframe field for the view bundles.
                let wf = model.framework().clone();
                Ok(Model::new(
                    model,
                    wf,
                    top_geom,
                    opts,
                    pipes,
                    fluid,
                    boi,
                    bgi,
                    None,
                    f64::NAN,
                    min_thickness_m,
                    true,
                    Some(stack_for_mc),
                    zone_priors,
                    collapse_below_m,
                    zone_pipes,
                    well_ties,
                    sugar_cube,
                ))
            }
        }
    }
}

/// Build positioned `WellLog`s for `mnemonic`: sample each **bore's** log along
/// its own trajectory, dropping unpositioned or non-finite samples. Each bore of
/// a multi-sidetrack well is an independent positioned "well" (R-a).
///
/// `net_cutoff = Some(cutoff)` masks the samples to **net rock**: a sample is
/// kept only where the bore's own `NTG` curve resolves to a value > `cutoff` at
/// that depth. A bore carrying no `NTG` curve is unfiltered (kept whole — "where
/// present"); a depth its `NTG` curve cannot resolve is treated as non-net.
fn positioned_logs(
    wells: &[BoreWell<'_>],
    mnemonic: &str,
    net_cutoff: Option<f64>,
) -> Vec<WellLog> {
    let mut out = Vec::new();
    for bw in wells {
        let Some(view) = bw.log(mnemonic) else {
            continue;
        };
        let mds = view.md();
        let vals = view.values();
        // The net mask: the bore's positioned NTG curve (where present) + cutoff.
        let net = net_cutoff.and_then(|cut| bw.log("NTG").map(|ntg| (cut, ntg)));
        let mut samples = Vec::new();
        // Accumulate the samples' own areal positions: on a deviated bore the
        // logged reservoir section is laterally offset from the wellhead, so the
        // conditioning column belongs at the section (its mean x, y), not the head
        // (complements F6's real-trajectory positioning). For a vertical well this
        // equals the wellhead.
        let (mut sx, mut sy) = (0.0, 0.0);
        for (md, v) in mds.iter().zip(vals.iter()) {
            if !v.is_finite() {
                continue;
            }
            // Net masking: keep only samples where NTG at this depth exceeds the
            // cutoff (an aquifer / non-net interval is dropped before upscaling).
            if let Some((cut, ntg)) = &net {
                if !matches!(ntg.at_md(*md), Some(f) if f > *cut) {
                    continue;
                }
            }
            if let Some(p) = bw.xyz(*md) {
                if p.x.is_finite() && p.y.is_finite() && p.z.is_finite() {
                    samples.push((-p.z, *v)); // subsea TVD positive-down
                    sx += p.x;
                    sy += p.y;
                }
            }
        }
        if !samples.is_empty() {
            let n = samples.len() as f64;
            out.push(WellLog::new(sx / n, sy / n, samples));
        }
    }
    out
}

/// The world-georeference args for a `top_geom`, in the **column-centroid**
/// convention the view-bundle frame expects: `origin = node origin + half a node
/// spacing`, `spacing = node spacing`. Shared by the `model()` builder and the MC
/// `template()` so both realizations carry the same world frame.
pub(crate) fn georef_args(g: &GridGeometry) -> (f64, f64, f64, f64) {
    (g.xori + 0.5 * g.xinc, g.yori + 0.5 * g.yinc, g.xinc, g.yinc)
}

/// The [`StackFrame`] a raw-scatter stack conditions onto
/// ([`StaticModelBuilder::from_scatter_stack`]): the top lattice's `ni × nj`
/// areal cells (node counts `ncol × nrow`) registered by the same
/// column-centroid world georeference the built model + MC template carry
/// ([`georef_args`]). The engine owns the scatter→node gridding onto this frame;
/// the facade only chooses the lattice resolution (`cell_size_m`, a modelling
/// choice).
///
/// # Errors
/// [`SrsError::InvalidInput`] if the top geometry yields a degenerate world frame
/// (non-finite / non-positive spacing) — a scatter build cannot register onto it.
pub(crate) fn stack_frame(g: &GridGeometry) -> Result<StackFrame, SrsError> {
    let (gx, gy, gsx, gsy) = georef_args(g);
    let georef = Georef::new(gx, gy, gsx, gsy).ok_or_else(|| {
        SrsError::InvalidInput(
            "raw-scatter stack needs a finite, positive-spacing world frame from the top \
             horizon lattice"
                .into(),
        )
    })?;
    Ok(StackFrame {
        ni: g.ncol.saturating_sub(1),
        nj: g.nrow.saturating_sub(1),
        georef,
    })
}

fn with_contacts(wf: &Wireframe, goc_m: Option<f64>, lower_m: f64) -> Wireframe {
    let mut contacts = Vec::new();
    if let Some(goc) = goc_m {
        contacts.push(Contact {
            kind: ContactKind::Goc,
            depth_m: goc,
            hardness: Hardness::Hard,
        });
    }
    contacts.push(Contact {
        kind: ContactKind::Owc,
        depth_m: lower_m,
        hardness: Hardness::Hard,
    });
    Wireframe {
        boundary: wf.boundary.clone(),
        horizons: wf.horizons.clone(),
        contacts,
    }
}

fn deepest(wf: &Wireframe) -> f64 {
    wf.horizons
        .iter()
        .flat_map(|h| h.surface.depth_m.iter().copied())
        .filter(|d| d.is_finite())
        .fold(f64::NEG_INFINITY, f64::max)
        .max(0.0)
}

#[cfg(test)]
mod tests {
    //! F5 regression: property upscaling must condition cells when the wells
    //! carry **real UTM** coordinates (6–7 digit x/y) and the grid is built from
    //! UTM-origin horizons — the exact shape the final validation could not run.
    use super::*;
    use petekstatic::gridder::{Conformity, SolveOpts};
    use petekstatic::model::ConstantPriors;
    use petekstatic::wireframe::{Boundary, GriddedDepth, Hardness, Horizon, HorizonRole};

    const N: usize = 11; // 11x11 nodes -> 10x10 cells
    const XORI: f64 = 431_000.0; // ED50/UTM31N-magnitude origin
    const YORI: f64 = 6_521_000.0;
    const INC: f64 = 120.0;
    const TOP_M: f64 = 2000.0;
    const BASE_M: f64 = 2040.0;

    fn utm_geom() -> GridGeometry {
        GridGeometry {
            xori: XORI,
            yori: YORI,
            xinc: INC,
            yinc: INC,
            ncol: N,
            nrow: N,
            rotation_deg: 0.0,
            yflip: false,
        }
    }

    fn horizon(name: &str, role: HorizonRole, depth: f64) -> Horizon {
        Horizon {
            name: name.into(),
            role,
            surface: GriddedDepth {
                ncol: N,
                nrow: N,
                depth_m: vec![depth; N * N],
                is_control: vec![true; N * N],
            },
        }
    }

    fn utm_grid() -> StaticGrid {
        let geom = utm_geom();
        let side = INC * (N - 1) as f64; // 1200 m; area = side^2 -> dx == INC
        let wf = Wireframe {
            boundary: Boundary {
                ring: vec![
                    [XORI, YORI],
                    [XORI + side, YORI],
                    [XORI + side, YORI + side],
                    [XORI, YORI + side],
                    [XORI, YORI],
                ],
                hardness: Hardness::Interpolated,
            },
            horizons: vec![
                horizon("Top", HorizonRole::Top, TOP_M),
                horizon("Base", HorizonRole::Base, BASE_M),
            ]
            .into(),
            contacts: Vec::new(),
        };
        let opts = BuildOpts {
            area_m2: side * side,
            gross_height_m: BASE_M - TOP_M,
            nk: 8,
            conformity: Conformity::Proportional,
            solve_opts: SolveOpts::default(),
            priors: ConstantPriors {
                porosity: 0.25,
                net_to_gross: 0.8,
                water_saturation: 0.3,
            },
        };
        StaticGrid::new(
            Frame::Wireframe(wf),
            geom,
            opts,
            None,
            None,
            Vec::new(),
            Vec::new(),
            Vec::new(),
        )
    }

    /// A synthetic log at a real UTM node inside the framework, samples spanning
    /// the reservoir depth range.
    fn utm_well_log() -> WellLog {
        WellLog::new(
            XORI + 5.0 * INC, // 431600 — 6-digit easting
            YORI + 5.0 * INC, // 6521600 — 7-digit northing
            vec![
                (2005.0, 0.20),
                (2015.0, 0.22),
                (2025.0, 0.24),
                (2035.0, 0.26),
            ],
        )
    }

    #[test]
    fn upscale_conditions_cells_at_real_utm_coordinates() {
        let grid = utm_grid();
        // Register a world-UTM log onto the model frame, then upscale.
        let registered = grid.register_logs(&[utm_well_log()]);
        assert_eq!(
            registered.len(),
            1,
            "UTM well should register onto the grid"
        );
        let scratch = grid.scratch_grid().unwrap();
        let (_cells, qc) = PropertyPipeline::new("PORO")
            .upscale(registered, UpscaleMethod::Arithmetic)
            .upscale_cells(scratch.grid())
            .unwrap();
        // The decisive assertion: cells DO condition at UTM georeference.
        assert!(
            qc.conditioned_cells > 0,
            "expected >0 conditioned cells at UTM coords, got {}",
            qc.conditioned_cells
        );
        assert!(qc.log_mean.is_finite());
        assert!((0.19..=0.27).contains(&qc.upscaled_mean));
    }

    #[test]
    fn set_upscale_registers_and_conditions() {
        // End-to-end through the public seam (minus the petekio Well): register +
        // QC report a positive conditioned-cell count.
        let mut grid = utm_grid();
        let registered = grid.register_logs(&[utm_well_log()]);
        grid.pipes.entry("PORO".into()).or_default().wells = registered;
        let qc = grid.upscale_qc("PORO", None).unwrap();
        assert!(
            qc.conditioned_cells > 0,
            "0 cells conditioned at UTM coords"
        );
    }

    #[test]
    fn well_far_off_the_framework_is_dropped() {
        // A well nowhere near the UTM footprint carries no conditioning — it must
        // be dropped so the loud "0 cells" guard can fire, not snap to an edge.
        let grid = utm_grid();
        let off = WellLog::new(100.0, 100.0, vec![(2005.0, 0.2)]);
        assert!(grid.register_logs(&[off]).is_empty());
    }
}
