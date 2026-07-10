//! `Framework` — declares the structural framework from loaded horizons + an
//! outline, hard-ties each horizon to its same-named well-top picks, and reports
//! per-horizon-per-well tie residuals. Produces a [`StaticGrid`] carrying a
//! contact-less [`Wireframe`] + build options; contacts join at `grid.model(...)`
//! (the seam requires a contact only to *build*, so the facade defers it).
//!
//! The seam (`petekstatic::model::StaticModelBuilder::from_wireframe`) owns the gridding
//! and warm-start solve; this module only assembles its inputs and computes
//! residuals against the raw horizon surfaces.

use crate::core::facade::grid::{self, Frame, StaticGrid};
use crate::core::facade::project::Project;
use crate::core::facade::spec::LayersSpec;
use crate::core::facade::zonation::{self, ZoneSpec};
use crate::units::SrsError;
use petekio::{GridGeometry, GridMethod, PointSet, Surface};
use petekstatic::gridder::{Conformity, SolveOpts};
use petekstatic::model::{
    BuildOpts, ConstantPriors, HorizonSource, HorizonStack, Pick, StackHorizon, StaticModelBuilder,
    WellTie, WorldPoint,
};
use petekstatic::wireframe::{Boundary, GriddedDepth, Hardness, Horizon, HorizonRole, Wireframe};

/// One well's tie against one horizon: the raw-surface depth at the well, the
/// well-top pick, and their residual. A missing pick or an off-grid well is an
/// `ok = false` row (loud).
#[derive(Debug, Clone)]
pub struct TieResidual {
    pub horizon: String,
    pub well_id: String,
    /// Raw horizon depth at the well `(x, y)` before tying \[m, positive-down
    /// subsea — converted from petekio's negative-down elevation\].
    pub surface_m: f64,
    /// The well-top pick depth \[m, positive-down subsea\].
    pub pick_m: f64,
    /// `pick_m - surface_m` — the mis-tie the hard control corrects \[m\].
    pub residual_m: f64,
    /// False when the well could not be tied (no pick, or off the grid).
    pub ok: bool,
    /// A human reason when `!ok`.
    pub note: String,
}

/// A raw well-tie table row from the facade: `(well_id, x, y, [(horizon,
/// measured_depth_m)])`, depths positive-down. Mapped onto the top lattice by
/// [`Framework::set_well_ties`].
pub type WellTieInput = (String, f64, f64, Vec<(String, f64)>);

/// A declared structural framework, pre-`build_grid`.
pub struct Framework {
    /// The facade-gridded single-`ZoneTable` wireframe — **lazily materialized**
    /// ([`Self::materialize_wireframe`]). `None` until a wireframe consumer needs it
    /// (`tie_report`/`tie_ok` or a wireframe `build_grid`). A **stack** build
    /// (`set_zonation`) NEVER materializes it: the engine builds its own framework
    /// from the conditioned stack, so gridding all point-set horizons a second time
    /// facade-side would be pure waste (`task_suite_scatter_perf`). See
    /// [`Self::build`].
    wireframe: Option<Wireframe>,
    /// Whether to hard-tie each horizon to its same-named well picks — deferred to
    /// materialization (the tie report needs the gridded surface to sample).
    tie_to_tops: bool,
    /// The footprint outline polygon name (`outline`), carried for the lazy
    /// wireframe boundary (`boundary_of`); the areal `area_m2` is computed eagerly.
    outline: Option<String>,
    top_geom: GridGeometry,
    area_m2: f64,
    gross_m: f64,
    nk: usize,
    conformity: Conformity,
    priors: ConstantPriors,
    /// The facade tie residuals — populated by [`Self::materialize_wireframe`] (empty
    /// on a stack build, where ties come from the engine `with_well_ties`
    /// provenance, `Model::well_tie_residuals`).
    tie: Vec<TieResidual>,
    zones_limited: bool,
    /// Every declared horizon in order (top→down) resolved to its stack source:
    /// **`Scatter`** raw world points for a loaded point-set (the engine grids it
    /// itself inside `from_scatter_stack` — no facade pre-gridding, so a merged
    /// margin collapses to zero instead of carrying independently-extrapolated
    /// fill), **`Mapped`** for a genuinely pre-gridded loaded grid surface (the
    /// escape hatch, bypassing solve/conditioning fidelity), or **`TopsOnly`**
    /// node-lattice picks (a horizon with well picks but no mapped surface). Feeds
    /// the multi-zone `from_scatter_stack` path.
    resolved: Vec<StackHorizon>,
    /// The declared horizon names, parallel to [`Self::resolved`].
    horizon_names: Vec<String>,
    /// The declared zonation (`set_zonation`) — `Some` selects the multi-zone
    /// stack build over the single-`ZoneTable` wireframe build.
    zonation: Option<Vec<ZoneSpec>>,
    /// Per-zone base priors (`set_zone_priors`) threaded to the stack build seam's
    /// [`petekstatic::model::StaticModelBuilder::with_zone_priors`]. By zone name; a zone
    /// absent here uses the whole-model `priors`.
    zone_priors: Vec<(String, ConstantPriors)>,
    /// Per-horizon well ties (`set_well_ties`) threaded to
    /// [`petekstatic::model::StaticModelBuilder::with_well_ties`] on the stack build — the
    /// engine re-solves each mapped horizon with the measured top as a hard control
    /// and records the residuals into `Provenance.well_ties`.
    well_ties: Vec<WellTie>,
    /// Opt-in Petrel-style cell-collapse floor threaded to the stack build seam
    /// ([`petekstatic::model::StaticModelBuilder::with_collapse_below_m`]): sub-threshold
    /// cells merge volume-conservingly into a thicker zone-interior neighbour.
    collapse_below_m: Option<f64>,
    /// Post-gridding order-repair floor carried onto the build seam
    /// ([`petekstatic::model::StaticModelBuilder::with_min_thickness_m`]): where the gridded
    /// base sits less than this below the top (a thin or crossed column at a
    /// crossing margin), pull the base down to `top + min_thickness_m` (top
    /// preserved) and record a `ThinColumnsRepaired` warning. `None` (default) →
    /// the crossing guard errors on a crossed base.
    min_thickness_m: Option<f64>,
}

