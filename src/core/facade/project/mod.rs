//! `Project` — the ingest front-door. Walks a Petrel-export tree, routes each
//! file to the owning petekio loader (extension-dispatch does the per-file
//! work), and records a loud [`Inventory`] of what loaded and what was
//! skipped-with-reason. No parsing lives here; classification + routing only.

use super::spec::TrendSpec;
use crate::units::SrsError;
use petekio::{
    detect, BBox, FormatKind, GeoData, GridGeometry, GridMethod, Interval, LogView, NameMap,
    Point3, Sidetrack, Surface, Unit,
};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

mod helpers;
use helpers::*;

/// What a [`Project::load`] found — every loaded artifact by kind, plus the
/// files it skipped and why (never silent).
#[derive(Debug, Clone, Default)]
pub struct Inventory {
    pub surfaces: Vec<String>,
    pub polygons: Vec<String>,
    pub points: Vec<String>,
    /// Loaded wells as **bore-level** ids — one entry per positioned bore under
    /// its bore-qualified id (matching [`Project::wells`]), not the parent well id.
    pub wells: Vec<String>,
    /// Named tops-pick surfaces available across the loaded wells (e.g. "GOC").
    pub tops: Vec<String>,
    /// Core LAS files folded into a bore by petekio's well loader (the LAS 3.0
    /// core-file merge): `(filename, bore_id)`. Without this the merge leaves no
    /// inventory trace — the core log is absorbed into its bore and the file
    /// vanishes (F8). `bore_id` names the bore it merged into (`"<id> <bore>"`,
    /// or the well id for a main-bore merge).
    pub merged: Vec<(String, String)>,
    /// `(path, reason)` for every file the loader could not place.
    pub skipped: Vec<(String, String)>,
}

/// A named tops surface's per-well picks and a representative level.
#[derive(Debug, Clone)]
pub struct TopsPick {
    pub name: String,
    /// `(well_id, depth_m)` — subsea TVD (positive-down) at each well's pick.
    pub picks: Vec<(String, f64)>,
    /// Mean pick depth \[m\] — the representative contact level.
    pub level_m: f64,
    /// Sample standard deviation of the picks \[m\] (0 for a single pick).
    pub spread_m: f64,
}

/// One **bore** of a loaded well, presented as an independent positioned "well"
/// for modelling (the R-a wiring). petekio makes every bore of a multi-sidetrack
/// well a first-class [`Sidetrack`] with its own trajectory + logs + tops; this
/// wrapper pairs a bore with its **bore-qualified id** (`"<id> <bore>"`, or the
/// plain well id for the main bore) — the same key petekio's `model_inputs`
/// emits — so the framework tie + property upscaling consume each bore's *own*
/// positions. We never call the well-level accessors on a multi-bore well (they
/// raise / resolve-empty by design — that raise is the regression signal, not a
/// thing to swallow); we go straight to the bore.
pub struct BoreWell<'a> {
    /// Bore-qualified id: `"<well id> <bore>"` (or the plain well id for the
    /// main bore) — matches `model_inputs().spatial.well_curves[..].well_id`.
    pub id: String,
    bore: &'a Sidetrack,
}

impl BoreWell<'_> {
    /// The interval named by top `name` on this bore (its own picks), or `None`.
    pub fn top(&self, name: &str) -> Option<Interval<'_>> {
        self.bore.top(name)
    }

    /// Interpolated world position at `md` on this bore's own trajectory
    /// (`z` = negative-down elevation), or `None` if unpositioned.
    pub fn xyz(&self, md: f64) -> Option<Point3> {
        self.bore.xyz(md)
    }

    /// The bore's positioned path as world `[x, y, tvd]` stations (tvd
    /// **positive-down**, `tvd == -xyz.z`), sampled evenly along its active
    /// survey's MD range. Empty when the bore carries no trajectory. Feeds the
    /// viewer's well markers + along-bore section (`intersection_bundle(well=..)`).
    pub fn trajectory(&self) -> Vec<[f64; 3]> {
        if self.bore.trajectories().is_empty() {
            return Vec::new();
        }
        let (md0, md1) = self.bore.active().md_range();
        if md1 <= md0 {
            return Vec::new();
        }
        const N: usize = 48;
        (0..=N)
            .filter_map(|s| {
                let md = md0 + (md1 - md0) * (s as f64 / N as f64);
                self.bore.xyz(md).map(|p| [p.x, p.y, -p.z])
            })
            .collect()
    }

    /// A full-curve view of log `mnemonic` on this bore, or `None`.
    pub fn log(&self, mnemonic: &str) -> Option<LogView<'_>> {
        self.bore.log(mnemonic)
    }

    /// This bore's stratigraphic intervals (name + `[top_md, base_md)`), in depth
    /// order — used to colour a crossplot sample by the zone it falls in.
    pub fn zones(&self) -> Vec<Interval<'_>> {
        self.bore.zones()
    }
}

