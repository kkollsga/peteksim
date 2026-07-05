# Changelog

All notable changes to petekSim are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres
to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

petekSim is pre-release (`0.0.0`); the surface below has not yet been cut into a
tagged version. This entry summarizes the notable state accrued to date; finer
detail starts accruing per-change from here on.

### Performance — the zoned-MC stack template is built + primed once per model
Each `zoned_uncertainty` call used to rebuild a **fresh** stack-aware MC template
from scratch (`Model::stack_template` → `from_horizon_stack` re-resolve of the
surfaces + the per-zone `LevelShift` SGS pattern re-propagated on every clone's
first realize) — a fixed ~1.4 s independent of `n`, paid **again** on every call.
On the canonical run this meant the n=8 zero-spread parity QC rebuilt the whole
template just to record 8 zero-spread draws. The facade now builds the template
**once** and caches it on the `Model` (a `OnceLock`), and **primes** it (one
throwaway realize warms the draw-invariant `LevelShift` SGS pattern caches), so
every subsequent `zoned_uncertainty` call — and every shard clone — reuses the
warm template instead of re-resolving + re-propagating. The template is a pure
function of the model's immutable build state (the SGS pattern is areal + log-
conditioned, independent of the per-draw contacts/level shifts/structural
perturbation), so a cached clone is **byte-identical** to a fresh build: results
on the canonical `results_ntg_run` fields are unchanged. On a 251×251 synth model
the second MC call drops ~2.0 s → ~0.6 s (~3×); the first call is unchanged
(priming only front-loads the SGS the first realize paid anyway).

### Fixed — stack builds condition anchorless (real seismic) scatter on the engine path
A multi-zone (**stack**) build from **irregular** scattered point-set horizons —
real seismic interpretations, where no sample lands exactly on a frame node —
used to fail with `grid error: scatter horizon '…' landed no point on the N×M
frame lattice`, because petekStatic's fast per-horizon **bilinear** minimum-
curvature conditioner was *singular* on scatter with **no hard anchors** (no
on-node sample to pin the affine null space). petekSim first shipped a facade-side
fallback for this; **petekTools 2f904bc** then fixed it at the root — the engine
conditioner now factors the anchorless system via a **null-mode ridge**, so
irregular world scatter grids directly on the fast engine path (factored once,
reused by build/QC/MC). The real canonical model no longer touches the fallback.

`Framework::build_grid_stack` **retains** its fallback to petekIO's node-
conditioned minimum-curvature for the one residual case the direct factor
legitimately cannot solve — genuine **under-constraint** (a horizon whose usable
control collapses below the bilinear minimum, `<4` controls) — snapping each
sample to its nearest node so even a sparse cloud still grids into a `Mapped`
field. Regression coverage in `srs-core` (`facade::framework::tests`, doctrine-R1
**world-georef** fixtures): the engine conditions anchorless scatter to finite
fields; it errors on `<4`-control under-constraint; the facade fallback grids that
under-constrained cloud.

### Changed — stack builds skip the second, facade-side horizon gridding
A multi-zone (**stack**) model built from scattered point-set horizons used to
grid every point-set horizon a **second** time, facade-side: `Framework::build`
eagerly ran petekIO/petekTools minimum-curvature over all horizons to assemble a
`Wireframe` + the surface tie report — even though a stack build **discards** the
facade wireframe (the engine builds its own framework from the conditioned raw
scatter) and reads its tie residuals from the engine `with_well_ties` provenance.
On real data (11 horizons, ~39k points each, a 122×116 lattice, serial) that
wasted gridding ran into the **tens of minutes**.

The facade wireframe + tie report are now **lazy**. `Framework::build` does only
the cheap work — resolving each horizon to its stack source (raw `Scatter`
points, a `Mapped` grid, or `TopsOnly` picks; no minimum-curvature solve) and
deriving the lattice geometry via the new `Project::horizon_geom` (bbox lattice,
no solve). The expensive facade gridding is deferred to
`Framework::materialize_wireframe`, triggered **only** by a wireframe consumer:
`tie_report`/`tie_ok`, or a single-`ZoneTable` `build_grid`. A **stack**
`build_grid` (`set_zonation`) never materializes it, so the second gridding is
skipped entirely. The `Framework` pyclass now holds the loaded `Project` (the
same handle `Wells`/`Surface` keep) to re-grid lazily; the stack `build_grid`
needs no project and still releases the GIL for the engine conditioning solve.
Consumer routing: the 3D viewer's well-hover tie residuals come from the facade
grid on the wireframe path (unchanged) and from the **engine** `with_well_ties`
provenance (`Model::well_tie_residuals`) on the stack path — the residuals the
Wells tab already shows, and measured against the surface the stack model
actually built (the old facade residual was sampled off a petekIO grid the stack
never uses). The **wireframe** path is byte-for-byte unchanged (same gridding,
same ties — only its timing moved to first use). Amplified synth
(`surfaces_as_points=True`, ncol=61, ~3.7k pts/horizon on a fine frame): stack
build+MC wall **16.5 s → 7.0 s**; the canonical real-model win is far larger and
lands with the coordinator's post-merge run (`task_suite_scatter_perf`).

### Changed — scatter-horizon conditioning deduplicated (condition once per model)
A multi-zone model built from **scattered point-set horizons** used to re-run
petekStatic's expensive per-horizon cold bilinear conditioning solve
(`condition_scatter` → petekTools `grid_min_curvature`) up to **three times** per
model lifecycle — once in the stack build (`grid.model`), once in the MC template
(`Model.stack_template`), and once in the QC scratch build
(`StaticGrid.scratch_grid`). petekSim now adopts petekStatic's
`StaticModelBuilder::condition_scatter_stack` dedup seam: `Framework.build_grid`
conditions the raw scatter **once**, stores the all-`Mapped` handle, and every
downstream path (build, QC scratch, MC template) reuses it via `from_horizon_stack`
with no re-conditioning. Byte-for-byte identical to the prior geometry
(petekStatic's proven `scatter_dedup_seam_is_bit_identical`); it only removes the
redundant solves. On the canonical scatter model this is the dominant cost, so the
build + zoned-MC wall drops materially (`task_suite_scatter_perf`; the kernel
direct-solve lands separately in petekTools and multiplies on top). Grid-surface
(`Mapped`) horizons are unaffected (conditioning was already a no-op there).
`peteksim.synth_asset(..., surfaces_as_points=True)` emits the horizons as
point-sets so the suite exercises this scatter path (the canonical real-model
shape); a new `scatter_perf`-marked test proves the build+QC+zoned-MC run conditions
each horizon exactly once via petekStatic's `SRS_PROFILE` hooks.

### Fixed — viewer volume tab renders (v3 block wire on every emission path)
The viewer **Volume** tab crashed in the browser (`TypeError: Cannot read
properties of undefined (reading 'positions')`) because peteksim serialized the
`VolumeBundle` through its serde derive (inline `positions`/`indices` arrays, no
`blocks`), while the petekTools decode kernel routes any `schema_version>=3`
volume to the binary-block decoder (`env.blocks`). Every peteksim volume-emission
path now emits petekStatic's **v3 self-contained wire envelope** (base64 blocks
via `VolumeBundle::write_self_contained`): the payload's `volume` (save_view /
serve), the live `/volume` re-cut endpoint (`_volume_json`), and the
dict-returning `model.volume_bundle()`. Self-contained base64 (not sidecar) —
the payload is one `model.json` with the volume inlined, no companion `model.bin`
is served. The Playwright browser-render acceptance leg (`acceptance_render`)
now passes end to end.

