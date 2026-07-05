# Changelog

All notable changes to petekSim are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres
to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

Nothing yet.

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
  `Props`, `Mc`) or HOW (`TieSettings`, `Gridding`, `Run`, `LoadSettings`,
  `ViewSettings`). A spec holds **names**, not project objects — so it is
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

### Licensing

- petekSim is licensed under the **Business Source License 1.1** (BUSL-1.1); each
  released version converts to Apache-2.0 four years after publication. See `LICENSE`.

[Unreleased]: https://github.com/kkollsga/peteksim/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/kkollsga/peteksim/releases/tag/v0.1.0
