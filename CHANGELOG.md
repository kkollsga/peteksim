# Changelog

All notable changes to petekSim are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres
to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.7] - 2026-07-08

### Changed

- Updated release-train dependency floors to `petektools 0.2.5`,
  `petekio 0.3.6`, and `petekstatic 0.1.9`; the Python wheel now requires
  those published floors so petekSim consumes the current project/object,
  interpolation, and static workflow releases.

## [0.1.6] - 2026-07-08

### Changed

- Updated release-train dependency floors to `petekio 0.3.5` and
  `petekstatic 0.1.8`; the Python wheel now requires `petekio>=0.3.5` and
  `petekstatic>=0.1.8`.
- Updated public docs to use petekIO's current raw-import API:
  `petekio.Project.import_data(..., settings=petekio.ImportSettings(...))`.

## [0.1.5] - 2026-07-07

### Changed

- Updated release-train dependency floors to `petekio 0.3.4` and
  `petekstatic 0.1.7`; the Python wheel now requires `petekio>=0.3.4` and
  `petekstatic>=0.1.7`.

## [0.1.4] - 2026-07-07

### Changed

- Removed public project-loading/project-handling exports from `peteksim`; the
  project container and loading API now live in `petekio`.
- Updated docs and tests so examples use `petekio.Project.load(...)` and
  `project.wells.logs` instead of project handling through petekSim.
- Updated release-train dependency floors to `petekio 0.3.3`,
  `petektools 0.2.4`, and `petekstatic 0.1.6`; the Python wheel requires
  `petektools>=0.2.4`, `petekio>=0.3.3`, and `petekstatic>=0.1.6`.
- Updated CI and release workflows to current action versions and the
  Actions-owned release flow.

## [0.1.3] - 2026-07-07

### Changed

- **Static property workflow compatibility now delegates to petekStatic.**
  `peteksim.upscale`, `peteksim.distributions`, `peteksim.Var`,
  `peteksim.Grid`, `peteksim.PropertyPipelineSpec`, `peteksim.WellLogSpec`,
  `peteksim.PropertyPipeline`, and `peteksim.WellLog` are a narrow shim to the
  canonical petekStatic workflow API. Legacy `ps.Prop(...)` / `ps.Props(...)`
  constructors still work for existing petekSim specs, but emit deprecation
  guidance toward petekStatic.
- **Release-train dependency floors are current.** The Rust crate resolves
  against `petekio 0.3.2`, `petektools 0.2.3`, and `petekstatic 0.1.5`; the
  Python wheel requires `petektools>=0.2.3` and `petekstatic>=0.1.5`.

## [0.1.2] - 2026-07-06

### Changed

- **Consume the fortified petekIO/petekTools foundations.** Project ingest now
  classifies files through `petekio::detect()` with legacy fallback only for
  `Unknown`, loads detected well files anywhere under the project root, and reads
  Petrel `Type == "Other"` fluid contacts from petekIO's bore-contact seam instead
  of the old facade-side parser.
- **`peteksim.synth_asset` is now a compatibility shim over
  `petektools.synth_asset`.** The petekSim-owned `spill_recipe` helper remains,
  and the shim preserves the manifest `spill_recipe` key for existing callers.

### Fixed

- **Outline resolution is loud.** An explicit missing `outline="Name"` now errors
  with the loaded polygon inventory. An omitted outline still tries `ModelEdge`,
  then falls back to the framework bbox with a visible warning if `ModelEdge` is
  absent.

## [0.1.1] - 2026-07-05

### Changed

- **Require `petektools>=0.2.1` in the wheel.** The published 0.1.0 wheel's
  metadata allowed petektools 0.2.0, whose wheels are unimportable on Python
  3.10/3.11 — so upgraders could keep the broken viewer dependency. The 0.1.1
  floor forces the fixed release, repairing `model.view()` / `save_view()`
  imports on Python 3.10/3.11 for anyone upgrading.