impl Framework {
    /// Declare a framework from `proj`: `horizons` in stratigraphic order (first
    /// = Top, last = Base when >1), `outline` a loaded polygon name for the
    /// footprint area, and `tie_to_tops` to hard-tie each horizon to its
    /// same-named well picks. `gross_m` seeds the single-horizon column (ignored
    /// once a Base horizon supplies real relief). `min_thickness_m` (opt-in) is
    /// the post-gridding order-repair floor threaded to the build seam: raw
    /// point-horizon builds with thin crossing margins (100–300 crossing nodes per
    /// structure) build cleanly and report a `ThinColumnsRepaired` warning instead
    /// of erroring on the crossing.
    #[allow(clippy::too_many_arguments)]
    pub fn build(
        proj: &Project,
        horizons: &[String],
        outline: Option<&str>,
        tie_to_tops: bool,
        gross_m: f64,
        min_thickness_m: Option<f64>,
        cell_size_m: Option<f64>,
        collapse_below_m: Option<f64>,
    ) -> Result<Self, SrsError> {
        if horizons.is_empty() {
            return Err(SrsError::InvalidInput(
                "framework needs at least one horizon".into(),
            ));
        }
        // The Top horizon fixes the shared areal lattice; others grid/resample
        // onto it. Derive that lattice CHEAPLY — geometry only, NO gridding solve
        // (`horizon_geom`): a point-set top yields its bbox lattice, a loaded surface
        // its own geometry. The top's gridded VALUES are needed only on the wireframe
        // path (lazily materialized); the stack path conditions the raw scatter itself.
        let top_geom = proj.horizon_geom(&horizons[0], cell_size_m)?;

        // Resolve every horizon to its **stack source** only — CHEAPLY, no wireframe
        // gridding here. A loaded **grid** surface resolves to `Mapped` (a cheap
        // resample onto the lattice, no solve — the pre-gridded escape hatch); a
        // loaded **point-set** resolves to `Scatter` (raw world points, no solve —
        // the engine grids them itself in `from_scatter_stack`, so merged margins
        // collapse instead of carrying independently-extrapolated fill); a horizon
        // that is neither, but whose wells carry its picks, is `TopsOnly` (an untied
        // internal split feeding only the multi-zone stack path).
        //
        // The facade wireframe + tie report (which DO grid every point-set horizon)
        // are DEFERRED to `materialize_wireframe` and computed only on the wireframe
        // `build_grid` path — never on a stack build, where the engine owns the
        // framework and the second facade-side gridding was pure waste
        // (`task_suite_scatter_perf`).
        let mut resolved: Vec<StackHorizon> = Vec::with_capacity(horizons.len());
        for name in horizons.iter() {
            let source = if proj.geo().surface(name).is_some() {
                // Loaded grid surface → Mapped: resample onto the shared lattice
                // (cheap; no minimum-curvature solve) then flip to positive-down.
                let s = proj.horizon_surface(name, Some(&top_geom), None)?;
                let s = if geom_eq(&s.geom, &top_geom) {
                    s
                } else {
                    s.resample(&top_geom)?
                };
                HorizonSource::Mapped(surface_to_gridded(&s))
            } else if proj.geo().points(name).is_some() {
                // Loaded point-set → Scatter: raw world points (no solve).
                HorizonSource::Scatter(scatter_points(proj, name)?)
            } else {
                // Neither a surface nor a point-set: tops-only iff the wells carry
                // this horizon's picks (else a hard error, as before).
                let picks = resolve_picks(proj, name, &top_geom);
                if picks.is_empty() {
                    return Err(SrsError::InvalidInput(format!(
                        "horizon '{name}' is neither a loaded surface nor a loaded \
                         point-set, and no well carries its pick"
                    )));
                }
                HorizonSource::TopsOnly(picks)
            };
            resolved.push(StackHorizon {
                name: name.clone(),
                source,
            });
        }

        let (outline, area_m2) = resolve_outline(proj, outline, &top_geom)?;
        // Gross from Top→Base separation across the resolved sources (exact per-node
        // mean for a Mapped pair — identical to the old wireframe `mean_separation`;
        // difference of per-source mean depths for a Scatter pair, no gridding), else
        // the caller's seed. Feeds only the whole-model layer count / `gross_height_m`
        // (a stack build allocates layers per-zone from the zonation).
        let gross = stack_gross(&resolved).unwrap_or(gross_m).max(1e-3);

        Ok(Self {
            wireframe: None,
            tie_to_tops,
            outline,
            top_geom,
            area_m2,
            gross_m: gross,
            // Default layering is dz-based (~1 m cells, the owner convention) rather
            // than a coarse fixed count; `set_layering(ps.layers(..))` overrides.
            nk: default_nk(gross),
            conformity: Conformity::Proportional,
            priors: ConstantPriors {
                porosity: 0.25,
                net_to_gross: 0.8,
                water_saturation: 0.3,
            },
            tie: Vec::new(),
            zones_limited: false,
            resolved,
            horizon_names: horizons.to_vec(),
            zonation: None,
            zone_priors: Vec::new(),
            well_ties: Vec::new(),
            collapse_below_m,
            min_thickness_m,
        })
    }