### Changed — cross-library single-home cleanups (P10 census)
- `M2_PER_KM2` is no longer redeclared in `srs-py`/`srs-core`; the km²→m² area
  scale is consumed from petekTools (`petektools::units::{M2_PER_KM2, km2_to_m2}`,
  re-exported through `srs-core`) — the units single home.
- The spill-forcing recipe (`peteksim.synth_asset.spill_recipe`) reads
  petekStatic's memory-budget formula **across the seam** via a new
  `peteksim._core.live_set_bytes(ni, nj, nk, n_cubes)` binding (over
  `srs_model::live_set_bytes`) instead of mirroring its `WARM_FACTOR`/`ELEM_BYTES`
  constants in Python — no more silent drift if petekStatic changes them.
- The facade property-name normaliser (`grid.model` property vocabulary) routes
  the raw name through petekio's shared mnemonic-alias table
  (`petekio::canonical_mnemonic`) before renaming the canonical `PHIE` mnemonic to
  the `PORO` cube — so it inherits the family alias set (`PHI`/`EFFPHI`/vintage
  tags) instead of a private exact-match lookup.

### Added — structural uncertainty wired onto the zoned MC (`ps.hz(sd=, vgm=)`)
petekStatic landed the structural-uncertainty engine
(`task_petekstatic_structural_uncertainty`), so `ps.hz(sd=, vgm=(model, range))`
is now **applied** through the zoned/stack Monte Carlo instead of raising
`NotYetSupported`. The `Horizons` rows map onto each realization draw
(`decision_structural_uncertainty_isochore`): the **top** row's `sd`/`vgm` becomes
a correlated top-surface **depth** field (`RealizationDraw::with_top_structural`);
each deeper row `k` becomes zone `k-1`'s **isochore** (thickness) field
(`ZoneDraw::with_isochore_structural`). The same field is stamped on every draw
(only `seed_index` varies); `vgm=(model, range)` builds a nugget-0 sill-1 variogram
(the field is rescaled to `sd_m`); `sd_m <= 0` is a no-op. Non-zoned (flat) models
still raise `NotYetSupported` — the flat `McInputs` surface has no structural hook
yet. New acceptance coverage: a top + zone-isochore field widens the total in-place
spread vs. the control, reproducible per seed and differing across seeds.

### Added — modelling API v2: declarative spec objects applied at explicit moments (`task_peteksim_api_v2`)
The primary model-build surface is now the **ratified modelling API v2**
(`petekSuite/dev-docs/designs/modeling-api-v2.md`): specs are declarative,
immutable values (they say WHAT / HOW and hold NAMES, resolved at apply time);
applications are explicit moments. The v2 layer is a thin Python facade
(`peteksim.specs` + `peteksim.apply`) over the unchanged Rust `_core` engine.

    geom  = proj.grid_geometry(cell=(dx, dy), orient=0)       # (extent advisory)
    grid  = geom.build(hz, sz, lay, collapse_negative=True)
    model = grid.model(props, con, fluid=, fvf=, wells=)
    mc    = model.zoned_uncertainty(ps.Mc(...))               # auto-routes on is_zoned

- **Structure specs** — `ps.Horizons`/`ps.hz` (ordered column top→down; `surface`
  defaults to the horizon name; a loaded point-set resolves to Scatter, a loaded
  grid to Mapped), `ps.Subzones`/`ps.splits`, `ps.Layering` (glob per-zone
  overrides), `ps.Contacts` (per-zone by glob), `ps.zone(name, color=)`.
- **Settings specs** — `ps.TieSettings`, `ps.Gridding` (wrapping the
  extrapolation policy `ps.decay_to_flat`/`ps.flat`/`ps.nearest` + `collapse` +
  `min_cell`), `ps.Run` (memory budget + workers), `ps.LoadSettings`,
  `ps.ViewSettings`.
- **Property + MC specs** — `ps.Prop`/`ps.Props` (per-cube upscale + SGS
  propagate + collocated trend BY NAME, applied at `grid.model(props=)`),
  `ps.Mc`/`ps.McSettings` (unify the `uncertainty`/`zoned_uncertainty` kwarg pair;
  auto-route on `model.is_zoned()`; per-zone overrides). `ps.AssetSpec` composes a
  whole scenario into one durable value; `ps.Crossplot`/`ps.Tornado`/
  `ps.Distribution` name chart data.
- **Value hardening (family-wide, testing-doctrine R7)** — every spec supports
  `to_dict`/`from_dict` with a `"spec"` type tag (a scenario is a savable file),
  value `==`/`hash`, `.replace()` derivation (a leading name/glob targets entries:
  `hz.replace("H1", surface=...)`, `lay.replace("Z*", dz=0.5)`), and a domain-table
  `repr`. Shipped with the **conformance battery** (`test_spec_conformance.py`) +
  a completeness check (a new spec cannot ship without a battery entry).
- **Acceptance API-v2 leg** (`test_acceptance_v2.py`, `-m acceptance`) — the
  canonical sketch runs verbatim on synth asset v2, recovers the planted truths
  through the new surface (per-zone net-conditioned PORO, the contact plan,
  zero-spread zoned MC == deterministic + conservation), pins the
  application-moment signatures, and asserts scenario derivation (two derived
  specs → two deterministic, differing models).
- **Back-compat (dual-accept, deprecation window: two minors)** — the v1
  eight-call chain (`proj.framework(...)` + the `_core` objects it returns) keeps
  working, emitting a `DeprecationWarning`. `ps.collocated(surface_or_name, ...)`
  accepts a **name** (v2, deferred) or a `_core.Surface` (v1, deprecated).
  An application given BOTH a spec and its legacy kwargs errors loudly
  (`Project.load(settings=..., crs=...)`, `zoned_uncertainty(mc, porosity=...)`).
  The box path (`run_box_model`) is frozen and accepts `ps.Dist`.
- **Forward-declared (recorded + serialized; engine effect lands elsewhere)** —
  `ps.hz(sd=, vgm=)` structural uncertainty is now applied on the zoned path (see
  the structural-uncertainty entry above); only the non-zoned (flat) case still
  raises `NotYetSupported`. `grid_geometry(orient≠0)` →
  `task_suite_grid_rotation`; `Gridding.fidelity_m`/`extrapolation` are honoured
  once the structure-investigation branch wires SolveOpts (a one-time advisory,
  never a silent no-op).

### Changed — license: adopt Business Source License 1.1 (`decision_license_ratified`)
- Before its first-ever publish, petekSim adopts the **Business Source License
  1.1** (was `license = "MIT"` in the workspace manifest, with no LICENSE file).
  The BSL text is added as [LICENSE](LICENSE) with `Licensor` / `Additional Use
  Grant` / `Change License = Apache-2.0` filled; the `{VERSION}` and Change Date
  parameters are release placeholders, filled at each release cut. The Cargo
  `[workspace.package]` `license = "MIT"` field is replaced by `license-file =
  "LICENSE"` (BSL has no SPDX expression), inherited by all four crates via
  `license-file.workspace = true`. Each released version converts to Apache-2.0
  four years after its first publication. Never published under MIT, so no
  legacy-version note is needed.

