# petekSim — design constitution

> Repo: `Koding/Rust/petekSuite/petekSim` · crate family: `srs-*` · Python wheel:
> `peteksim`. The **SIMULATION + product** layer of the petek subsurface-modelling
> ecosystem. This is the design constitution (the *why* / *how*); the locked public
> contract is [API.md](API.md); the shared conventions are the petek family house
> style (`petekSuite/dev-docs/petek-house-style.md`). Build conventions are in
> `CLAUDE.md`; contributor workflow in [CONTRIBUTING.md](CONTRIBUTING.md).
> Cross-library seams + lifecycle live in the suite planning graph (`contract` MCP).

petekSim is two things at once (graph `decision_layer_charters`, 2026-07-03):

1. **The dynamic / engineering core** — recoverable / forecast work (decline, p/z,
   material balance: Havlena–Odeh, Ramagost–Farshad; later full dynamic flow) plus
   **PVT** (`srs-pvt`, formation-volume factors).
2. **THE product** — the `peteksim` wheel, the single Python-facing facade over the
   whole DAG-downward stack (ingest → geomodel → volumetrics → uncertainty). From a
   Petrel export to a STOIIP P-curve + tornado in a handful of calls.

**Litmus test for what belongs here:** a *dynamic/engineering* or *product-facade*
concern. Volumetrics + static uncertainty (GRV / in-place, MC over static-model
realizations, tornado) are **petekStatic's** — petekSim *consumes* those results
across the seam and presents them; it does not re-own them. FVF crosses the seam as
a validated scalar input; `srs-pvt` keeps its own copies for the dynamic work.

---

## 1. Place in the one-way DAG — consumes, never reaches up

Dependencies flow downward only; petekSim is the bottom (most-downstream) vertical
layer, with petekTools the horizontal toolkit:

```
petekIO      DATA       -> model-ready inputs (ModelInputs / .pproj)     [upstream dep]
   ↓
petekStatic  GEOMODEL   -> populated StaticModel + volumetrics/uncertainty [upstream dep]
   ↓
petekSim     SIMULATION -> dynamic/engineering + the peteksim product facade [THIS LIBRARY]

petekTools   TOOLKIT    -> numeric kernels + units + the viewer unit      [horizontal dep]
```

petekSim depends on petekStatic, petekIO, and petekTools; it **never** depends on
anything (it is the DAG bottom, no downstream consumer). No cycles, no sideways
code-sharing — **share conventions, convert small types at the seam**. The FVF value
types are the sanctioned duplicate-at-the-seam: `srs-pvt` keeps its own copies of
petekStatic's `srs-volumetrics` FVF types; no dependency crosses sideways. When in
doubt, duplicate a small type and convert at the boundary.

## 2. The crate workspace — small, single-responsibility crates

A Cargo workspace of four **local** crates; the geomodel + static-uncertainty crates
were extracted to petekStatic (2026-07-01) and the volumetrics/uncertainty half
followed (2026-07-03), consumed here across the repo seam via path deps.

```
srs-units    the workspace error type (SrsError). petekstatic-error composes in
             via #[from]; it reaches petekio::GeoError transitively, so `?` chains
             DATA -> GEOMODEL -> SIM and source() reaches the origin.
srs-pvt      PVT correlations + FVF handling — the dynamic/engineering core.
srs-core     the product facade: facade::{Project, Framework, StaticGrid, Model,
             uncertainty}; the analytic box path (run_box_model); the thin
             RefiningModel facade over petekStatic's srs-model; distribution_of
             (a petekio Distribution DTO -> a petekTools sampler); model.view().
srs-py       PyO3 / maturin bindings + the v2 spec facade (python/peteksim/**) +
             thin view()/save_view glue over petektools.viewer.
```

Consumed across the seam (path deps, not built here): `../petekStatic/crates/*`
(srs-grid · srs-gridder · srs-wireframe · srs-petro · srs-data · srs-volumetrics ·
srs-uncertainty · srs-model · petekstatic-error), `../petekTools` (petektools), and
the published `petekio` DATA crate.

## 3. Split the elephant 🐘

Reservoir modelling is an extremely complex domain; we tame it by **decomposition,
not heroics** — component by component. We never reach god-file sizes and never let
complexity cluster.

1. **One crate per component; one module per concept; one concept per file.** If a
   file owns more than one concept, split it.
2. **Hard-ceiling mindset.** Past a few hundred lines or a second job, split —
   *before* it becomes a god file.