    /// Lazily grid the facade **wireframe** (single-`ZoneTable`) + the **tie report**
    /// from the loaded project — the expensive per-horizon minimum-curvature solve
    /// deferred out of [`Self::build`]. Idempotent (a no-op once materialized).
    ///
    /// This is the SECOND facade-side gridding of every point-set horizon. It is
    /// needed ONLY by a wireframe consumer — `tie_report`/`tie_ok` or a wireframe
    /// `build_grid` (no `set_zonation`). A **stack** build never calls it: the engine
    /// builds its framework from the conditioned raw scatter, so re-gridding here
    /// would be pure waste (`task_suite_scatter_perf`).
    ///
    /// # Errors
    /// [`SrsError`] if a horizon cannot grid onto the shared lattice.
    pub fn materialize_wireframe(&mut self, proj: &Project) -> Result<(), SrsError> {
        if self.wireframe.is_some() {
            return Ok(());
        }
        let mut wf_horizons: Vec<Horizon> = Vec::with_capacity(self.resolved.len());
        let mut tie = Vec::new();
        for (name, sh) in self.horizon_names.iter().zip(&self.resolved) {
            // A tops-only horizon has no surface to grid — it never joins the
            // single-`ZoneTable` wireframe (it is a stack-only untied split), so skip
            // it here exactly as the old eager build did.
            if matches!(sh.source, HorizonSource::TopsOnly(_)) {
                continue;
            }
            // Grid onto the shared lattice: a loaded surface resamples (cheap); a
            // point-set runs the minimum-curvature solve (the deferred cost). The top
            // grids onto its own derived lattice, so the result matches the old eager
            // `horizon_surface(top, None, cell_size_m)` byte-for-byte.
            let s = proj.horizon_surface(name, Some(&self.top_geom), None)?;
            let s = if geom_eq(&s.geom, &self.top_geom) {
                s
            } else {
                s.resample(&self.top_geom)?
            };
            let mut gridded = surface_to_gridded(&s);
            if self.tie_to_tops {
                tie_horizon(proj, name, &s, &self.top_geom, &mut gridded, &mut tie);
            }
            wf_horizons.push(Horizon {
                name: name.clone(),
                role: HorizonRole::Intermediate, // fixed up below
                surface: gridded,
            });
        }
        // Fix up the mapped wireframe horizons' roles (Top / Base bracket the rest).
        let n_wf = wf_horizons.len();
        for (i, h) in wf_horizons.iter_mut().enumerate() {
            h.role = horizon_role(i, n_wf);
        }
        let boundary = boundary_of(proj, self.outline.as_deref(), &self.top_geom);
        self.wireframe = Some(Wireframe {
            boundary,
            // petekStatic shares horizons as `Arc<Vec<Horizon>>` (its P2 borrow-share);
            // wrap the owned build result at this seam.
            horizons: wf_horizons.into(),
            contacts: Vec::new(),
        });
        self.tie = tie;
        Ok(())
    }

    /// The per-horizon-per-well tie residuals.
    pub fn tie_report(&self) -> &[TieResidual] {
        &self.tie
    }

    /// Whether any tie failed (missing pick / off-grid) — the loud flag.
    pub fn tie_ok(&self) -> bool {
        self.tie.iter().all(|t| t.ok)
    }

    /// Set constant priors (porosity / NTG / Sw) used where no property pipeline
    /// or log conditions a cube.
    pub fn set_priors(&mut self, porosity: f64, net_to_gross: f64, water_saturation: f64) {
        self.priors = ConstantPriors {
            porosity,
            net_to_gross,
            water_saturation,
        };
    }

    /// Declare zones between horizon pairs. **Documented limitation:** the
    /// current stack is a single implicit `ZoneTable`, so zones are recorded for
    /// provenance but do **not** yet grant per-zone property independence (the
    /// P5 zones task). Zones inform only the total layer allocation.
    pub fn set_zones(&mut self, n_zones: usize) {
        self.zones_limited = n_zones > 1;
    }

    /// Allocate layering + the conformity style. Under the single-`ZoneTable`
    /// limitation the allocation maps to a whole-column layer count (`ps.layers(n=..)`
    /// directly; `dz_m` derives the count from the gross column). `conformity`
    /// selects Proportional (equal-fraction) or a Follow style (constant-dz drape;
    /// `nk` dz-derived at the build seam, deep/shallow layers truncate at pinch-outs).
    pub fn set_layering(&mut self, spec: LayersSpec, conformity: Conformity) {
        self.nk = spec.nk(self.gross_m);
        self.conformity = conformity;
    }

    /// Whether the requested zonation exceeds the single-`ZoneTable` capability.
    pub fn zones_limited(&self) -> bool {
        self.zones_limited
    }

    /// Declare a **multi-zone horizon stack** (`fw.set_zonation([...])`): one
    /// [`ZoneSpec`] per horizon gap (top→down), each with its own conformity,
    /// layering, and contacts. This selects petekStatic's `from_horizon_stack`
    /// build over the single-`ZoneTable` wireframe build — granting real per-zone
    /// layering + per-zone volumetrics ([`crate::core::facade::Model::in_place_by_zone`]).
    /// Validated eagerly (length + top→down ordering).
    ///
    /// # Errors
    /// [`SrsError::InvalidInput`] if the zone count ≠ `horizons − 1`, or a zone's
    /// `below_horizon` is not the next horizon down.
    pub fn set_zonation(&mut self, zones: Vec<ZoneSpec>) -> Result<(), SrsError> {
        zonation::validate(&self.horizon_names, &zones)?;
        self.zonation = Some(zones);
        Ok(())
    }

    /// Set the opt-in cell-collapse floor (Petrel-style) for the stack build.
    pub fn set_collapse_below_m(&mut self, collapse_below_m: f64) {
        self.collapse_below_m = Some(collapse_below_m);
    }

    /// Give a **named zone** its own base priors (`with_zone_priors` on the stack
    /// build): the PORO/NTG/SW level a sand vs a shale zone sits at, over that zone's
    /// `k`-range. Replaces any prior for the same zone. Only honoured on the
    /// multi-zone stack path (`set_zonation`).
    pub fn set_zone_priors(&mut self, zone: &str, porosity: f64, net_to_gross: f64, sw: f64) {
        let priors = ConstantPriors {
            porosity,
            net_to_gross,
            water_saturation: sw,
        };
        self.zone_priors.retain(|(z, _)| z != zone);
        self.zone_priors.push((zone.to_string(), priors));
    }