/// A loaded project: the petekio data substrate + the load inventory.
pub struct Project {
    geo: GeoData,
    inventory: Inventory,
    crs: Option<String>,
    /// Compatibility bridge for Petrel `Type == "Other"` rows that petekIO cannot
    /// attach to a loaded bore (for example field-level/contact-inventory rows).
    /// Matched bore contacts come from petekIO; this only preserves the old
    /// facade-visible inventory/pick behaviour for unassigned rows.
    compat_contacts: Vec<(String, String, f64)>,
}

impl Project {
    /// Walk `root`, classify + load every recognised file, and return the
    /// populated project. `crs` is recorded as a provenance label (petekio does
    /// not reproject); `aliases` canonicalise log mnemonics at load.
    ///
    /// # Errors
    /// [`SrsError`] only on an unreadable `root`; per-file failures are recorded
    /// on the [`Inventory`] as skips, never aborting the walk.
    pub fn load(
        root: impl AsRef<Path>,
        crs: Option<String>,
        aliases: Option<Vec<(String, String)>>,
    ) -> Result<Self, SrsError> {
        let root = root.as_ref();
        if !root.is_dir() {
            return Err(SrsError::InvalidInput(format!(
                "project root '{}' is not a directory",
                root.display()
            )));
        }
        let mut geo = GeoData::new(Unit::Metres);
        if let Some(pairs) = aliases {
            geo.set_curve_aliases(NameMap::from_pairs(pairs));
        }
        let mut inv = Inventory::default();

        let mut files = Vec::new();
        collect_files(root, &mut files);
        let mut classified: Vec<(PathBuf, FormatKind)> = Vec::new();
        for path in files {
            match detect(&path) {
                Ok(kind) => classified.push((path, kind)),
                Err(e) => inv
                    .skipped
                    .push((path.display().to_string(), e.to_string())),
            }
        }

        // 1) Wells first (so tops can attach to already-loaded wells). Discover
        //    each well id from detected survey/log filenames anywhere in the tree,
        //    then route it through petekio's directory-walking `load_well` (F1 + F6):
        //
        //    - `load_well` walks the tree **recursively** and keeps only this
        //      well's files, so the real Petrel split `Wells/Paths/` +
        //      `Wells/Logs/` layout and loose well files outside a hardcoded
        //      `Wells/` directory both work.
        //    - A single `.wellpath` is the well's one (main) bore, so its logs
        //      position on the **real deviated trajectory** — no synthesized
        //      vertical (F6) — and the `.wellpath` header supplies the
        //      authoritative wellhead datum (we pass a zero placeholder).
        let mut tops_files: Vec<PathBuf> = Vec::new();
        let ids = discover_well_ids(&classified);
        for id in &ids {
            // Discovery is by well id; the bore-level inventory is derived from
            // the loaded substrate at the end (so `inventory().wells` matches
            // `proj.wells()` exactly). A load failure is a loud skip.
            if let Err(e) = geo.load_well(id, (0.0, 0.0), 0.0, root) {
                inv.skipped
                    .push((format!("{} [well '{id}']", root.display()), e.to_string()));
            }
        }
        // F8: LAS 3.0 **core** files (stem carries "core") are folded into a bore
        // by `load_well` above and would otherwise leave no trace. Record each
        // against its now-loaded well, naming the bore it merged into (mirroring
        // petekio's filename→bore routing).
        record_core_merges(&geo, root, &ids, &mut inv.merged);

        // 2) Everything else by detected content / parent-dir hint (recursive),
        //    holding tops files for step 3. Unknown keeps the old extension
        //    fallback so odd but supported exports do not regress.
        for (path, kind) in &classified {
            if matches!(
                kind,
                FormatKind::Las | FormatKind::WellPath | FormatKind::CrsMetaXml
            ) {
                continue; // already ingested well-side
            }
            let ext = ext_of(path);
            let parent = parent_name(path);
            if *kind == FormatKind::PetrelTops || is_tops_file(path, &ext) {
                // `.tops`/`.petrel`, or an extension-less Petrel tops export whose
                // name carries "tops" (e.g. `FieldTops`) — the real raw layout (F3).
                tops_files.push(path.clone());
            } else if let Some(load_kind) = detected_load_kind(*kind, &parent, &ext) {
                load_named(&mut geo, path, &mut inv, load_kind);
            } else if let Some(load_kind) = unknown_extension_load_kind(&ext) {
                load_named(&mut geo, path, &mut inv, load_kind);
            } else {
                inv.skipped.push((
                    path.display().to_string(),
                    format!("unrecognised format ({kind:?}, extension '.{ext}')"),
                ));
            }
        }

        // 3) Tops (multi-well Petrel picks) after the wells exist. petekio
        //    distributes both `Type == "Horizon"` stratigraphic picks and
        //    `Type == "Other"` fluid contacts onto the matching bores.
        let mut compat_contacts: Vec<(String, String, f64)> = Vec::new();
        for tp in tops_files {
            compat_contacts.extend(parse_other_contacts(&tp));
            if let Err(e) = geo.load_well_tops(&tp) {
                inv.skipped.push((tp.display().to_string(), e.to_string()));
            }
        }
        inv.tops = discover_tops(&geo, &compat_contacts);
        inv.surfaces.sort();
        inv.surfaces.dedup();

        let mut this = Self {
            geo,
            inventory: inv,
            crs,
            compat_contacts,
        };
        // Inventory wells are **bore-level**, derived from the same positioned
        // bores `proj.wells()` yields — one entry per positioned bore under its
        // bore-qualified id (`"99/9-1 A"`; a single-bore well keeps its plain id).
        // So the inventory advertises the exact ids the modelling seams consume,
        // not the parent well id.
        this.inventory.wells = this.wells().iter().map(|bw| bw.id.clone()).collect();
        Ok(this)
    }