### Changed — raw-scatter facade switch (task_petekstatic_facade_engagement)
The facade half of the "fix at the root" ruling: stop pre-gridding point-set
horizons before the engine sees them. A point-set horizon now feeds the multi-zone
stack build as **`HorizonSource::Scatter`** (raw world points, `depth_m = -z`), and
the stack build/scratch/MC-template paths call petekStatic's
**`from_scatter_stack`** (`StaticModelBuilder` + `StaticModelTemplate`) so the
engine conditions the points onto the model lattice itself (petekTools bilinear,
genuine voids left `NaN`) before the single structural solve + `DecayToData` +
isochore build-down. The result: a data-void margin between two exactly-merged
horizons collapses to ~0 instead of carrying independently-extrapolated
min-curvature fill, and on-data fidelity tightens toward the lattice floor.
- **`Mapped` stays the escape hatch** for a genuinely pre-gridded *loaded grid*
  surface (`proj.geo().surface(name)`); the documented caveat that it bypasses the
  engine's solve/conditioning fidelity now applies. The single-`ZoneTable`
  **wireframe path is unchanged** — it still grids point sets onto the lattice (the
  gridding also backs the `tie_report()` surface residual).
- **`StackFrame` passthrough.** The facade keeps the `cell_size_m` lattice
  inference (a modelling choice) and hands the engine the top lattice as a
  `StackFrame` (`ni×nj` cells + the column-centroid world georef); the redundant
  post-build `with_georef` is dropped on the stack path (`from_scatter_stack`
  registers the frame).
- **Zone gross for Scatter.** The per-zone Proportional layer count now estimates a
  zone's gross thickness from each bounding horizon's mean depth when a bounding
  horizon is `Scatter` (previously only `Mapped×Mapped` on one lattice; else a
  nominal 20 m).
- **Truncation warning de-mislabelled.** `layers_truncated` no longer hard-codes
  "under Follow conformity" (any conformity truncates at a pinch-out / merged
  envelope); it now reads "collapsed to zero thickness (pinch-out / merged
  envelope)".
- **Tie residuals.** On the raw-scatter stack path, per-horizon tie residuals are
  the engine's (`set_well_ties` → `Provenance.well_ties`, surfaced by
  `model.well_tie_residuals()` — the pre-tie residual vs the *built* surface); the
  facade-side `tie_report()` residual remains the wireframe-path mechanism.
- Requires petekIO's additive `PointSet::coords()` reader (routed to petekIO).

### Changed — pyo3 boundary hardening (task_peteksim_boundary_perf)
The largest-impact boundary wave from the 2026-07-04 structure audit: release the
GIL across the viewer payload path, collapse the bundle→Python conversion, memoize
the zoned-MC getters, and push three logic leaks down into `srs-core`.
- **The viewer path now releases the GIL.** `map_bundle` / `volume_bundle` /
  `intersection_bundle` / `wells_bundle` / `view` / `save_view` / `_section_json`
  / `_volume_json` now assemble the mesh + bundle **and serialize** off the GIL
  (`py.detach`); only the final `Value → Python` walk (which fundamentally needs
  the GIL) holds it. Concurrency measured **≈2.7× throughput** on 4 threads calling
  `volume_bundle` on a 0.5 M-cell model (vs GIL-serial before).
- **`intersection_bundle` no longer double-/triple-materializes.** It built a JSON
  string, reparsed it to a `Value`, then re-serialized (`to_value`) and walked to
  Python — four passes. It now builds the bundle → one `Value` → one walk:
  **≈−76 %** wall time at 1 M cells (32 → 8 ms), ≈−83 % at 0.5 M (24 → 4 ms). The
  `Value → Python` walk (`json_to_py`) is retained as the fastest measured
  converter (a direct walk beat round-tripping through `json.loads` for the
  structured map bundle). Single-thread `volume`/`map` conversion is unchanged
  (same work) — the win there is the freed GIL, not latency.
- **`ZonedUncertainty.total` / `.zones` are memoized.** Each getter rebuilt the
  whole dict tree + `to_vec()` per access; they are now built once at construction
  and return a `clone_ref` — the same pattern `Uncertainty.stoiip`/`.giip` uses.
- **Three logic leaks pushed down to `srs-core` (thin shims remain in the
  binding):** the collocated-surface transform (`Project::collocated_trend`), the
  bore↔parent-well tie matching (`StaticGrid::well_ties_for`), and the km²→m² area
  rescale (`srs_core::scale_area_km2_to_m2`). `Surface.value_at` moves down too and
  gains a batched **`Surface.values_at(points)`** (`Project::surface_values_at`).

### Added — facade catch-up riders
- **`grid.model(sugar_cube=False)`** exposes petekStatic's `with_sugar_cube`:
  section cells render as flat boxes (edge depths collapse to the centroid) instead
  of dip-following trapezoids. Threaded onto the MC templates so realizations render
  identically. The acceptance suite's deferred assertion is now live
  (`test_sugar_cube_flattens_section_edges_to_centroid`).
- **`map_bundle` zone-average entries carry machine-readable `property` + `zone`
  keys** (split from petekStatic's `"<property>::<zone>"` layer name; the composite
  `name` is preserved) so a consumer keys on the zone directly.
- **`Property.qc()` is zone-aware.** For a zone-scoped pipe
  (`grid.property(name, zone=..)`) the digest now reports that zone's own
  conditioning and carries a `zone` key (`None` for a whole-model pipe). Previously
  a zone-scoped `qc()` read the whole-model pipe (or errored).

### Added — the end-to-end acceptance suite (doctrine R6, task_suite_acceptance_suite)
`crates/srs-py/tests/test_acceptance.py` is THE end-to-end acceptance suite — the
standing pre-stamp gate the coordinator runs before stamping a cross-repo feature
(testing-doctrine R6, `petekSuite/dev-docs/designs/testing-doctrine.md`). It runs
the whole chain on synth asset v2 (world georef, deviated wells, tops-only H3b,
per-zone contacts, Z5 pinch-out): generated tree → `Project.load` (zero non-noise
skips) → framework → `build_grid` → staged upscale/propagate (net-conditioned
PORO, NTG, a Z2 zone-scoped pipe, a collocated trend) → `set_zonation` +
`set_well_ties` → `grid.model` → zoned MC → every bundle kind → `save_view` +
served session.
- **Payload invariants asserted** (the escapes catalogue): section
  `layer_tops_l != layer_tops_r` on dipping cells for BOTH `Polyline` and
  `AlongBore` specs on the world-georef model (edge arrays null-aligned to the
  centroid where a layer is inactive); non-empty index-consistent volume shells;
  map `outline` extent == `frame` extent (world frame, within the half-cell
  centroid inset); `wells[].ties` populated for tied bores; `wells_logs` lanes
  byte-identical to petektools' reference `encode_lane`; `horizon_traces` present
  and the section NaN-gapped over the Z5 pinch-out (east columns truncate, west
  full).
- **Planted-truth recoveries:** rho (collocated trend), per-zone net-conditioned
  PORO means, the per-zone contact plan (single / two-contact / contactless HC
  pattern), zero-spread zoned MC == `in_place_by_zone` per zone, conservation
  (total == sum of zones), deviated tops at the trajectory (x, y) vs vertical tops
  at the wellhead.
- **Three legs (pytest markers):** `acceptance` (the fast per-wave gate, ~4 s at
  the default 21-node asset; run via `make acceptance-gate` or `pytest -m
  acceptance`); `acceptance_spill` (opt-in — forces the out-of-core path); and
  `acceptance_render` (opt-in — a headless-Chromium Playwright round-trip of the
  `save_view` export; skips cleanly without node/playwright/chromium). `make
  acceptance` rebuilds the wheel and runs all three.