    /// Accept a **well-tie table** (`(well_id, x, y, [(horizon, measured_depth_m)])`,
    /// depths positive-down): map each well's world `(x, y)` onto the top lattice
    /// node and assemble the engine [`WellTie`]s threaded to `with_well_ties` on the
    /// stack build. Off-grid wells (outside the framework extent) are dropped. Only
    /// honoured on the multi-zone stack path; the residuals surface on
    /// `model.well_tie_residuals()`.
    pub fn set_well_ties(&mut self, ties: Vec<WellTieInput>) {
        let g = &self.top_geom;
        let mut out = Vec::new();
        for (id, x, y, tops) in ties {
            let Some((fi, fj)) = g.xy_to_ij(x, y) else {
                continue; // off the framework — cannot tie
            };
            if !(fi.is_finite() && fj.is_finite()) {
                continue;
            }
            let ip = fi.round().clamp(0.0, (g.ncol - 1) as f64) as usize;
            let jp = fj.round().clamp(0.0, (g.nrow - 1) as f64) as usize;
            let mut wt = WellTie::new(&id, x, y, ip, jp);
            for (horizon, depth_m) in tops {
                wt = wt.with_top(horizon, depth_m);
            }
            out.push(wt);
        }
        self.well_ties = out;
    }

    /// Whether a multi-zone stack was declared (`set_zonation`).
    pub fn is_zoned(&self) -> bool {
        self.zonation.is_some()
    }

    /// Freeze a **multi-zone stack** framework (`set_zonation`) into a [`StaticGrid`]
    /// — assembling the [`petekstatic::model::HorizonStack`] from the resolved sources and
    /// conditioning the raw scatter ONCE. Needs NO loaded project: the engine builds
    /// its own framework from the conditioned stack, so the facade wireframe is never
    /// materialized (the second facade-side gridding is skipped — the
    /// `task_suite_scatter_perf` win). The caller must have declared a zonation.
    ///
    /// # Errors
    /// [`SrsError`] if called without a zonation, or the stack cannot assemble /
    /// condition against the resolved horizons.
    pub fn build_grid_stack(self) -> Result<StaticGrid, SrsError> {
        let Some(zones) = &self.zonation else {
            return Err(SrsError::InvalidInput(
                "build_grid_stack requires a declared zonation (set_zonation)".into(),
            ));
        };
        // Assemble the raw stack (may carry `Scatter` horizons), then CONDITION IT
        // ONCE here — the expensive per-horizon cold bilinear solve
        // (`task_suite_scatter_perf`). The returned all-`Mapped` handle then feeds the
        // build, the QC scratch build, AND the MC template with NO re-conditioning
        // (each via `from_horizon_stack`), so the canonical model's scatter is gridded
        // exactly once per model lifecycle. `condition_scatter_stack` is a no-op on an
        // already-`Mapped` stack, bit-identical to conditioning inside
        // `from_scatter_stack` (petekStatic's `scatter_dedup_seam_is_bit_identical`).
        let raw = zonation::assemble_stack(&self.horizon_names, &self.resolved, zones)?;
        let stack_frame = grid::stack_frame(&self.top_geom)?;
        // Condition the raw scatter ONCE. The engine's direct-factor conditioner is
        // the fast path (factored once, reused by build/QC/MC) and — since petekTools
        // 2f904bc factored the anchorless bilinear system via a null-mode ridge — it
        // now grids **anchorless** irregular seismic scatter directly (no sample need
        // land on a frame node), so the real canonical model takes the engine path.
        // The only residual engine error is genuine **under-constraint**: a horizon
        // whose usable control collapses below the bilinear minimum (<4 controls), for
        // which the direct factor legitimately has no solution. For that degenerate
        // case, fall back to petekio's node-conditioned min-curvature, which pins the
        // solve by snapping each sample to its nearest node and still grids the sparse
        // cloud — the shape the pre-raw-scatter facade always used.
        let conditioned =
            match StaticModelBuilder::condition_scatter_stack(raw.clone(), &stack_frame) {
                Ok(c) => c,
                Err(_) => {
                    let mut raw = raw;
                    condition_scatter_facade(&mut raw, &self.top_geom)?;
                    raw
                }
            };
        Ok(self.into_grid(Frame::Stack(conditioned)))
    }

    /// Freeze a **single-`ZoneTable` wireframe** framework (no `set_zonation`) into a
    /// [`StaticGrid`], materializing the facade wireframe + tie report from `proj`
    /// (the deferred per-horizon gridding — see [`Self::materialize_wireframe`]).
    ///
    /// # Errors
    /// [`SrsError`] if the wireframe cannot grid against the loaded project.
    pub fn build_grid_wireframe(mut self, proj: &Project) -> Result<StaticGrid, SrsError> {
        self.materialize_wireframe(proj)?;
        let wireframe = self
            .wireframe
            .take()
            .expect("materialize_wireframe populates the wireframe");
        Ok(self.into_grid(Frame::Wireframe(wireframe)))
    }

    /// Freeze into a [`StaticGrid`] over the given frame — the shared tail of the
    /// stack / wireframe `build_grid` paths (build options + carried tie residuals,
    /// zone priors, well ties).
    fn into_grid(self, frame: Frame) -> StaticGrid {
        let opts = BuildOpts {
            area_m2: self.area_m2,
            gross_height_m: self.gross_m,
            nk: self.nk,
            conformity: self.conformity,
            solve_opts: SolveOpts::default(),
            priors: self.priors,
        };
        StaticGrid::new(
            frame,
            self.top_geom,
            opts,
            self.min_thickness_m,
            self.collapse_below_m,
            self.tie,
            self.zone_priors,
            self.well_ties,
        )
    }
}

