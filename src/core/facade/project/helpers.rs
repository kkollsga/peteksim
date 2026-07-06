//! Ingest helpers for [`super::Project`] — file discovery + classification,
//! Petrel tops/contacts parsing, lattice geometry, and path utilities. No
//! public surface; `Project` (in the parent module) drives these.

use super::*;

/// The stored value at the nearest node of a surface's axis-aligned lattice to
/// world `(x, y)`. `NaN` for a degenerate (zero-spacing) geometry. The `(x, y)`
/// is mapped to node indices by rounding on each axis and clamping into range —
/// the same node→world convention the collocated trend georeference uses.
pub(super) fn nearest_node_value(s: &Surface, x: f64, y: f64) -> f64 {
    let g = &s.geom;
    if g.xinc == 0.0 || g.yinc == 0.0 {
        return f64::NAN;
    }
    let i = (((x - g.xori) / g.xinc).round() as i64).clamp(0, g.ncol as i64 - 1) as usize;
    let j = (((y - g.yori) / g.yinc).round() as i64).clamp(0, g.nrow as i64 - 1) as usize;
    s.values()[[i, j]]
}

/// Discover well ids from detected survey/log filenames anywhere in the project
/// tree. Ids come from **wellpath** stems first (the surveys define wells); when
/// a tree carries no surveys they come from LAS stems. Each stem's trailing bore
/// token is dropped so every bore of a well folds to one id, which petekio's
/// `load_well` then re-groups into bores. A core LAS thus shares its well's id
/// rather than spawning a spurious well.
pub(super) fn discover_well_ids(files: &[(PathBuf, FormatKind)]) -> Vec<String> {
    let mut ids = ids_from_detected_stems(files, FormatKind::WellPath);
    if ids.is_empty() {
        ids = ids_from_detected_stems(files, FormatKind::Las);
    }
    ids
}

/// The deduped, sorted well ids from every detected `files` entry with `kind`
/// (case-insensitive dedup so `99/9-1` variants collapse).
pub(super) fn ids_from_detected_stems(
    files: &[(PathBuf, FormatKind)],
    kind: FormatKind,
) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    for p in files.iter().filter(|(_, k)| *k == kind).map(|(p, _)| p) {
        let id = well_id_from_stem(&file_stem(p));
        if !ids.iter().any(|e| e.eq_ignore_ascii_case(&id)) {
            ids.push(id);
        }
    }
    ids.sort();
    ids
}

/// The well id of a survey/log filename stem: the stem with a trailing
/// underscore-delimited **bore token** removed (`99_9-1_ST2` → `99_9-1`,
/// `99_9-1_A` → `99_9-1`) so every bore of a well shares one id. A stem with no
/// bore token (a per-well-subdir name like `A1`, or an id like `99_9-1`) is
/// returned unchanged.
pub(super) fn well_id_from_stem(stem: &str) -> String {
    if let Some((head, last)) = stem.rsplit_once('_') {
        if !head.is_empty() && is_bore_token(last) {
            return head.to_string();
        }
    }
    stem.to_string()
}

/// Whether `t` looks like a Petrel bore/sidetrack token: a sidetrack `ST<n>`, or
/// a bore letter `A`..`Z` optionally with 1–2 trailing digits (`A`, `B`, `A1`).
pub(super) fn is_bore_token(t: &str) -> bool {
    if t.is_empty() || t.len() > 4 {
        return false;
    }
    let up = t.to_ascii_uppercase();
    if let Some(rest) = up.strip_prefix("ST") {
        return !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit());
    }
    let mut chars = up.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic()) && chars.all(|c| c.is_ascii_digit())
}

/// Record the LAS 3.0 **core** files merged into a bore (F8). A core file is a
/// LAS whose stem carries "core" (case-insensitive) — the same signal petekio's
/// loader uses to tag the curves `LogKind::Core` before folding them into a bore.
/// We look each up against its now-loaded well and name the bore it merged into,
/// mirroring petekio's filename→bore routing (a token matching a **named** bore,
/// else the main bore — a single-bore well always merges into main). `out` gains
/// `(filename, bore_id)` per core file.
pub(super) fn record_core_merges(
    geo: &GeoData,
    wells_dir: &Path,
    ids: &[String],
    out: &mut Vec<(String, String)>,
) {
    let mut files = Vec::new();
    collect_files(wells_dir, &mut files);
    for p in &files {
        let stem = file_stem(p);
        if ext_of(p) != "las" || !stem.to_ascii_lowercase().contains("core") {
            continue;
        }
        let Some(id) = ids.iter().find(|id| file_belongs_to(p, id)) else {
            continue;
        };
        let Some(well) = geo.well(id) else {
            continue;
        };
        let tokens: Vec<&str> = stem.split(['_', '-', '.', ' ']).collect();
        let label = well
            .bores()
            .find(|l| !l.is_empty() && tokens.iter().any(|t| t.eq_ignore_ascii_case(l)))
            .unwrap_or("")
            .to_string();
        let file = p
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();
        out.push((file, well.bore_id(&label)));
    }
}