- **Release workflow is now gated on CI.** The tag-triggered Release run
  executes the same fmt / clippy / test bar as CI in a `gates` job before any
  build or publish job starts — a red gate blocks the release by construction.

### Fixed

- **Clippy `manual_option_zip` (Rust 1.96).** Lint fix only; no behaviour
  change.

## [0.1.0] - 2026-07-05

First public release of **`peteksim`** — the Python-facing appraisal toolkit: a
pure-Rust reservoir core with thin bindings that presents the whole
subsurface-modelling stack (ingest → geomodel → volumetrics → uncertainty) as one
facade. From a Petrel export to a STOIIP P-curve in a handful of calls.

All inputs and outputs are **SI/metric**: areas in km², lengths/depths in metres
(positive-down), volumes in Sm³ (reported in MSm³ for oil, bcm for gas), GRV in
mcm (10⁶ m³), FVF as dimensionless Rm³/Sm³. Imperial is opt-in on your side, never
a default.

### Added — the declarative modelling API (v2, the primary surface)

- **Specs applied at explicit moments.** A model is built from immutable, declarative
  **spec** values that say WHAT (`Horizons`, `Subzones`, `Layering`, `Contacts`,
  `Props`, `Mc`) or HOW (`TieSettings`, `Gridding`, `Run`, `ViewSettings`).
  A spec holds **names**, not project objects — so it is
  project-independent, reusable across re-exports and synthetic assets. It is applied
  at three explicit moments: `proj.grid_geometry(...)` → `geom.build(...)` →
  `grid.model(...)` → `model.zoned_uncertainty(...)`.
- **Loud at apply.** Errors when a spec is applied name **both** the missing project
  object and the spec entry. A spec field the engine cannot yet honour raises
  `NotYetSupported` (never a silent no-op); an unresolvable name raises `ApplyError`.
- **Value semantics.** Every spec round-trips through a dict (`to_dict` / `from_dict`
  / `ps.spec_from_dict`), compares and hashes by value, derives with `.replace(...)`,
  and pretty-prints as its own domain table — so a scenario is a savable, diffable
  file. `ps.AssetSpec` bundles a whole scenario (load + structure + props + mc) into
  one durable value.
- **Structure specs:** `ps.Horizons(ps.hz(...), zones=[...])` (the ordered
  stratigraphic column + the zones between horizons; per-row well ties and structural
  uncertainty via `ps.hz(tie=, sd=, vgm=)`), `ps.Subzones` / `ps.splits` (intra-zone
  splits + conformity), `ps.Layering(dz= | nk=)`, `ps.Contacts({zone: {...}})`, and
  `ps.zone(name, color=)`.
- **Property specs:** `ps.Props(ps.Prop("PORO", net_only=True, propagate=...))` — a
  visible per-property pipeline of well upscaling then SGS propagation
  (`ps.Propagate` + `ps.variogram(...)` + optional `ps.collocated(name, corr=)` trend,
  `level_shift` / `resimulate` MC modes).

### Added — volumetrics, zoned Monte-Carlo, and P-curves

- **In-place volumes.** `grid.model(...)` returns a populated model; `model.summary()`
  reports STOIIP / GIIP (MSm³) and GRV (mcm). Two-contact columns (gas cap + oil rim)
  split automatically from the contact picks.
- **Zoned uncertainty.** `model.zoned_uncertainty(ps.Mc(...))` runs Monte-Carlo over
  static-model realizations — per-zone contact draws and per-zone property level shifts
  — and returns per-zone and total **P-curves** (`p90` / `p50` / `p10` / `mean`, with
  `*_msm3` reporting scales) plus the full per-realization sample vectors.
- **Multi-zone stacks.** A multi-horizon stack unlocks per-zone layering, per-zone
  contacts (a contactless zone contributes GRV with zero hydrocarbon), and per-zone
  property pipelines. `model.in_place_by_zone()`, `model.zone_stats(prop)`, and
  `model.well_tie_residuals()` report the breakdown; well ties tie each horizon to its
  tops picks.