/// Default target layer thickness (m) when the caller does not set layering — the
/// owner convention is ~1 m cells.
const DEFAULT_DZ_M: f64 = 1.0;
/// Cap on the auto-derived default layer count (a very tall column would otherwise
/// blow the cell budget); hitting the cap warns.
const MAX_DEFAULT_NK: usize = 200;

/// The dz-based default layer count over a `gross_m` column: `ceil(gross / dz)`,
/// capped at [`MAX_DEFAULT_NK`] (loud when capped).
fn default_nk(gross_m: f64) -> usize {
    let n = if gross_m.is_finite() && gross_m > 0.0 {
        ((gross_m / DEFAULT_DZ_M).ceil() as usize).max(1)
    } else {
        1
    };
    if n > MAX_DEFAULT_NK {
        eprintln!(
            "peteksim: default layering capped at {MAX_DEFAULT_NK} layers (gross \
             {gross_m:.0} m / {DEFAULT_DZ_M} m → {n}); pass ps.layers(dz_m=..) or n=.. to choose"
        );
        MAX_DEFAULT_NK
    } else {
        n
    }
}

fn horizon_role(idx: usize, n: usize) -> HorizonRole {
    if idx == 0 {
        HorizonRole::Top
    } else if idx == n - 1 {
        HorizonRole::Base
    } else {
        HorizonRole::Intermediate
    }
}

fn geom_eq(a: &GridGeometry, b: &GridGeometry) -> bool {
    a.ncol == b.ncol
        && a.nrow == b.nrow
        && (a.xori - b.xori).abs() < 1e-6
        && (a.yori - b.yori).abs() < 1e-6
        && (a.xinc - b.xinc).abs() < 1e-9
        && (a.yinc - b.yinc).abs() < 1e-9
}

/// petekio `Surface` (values `[[i, j]]`, shape `(ncol, nrow)`) →
/// `GriddedDepth` (flat row-major `depth[j*ncol + i]`), all nodes control.
///
/// ## z-datum flip (the Wireframe convention, Z1 family)
/// petekio delivers surface z as **negative-down subsea elevation** (its
/// documented convention; matches `Trajectory::xyz` z = -tvd). The Wireframe —
/// and the whole geomodel seam behind it — works in **positive-down `depth_m`**
/// (the datum contacts and tops picks already use), so we **negate at this
/// ingest boundary**, exactly as the pick path does (`-p.z` in [`tie_horizon`])
/// and as srs-data's `surface_depths` does at its own seam. `NaN` (undefined)
/// negates to `NaN`, so the defined-node mask is unaffected.
fn surface_to_gridded(s: &Surface) -> GriddedDepth {
    let g = &s.geom;
    let vals = s.values();
    let (ncol, nrow) = (g.ncol, g.nrow);
    let mut depth = vec![f64::NAN; ncol * nrow];
    for j in 0..nrow {
        for i in 0..ncol {
            depth[j * ncol + i] = -vals[[i, j]];
        }
    }
    GriddedDepth {
        ncol,
        nrow,
        depth_m: depth,
        is_control: vec![true; ncol * nrow],
    }
}

/// Hard-tie a horizon to each well's same-named pick: snap the nearest node to
/// the pick depth and record the residual against the raw surface.
fn tie_horizon(
    proj: &Project,
    name: &str,
    raw: &Surface,
    geom: &GridGeometry,
    gridded: &mut GriddedDepth,
    tie: &mut Vec<TieResidual>,
) {
    for bw in proj.wells() {
        let Some(iv) = bw.top(name) else {
            continue; // this bore has no pick of this horizon — nothing to tie
        };
        // F4/R-a: tie at the pick's OWN areal position — the (x, y, z) *this bore's*
        // trajectory reaches at the pick MD — not the shared wellhead. Deviated
        // sidetracks (A/B/C/ST2) share one wellhead, so head-tying sampled the
        // surface at one identical point and snapped every pick onto one node
        // (last-well-wins), corrupting the controlled surface with a large mis-tie.
        // Each bore is now an independent positioned "well" (id = "<id> <bore>").
        let (x, y, pick) = match bw.xyz(iv.top_md) {
            Some(p) => (p.x, p.y, -p.z),
            None => {
                tie.push(TieResidual {
                    horizon: name.into(),
                    well_id: bw.id.clone(),
                    surface_m: f64::NAN,
                    pick_m: f64::NAN,
                    residual_m: f64::NAN,
                    ok: false,
                    note: "well trajectory could not position the pick MD".into(),
                });
                continue;
            }
        };
        // The raw petekio sample is negative-down elevation; report it on the
        // same positive-down depth_m datum as the pick (see surface_to_gridded).
        let surface_m = raw.sample(x, y).map(|z| -z).unwrap_or(f64::NAN);
        match geom.xy_to_ij(x, y) {
            Some((fi, fj)) => {
                let i = fi.round().clamp(0.0, (geom.ncol - 1) as f64) as usize;
                let j = fj.round().clamp(0.0, (geom.nrow - 1) as f64) as usize;
                let flat = j * geom.ncol + i;
                gridded.depth_m[flat] = pick;
                gridded.is_control[flat] = true;
                tie.push(TieResidual {
                    horizon: name.into(),
                    well_id: bw.id.clone(),
                    surface_m,
                    pick_m: pick,
                    residual_m: pick - surface_m,
                    ok: surface_m.is_finite(),
                    note: if surface_m.is_finite() {
                        String::new()
                    } else {
                        "horizon undefined at the well location".into()
                    },
                });
            }
            None => tie.push(TieResidual {
                horizon: name.into(),
                well_id: bw.id.clone(),
                surface_m,
                pick_m: pick,
                residual_m: f64::NAN,
                ok: false,
                note: "well is outside the horizon grid".into(),
            }),
        }
    }
}

