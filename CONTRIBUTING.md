# Contributing to petekSim

petekSim is the **dynamic-simulation / product** layer of the petek
subsurface-modelling ecosystem — a pure-Rust core with thin PyO3 bindings, built
and published with maturin. This file is the map for working *on* petekSim; the
public design constitution is [SPEC.md](SPEC.md) and the locked public API is
[API.md](API.md). Build conventions live in `CLAUDE.md`.

## Layout

Workspace packages:

```
src/                 the single peteksim Rust crate (units, PVT, simulation/product core)
crates/srs-py        PyO3 bindings + thin view()/save_view glue over petektools.viewer
examples/            runnable model-building scripts + the synthetic-tree generator
```

The sibling libraries are independently released packages. Normal builds consume
the registry floors recorded in `Cargo.toml` / `crates/srs-py/pyproject.toml`; a
coordinated unreleased-seam check may temporarily patch them to sibling source
trees as documented below.

```
petekio       DATA layer — ingest, canonical wells/logs/surfaces, model-ready inputs
petekstatic   GEOMODEL layer — framework/grid/properties/volumetrics/static uncertainty
petektools    horizontal toolkit — numeric kernels, units and the packaged viewer
peteksim      this SIMULATION/product layer and Python-facing composition surface
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

`crates/srs-py/tests/test_acceptance.py` is the **round-trip acceptance suite**
(testing-doctrine R6). It imports the canonical tree through
`petekio.Project.import_data` for DATA planted-truth checks. For exact unreleased
viewer seams, a conditional test-only binding creates one coherent typed Rust
`StaticModel`; its map, volume, polyline + AlongBore sections, well ties, contacts,
zero-spread realization and save/serve payload all share one `inputs_ref` tied to
that imported project. The public Python `petekstatic.Grid.from_project` workflow
does not yet lower/export a typed Rust `StaticModel`, so this is explicit provenance,
not a fabricated claim of continuous object lowering. The suite never restores the
removed `peteksim.Project` facade. It asserts the payload invariants
(section `layer_tops_l != layer_tops_r` on dipping cells, non-empty volume shells,
`outline == frame` extent, `wells[].ties` populated, producer horizon traces
present, and collapsed pinch-out layers null-gapped) and the planted truths
recovered (rho, per-zone net PORO, contacts, zero-spread zoned MC ==
deterministic, conservation, deviated tops).

One documented entry point:

```sh
make acceptance        # released-floor wheel + fast gate; exact/browser legs skip if unavailable
make acceptance-gate   # just the fast gate (assumes a fresh wheel), ~4 s on the 21-node asset
# or directly:
.venv-srs/bin/python -m pytest crates/srs-py/tests/test_acceptance.py -m acceptance -q
```

Three legs (pytest markers): `acceptance` (the fast per-wave gate, target < ~5 min),
`acceptance_spill` (exact-source only: consumes map/section/volume from an actual
`MemoryBudget::bytes(1024)` spilled model and asserts its loud mode notice), and
`acceptance_render`
(opt-in — a headless-Chromium Playwright round-trip of the `save_view` export; skips
cleanly when node/playwright/chromium are absent).

This is the standing gate the petekSuite coordinator requires from the directly
spawned petekSim agent before stamping a cross-repo task.

### Unreleased viewer-seam acceptance

During a coordinated rolling upgrade, the sibling libraries can expose a schema
that is newer than petekSim's published dependency floors.  The opt-in
``viewer::tests`` module checks that exact source seam without restoring a
petekSim project-construction API or changing release floors:

```sh
PETEK_VIEW_SCHEMA_V6=1 cargo test -p srs-py viewer::tests \
  --features viewer-schema-acceptance \
  --config 'patch.crates-io.petektools.path="../petekTools"' \
  --config 'patch.crates-io.petekio.path="../petekIO"' \
  --config 'patch.crates-io.petekstatic.path="../petekStatic"'
```

Build the conditional Python bridge against those same sources, then run the
producer and spill cases through Python:

```sh
PETEK_VIEW_SCHEMA_V6=1 VIRTUAL_ENV="$PWD/.venv-srs" \
  .venv-srs/bin/maturin develop -m crates/srs-py/Cargo.toml \
  --features viewer-schema-acceptance \
  --config 'patch.crates-io.petektools.path="../petekTools"' \
  --config 'patch.crates-io.petekio.path="../petekIO"' \
  --config 'patch.crates-io.petekstatic.path="../petekStatic"'
.venv-srs/bin/python -m pytest crates/srs-py/tests/test_acceptance.py \
  -m "acceptance or acceptance_spill" -q
```

Resolve the lockfile to the sibling versions only for that local verification,
then restore it before committing.  The always-on
``test_viewer_schema_v6_delivery.py`` separately checks that save/serve glue
preserves additive frame fields and remains compatible with zero/pre-v6 payloads.

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

Technical designs, benchmark records, acceptance tooling, and internal study
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