- **Spilled leg status vs petekStatic base 9a846ae:** the spilled model's
  **volume** bundle renders and the loud out-of-core mode-switch advisory is
  asserted; the spilled **map** and **section** bundles are `xfail` (documented,
  not skipped) — a spilled model reports a 1×1 areal frame to the view producers
  ("view frame needs an areal lattice of at least 2×2 columns, got 1×1"); the fix
  is on an unmerged petekStatic branch (coordinator merge window).

### Added — `memory_budget_bytes=` on `grid.model()` (petekStatic `MemoryBudget` at the seam)
`StaticModel.model(..., memory_budget_bytes=<int>)` forwards a thin byte budget to
petekStatic's `StaticModelBuilder::with_memory_budget(MemoryBudget::bytes(..))` on
both the wireframe and multi-zone stack build paths. When the built model's
live-set estimate exceeds it the engine switches to its out-of-core (spilled)
backing store and emits a loud mode-switch advisory on stderr (never a silent
switch, never an OOM kill). `None` (default) keeps petekStatic's own default budget
(a fraction of physical RAM). This is the knob the R6 spill leg forces the
out-of-core path through.

### Synthetic asset v2 — structurally isomorphic to the canonical model (task_peteksim_synth_asset_2)
`peteksim.synth_asset` grows into the canonical-strength dataset the testing
doctrine (`petekSuite/dev-docs/designs/testing-doctrine.md`) derives from — the
asset every capability that must survive real data is proven on. **Additive: every
v1 call keeps working and every v1 manifest key is preserved; the manifest now
carries `asset_version == 2`.**
- **Mixed well program.** Some bores are now **deviated** — `build_hold` /
  `build_hold_drop` profiles via petekTools' `synth_trajectory_profile`
  (minimum-curvature stations, believable dogleg ≤ ~4 °/30 m). A deviated bore
  kicks off just above the reservoir and holds at a high angle *through* it, so it
  crosses several 100 m columns at reservoir depth; its logs and tops follow the
  **true trajectory (x, y)** at depth (world-frame round-trip), not the wellhead.
- **Tops-only internal split horizon** (`H3b`): well **picks only, NO mapped
  surface** in the emitted tree — the conformal-drape case.
- **Per-zone contacts** now span a two-contact zone (GOC+FWL), a single-contact
  zone (OWC) and CONTACTLESS zones (GRV only), planted in the `Type="Other"` tops
  rows + the manifest `contact_plan`.
- **Pinch-out zone.** One zone's isochore ramps to a sub-threshold band and then
  **exactly zero** across the eastern columns — genuine degenerate columns
  (R5 collapse / order-repair food).
- **World georef end to end** via petekTools' `Georef` idiom (one fictional
  431000/6521000 frame for every surface/trend/polygon/pick/trajectory).