/// Whether `path`'s stem names the well `id` — its normalized stem equals `id`'s
/// key or extends it at an `_` boundary (`99_9-1_A_core` belongs to `99_9-1`).
pub(super) fn file_belongs_to(path: &Path, id: &str) -> bool {
    let norm = norm_key(&file_stem(path));
    let nid = norm_key(id);
    norm == nid || norm.strip_prefix(&nid).is_some_and(|r| r.starts_with('_'))
}

/// A separator/case-folded id key (mirrors petekio's `normalize_id`): lowercase,
/// with `/`, `-`, and space folded to `_`.
pub(super) fn norm_key(s: &str) -> String {
    s.trim().to_ascii_lowercase().replace(['/', '-', ' '], "_")
}

/// Fixed per-axis cell count when no `cell_size_m` is given (the legacy fallback).
const DEFAULT_TARGET_CELLS: usize = 100;
/// Loud cap on the per-axis cell count derived from a `cell_size_m` — a regional
/// ModelEdge grid at 100 m cells legitimately needs several hundred nodes per
/// axis, but an accidental sub-metre `cell_size_m` over a basin extent would
/// otherwise blow the cell budget. Hitting the cap warns.
pub(super) const MAX_CELLS_PER_AXIS: usize = 2000;

/// Node count + node spacing for one axis: `ceil(extent / cell)` cells (so the
/// lattice covers the extent at exactly `cell` spacing), capped loudly at
/// [`MAX_CELLS_PER_AXIS`] (spacing then stretched to span the extent). Returns
/// `(nodes, spacing)`.
pub(super) fn axis_from_cell(extent: f64, cell: f64) -> (usize, f64) {
    let ncells = ((extent / cell).ceil() as usize).max(1);
    if ncells > MAX_CELLS_PER_AXIS {
        eprintln!(
            "peteksim: point-horizon lattice capped at {MAX_CELLS_PER_AXIS} cells/axis \
             (extent {extent:.0} m / cell {cell:.1} m -> {ncells} cells); coarsen cell_size_m \
             or accept the cap"
        );
        (MAX_CELLS_PER_AXIS + 1, extent / MAX_CELLS_PER_AXIS as f64)
    } else {
        (ncells + 1, cell)
    }
}

/// Derive a regular build lattice from a scattered point-set's bounding box
/// (F2), axis-aligned, origin at the box corner — used to grid the **first**
/// horizon point-set when no shared lattice exists yet. `cell_size_m = Some(cs)`
/// sizes the lattice at `cs`-metre cells (`ceil(extent/cs)+1` nodes/axis, capped);
/// `None` falls back to a fixed [`DEFAULT_TARGET_CELLS`]²-node lattice regardless
/// of extent (the legacy default).
pub(super) fn lattice_from_bbox(b: &BBox, cell_size_m: Option<f64>) -> GridGeometry {
    let w = (b.xmax - b.xmin).abs().max(1.0);
    let h = (b.ymax - b.ymin).abs().max(1.0);
    let (ncol, nrow, xinc, yinc) = match cell_size_m {
        Some(cs) if cs.is_finite() && cs > 0.0 => {
            let (ncol, xinc) = axis_from_cell(w, cs);
            let (nrow, yinc) = axis_from_cell(h, cs);
            (ncol, nrow, xinc, yinc)
        }
        _ => (
            DEFAULT_TARGET_CELLS + 1,
            DEFAULT_TARGET_CELLS + 1,
            w / DEFAULT_TARGET_CELLS as f64,
            h / DEFAULT_TARGET_CELLS as f64,
        ),
    };
    GridGeometry {
        xori: b.xmin,
        yori: b.ymin,
        xinc,
        yinc,
        ncol,
        nrow,
        rotation_deg: 0.0,
        yflip: false,
    }
}

pub(super) fn discover_tops(
    geo: &GeoData,
    compat_contacts: &[(String, String, f64)],
) -> Vec<String> {
    let mut names: BTreeSet<String> = BTreeSet::new();
    // Read zones **per bore** — a multi-sidetrack well carries its strat column on
    // each bore, and the well-level `zones()` would resolve-empty on it.
    for well in geo.wells().iter() {
        for bore in well.sidetracks() {
            for iv in bore.zones() {
                names.insert(iv.name.clone());
            }
            for c in bore.contacts() {
                names.insert(c.name.clone());
            }
        }
    }
    for (name, _, _) in compat_contacts {
        names.insert(name.clone());
    }
    names.into_iter().collect()
}

