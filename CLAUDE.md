# petekSim — build conventions

petekSim follows the shared **petek house style** (canonical:
`petekSuite/dev-docs/petek-house-style.md`) — the conventions below are this
library's slice of it. petekSim is a peer library; the coordinator is petekSuite
(see `CLAUDE.local.md`).

**Identity (graph `decision_layer_charters`, 2026-07-03): dynamic/engineering
simulation + THE product.** The engineering core is recoverable/forecast work
(decline, p/z, Havlena-Odeh, Ramagost-Farshad; later full dynamic flow) plus
PVT (`srs-pvt`); the product is the **`peteksim` wheel** — the single
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

## Structure — a Cargo workspace of small, single-responsibility crates

petekSim is the **dynamic-simulation / product** layer of the petek suite.
Dependencies flow one direction, downward only — petekIO → petekStatic →
petekSim, with petekTools as the horizontal toolkit. The geomodel crates were
**extracted to petekStatic on 2026-07-01**; **srs-volumetrics + srs-uncertainty
followed in the 2026-07-03 static lift** (with the `RefiningModel`
structural/population half, now petekStatic's `srs-model`); petekSim consumes
them all across the repo seam via path deps (no cycles).

**Local crates (this workspace, 4):**

```
srs-units         # the workspace error type (SrsError)
srs-pvt           # PVT correlations and FVF handling (the dynamic/engineering core's)
srs-core          # facade orchestration: the analytic box path (run_box_model),
                  #   the thin RefiningModel facade over petekStatic's srs-model,
                  #   distribution_of (petekio DTO -> sampler), model.view()
srs-py            # PyO3/maturin bindings + the three.js viewer (package data)
```

**Consumed across the seam (deps, not built here):**

```
../petekStatic/crates/*   # geomodel + static-uncertainty layer, path deps:
                          #   srs-grid · srs-gridder · srs-wireframe · srs-petro ·
                          #   srs-data · srs-volumetrics · srs-uncertainty ·
                          #   srs-model · petekstatic-error
                          #   (petekstatic-error composes into SrsError via #[from];
                          #    it reaches petekio::GeoError transitively)
../petekTools             # petektools, path dep: numeric kernels + units —
                          #   the horizontal toolkit, downstream DAG leaf
petekio = "0.2.1"         # published DATA layer: srs-core names its neutral
                          #   Distribution DTO in distribution_of
```

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