3. **Boundaries are traits.** Swappable, independently-testable interfaces;
   components depend on traits, not concretions. Enums where the set is small and
   closed.
4. **One-way dependencies, no cycles.** If you need a cycle, a boundary is wrong.
5. **Every component owns its tests.** Each crate is verifiable in isolation; the
   public API is the only surface other crates touch.

## 4. The spec pattern — the product's design (api-consistency contract)

The product surface is a **declarative spec layer applied at explicit moments**, not
an imperative call chain. A **spec** is an immutable value:

- It says **WHAT** (`Horizons`, `Subzones`, `Layering`, `Contacts`, `Props`, `Mc`)
  or **HOW** (`TieSettings`, `Gridding`, `Run`, `LoadSettings`, `ViewSettings`).
- It **holds names, not project objects** — resolved against a loaded project only at
  the apply moment (`geom.build`, `grid.model`, `model.zoned_uncertainty`), so a
  spec is project-independent and reusable across re-exports and synthetic assets.
- It carries **no compute** — the compute stays in the Rust `_core` engine; the
  `peteksim.apply` driver resolves a spec's names and calls the engine.
- **Errors at apply are loud**, naming both the missing project object and the spec
  entry (`ApplyError`); a serializable-but-not-yet-honoured field raises
  `NotYetSupported`, never a silent no-op.
- It carries **value semantics** (the R7 conformance battery): dict round-trip with a
  `"spec"` type tag, value `==` + hash, `.replace(...)` derivation, and a
  domain-table `repr`. A scenario is a savable, diffable file; `AssetSpec` bundles a
  whole scenario into one durable value.

The v1 eight-call staged chain is the deprecated predecessor (a two-minor window,
`DeprecationWarning`) — kept working, but the spec layer is the primary surface.

## 5. Rust core + thin PyO3

All logic is Rust; the bindings **only marshal**. Public Rust signatures stay
binding-friendly (owned types, no public lifetimes, plain numerics / `ndarray`). The
Python API mirrors the Rust API. The v2 spec layer (`python/peteksim/specs/**`,
`apply.py`) is a thin Python facade — value objects + name resolution — over the
`_core` engine; it adds no compute of its own.

## 6. Conventions (the family house-style slice)

- **SI / metric everywhere** (`decision_si_units_standard`): coordinates / depths in
  **metres, positive-down**; areas km²; volumes Sm³ internally, reported **MSm³**
  (oil) / **bcm** (gas); GRV **mcm** (10⁶ m³); FVF dimensionless **Rm³/Sm³**
  (a relabel of the legacy rb/STB & rcf/scf, not a conversion). Imperial is opt-in
  conversion only, never a default.
- **One error enum** (`SrsError`, `thiserror`) + `Result<T>` everywhere; it composes
  petekStatic's `StaticError` via `#[from]` and reaches `GeoError` transitively.
- **`f64::NAN` = undefined**; arithmetic propagates NaN, stats skip it.
- **Domain objects carry their operations** — fluent, chainable, immutable (ops
  return new objects; mutation is explicit `set_*`).
- **Open/closed** — extend by adding readers/specs/artifacts, not by editing.
- **Compose deps, don't reinvent** — the gridding/kriging/warm-start/SGS kernels and
  samplers are petekTools'; the geomodel + volumetrics are petekStatic's; input-data
  work is petekIO's. petekSim orchestrates; it does not rebuild them.

## 7. Code-first visualization

The viewer is a **consumer** relationship, not owned code: the render bundle
(map / intersection / volume + charts) is petekStatic-computed and petekSim-composed
(in Rust, from petekStatic's typed bundles) onto petekTools' **generic render
schema**; the horizontal `petektools.viewer` unit renders it
(`decision_viewer_home_petektools`). `model.view()` / `save_view()` and the live
`/section` + `/volume` endpoints are thin glue — build a model in Python, call
`model.view()`, and a browser view opens; `save_view` writes one self-contained,
confidential-data-safe HTML file.

## 8. The planning graph is the cross-library source of truth

The suite planning graph (`petekSuite/research/graph`, `contract` MCP) holds the
inter-library contracts, decisions, and open questions. petekSim is a **participant,
not the coordinator** (that role is petekSuite). Reach for the graph on anything
cross-cutting; write results back with runtime types only (`Question` / `Decision` /
`Artifact` / `Task`), MERGE on id, provenance `modified_by='peteksim'`. No direct
graph access → route via the inbox to petekSuite. Full protocol: petek house style
§8 + `CONTRIBUTING.md`.