/// Resolve a **tops-only** horizon's well picks to node-lattice [`Pick`]s (the
/// facade owns the world-xy→ij mapping the engine kept out of `from_horizon_stack`):
/// for each bore carrying a `name` top, position the pick on its own trajectory
/// (world `x, y`, subsea depth), snap `(x, y)` to the nearest node `(ip, jp)` on
/// the top lattice, and record depth positive-down. Off-grid / unpositioned picks
/// are dropped. An empty result means the horizon has no picks (not tops-only).
fn resolve_picks(proj: &Project, name: &str, geom: &GridGeometry) -> Vec<Pick> {
    let mut picks = Vec::new();
    for bw in proj.wells() {
        let Some(iv) = bw.top(name) else {
            continue;
        };
        let Some(p) = bw.xyz(iv.top_md) else {
            continue;
        };
        let (x, y, depth) = (p.x, p.y, -p.z);
        if !(x.is_finite() && y.is_finite() && depth.is_finite()) {
            continue;
        }
        let Some((fi, fj)) = geom.xy_to_ij(x, y) else {
            continue;
        };
        if !(fi.is_finite() && fj.is_finite()) {
            continue;
        }
        let ip = fi.round().clamp(0.0, (geom.ncol - 1) as f64) as usize;
        let jp = fj.round().clamp(0.0, (geom.nrow - 1) as f64) as usize;
        picks.push(Pick {
            ip,
            jp,
            depth_m: depth,
        });
    }
    picks
}

/// A loaded point-set horizon's **raw scatter** in world coordinates for
/// [`HorizonSource::Scatter`]: every `[x, y, z]` point flipped to positive-down
/// `depth_m = -z` (the Z1 subsea convention `surface_to_gridded` / `tie_horizon`
/// use), non-finite points dropped. The engine conditions these onto the model
/// lattice inside `from_scatter_stack` — no facade pre-gridding, so a data-void
/// margin between merged horizons collapses to zero.
///
/// # Errors
/// [`SrsError::InvalidInput`] if `name` names no loaded point-set (the caller
/// only reaches here for a horizon that gridded from a point-set, so this is a
/// belt-and-braces guard) or the set yields no finite point.
fn scatter_points(proj: &Project, name: &str) -> Result<Vec<WorldPoint>, SrsError> {
    let ps = proj.geo().points(name).ok_or_else(|| {
        SrsError::InvalidInput(format!(
            "horizon '{name}' has no loaded point-set to scatter"
        ))
    })?;
    let points: Vec<WorldPoint> = ps
        .coords()
        .iter()
        .filter(|c| c[0].is_finite() && c[1].is_finite() && c[2].is_finite())
        .map(|c| WorldPoint {
            x: c[0],
            y: c[1],
            depth_m: -c[2],
        })
        .collect();
    if points.is_empty() {
        return Err(SrsError::InvalidInput(format!(
            "horizon point-set '{name}' has no finite points to scatter"
        )));
    }
    Ok(points)
}

/// Grid every raw-[`HorizonSource::Scatter`] horizon in `stack` onto `geom` via
/// petekio's min-curvature, turning it [`HorizonSource::Mapped`] — the facade-side
/// fallback for [`Framework::build_grid_stack`] on genuine engine **under-constraint**.
/// Since petekTools 2f904bc the engine grids anchorless scatter directly, so this
/// path is reached only when a horizon's usable control collapses below the bilinear
/// minimum (<4 controls) and the engine's direct factor legitimately has no solution.
/// petekio's node-conditioned kernel pins the solve by snapping each sample to its
/// nearest node, so it still grids the sparse cloud the direct factor cannot.
/// `Mapped`/`TopsOnly` horizons pass through untouched.
///
/// World points carry positive-down `depth_m`; petekio grids the negative-down
/// elevation (`-depth_m`) and [`surface_to_gridded`] flips it back — the same Z1
/// convention the `Mapped`-surface path uses, so the two conditioners agree in sign.
///
/// # Errors
/// [`SrsError`] if a scatter horizon still cannot grid (e.g. no finite points).
fn condition_scatter_facade(stack: &mut HorizonStack, geom: &GridGeometry) -> Result<(), SrsError> {
    for h in stack.horizons.iter_mut() {
        let HorizonSource::Scatter(points) = &h.source else {
            continue;
        };
        let coords: Vec<[f64; 3]> = points.iter().map(|p| [p.x, p.y, -p.depth_m]).collect();
        let surf = PointSet::from_coords(coords)
            .to_surface(geom.clone(), GridMethod::MinimumCurvature)
            .map_err(SrsError::from)?;
        h.source = HorizonSource::Mapped(surface_to_gridded(&surf));
    }
    Ok(())
}

fn resolve_outline(
    proj: &Project,
    outline: Option<&str>,
    geom: &GridGeometry,
) -> Result<(Option<String>, f64), SrsError> {
    if let Some(name) = outline {
        let poly = proj.geo().polygons(name).ok_or_else(|| {
            SrsError::InvalidInput(format!(
                "outline polygon '{name}' not loaded; loaded polygons: {}",
                polygon_inventory(proj)
            ))
        })?;
        return Ok((Some(name.to_string()), poly.area()));
    }
    if let Some(poly) = proj.geo().polygons("ModelEdge") {
        return Ok((Some("ModelEdge".to_string()), poly.area()));
    }
    eprintln!(
        "peteksim: default outline 'ModelEdge' not loaded; using framework bbox area/boundary. \
         Loaded polygons: {}",
        polygon_inventory(proj)
    );
    Ok((None, footprint_area(geom)))
}

fn polygon_inventory(proj: &Project) -> String {
    let names: Vec<&str> = proj.geo().polygons_named().map(|(name, _)| name).collect();
    if names.is_empty() {
        "<none>".to_string()
    } else {
        names.join(", ")
    }
}

