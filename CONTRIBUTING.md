# Contributing to petekSim

petekSim is the **dynamic-simulation / product** layer of the petek
subsurface-modelling ecosystem — a pure-Rust core with thin PyO3 bindings, built
and published with maturin. This file is the map for working *on* petekSim; the
public design constitution is [SPEC.md](SPEC.md) and the locked public API is
[API.md](API.md). Build conventions live in `CLAUDE.md`.

## Layout

Local crates (this workspace):

```
crates/
  srs-units         the workspace error type (SrsError)
  srs-pvt           PVT / formation volume factors (the dynamic/engineering core)
  srs-core          the product facade (facade::{Project, Framework, StaticGrid,
                      Model, uncertainty}) + the analytic box path + model.view()
  srs-py            PyO3 bindings + thin view()/save_view glue over petektools.viewer
examples/           runnable model-building scripts + the synthetic-tree generator
```

The geomodel + static-uncertainty crates live in **petekStatic** (extracted
2026-07-01; volumetrics/uncertainty followed 2026-07-03). petekSim consumes them
across the repo seam via path deps (no cycles); the facade is a thin orchestration
over these seams:

```
../petekStatic/crates/*   geomodel + static-uncertainty — srs-grid · srs-gridder ·
                          srs-wireframe · srs-petro · srs-data · srs-volumetrics ·
                          srs-uncertainty · srs-model · petekstatic-error
../petekTools             petektools — gridding/kriging/SGS kernels + samplers + units
                          + the horizontal petektools.viewer unit
petekio                   DATA layer — readers, ModelInputs, the Distribution DTO
```

Dependencies flow one direction, downward only (petekIO → petekStatic → petekSim,
with petekTools as the horizontal toolkit). petekSim never reaches up.

## Develop

```sh
make build    # cargo build the workspace
make test     # cargo test
make lint     # clippy (warnings = errors) + rustfmt check
make gate     # fmt + lint + build + test
make view     # build a model and open the viewer
```

Python bindings build with maturin:

```sh
VIRTUAL_ENV="$PWD/.venv-srs" .venv-srs/bin/maturin develop -m crates/srs-py/Cargo.toml
.venv-srs/bin/python -m pytest crates/srs-py/tests
```

**Tooling discipline (don't relearn it the hard way):**
- **Never read a gate's status through a `tail`/`head` pipe** — a pipeline's exit
  code is the *last* command's. Run the gate bare, or `set -o pipefail`, or
  `cmd; echo "exit=$?"`.
- **After `maturin develop`, confirm it printed `Installed`.** A build error
  upstream leaves the old `.so` in place → the next `pytest` silently tests stale
  code.
- **After any Rust *behaviour* change, run `cargo test`, not just `pytest`** — the
  golden/engine assertions live on the Rust side.
- `python` is shell-aliased — always call `.venv-srs/bin/python` explicitly.

## The acceptance gate (R6)

`crates/srs-py/tests/test_acceptance.py` is the **end-to-end acceptance suite**
(testing-doctrine R6, the round-trip rule): the whole chain — generated tree →
`Project.load` → framework → `build_grid` → upscale/propagate → multi-zone stack +
well ties → `grid.model` → zoned MC → every bundle kind → `save_view` + served
session — run on the canonical synthetic asset, with the payload invariants asserted
(section `layer_tops_l != layer_tops_r` on dipping cells, non-empty volume shells,
`outline == frame` extent, `wells[].ties` populated, `horizon_traces` present +
pinch-out NaN-gapped) and the planted truths recovered (rho, per-zone net PORO,
contacts, zero-spread zoned MC == deterministic, conservation, deviated tops).

One documented entry point:

```sh
make acceptance        # rebuild the wheel, then the fast gate + opt-in spill/render legs
make acceptance-gate   # just the fast gate (assumes a fresh wheel), ~4 s on the 21-node asset
# or directly:
.venv-srs/bin/python -m pytest crates/srs-py/tests/test_acceptance.py -m acceptance -q
```

Three legs (pytest markers): `acceptance` (the fast per-wave gate, target < ~5 min;
~4 s at the default size), `acceptance_spill` (opt-in — forces the out-of-core path
via `grid.model(..., run=ps.Run(memory_budget=<bytes>))`), and `acceptance_render`
(opt-in — a headless-Chromium Playwright round-trip of the `save_view` export; skips
cleanly when node/playwright/chromium are absent).

This is the standing gate the petekSuite coordinator requires from the directly
spawned petekSim agent before stamping a cross-repo task.

## Testing doctrine

Committed tests and examples use **synthetic data only** — hand-authored to format
spec (`examples/synthetic_tree.py`, `peteksim.synth_asset(...)`). No real-dataset
contents (coordinates, values, well/field names) ever land in the repo, a fixture, a
commit message, or the CHANGELOG. Frame-sensitive tests (views, ties, sections, maps)
carry a world-georeferenced variant; every modelling capability carries a
planted-truth recovery test; every fallback branch is loud and its loudness is
asserted. The family-wide six rules are in
`petekSuite/dev-docs/designs/testing-doctrine.md`.

## Central coordination

petekSuite is the single operational control plane. It owns direct agent
management, the cross-library planning graph, actionable todos, GitHub Actions
operations, and releases. A petekSim task is scoped and supervised through the
central `run-library-task` skill; the spawned owning agent edits this repository,
runs the gates above, and reports evidence for the coordinator to record.

Technical designs, benchmark records, acceptance tooling, and clean-room study
material remain local because they describe or validate this library. Actionable
state lives under `petekSuite/dev-docs/libraries/petekSim/`. There is no local
skill tree, inbox, todo index, MCP configuration, Actions authority, or release
authority.

## Commits & releases

Commit format: `type: short description` (`feat`, `fix`, `docs`, `refactor`,
`test`, `chore`). Update `CHANGELOG.md` `[Unreleased]` for user-visible changes; skip
for internal refactors, CI, test-only, or formatting. **Pushing requires explicit,
in-the-moment approval.** Invoking petekSuite's central `release` skill grants
push/tag/publish authority only for that release run. The version source of truth
is the root `Cargo.toml` `[workspace.package] version`; all workspace members bump
in lockstep. Full conventions in `CLAUDE.md`.