    /// The load inventory (loaded artifacts + skipped-with-reason).
    pub fn inventory(&self) -> &Inventory {
        &self.inventory
    }

    /// The recorded CRS provenance label, if any.
    pub fn crs(&self) -> Option<&str> {
        self.crs.as_deref()
    }

    /// The loaded wells as **bore-level** entries — one [`BoreWell`] per
    /// positioned bore (the R-a wiring). A single-bore well yields one entry
    /// under its plain id; a multi-sidetrack well (`99/9-1` with bores A/B/ST2)
    /// yields one entry *per bore* under `"99/9-1 A"` etc., each positioned by
    /// that bore's own trajectory. Unpositioned bores (e.g. the empty main bore
    /// of a multi-bore well) carry no penetration and are skipped, so the
    /// framework tie + property upscaling see exactly the real, positioned bores.
    pub fn wells(&self) -> Vec<BoreWell<'_>> {
        let mut out = Vec::new();
        for well in self.geo.wells().iter() {
            for bore in well.sidetracks() {
                if bore.trajectories().is_empty() {
                    continue; // no trajectory → not an independent positioned bore
                }
                out.push(BoreWell {
                    id: well.bore_id(&bore.label),
                    bore,
                });
            }
        }
        out
    }

    /// A loaded surface by name (grid or gridded scattered set), or `None`.
    pub fn surface(&self, name: &str) -> Option<&Surface> {
        self.geo.surface(name)
    }

    /// The raw stored value of a loaded surface at world `(x, y)` — the nearest
    /// node on the surface's own axis-aligned lattice (`NaN` for a degenerate,
    /// zero-spacing geometry). The value is returned as stored: petekio's
    /// negative-down elevation for a depth surface, or the value itself for a
    /// value grid (a depositional-trend map). The same node→world mapping the
    /// collocated trend reads, so a value sampled here matches what steers a
    /// collocated population.
    ///
    /// # Errors
    /// [`SrsError::InvalidInput`] if `name` names no loaded surface.
    pub fn surface_value_at(&self, name: &str, x: f64, y: f64) -> Result<f64, SrsError> {
        let s = self.require_surface(name)?;
        Ok(nearest_node_value(s, x, y))
    }

    /// Batched [`surface_value_at`](Self::surface_value_at): sample the surface at
    /// each `(x, y)` in `points` with one name→surface resolution (the chatty
    /// per-point lookup collapsed to a single crossing + loop).
    ///
    /// # Errors
    /// [`SrsError::InvalidInput`] if `name` names no loaded surface.
    pub fn surface_values_at(&self, name: &str, points: &[[f64; 2]]) -> Result<Vec<f64>, SrsError> {
        let s = self.require_surface(name)?;
        Ok(points
            .iter()
            .map(|p| nearest_node_value(s, p[0], p[1]))
            .collect())
    }

    /// Build a collocated-cokriging [`TrendSpec`] from a loaded surface.
    ///
    /// **A trend is not a depth.** With `as_depth = false` the surface is read as
    /// a trend in its own units (a net-sand fraction, an amplitude, …), so `corr`
    /// reads against the surface value directly. With `as_depth = true` a
    /// structural surface's petekio negative-down elevation is flipped to
    /// positive-down depth (the framework's Z-flip) so `corr > 0` means the
    /// property increases with depth. Either way the field is shifted up to be
    /// non-negative when needed (the seam's trend is a multiplier domain);
    /// because the collocated secondary is standardized internally (normal
    /// scores), the additive offset is steering-neutral — it only satisfies the
    /// constructor.
    ///
    /// # Errors
    /// [`SrsError::InvalidInput`] if `name` names no loaded surface; the trend
    /// constructor's error otherwise.
    pub fn collocated_trend(
        &self,
        name: &str,
        corr: f64,
        as_depth: bool,
    ) -> Result<TrendSpec, SrsError> {
        let s = self.require_surface(name)?;
        let g = &s.geom;
        let (ncol, nrow) = (g.ncol, g.nrow);
        let vals = s.values();
        let mut values = vec![f64::NAN; ncol * nrow];
        let mut min_finite = f64::INFINITY;
        for j in 0..nrow {
            for i in 0..ncol {
                // as_depth: structural elevation → positive-down depth; else the
                // trend value as-is (its own units).
                let v = if as_depth {
                    -vals[[i, j]]
                } else {
                    vals[[i, j]]
                };
                values[j * ncol + i] = v;
                if v.is_finite() {
                    min_finite = min_finite.min(v);
                }
            }
        }
        // Shift any negative field up into the multiplier domain (steering-neutral).
        if min_finite.is_finite() && min_finite < 0.0 {
            for v in values.iter_mut() {
                if v.is_finite() {
                    *v -= min_finite;
                }
            }
        }
        TrendSpec::collocated(ncol, nrow, values, g.xori, g.yori, g.xinc, g.yinc, corr)
    }

    /// Resolve a loaded surface by name or raise the shared "not loaded" error.
    fn require_surface(&self, name: &str) -> Result<&Surface, SrsError> {
        self.geo
            .surface(name)
            .ok_or_else(|| SrsError::InvalidInput(format!("surface '{name}' not loaded")))
    }

    /// A named horizon as an owned gridded [`Surface`] (F2): a loaded **grid**
    /// surface is returned as-is; a loaded **scattered point-set** (real seismic
    /// horizons arrive as `.IrapClassicPoints`) is gridded onto `lattice` with
    /// Briggs minimum-curvature — the same kernel the Rust stack uses — via
    /// petekio's `PointSet::to_surface`. `lattice = None` derives a build lattice
    /// from the point-set's bounding box (the first horizon establishes the shared
    /// framework lattice; later horizons pass `Some(top_geom)` to grid onto it).
    ///
    /// `cell_size_m` sets the derived lattice's cell size when `lattice = None`
    /// (the first, lattice-establishing horizon): the node counts are `ceil(extent
    /// / cell_size) + 1` per axis, capped loudly at [`MAX_CELLS_PER_AXIS`]. `None`
    /// falls back to the legacy fixed [`DEFAULT_TARGET_CELLS`]²; ignored once a
    /// `lattice` is supplied.
    ///
    /// # Errors
    /// [`SrsError::InvalidInput`] if `name` names neither a loaded surface nor a
    /// loaded point-set (or the point-set is empty); the gridder's error otherwise.
    pub fn horizon_surface(
        &self,
        name: &str,
        lattice: Option<&GridGeometry>,
        cell_size_m: Option<f64>,
    ) -> Result<Surface, SrsError> {
        if let Some(s) = self.geo.surface(name) {
            return Ok(s.clone());
        }
        if let Some(ps) = self.geo.points(name) {
            if ps.is_empty() {
                return Err(SrsError::InvalidInput(format!(
                    "horizon point-set '{name}' has no points to grid"
                )));
            }
            let geom = lattice
                .cloned()
                .unwrap_or_else(|| lattice_from_bbox(&ps.bbox(), cell_size_m));
            return ps
                .to_surface(geom, GridMethod::MinimumCurvature)
                .map_err(SrsError::from);
        }
        Err(SrsError::InvalidInput(format!(
            "horizon '{name}' is neither a loaded surface nor a loaded point-set"
        )))
    }

    /// The horizon's build **lattice geometry only** — cheaply, WITHOUT gridding.
    ///
    /// A loaded grid surface returns its own geometry; a scattered point-set returns
    /// the lattice derived from its bounding box ([`lattice_from_bbox`], identical to
    /// the lattice [`Self::horizon_surface`] would grid it onto) — but skips the
    /// expensive minimum-curvature solve. Used by the framework to fix the shared
    /// areal lattice (`top_geom`) for the **raw-scatter stack** path, where the top's
    /// gridded VALUES are never used (the engine conditions the scatter itself) — only
    /// its lattice is needed. `cell_size_m` sizes a derived point-set lattice (ignored
    /// for a loaded surface); see [`Self::horizon_surface`].
    ///
    /// # Errors
    /// [`SrsError::InvalidInput`] if `name` names neither a loaded surface nor a loaded
    /// (non-empty) point-set.
    pub fn horizon_geom(
        &self,
        name: &str,
        cell_size_m: Option<f64>,
    ) -> Result<GridGeometry, SrsError> {
        if let Some(s) = self.geo.surface(name) {
            return Ok(s.geom.clone());
        }
        if let Some(ps) = self.geo.points(name) {
            if ps.is_empty() {
                return Err(SrsError::InvalidInput(format!(
                    "horizon point-set '{name}' has no points to grid"
                )));
            }
            return Ok(lattice_from_bbox(&ps.bbox(), cell_size_m));
        }
        Err(SrsError::InvalidInput(format!(
            "horizon '{name}' is neither a loaded surface nor a loaded point-set"
        )))
    }

    /// The underlying petekio substrate (for the framework/property seams).
    pub fn geo(&self) -> &GeoData {
        &self.geo
    }

    /// Per-well picks of a named tops surface (e.g. "GOC"/"FWL") + a
    /// representative level with spread. `None` if no well carries the pick.
    ///
    /// `wells` optionally restricts the aggregation to a subset of bores/wells: a
    /// pick is kept when its bore-qualified id is listed, or a listed **well** id
    /// prefixes it (`"99/9-1"` selects both `"99/9-1 A"` and `"99/9-1 B"`). `None`
    /// (or an empty list) aggregates across every well.
    pub fn pick(&self, name: &str, wells: Option<&[String]>) -> Option<TopsPick> {
        let keep = |id: &str| match wells {
            None | Some([]) => true,
            Some(ws) => ws
                .iter()
                .any(|w| id == w || id.starts_with(&format!("{w} "))),
        };
        let mut picks: Vec<(String, f64)> = Vec::new();
        // Stratigraphic (Type="Horizon") picks distributed onto the wells — read
        // **per bore** (a multi-sidetrack well's picks live on its bores; the
        // well-level accessor would resolve-empty on a multi-bore well). Depth is
        // subsea TVD (`-z`) at the pick MD on the bore's own trajectory.
        for bw in self.wells() {
            if !keep(&bw.id) {
                continue;
            }
            if let Some(iv) = bw.top(name) {
                if let Some(d) = bw.xyz(iv.top_md).map(|p| -p.z) {
                    picks.push((bw.id.clone(), d));
                }
            }
        }
        let compat_has_name = self
            .compat_contacts
            .iter()
            .any(|(surf, _, _)| surf.eq_ignore_ascii_case(name));
        for (surf, well, depth) in &self.compat_contacts {
            if surf.eq_ignore_ascii_case(name)
                && keep(well)
                && !picks
                    .iter()
                    .any(|(w, d)| w.eq_ignore_ascii_case(well) && (*d - *depth).abs() < 1e-9)
            {
                picks.push((well.clone(), *depth));
            }
        }
        // PetekIO is the primary owner of contact assignment to bores. Today that
        // seam exposes contact MD, while this facade's public `TopsPick.level_m`
        // is TVDSS from the Petrel Z column; when a parsed Z-row exists, use it.
        // Fall back to petekIO contacts only for contacts that have no file-level
        // compatibility row.
        if !compat_has_name {
            for bw in self.wells() {
                if !keep(&bw.id) {
                    continue;
                }
                for c in bw.bore.contacts() {
                    if c.name.eq_ignore_ascii_case(name) {
                        let depth = bw.xyz(c.md).map(|p| -p.z).unwrap_or(c.md);
                        picks.push((bw.id.clone(), depth));
                    }
                }
            }
        }
        if picks.is_empty() {
            return None;
        }
        let n = picks.len() as f64;
        let level = picks.iter().map(|(_, d)| *d).sum::<f64>() / n;
        let spread = if picks.len() < 2 {
            0.0
        } else {
            let var = picks.iter().map(|(_, d)| (d - level).powi(2)).sum::<f64>() / (n - 1.0);
            var.sqrt()
        };
        Some(TopsPick {
            name: name.to_string(),
            picks,
            level_m: level,
            spread_m: spread,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cell_size_derives_lattice_from_extent() {
        // A synthetic ~30 km × 20 km extent at 100 m cells → ~300 × 200 cells
        // (301 × 201 nodes), not the fixed 100² fallback. (coordinator addendum:
        // regional point-horizon frameworks need resolution control.)
        let b = BBox {
            xmin: 500_000.0,
            xmax: 530_000.0, // 30 km
            ymin: 6_700_000.0,
            ymax: 6_720_000.0, // 20 km
        };
        let g = lattice_from_bbox(&b, Some(100.0));
        assert_eq!(g.ncol, 301, "30 km / 100 m → 300 cells → 301 nodes");
        assert_eq!(g.nrow, 201, "20 km / 100 m → 200 cells → 201 nodes");
        assert!((g.xinc - 100.0).abs() < 1e-9 && (g.yinc - 100.0).abs() < 1e-9);
        // No cell_size_m → the legacy fixed 100²-node fallback, extent-independent.
        let f = lattice_from_bbox(&b, None);
        assert_eq!((f.ncol, f.nrow), (101, 101));
    }

    #[test]
    fn cell_size_caps_loudly_at_the_axis_maximum() {
        // A sub-metre cell over a basin extent would blow the budget → capped.
        let b = BBox {
            xmin: 0.0,
            xmax: 1_000_000.0,
            ymin: 0.0,
            ymax: 10.0,
        };
        let g = lattice_from_bbox(&b, Some(0.5));
        assert_eq!(g.ncol, MAX_CELLS_PER_AXIS + 1, "x-axis capped");
        assert_eq!(
            g.nrow, 21,
            "small y-axis uncapped (10 m / 0.5 m = 20 cells)"
        );
    }

    #[test]
    fn detector_routes_ambiguous_points_by_parent_or_extension() {
        assert!(matches!(
            detected_load_kind(
                FormatKind::IrapClassicPoints,
                "surfaces",
                "irapclassicpoints"
            ),
            Some(Kind::Points)
        ));
        assert!(matches!(
            detected_load_kind(FormatKind::IrapClassicPoints, "polygons", "pol"),
            Some(Kind::Polygons)
        ));
    }
}