fn boundary_of(proj: &Project, outline: Option<&str>, geom: &GridGeometry) -> Boundary {
    if let Some(name) = outline {
        if let Some(poly) = proj.geo().polygons(name) {
            let rings = poly.rings();
            if let Some(first) = rings.into_iter().next() {
                let ring: Vec<[f64; 2]> = first.iter().map(|p| [p[0], p[1]]).collect();
                if ring.len() >= 4 {
                    return Boundary {
                        ring,
                        hardness: Hardness::Interpolated,
                    };
                }
            }
        } else {
            eprintln!(
                "peteksim: resolved outline '{name}' disappeared before boundary build; \
                 using framework bbox. Loaded polygons: {}",
                polygon_inventory(proj)
            );
        }
    }
    bbox_ring(geom)
}

fn bbox_ring(g: &GridGeometry) -> Boundary {
    let (x0, y0) = (g.xori, g.yori);
    let x1 = g.xori + g.xinc * (g.ncol.saturating_sub(1)) as f64;
    let y1 = g.yori + g.yinc * (g.nrow.saturating_sub(1)) as f64;
    Boundary {
        ring: vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1], [x0, y0]],
        hardness: Hardness::Interpolated,
    }
}

fn footprint_area(g: &GridGeometry) -> f64 {
    let w = g.xinc * (g.ncol.saturating_sub(1)) as f64;
    let h = g.yinc * (g.nrow.saturating_sub(1)) as f64;
    (w * h).abs()
}

/// Mean Top→Base separation across the **resolved sources** — the whole-model gross
/// thickness, computed WITHOUT gridding the wireframe.
///
/// For a `Mapped`↔`Mapped` top/base pair on the same lattice this is the exact
/// per-node mean `(base − top)` — byte-identical to the old wireframe
/// `mean_separation`. For any other pair (a `Scatter` point-set, or a mixed pair) it
/// is the difference of each source's per-node/per-point mean depth (a robust
/// estimate without co-locating the scatter — mirrors `zonation::zone_gross`).
/// `None` when fewer than two horizons, or neither end offers a finite depth (both
/// `TopsOnly`).
fn stack_gross(resolved: &[StackHorizon]) -> Option<f64> {
    if resolved.len() < 2 {
        return None;
    }
    let top = &resolved[0].source;
    let base = &resolved[resolved.len() - 1].source;
    if let (HorizonSource::Mapped(a), HorizonSource::Mapped(b)) = (top, base) {
        if a.depth_m.len() == b.depth_m.len() {
            let mut sum = 0.0;
            let mut n = 0usize;
            for (t, d) in a.depth_m.iter().zip(&b.depth_m) {
                if t.is_finite() && d.is_finite() {
                    sum += d - t;
                    n += 1;
                }
            }
            if n > 0 {
                return Some((sum / n as f64).abs());
            }
        }
    }
    match (source_mean_depth(top), source_mean_depth(base)) {
        (Some(a), Some(b)) => Some((b - a).abs()),
        _ => None,
    }
}

/// A horizon source's mean finite depth (positive-down): the per-node mean of a
/// `Mapped` surface or the per-point mean of a `Scatter` set; `None` for a
/// `TopsOnly` horizon (node-index picks, not a depth field) or no finite depth.
fn source_mean_depth(source: &HorizonSource) -> Option<f64> {
    let (mut sum, mut n) = (0.0, 0usize);
    match source {
        HorizonSource::Mapped(g) => {
            for &z in &g.depth_m {
                if z.is_finite() {
                    sum += z;
                    n += 1;
                }
            }
        }
        HorizonSource::Scatter(points) => {
            for p in points {
                if p.depth_m.is_finite() {
                    sum += p.depth_m;
                    n += 1;
                }
            }
        }
        HorizonSource::TopsOnly(_) => return None,
    }
    (n > 0).then(|| sum / n as f64)
}

#[cfg(test)]
mod tests {
    //! R1 world-georef regression for the raw-scatter conditioning path
    //! (`task_suite_scatter_perf`). The real canonical model derives an
    //! outline-driven extent at a fictional-UTM world georef and feeds **irregular**
    //! seismic scatter (no sample lands on a frame node). Since petekTools 2f904bc
    //! factored the anchorless bilinear system via a null-mode ridge, the engine
    //! conditions such **anchorless** scatter directly to finite fields — the real
    //! canonical model stays on the fast engine path. `build_grid_stack` still keeps
    //! the facade fallback for genuine **under-constraint** (a horizon whose control
    //! collapses below the bilinear minimum, <4 controls), where the engine's direct
    //! factor legitimately errors and petekio's node-conditioned gridding still grids
    //! the sparse cloud so the stack builds. Local-frame / grid-as-points fixtures
    //! never exercise the world-georef seam, so the doctrine R1 world-georef variant
    //! is mandatory here.
    use super::*;
    use petekstatic::gridder::Conformity;
    use petekstatic::model::StackZone;

    // A fictional ED50/UTM31N-magnitude origin with an OFFSET (non-zero, non-round)
    // extent — the outline-driven world frame shape, distinct from a local lattice.
    const XORI: f64 = 431_000.0;
    const YORI: f64 = 6_521_000.0;
    const CELL: f64 = 100.0;
    // Keep the regression's world-coordinate/non-node shape while avoiding a
    // production-sized minimum-curvature solve in every unit-test run.
    const NCOL: usize = 13;
    const NROW: usize = 11;

    fn world_geom() -> GridGeometry {
        GridGeometry {
            xori: XORI,
            yori: YORI,
            xinc: CELL,
            yinc: CELL,
            ncol: NCOL,
            nrow: NROW,
            rotation_deg: 0.0,
            yflip: false,
        }
    }