- **Spill-forcing size recipe** (`spill_recipe()` + `manifest["spill_recipe"]`):
  the live-set estimate (mirroring petekStatic's budget formula) and a
  `force_budget_bytes` just below it, for the R6 acceptance suite's spilled cell.
- **Manifest v2** extends the planted truths: `asset_version`, `georef`,
  `well_program` (per-well profile + params + max DLS), `tops_only_horizon`,
  `contact_plan`, `pinch_out`, `spill_recipe` — alongside every v1 key.
- **In-repo acceptance** (`crates/srs-py/tests/test_synth_asset.py`): `Project.load`
  keeps zero non-noise skips; new tests assert the deviated/tops-only/contactless/
  pinch-out planted truths (deviated tops recovered at trajectory x,y, not
  wellhead), a collapse-ENABLED zoned build on the pinched asset terminates under a
  hard timeout (no livelock), and byte-determinism per seed holds across the new
  generators. The R4 loudness test moves to a deliberately sparse single-well
  fixture (v2's richer coverage no longer leaves a whole-field layer data-less).

### Facade catch-up to petekStatic canonical fixes (task_peteksim_facade_mz3)
Threads petekStatic's zoned-MC zone-cube fix + loud no-conditioning errors +
template ties through the `peteksim` facade.
- **`property.propagate(..., allow_mean_fill=False)`** — new kwarg (default
  `False`). A simulated layer that carries **no conditioning data** now errors
  loudly at build time naming the property (and, on the zone-scoped
  `grid.property(name, zone=..)` path, the zone) — petekStatic's `Gaussian`
  loud default. Pass `allow_mean_fill=True` to opt into the (structureless)
  constant mean-fill for such an expected data-less layer (any collocated trend is
  lost for those layers).
- **Zoned MC realizes zone-scoped pipes from their actual cubes.** `stack_template()`
  now threads each staged zone-scoped pipeline (`grid.property(name, zone=..)`)
  into `StaticModelTemplate::with_zone_property_mode`, so a zone piped through a
  zone-scoped upscale+SGS cube realizes from that cube (level shift on top of its
  per-zone priors), not from the zone priors alone. A zero-spread zoned MC now
  equals `in_place_by_zone` on every piped zone (`test_synth_asset`'s zone-pipe
  parity assertion, previously an xfail on `question_zoned_mc_zone_pipe_parity`).
- **Template well ties.** `stack_template()` threads `fw.set_well_ties(..)` into
  `StaticModelTemplate::with_well_ties` (draw-invariant, applied once at template
  construction), so every zoned-MC realization inherits the tied geometry.
- **Section bundle carries `sugar_cube` + dip-following trapezoid edge arrays**
  (per-column `layer_tops_l/r`, `layer_bases_l/r`) — passed through from
  petekStatic's `IntersectionBundle` v4-additive schema.

### Added — the Wells tab producer (task_peteksim_wells_payload)
The viewer's fourth tab (well correlation) fed from a loaded project + its
populated model. A `wells_logs` bundle (viewer `SCHEMA.md` § WellLogBundle,
`schema_version` 4) is assembled in Rust (`srs_core::build_well_log_bundle`) and
rides both `view()` (served) and `save_view` (self-contained).
- **`StaticModel.wells_bundle()`** — the `wells_logs` dict (or `None` when no
  attached bore carries a correlatable curve). Per bore: `md_m`/`tvd_m` lanes as
  v3-style f32 base64 blocks (`NaN`=`0x7FC00000`, byte-consistent with the
  petektools reference fixture + the petekio producer twin); the **raw** logs
  (`PHIE` with a net cutoff line + fill, `NTG`, `SW`, a `FACIES` net strip) **and**
  the **upscaled** cube curves (`PHIE_UP`/`NTG_UP`/`SW_UP`, one per populated cube,
  the blocky cell value sampled back along the bore); framework `tops[]` (top→down)
  + `zones[]` bands from the model; and tie residuals from `Provenance.well_ties`
  (`model.well_tie_residuals()`). Attaching bores via `grid.model(..., wells=
  proj.wells())` populates it automatically; the payload also carries a
  `flatten_default` pick (flatten-on-pick is viewer-side).
- **`Wells.heads()`** — `(id, x, y)` per positioned bore (wellhead world coord, no
  model build), e.g. to seed a `set_well_ties` table on each real bore.

### Added — multizone_2 through the facade (task_peteksim_multizone2_facade)
Exposes petekStatic's `multizone_2` wave (per-zone population + stack-aware MC +
per-horizon well ties) through the `peteksim` facade — the last gate before the
canonical framework build. All SI (Sm³/MSm³/bcm; metres; contact scalars
positive-down `*_depth_m`).
- **Zoned Monte Carlo — per-zone + total P-curves.** `model.zoned_uncertainty(
  porosity=, net_to_gross=, water_saturation=, contacts=ps.pick_spread(sd_m=),
  goc=, fvf=, gas_fvf=, zones={zone: {...}}, n=, seed=, workers=)` runs the
  stack-aware MC: each draw carries a per-zone `ZoneDraw` (per-zone contact draws +
  per-zone property level shifts) realized on a `from_horizon_stack` template, and
  the per-draw per-zone volumes roll up into **per-zone AND total** STOIIP/GIIP
  P-curves. Returns a `ZonedUncertainty` (`.total`, `.zones`,
  `.distribution_bundle(gas=, zone=)`). A contactless zone contributes GRV, zero
  hydrocarbon; conservation holds per draw (`total == Σ zone`). `workers>1` shards
  the realize loop across template clones (serial-identical means). peteksim owns
  this sampler layer — petekStatic's whole-model `run_structured_mc` rolls a single
  total, so the per-zone rollup is driven here over its `realize_into` +
  `in_place_by_zone` primitives.
- **Per-zone parameterization in the staged build.** `fw.set_zone_priors(zone,
  porosity=, net_to_gross=, water_saturation=)` (petekStatic's `with_zone_priors` —
  a sand vs shale level over the zone k-range) and `grid.property(name, zone=..)`
  (a zone-scoped pipeline → `with_zone_property`, simulated over the per-zone-priors
  baseline). Per-zone contacts continue to flow from `set_zonation`.
- **Per-horizon well ties.** `fw.set_well_ties([{ "id", "x", "y", "tops":
  {horizon: depth_m} }])` maps each well onto the top lattice and threads
  `with_well_ties` on the stack build; the engine re-solves each mapped horizon
  with the measured top as a hard control. `model.well_tie_residuals()` surfaces the
  `Provenance.well_ties` residuals (`well, horizon, measured_depth_m,
  model_depth_m, residual_m`); `wells[].ties` flows into the map-bundle payload.
- **Parity + coverage.** A cross-language parity vector (Rust unit test
  `zoned_mc_zero_spread_matches_deterministic` + Python
  `test_zoned_mc_parity_zero_spread`): the zoned MC at zero spread reproduces the
  deterministic `in_place_by_zone` per-zone volumes exactly. The synthetic-asset
  suite gains a ZONED end-to-end (`set_zonation` + per-zone contacts from the
  manifest → zoned MC → charts → `save_view`), an engine-well-tie test, and a
  per-zone priors/pipeline test. New pure-Rust compute paths run under
  `py.allow_threads` (the S1 convention); the `srs-core` seam adds a `zoned_mc`
  module (`rand`/`rayon`).

### Changed / Added — viewer payload polish (validation W6/W22 + coordinator items 13/14)
- **Tornado display names.** Each `TornadoBarOut` carries an optional
  `display_name` disambiguating the input dimension family: a property **level
  shift** (`"PORO level shift"`) vs a box/PVT **draw** (`"porosity (draw)"`), so the
  two never read as the same knob. (W6)
- **Well tie residuals in the viewer payload.** `WellTrack` gains `ties`
  (`[{horizon, residual_m}]`); `grid.model(wells=…)` attaches the framework's
  per-horizon surface-tie residuals to each bore (matched to its parent well id),
  and the viewer surfaces them on well hover + the layer panel. (W22)
- **dz-based default layering.** A framework with no explicit `set_layering`
  now defaults to ~1 m cells (`nk = ceil(mean_gross / 1.0)`, capped at 200 with a
  loud warning) instead of a fixed coarse count — the owner convention. Report:
  on the realshape model this hits the 200-cap (cold build ≈28 s, MC dominated by
  cell count), so callers with a large gross should pick a coarser `ps.layers(dz_m=)`.
- **Conformity styles.** `ps.layers(dz_m=1.0, style="proportional"|"follow_top"|
  "follow_base", n=)` maps to `Conformity::{Proportional, FollowTop, FollowBase}`;
  Follow styles are dz-driven (nk dz-derived at the seam, capped at `MAX_NK=200`,
  deep/shallow layers truncate at pinch-outs). `model.warnings()` now surfaces the
  additive `LayersTruncated` / `LayerCountCapped` advisories.

### Added — analytics charts (render-only mapping onto the viewer `charts` bundle)
- **MC results + logs → petekTools' generic `charts` payload.** peteksim maps its
  domain results onto the three viewer chart marks; strictly render-only (every
  number — tornado pivots, histogram bins, exceedance points, regression
  coefficients — is computed here in Rust and shipped in the payload; the viewer
  fits/bins nothing). Volumes report in **MSm³** (oil) / **bcm** (gas).
  - `mc.tornado_bundle(base=None, units="MSm³", fold_count=8)` — a **tornado**
    bundle from the ranked bars (inner P90→P10 band; `base` defaults to the STOIIP
    P50, pass the deterministic `summary()["stoiip_msm3"]` for the base-case anchor).
  - `mc.distribution_bundle(gas=False, name=None)` — a **distribution** bundle
    (histogram + exceedance CDF + P90/P50/P10 reservoir markers) from the kept
    realization vectors; binning is the deterministic 1/2/5·10ⁿ rule in the payload.
  - `ps.distribution_bundle([mc, …], aggregate=ps.aggregate([…]), names=[…])` — a
    **multi-series** overlay (per structure + the field aggregate) in one panel.
  - `proj.crossplot_bundle(x, y, wells=None, color_by="well", x_log, y_log,
    regression=False)` — a **scatter** (crossplot) bundle from positioned logs,
    `color_by` a well/zone identity or a continuous curve; the optional least-squares
    trend is fit in Rust (in the axes' log/linear space), only the line ships.
  - `model.view(charts=[…])` / `save_view(path, charts=[…])` attach the bundles to a
    model's Charts tab; `mc.view()` / `mc.save_view(path)` open a **pure-analytics**
    session (charts only, defaulting to tornado + the STOIIP distribution).
  - The top-level render-payload `schema_version` is **2** (the additive `charts`
    bundle); the per-bundle (volume/map/section) contract stays 1. `srs-core` gains
    a `charts` module (deterministic bins + least squares) + the crossplot seam over
    petekio per-bore logs.

### Changed — the viewer relocated to petekTools (peteksim now consumes it)
- **The bundle renderer is now petekTools' horizontal `petektools.viewer` unit**
  (owner ruling `decision_viewer_home_petektools`, 2026-07-04): it serves all
  layers (petekStatic, petekIO, peteksim), so it is horizontal capability, not a
  product feature. The viewer **JS assets moved out of the peteksim package data**
  into the petektools wheel; peteksim's `model.view()` / `save_view()` / the live
  `/section` fence endpoint are now **thin glue** over `petektools.viewer.serve` /
  `save_view` (a model-backed `section_provider` callback answers live fence/well
  requests — the domain half the renderer knows nothing about). No user-facing
  behaviour change; the render payload (composed in Rust from petekStatic's typed
  bundles) is unchanged and still `SCHEMA_VERSION=1`, mapped onto petektools'
  generic render schema.
- **Dependency:** the `peteksim` wheel now depends on the **`petektools` wheel**
  (Python-level, DAG-downward). Pre-publish (no PyPI yet), petektools is built from
  the sibling repo and dev-installed into the venv — see README "Install / build".

### Fixed — view-bundle world frame (UTM) wired through the facade
- **UTM map/section frame is now WORLD, not local.** petekStatic's engine fix
  (optional registered `Georef` on `StaticModel`) is wired here: `grid.model(...)`
  and the MC template call `.with_georef(...)` from the top-surface geometry
  (column-centroid convention: `origin = xori + 0.5·xinc`, `spacing = xinc`). So
  for real UTM data the map raster overlays the world outline/wells and a world
  fence / UTM along-bore trajectory now traces through the same `xy↔ij` as log
  registration — the previously-empty UTM bore section yields ordered columns.
  Box/flat/synthetic-square models (local-origin geometry) are unchanged.

### Added — the packaged viewer (map + intersection + volume)
- **Bundle-driven inspection viewer.** `model.view()` / `model.save_view(path)`
  open a tabbed SPA rendering petekStatic's typed view bundles: a **Map** (canvas
  2-D areal raster of any horizon-depth / property zone-average / k-slice layer,
  with outline, contact subcrop masks, well markers, pan/zoom, hover readout,
  draw-a-fence + click-a-well section tools), an **Intersection** (canvas 2-D
  cross-section — per-layer property fills, horizon + contact traces, bore-path
  overlay, vertical-exaggeration slider, hover readout), and a **Volume** (three.js
  corner-point mesh with per-cell colouring, threshold slider, zone toggles, i/j/k
  clip planes, orbit). Perceptually-uniform colormaps for continuous fields; a
  fixed categorical palette assigned per entity for identity; SI-labelled legends;
  a selectable light/dark theme. The viewer JS is domain-agnostic (renders whatever
  the JSON declares); zero external fetches (three.js vendored as a classic global,
  everything inlined). See `VIEWER.md`.
- **View-bundle plumbing** over petekStatic's `srs-model` exports:
  `model.map_bundle(property=…, k_slice=…)`, `model.intersection_bundle(line=[[x,y],…]
  | well="<id>", property=…)`, and `model.volume_bundle(property=…)` on the staged
  `StaticModel`, each returning a JSON-ready dict matching the `SCHEMA_VERSION=1`
  contract. `grid.model(..., wells=proj.wells())` attaches positioned bore tracks
  for well markers + along-bore sections.
- **Serve modes.** `model.view()` is now **non-blocking** — a background local
  server starts, prints its URL, returns it immediately, and answers a
  `GET /section?line=…|well=…` endpoint for live fence-draw / click-a-well
  (`block=True` restores the old blocking serve). `model.save_view(path)` writes
  **one self-contained HTML file** (all JS + payload inlined; opens via `file://`)
  — its capability delta vs live mode is documented in `VIEWER.md`.

### Changed
- **`save_json(path)` now writes the viewer payload** (`schema_version` + map +
  volume bundles + summary), not the old `{summary, mesh}` shape.

### Removed
- **Retired `srs-core/mesh.rs`.** The render-mesh builder moved **down** to
  petekStatic's `srs-model` (`VolumeBundle`); the box / refined / staged paths now
  export their mesh through that bundle. `build_mesh` / `model_mesh` /
  `deterministic_grid` / `Mesh` are gone from `srs-core`'s public surface.

### Known issues
- **World-frame (UTM) map/section misalignment (petekStatic seam).** For UTM source
  data, the bundle's map/section `frame` is the local grid-centroid lattice while
  the `outline`/contacts/wells stay in world coords, so the raster and outline don't
  overlay and a raw well trace yields zero section columns. Routed to petekStatic;
  the Volume tab and box/flat models are unaffected.

### Added — demo-gate wiring
- **`framework(..., min_thickness_m=<m>)` — post-gridding order-repair.** Threads
  petekStatic's `StaticModelBuilder::with_min_thickness_m` (and the MC template's
  equivalent) through the facade, so raw point-horizon builds with thin crossing
  margins (the real case — 100–300 crossing nodes per structure, where independent
  Top/Base gridding overshoots) build cleanly by pulling the base down to
  `top + min_thickness_m` at the offending columns (top preserved) instead of
  erroring. Left unset, a crossed base stays a loud build error (the crossing
  guard). The repair is surfaced — not swallowed — via **`model.warnings()`**, a
  new accessor returning the build advisories as dicts (a `thin_columns_repaired`
  entry carries the repaired-node `columns` count and the worst `worst_m`
  base−top separation; negative = a true crossing).