- **Tornado + aggregation.** A flat `model.uncertainty(...)` result exposes
  `.tornado()` (ranked input swings); `ps.aggregate([...], correlation=...)` sums
  segment realizations under an explicit dependence assumption.

### Added — the analytic box model

- **`ps.run_box_model(...)`** — a quick STOIIP/GIIP estimate with Monte-Carlo on each
  volumetric input (a constant, a `(min, mode, max)` triangular, or a tagged
  `{"normal"|"lognormal"|"uniform"|"triangular": [...]}` distribution). Returns
  P90/P50/P10/mean/deterministic plus the full sample vector.
- **`ps.Model(...)`** — a structured box with real structural relief built in code
  (`add_control(i, j, depth_m)` seeds a high), for a first look before a full project.

### Added — the browser viewer

- **`model.view()`** opens a tabbed, bundle-driven inspection viewer: **Map** (areal
  rasters, outline, contact subcrop masks, well markers, draw-a-fence to cut a
  section), **Intersection** (the vertical cross-section with horizon + contact traces
  and the bore path), and **Volume** (the corner-point mesh in three.js with property
  colouring, threshold, zone toggles, i/j/k clip planes). `view()` is non-blocking
  (prints a URL and returns).
- **`model.save_view("m.html")`** writes ONE self-contained HTML file that opens off
  `file://` with all data + JS inlined — no server, no network (confidential-data
  safe). Bundle accessors `map_bundle` / `intersection_bundle` / `volume_bundle`
  return the JSON directly. The renderer is petekTools' horizontal `petektools.viewer`
  unit, which `peteksim` consumes.

### Added — resources and out-of-core

- **`ps.Run(memory_budget=<bytes>, workers=N)`** carries the run resources: `workers`
  shards the MC realize loop; `memory_budget` forwards to the engine's out-of-core
  switch (a larger-than-memory model spills to disk with a loud notice, never an OOM
  kill).

### Deprecated

- **The v1 eight-call model-build chain** (`proj.framework(...)` → `set_zones` →
  `build_grid` → per-property `upscale`/`propagate` → `grid.model` → `uncertainty` →
  `tornado`) is deprecated in favour of the declarative v2 API, with a two-minor
  window. It keeps working and emits a `DeprecationWarning`; new code should use
  `proj.grid_geometry(...).build(ps.Horizons(...))`.

### Changed — packaging (internal, behaviour-neutral)

- **Consolidated the three published library crates into one.** `srs-units`,
  `srs-pvt` and `srs-core` were merged into a single crate, **`peteksim`**
  (0.1.0), at the repo root. Their boundaries are preserved as modules —
  `peteksim::{units, pvt, core}` — and the headline Rust API (`run_model`, the
  appraisal facade types, the `Distribution` seam, the view bundles) is
  re-exported at the crate root. File moves + mechanical path rewrites only; no
  logic changed. The workspace keeps its `crates/srs-py` member (the `peteksim`
  wheel source), which now binds the `peteksim` crate; the published PyPI wheel
  surface (`peteksim`) is unchanged.
- **Repointed the geomodel dependency at petekStatic's consolidated crate.** The
  former per-crate path deps on petekStatic's `srs-*` crates are now a single
  `petekstatic = "0.1.0"`; the `srs_model::` / `srs_grid::` / `srs_wireframe::`
  (…) imports rewrote to `petekstatic::{model, grid, wireframe, …}::`.
  petekio / petekTools pins unchanged.

### Licensing

- petekSim is licensed under the **Business Source License 1.1** (BUSL-1.1); each
  released version converts to Apache-2.0 four years after publication. See `LICENSE`.

[Unreleased]: https://github.com/kkollsga/peteksim/compare/v0.1.1...HEAD
[0.1.1]: https://github.com/kkollsga/peteksim/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/kkollsga/peteksim/releases/tag/v0.1.0