    /// Irregular (anchorless) world scatter spanning the frame extent: a jittered
    /// cloud whose points deliberately never land on a frame node (real seismic
    /// shape). `depth0` sets the horizon's mean subsea depth.
    fn anchorless_scatter(depth0: f64) -> Vec<WorldPoint> {
        let g = world_geom();
        let span_x = CELL * (NCOL - 1) as f64;
        let span_y = CELL * (NROW - 1) as f64;
        let mut pts = Vec::new();
        // Deterministic LCG jitter — keeps every sample strictly between nodes.
        let mut s: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut next = || {
            s = s
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((s >> 33) as f64) / (u32::MAX as f64)
        };
        for _ in 0..240 {
            let fx = next();
            let fy = next();
            // Guard the half-cell margin so a sample cannot round onto a node.
            let x = g.xori + (0.2 + 0.6 * fx) * span_x + 0.37;
            let y = g.yori + (0.2 + 0.6 * fy) * span_y + 0.29;
            let depth_m = depth0 + 30.0 * fx + 20.0 * fy; // gentle dip
            pts.push(WorldPoint { x, y, depth_m });
        }
        pts
    }

    fn scatter_stack() -> HorizonStack {
        HorizonStack {
            horizons: vec![
                StackHorizon {
                    name: "Top".into(),
                    source: HorizonSource::Scatter(anchorless_scatter(2000.0)),
                },
                StackHorizon {
                    name: "Base".into(),
                    source: HorizonSource::Scatter(anchorless_scatter(2050.0)),
                },
            ],
            zone_layers: vec![StackZone::new(
                "Zone",
                Conformity::Proportional,
                8,
                Vec::new(),
            )],
        }
    }

    #[test]
    fn engine_conditions_anchorless_world_scatter_to_finite_fields() {
        // The engine path (petekTools 2f904bc: null-mode ridge on the anchorless
        // bilinear system) now grids irregular world scatter directly — no sample
        // need land on a frame node. This is the shape that defeated the canonical
        // real-model build before the ridge fix; it must condition cleanly now.
        let geom = world_geom();
        let frame = grid::stack_frame(&geom).unwrap();
        let stack = scatter_stack();
        let conditioned = StaticModelBuilder::condition_scatter_stack(stack, &frame)
            .expect("engine must condition anchorless world scatter after the ridge fix");
        for h in &conditioned.horizons {
            let HorizonSource::Mapped(g) = &h.source else {
                panic!("horizon '{}' was not conditioned to Mapped", h.name);
            };
            let finite = g.depth_m.iter().filter(|z| z.is_finite()).count();
            assert!(
                finite > 0,
                "horizon '{}' conditioned to an all-NaN field",
                h.name
            );
        }
    }

    /// Genuine under-constraint: a horizon whose usable control collapses below the
    /// bilinear minimum (<4 controls). The engine's direct factor legitimately has no
    /// solution there, so `build_grid_stack` falls back to `condition_scatter_facade`
    /// (petekio node-conditioned min-curvature), which still grids the sparse cloud.
    fn under_constrained_scatter(depth0: f64) -> Vec<WorldPoint> {
        // Three off-node controls inside the frame extent — under the bilinear
        // minimum, but enough for petekio's node-snap min-curvature to grid.
        vec![
            WorldPoint {
                x: XORI + 2.37 * CELL,
                y: YORI + 2.29 * CELL,
                depth_m: depth0,
            },
            WorldPoint {
                x: XORI + 6.37 * CELL,
                y: YORI + 5.29 * CELL,
                depth_m: depth0 + 10.0,
            },
            WorldPoint {
                x: XORI + 10.37 * CELL,
                y: YORI + 8.29 * CELL,
                depth_m: depth0 + 20.0,
            },
        ]
    }

    fn under_constrained_stack() -> HorizonStack {
        HorizonStack {
            horizons: vec![
                StackHorizon {
                    name: "Top".into(),
                    source: HorizonSource::Scatter(under_constrained_scatter(2000.0)),
                },
                StackHorizon {
                    name: "Base".into(),
                    source: HorizonSource::Scatter(under_constrained_scatter(2050.0)),
                },
            ],
            zone_layers: vec![StackZone::new(
                "Zone",
                Conformity::Proportional,
                8,
                Vec::new(),
            )],
        }
    }

    #[test]
    fn engine_errors_on_under_constrained_world_scatter() {
        // The residual genuine-error path: the direct bilinear factor cannot solve a
        // horizon with fewer than 4 controls — this is what still reaches the fallback.
        let geom = world_geom();
        let frame = grid::stack_frame(&geom).unwrap();
        let r = StaticModelBuilder::condition_scatter_stack(under_constrained_stack(), &frame);
        assert!(
            r.is_err(),
            "expected the engine to error on genuine <4-control under-constraint; got Ok"
        );
    }

    #[test]
    fn facade_fallback_conditions_under_constrained_world_scatter() {
        // The fallback: petekio's node-conditioned min-curvature grids the same
        // under-constrained cloud, turning every Scatter horizon into a finite Mapped
        // field where the engine's direct factor has no solution.
        let geom = world_geom();
        let mut stack = under_constrained_stack();
        condition_scatter_facade(&mut stack, &geom).expect("facade fallback must grid");
        for h in &stack.horizons {
            match &h.source {
                HorizonSource::Mapped(g) => {
                    assert_eq!(g.ncol, NCOL);
                    assert_eq!(g.nrow, NROW);
                    let finite = g.depth_m.iter().filter(|z| z.is_finite()).count();
                    assert!(
                        finite > 0,
                        "horizon '{}' conditioned to an all-NaN field",
                        h.name
                    );
                    // Depths are positive-down subsea in the seeded range.
                    let mean: f64 =
                        g.depth_m.iter().filter(|z| z.is_finite()).sum::<f64>() / finite as f64;
                    assert!(
                        (1900.0..2150.0).contains(&mean),
                        "horizon '{}' mean depth {mean} out of the seeded band",
                        h.name
                    );
                }
                _ => panic!("horizon '{}' was not conditioned to Mapped", h.name),
            }
        }
    }
}