- **`inventory().wells` is now bore-level** — one entry per positioned bore under
  its bore-qualified id (matching `proj.wells()`), so a multi-sidetrack well
  advertises its A/B bores instead of the single parent well id.
- **`proj.tops.pick(name, wells=[...])`** — an optional bore/well filter on the
  pick aggregation. A listed well id (`"99/9-1"`) selects all its bores; a
  bore-qualified id selects exactly that bore; omitted aggregates across all wells.
- **`property(X).upscale(wells, net_only=True, net_cutoff=0.5)`** — net-masked
  upscaling. Filters the conditioning samples to net rock (the well's own `NTG`
  curve > `net_cutoff`, where present) before upscaling, so a conditioned cube
  reflects net rock (a conditioned SW ~ net-rock Sw) instead of the full log
  diluted by a non-net/aquifer interval. Facade-side sample filtering only.

### Fixed — real-data facade (final-validation F1–F7)
The facade's first real Petrel-export run surfaced seven blockers between the
synthetic smoke and a real-data, eight-call run. Each is fixed facade-side with a
synthesized regression at the real data's shape (split well dirs, UTM-magnitude
coordinates, deviated bores, `Type="Other"` contacts):
- **F5 — property upscaling conditioned 0 cells at real UTM coordinates** (the
  decisive downstream blocker: SGS / collocated trend / resimulate all
  unreachable). The petekStatic grid seam builds an area-scaled square at *local*
  origin (pillars at `ip·dx`, `dx = √area/ni`), so wells carrying real UTM `(x, y)`
  snapped off-grid. `grid.property(...).upscale(wells)` now **registers** each
  well's world `(x, y)` through the real horizon geometry onto the model's areal
  frame (GRV-neutral — the area scaling is untouched), and raises a loud "no well
  maps onto the model grid" error instead of an opaque downstream `propagate`
  failure when the coordinates are foreign.
