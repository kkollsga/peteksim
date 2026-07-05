# petekSim — build conventions

petekSim follows the shared **petek house style** (canonical:
`petekSuite/dev-docs/petek-house-style.md`) — the conventions below are this
library's slice of it. petekSim is a peer library; the coordinator is petekSuite
(see `CLAUDE.local.md`).

**Identity (graph `decision_layer_charters`, 2026-07-03): dynamic/engineering
simulation + THE product.** The engineering core is recoverable/forecast work
(decline, p/z, Havlena-Odeh, Ramagost-Farshad; later full dynamic flow) plus
PVT (the `pvt` module); the product is the **`peteksim` wheel** — the single
Python-facing facade over the whole DAG-downward stack. Volumetrics + static
uncertainty (GRV/in-place, MC over static-model realizations, tornado) are
**petekStatic's** — this library consumes those results across the seam and
presents them; it does not re-own them.

## Mantra: SPLIT UP THE ELEPHANT 🐘

**Keep individual components simple as the project's complexity increases.**

Reservoir modeling is an extremely complex domain. We tame it by *decomposition*,
not by heroics — **component by component**. We **never reach god-file sizes** and
**never let complexity cluster**. When something gets big or tangled, the answer is
always the same: split it up. Maintainability and flexibility come first.

## Stack

- A **pure Rust** core with thin **PyO3** bindings, built and published via
  **maturin**. Everything is Rust, with Rust tooling.
- Toolchain: `cargo` workspace · `cargo test` · `cargo clippy` (warnings = errors)
  · `rustfmt` · `criterion` (benches) · `maturin` (wheels) · `cargo doc`.
- **Code-first visualization:** build a model in Python, call `model.view()` — the
  core exports a render bundle (mesh + summary) that a small three.js viewer
  (shipped inside the `peteksim` package) renders in the browser.

## Structure — one consolidated library crate + the py binding

petekSim is the **dynamic-simulation / product** layer of the petek suite.
Dependencies flow one direction, downward only — petekIO → petekStatic →
petekSim, with petekTools as the horizontal toolkit. The geomodel crates were
**extracted to petekStatic on 2026-07-01** (the static-uncertainty pieces
followed in the 2026-07-03 static lift), and the remaining local crates were
**consolidated into a single `peteksim` crate at the repo root** — the former
crates live on as modules.

**This workspace (root crate + 1 member):**

```
peteksim (root)   # the consolidated library crate
  src/units       #   the error type (SrsError; family errors compose in via #[from])
  src/pvt         #   PVT correlations and FVF handling (the dynamic/engineering core's)
  src/core        #   facade orchestration: the analytic box path (run_box_model),
                  #     the thin RefiningModel facade over petekstatic's model,
                  #     distribution_of (petekio DTO -> sampler), model.view(), charts
crates/srs-py     # PyO3/maturin bindings + the pure-Python `peteksim` package
                  #   (viewer glue over petektools.viewer, synth_asset)
```

**Consumed across the seam (published family deps, from crates.io):**

```
petekstatic       # GEOMODEL layer: structural framework + grid + property modelling +
                  #   volumetrics/static uncertainty; its StaticError composes into
                  #   SrsError via #[from] (reaches petekio::GeoError transitively)
petektools        # horizontal TOOLKIT: numeric kernels + units + the viewer unit
                  #   (also a runtime dep of the wheel: petektools>=0.2.1 on PyPI)
petekio           # DATA layer: names its neutral Distribution DTO in distribution_of
```

Version pins live in the root `Cargo.toml` `[workspace.dependencies]`. To develop
against a sibling checkout, patch locally in a gitignored `.cargo/config.toml`
(`[patch.crates-io]`) — no tracked path deps, no cycles.

## Component rules (how we split the elephant)

1. **One crate per component; one module per concept; one concept per file.** If a
   file owns more than one concept, split it.
2. **Hard ceiling mindset.** If a file grows past a few hundred lines or starts
   doing two jobs, split it — *before* it becomes a god file.
3. **Boundaries are traits.** Swappable, independently-testable interfaces;
   components depend on traits, not concretions.
4. **One-way dependencies, no cycles.** If you need a cycle, a boundary is wrong.
5. **Every component owns its tests.** Each crate is verifiable in isolation; the
   public API is the only surface other crates touch.

## Working style

- **Reproduce before fixing.** Confirm the exact root cause with evidence before
  changing code.
- **No bugs left behind.** A pre-existing bug you encounter gets fixed in the same
  change, or surfaced explicitly — never silently stepped over.

## Commits & releases

Commit format: `type: short description` (`feat`, `fix`, `docs`, `refactor`,
`test`, `chore`). Update `CHANGELOG.md` `[Unreleased]` for user-visible changes
(once we keep one). **Pushing requires explicit, in-the-moment approval.**