/// Compatibility parser for Petrel `Type == "Other"` rows that petekIO cannot
/// attach to a loaded bore. petekIO-owned bore contacts are the primary source;
/// this preserves old facade-visible inventory/pick behaviour for unassigned
/// field-level rows and Latin-1 contact names until petekIO exposes those rows
/// separately.
pub(super) fn parse_other_contacts(path: &Path) -> Vec<(String, String, f64)> {
    let Ok(bytes) = std::fs::read(path) else {
        return Vec::new();
    };
    let text = decode_latin1(&bytes);
    let mut header: Vec<String> = Vec::new();
    let mut in_header = false;
    let mut done_header = false;
    let mut out = Vec::new();
    for line in text.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        if !done_header {
            if t.eq_ignore_ascii_case("BEGIN HEADER") {
                in_header = true;
            } else if t.eq_ignore_ascii_case("END HEADER") {
                done_header = true;
            } else if in_header {
                header.push(t.to_ascii_lowercase());
            }
            continue;
        }
        let toks = split_petrel_tokens(t);
        if toks.len() < header.len() {
            continue;
        }
        let col = |name: &str| {
            header
                .iter()
                .position(|h| h == name)
                .and_then(|i| toks.get(i))
        };
        let Some(kind) = col("type") else { continue };
        if !kind.eq_ignore_ascii_case("Other") {
            continue;
        }
        let Some(surface) = col("surface") else {
            continue;
        };
        let well = col("well").cloned().unwrap_or_default();
        let Some(depth) = col("z").and_then(|s| s.parse::<f64>().ok()).map(|z| -z) else {
            continue;
        };
        if depth.is_finite() {
            out.push((surface.clone(), well, depth));
        }
    }
    out
}

pub(super) fn decode_latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| b as char).collect()
}

pub(super) fn split_petrel_tokens(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_quote = false;
    for ch in line.chars() {
        match ch {
            '"' => in_quote = !in_quote,
            c if c.is_whitespace() && !in_quote => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Whether `path` is a Petrel well-tops file: extension `.tops`/`.petrel`, or an
/// **extension-less** export whose stem contains "tops" (real Petrel exports ship
/// the tops file with no extension, e.g. `FieldTops` — F3).
pub(super) fn is_tops_file(path: &Path, ext: &str) -> bool {
    ext == "tops"
        || ext == "petrel"
        || (ext.is_empty() && file_stem(path).to_ascii_lowercase().contains("tops"))
}

pub(super) enum Kind {
    Surface,
    Polygons,
    Points,
}

pub(super) fn detected_load_kind(kind: FormatKind, parent: &str, ext: &str) -> Option<Kind> {
    match kind {
        FormatKind::IrapClassicGrid | FormatKind::Cps3Grid => Some(Kind::Surface),
        FormatKind::Cps3Lines => Some(Kind::Polygons),
        FormatKind::GeoJson => {
            if dir_is(parent, &["point", "scatter"]) {
                Some(Kind::Points)
            } else {
                Some(Kind::Polygons)
            }
        }
        FormatKind::IrapClassicPoints => {
            if dir_is(parent, &["polygon", "outline"]) || matches!(ext, "pol" | "shp") {
                Some(Kind::Polygons)
            } else {
                Some(Kind::Points)
            }
        }
        FormatKind::EarthVisionGrid | FormatKind::CsvPoints => Some(Kind::Points),
        FormatKind::Las
        | FormatKind::WellPath
        | FormatKind::PetrelTops
        | FormatKind::CrsMetaXml
        | FormatKind::Unknown => None,
    }
}

pub(super) fn unknown_extension_load_kind(ext: &str) -> Option<Kind> {
    match ext {
        "irap" | "gri" | "cps3grid" => Some(Kind::Surface),
        "geojson" | "json" | "pol" | "shp" | "cps3lines" => Some(Kind::Polygons),
        "csv" | "xyz" | "dat" | "irapclassicpoints" | "earthvisiongrid" => Some(Kind::Points),
        _ => None,
    }
}

pub(super) fn load_named(geo: &mut GeoData, path: &Path, inv: &mut Inventory, kind: Kind) {
    let name = file_stem(path);
    let res = match kind {
        Kind::Surface => geo
            .load_surface(&name, path)
            .map(|_| ())
            .map_err(geo_reason),
        Kind::Polygons => geo
            .load_polygons(&name, path)
            .map(|_| ())
            .map_err(geo_reason),
        Kind::Points => geo.load_points(&name, path).map(|_| ()).map_err(geo_reason),
    };
    match res {
        Ok(()) => match kind {
            Kind::Surface => inv.surfaces.push(name),
            Kind::Polygons => inv.polygons.push(name),
            Kind::Points => inv.points.push(name),
        },
        Err(reason) => inv.skipped.push((path.display().to_string(), reason)),
    }
}

pub(super) fn geo_reason(e: petekio::GeoError) -> String {
    SrsError::from(e).to_string()
}

// --- path helpers ------------------------------------------------------------

pub(super) fn ext_of(p: &Path) -> String {
    p.extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

pub(super) fn file_stem(p: &Path) -> String {
    p.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unnamed")
        .to_string()
}

pub(super) fn parent_name(p: &Path) -> String {
    p.parent()
        .and_then(|d| d.file_name())
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

pub(super) fn dir_is(name: &str, needles: &[&str]) -> bool {
    needles.iter().any(|n| name.contains(n))
}

pub(super) fn collect_files(root: &Path, out: &mut Vec<PathBuf>) {
    if let Ok(rd) = std::fs::read_dir(root) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                collect_files(&p, out);
            } else if p.is_file() {
                out.push(p);
            }
        }
    }
    out.sort();
}