- **F1 — `Project.load` invented bogus wells from the split Petrel layout.** It
  treated each immediate subdir of `Wells/` as one well, so the real split
  `Wells/Logs/` + `Wells/Paths/` tree produced two wells named *Logs*/*Paths* and
  skipped every comp-log. Wells are now discovered by **id from the survey/log
  filenames** and routed through petekio's directory-walking `load_well` (which
  keeps only that well's files across the split tree), so the split layout and
  the per-well-subdir layout both load cleanly.
- **F6 — `Project.load` discarded the deviated wellpath** (it fed petekio only
  the first LAS + a hand-parsed datum, synthesizing a vertical trajectory that
  mis-positioned deviated logs). It now passes the whole well tree to
  `load_well`, so a single `.wellpath` positions the logs on the **real
  trajectory** and its header supplies the authoritative wellhead datum (the
  facade-level datum parsing is gone).
- **F2 — scattered-point horizons never reached `framework()`.** Real seismic
  horizons arrive as `.IrapClassicPoints` (scattered `x y z`), which the facade
  neither classified nor gridded. They now classify as point-sets, and
  `framework()` grids a point-set horizon onto the build lattice with Briggs
  **minimum-curvature** (petekio's `PointSet::to_surface`) — the first horizon
  derives the lattice from its bounding box, later horizons grid onto it.
  `.EarthVisionGrid` horizons classify as point-sets too.
- **F3 — fluid-contact picks (`Type="Other"`) never surfaced.** GOC/FWL/OWC picks
  are `Type="Other"` in real Petrel exports, which petekio's stratigraphic tops
  loader drops (it keeps only `Type="Horizon"`). The facade now parses the
  `Type="Other"` rows itself (header-driven, quote-aware), so
  `proj.tops.pick("GOC")` / `pick("FWL")` return the contact — exactly what the
  two-contact `grid.model(...)` API consumes — and `inventory().tops` advertises
  them. Also classifies the **extension-less** Petrel tops export (a name
  carrying "tops", e.g. `FieldTops`) and `.petrel`, not just `.tops`, so the raw
  export layout no longer needs renaming.
- **F4 — `tie_report()` was wrong on deviated sidetracks.** It tied the horizon
  at the shared **wellhead**, so A/B/C/ST2 (one wellhead, picks at different
  deviated bottomholes) all reported the same surface depth and snapped onto one
  node (last-well-wins, ~530 m RMS mis-tie). It now ties at the **pick's own
  `(x, y)`** (the trajectory position at the pick MD). Relatedly, property
  conditioning now positions each well's log column at its **reservoir section**
  (mean sample `x, y`), not the wellhead — deviated-aware, identical for a
  vertical well.
- **F7 — `ps.collocated(surface, corr)` rejected a non-depth trend surface.** It
  always negated the surface (elevation→depth), so a depo-trend / net-sand grid
  in `[0, 1]` went negative and the seam rejected it ("trend multiplier must be
  non-negative"). `collocated` now reads the surface as a **trend in its own
  units** by default (`as_depth=False`) — so a net-sand trend works directly —
  with `as_depth=True` opting into the structural elevation→depth flip; the field
  is shifted non-negative when needed (steering-neutral, since the collocated
  secondary is standardized internally).

### API notes
- **`grid.property("PHIE")` now populates the canonical `PORO` cube** (aliased),
  instead of a silent non-canonical cube that left porosity at its prior. The
  volumetrics + MC read `PORO`/`NTG`/`SW`.
- **Added `ps.resimulate()`** — a marker for `property.propagate(...,
  resimulate=...)` (the resimulate MC mode); the symbol was previously missing
  (only `resimulate=True` worked).

### Tests / examples
- **`examples/synthetic_tree.build_real_shape_tree`** grows a synthetic tree in
  the *real Petrel export's shape* — split `Wells/Paths/` + `Wells/Logs/` dirs,
  UTM-magnitude coordinates, deviated single-bore wells, scattered
  `.IrapClassicPoints` horizons, `Type="Other"` contacts, a net-sand trend grid,
  vendor log mnemonics — and `crates/srs-py/tests/test_realshape.py` drives the
  full eight-call sequence over it as the F1–F7 acceptance regression (no
  confidential data).

### Added
- **The staged model-build facade (`peteksim`) — the product surface.** Eight
  staged calls take a Petrel export to a STOIIP P-curve + tornado, each a thin
  wrap over the owning layer (petekio ingest · petekStatic
  framework/property/model/MC · petekTools kernels), with no algorithm logic in
  the facade:
  - `Project.load(path, crs=, aliases=)` walks a Petrel-export tree (surfaces,
    polygons, wells + logs, tops), classifying + routing each file to the petekio
    loaders; `proj.inventory()` lists what loaded and what was
    skipped-with-reason (never silent). `proj.wells()`, `proj.surface(name)`,
    `proj.tops.pick(name)` (per-well picks + a representative level/spread).
  - `proj.framework(horizons=[...], outline=, tie_to_tops=)` grids each horizon
    and hard-ties it to same-named well-top picks; `fw.tie_report()` gives
    per-horizon-per-well residuals (loud on failure); `fw.set_zones` /
    `set_layering` / `build_grid`.
  - `grid.property(name).upscale(wells).qc()` / `.propagate(ps.gaussian(...),
    trend=ps.collocated(...))` — thin over `PropertyPipeline` (log upscaling +
    SGS + collocated cokriging); `ps.spherical/exponential/gaussian_vgm` +
    `ps.fit_variogram`.
  - `grid.model(contacts=dict(goc=, fwl=), fluid=, fvf=, gas_fvf=)` → a populated
    `StaticModel`; `model.summary()` (STOIIP/GIIP in Sm³/MSm³, GRV in mcm).
  - `model.uncertainty(porosity=, net_to_gross=, water_saturation=, contacts=,
    n=, seed=)` runs the structured MC (`run_structured_mc`); the result exposes
    `.stoiip`/`.giip` (p90/p50/p10/mean + samples), `.tornado()`, and
    `ps.aggregate([...], correlation=)`.
  - Distribution + spec builders: `ps.normal/lognormal/uniform/triangular/
    truncated_normal` (+ `.clamped()`), `ps.level_shift`, `ps.pick_spread`,
    `ps.layers`, `ps.collocated`.
  - **Documented deltas from the design contract:** zones map to a single
    `ZoneTable` layer allocation (no per-zone property independence yet — the P5
    zones task); a bare `ps.pick_spread` spreads the lower contact only (GOC held,
    to keep two-contact draws from crossing); fixed template inputs (area/gross)
    use a negligible-width sampler (the MC seam has no point-mass); a per-well
    `.wellpath` supplies the datum but logs load on the main bore (deviated
    multi-bore surveys are a follow-up). Property cubes named PORO/NTG/SW drive
    volumetrics.
  - Proof: `examples/staged_build.py` runs the exact eight-call sequence on a
    synthesized Petrel-export tree (`examples/synthetic_tree.py`), producing a
    P-curve + tornado in MSm³; per-facade-class Python tests in
    `crates/srs-py/tests/test_facade.py`. Supersedes `task_peteksim_py_ingest`.
- **Distribution forms (Python, W15).** Every `run_box_model` volumetric input
  now also accepts a **tagged dict** — `{"normal": [mean, sd]}`,
  `{"lognormal": [mu, sigma]}`, `{"uniform": [lo, hi]}`,
  `{"triangular": [lo, mode, hi]}` — routed to the matching srs-uncertainty
  distribution, alongside the existing number (constant) and `(min, mode, max)`
  triangular tuple. An unknown tag or wrong parameter count raises a clear
  `ValueError`.
- **Per-realization sample (Python, W17).** `ModelResult.samples` exposes the
  full Monte Carlo in-place vector (draw order, length `realizations`) behind the
  P90/P50/P10 summary, so compositional workflows (EUR bridging, custom
  percentiles) need not reimplement STOIIP. The summary fields are unchanged.
- Volumetrics compute stack: gross rock volume + in-place (OOIP/OGIP) from the
  grid, with a hard fluid contact (`srs-volumetrics`).
- Monte Carlo uncertainty: SplitMix64 RNG, inverse-CDF distribution sampling
  (constant/uniform/triangular/normal/lognormal), and P90/P50/P10 summaries
  (`srs-uncertainty`).
- Live-refine loop (`srs-core`): a `RefiningModel` that adds top-surface depth
  controls, re-converges a structured grid, and recomputes volumes.
- Python bindings (`srs-py`, PyO3/maturin): `run_box_model` end-to-end appraisal
  + the code-first three.js render bundle (`view()` / `save_json()`).

### Fixed
- **⚠️ The facade now converts surface z from petekio's negative-down elevation
  (the Z1 family, facade instance — a latent wrong-column hazard on real data).**
  petekio's documented convention delivers surface z as **negative-down subsea
  elevation** (matching `Trajectory::xyz` z = -tvd), but the staged facade copied
  surface values **verbatim** into the model's positive-down `depth_m` — while
  its well-pick path already converted correctly (`-z`). On a synthetic tree
  written with the same stale positive-down assumption the two errors cancelled;
  on a **real** Petrel export the facade would have built an upside-down
  framework (negative depths, wrong column — the Z1 class of bug). Now every
  facade surface-ingest point negates onto positive-down depth: `framework`
  gridding (`surface_to_gridded`), the tie report's raw-surface column, and
  `ps.collocated` (whose seam trend field is a non-negative multiplier — subsea
  depths satisfy it, raw negative elevation does not; `corr` reads against
  *depth*, unchanged). `examples/synthetic_tree.py` now writes its `.irap`
  surfaces in the real convention (negative-down), and the facade tests lock the
  conversion (tie-report sign guards). If you hand-authored surface files in
  positive-down depth for the facade, rewrite them as negative-down elevation —
  real Petrel exports need no change. Same family as petekStatic's srs-data Z1
  fix and the `simple.irap` integration-fixture correction.

### Changed
- **Performance + ergonomics (2026-07-04 review fix wave, C3).**
  - **The bindings release the GIL around pure-Rust compute** (`py.detach`, pyo3
    0.29): `run_box_model` (the Monte Carlo) and `Model.solve` (grid
    re-convergence), plus the staged facade's `build_grid`,
    `property.upscale`/`propagate`, `grid.model`, and `model.uncertainty`. A long
    run no longer blocks other Python threads — a concurrent thread keeps
    computing while Rust does (S1; regression-locked by
    `test_uncertainty_releases_the_gil`).
  - **`Uncertainty` no longer double-stores the Monte Carlo samples** and builds
    each `.stoiip`/`.giip` P-curve dict **once** (cached) instead of rebuilding it
    and re-cloning the sample vector on every getter access (L). Two behaviour
    notes: repeated `.stoiip`/`.giip` access now returns the *same* dict object
    (memoised); a single-contact model's `giip["samples"]` is now `[]` rather than
    a vector of `n` zeros (there is no gas leg).
  - **Internal (no user-visible change):** `SrsError` composes
    `petekio::GeoError` + `petektools::AlgoError` directly via `#[from]`, so `?`
    chains in one hop at every seam (retiring the `map_err(geo/algo)` helpers, S2);
    srs-py consumes petekTools' canonical `SM3_PER_MSM3`/`SM3_PER_BCM` report
    scales (re-exported through srs-core) instead of bare `1e6`/`1e9` literals (S3).
- **⚠️ BREAKING (Python): the SI/metric standard (petekSuite
  `decision_si_units_standard`) — the whole surface is metric now.** Bundled
  with the contact-required change below, heading toward the next minor:
  - `run_box_model` and `Model` take `area_km2` (km², reservoir-scale-natural;
    ×10⁶ to m² inside the binding), `gross_height_m`, `top_m`, `contact_m`
    (m, positive down). The old `area_acres` / `gross_height_ft` /
    `top_depth_ft` / `contact_depth_ft` parameters are gone.
  - Results are **Sm³**: `p90/p50/p10/mean/deterministic_in_place/samples`
    (oil and gas both). New reporting conveniences: `summary_msm3` (MSm³,
    oil scale) and `summary_bcm` (bcm, gas scale) percentile dicts.
  - `grv_acre_ft` → **`grv_mcm`** (mcm = 10⁶ m³) on `ModelResult`, `Refined`,
    and the `save_json` bundle; the packaged viewer labels are SI.
  - Area distributions are parameterised in km² and rescaled exactly
    (location-scale families ×10⁶; lognormal `mu + ln 10⁶`).
  - FVF is relabelled **Rm³/Sm³** — numerically identical to rb/STB (both are
    reservoir/surface volume ratios), so existing FVF *values* carry over
    unchanged (`srs-pvt` + the petekStatic types petekSim consumes).
  - **Parity proven by test** (`si_matches_old_imperial_case`): the old
    imperial reference case expressed in SI reproduces the identical answer —
    old-imperial == new-SI × exact factor to 1e-9 (0.4 km² × 50 m: OOIP
    2.24 MSm³ = 14 089 176 STB).
  - Imperial is opt-in conversion on the caller's side, never a default.
- **⚠️ BREAKING (Python, W16): `run_box_model` now REQUIRES a contact**
  (today `contact_m`). The old silent infinity default let a caller omit the
  fluid contact and receive a whole-box volume with no hydrocarbon column cut. A
  non-finite contact is now rejected up front with a clear `ValueError`
  (`"contact_m must be a finite depth in metres … it is required, not
  optional"`), matching `Model.__new__`'s existing guard. Pass an explicit finite
  depth, e.g. `contact_m=2743`.
- **Toolchain: pyo3 0.24 → 0.29 (CPython 3.14).** pyo3 0.24 could not link
  against CPython 3.14 (unresolved `_Py_*` symbols); 0.29 restores it while
  keeping the `abi3-py39` stable-ABI floor (wheels still install on Python 3.9+).
  Only API break in the binding: `Bound::downcast` → `Bound::cast`. CI now builds
  the wheel across a Python matrix (3.9 oldest-abi3 … latest stable) so toolchain
  rot is caught in CI, not by users.
- **2026-07-03 — the static lift (`decision_layer_charters`).** petekSim's
  identity is now **dynamic/engineering simulation + the Python product facade**:
  - `srs-volumetrics` and `srs-uncertainty` **relocated to petekStatic** (the
    static layer owns volumetrics + static uncertainty); petekSim consumes them
    as path deps. `srs-pvt` stays (FVF crosses the seam as a scalar input).
  - The `RefiningModel` structural/population half relocated into petekStatic's
    new `srs-model`; `srs-core`'s `RefiningModel` is now a thin facade that
    builds a `StaticModel` across the seam, reads the model's own in-place, and
    applies FVF. Public Rust/Python surfaces unchanged; `run_box_model` is
    bit-identical for the same seed.
  - **Fixed (via the relocated crates):** `realizations=0` now raises a clean
    `ValueError` (was a `PanicException` aborting the interpreter);
    non-physical inputs (φ/NG/Sw outside [0,1], non-positive area/height/FVF)
    are rejected with `ValueError` instead of silently producing negative or
    infinite in-place volumes.
  - Workspace is 4 local crates: `srs-units`, `srs-pvt`, `srs-core`, `srs-py`.
- **2026-07-01 — geomodel extraction.** The five geomodel crates (`srs-grid`,
  `srs-gridder`, `srs-wireframe`, `srs-petro`, `srs-data`) moved to the new
  **petekStatic** library (the GEOMODEL layer); petekSim consumes them as path
  deps across the repo seam. petekSim keeps `srs-units`, `srs-pvt`,
  `srs-volumetrics`, `srs-uncertainty`, `srs-core`, `srs-py`.
- Shared unit conversions moved to the horizontal **petekTools** toolkit;
  petekSim depends on `petektools::units` rather than a local units home.
