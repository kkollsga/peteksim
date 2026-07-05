# petekSim

A fast field/discovery **appraisal toolkit** — a pure-Rust reservoir core with
thin Python bindings (`peteksim`). `peteksim` is the single Python-facing facade
over the whole subsurface-modelling stack (ingest → geomodel → volumetrics →
uncertainty): from a Petrel export to a STOIIP P-curve + tornado in a handful of
calls.

## Install / build

```sh
# build the Python extension into a venv
python3 -m venv .venv-srs
# the viewer is petekTools' horizontal petektools.viewer unit — peteksim depends
# on the petektools wheel. Pre-publish (no PyPI yet) build it from the sibling repo:
.venv-srs/bin/python -m pip install ../petekTools        # or: maturin build + pip install the wheel
VIRTUAL_ENV="$PWD/.venv-srs" .venv-srs/bin/maturin develop -m crates/srs-py/Cargo.toml
```

CI mirrors this: build the sibling `petektools` wheel first (`maturin build` in
`../petekTools`), `pip install` it, then build + test `peteksim`. Once both wheels
publish to PyPI the sibling-build step drops to a plain `pip install petektools`.

## The model build — declarative specs applied at explicit moments (API v2)

The **primary** model-build surface. A **spec** is a declarative, immutable value
that says WHAT (`Horizons`, `Subzones`, `Layering`, `Contacts`, `Props`, `Mc`) or
HOW (`TieSettings`, `Gridding`, `Run`); it holds **names**, not project objects,
resolved at apply time — so a spec is project-independent, reusable across
re-exports and synthetic assets, serializes to/from a dict (a scenario is a
savable file), compares by value, derives with `.replace()`, and pretty-prints as
its domain table. **Applications are explicit moments** (`geom.build`,
`grid.model`, `model.zoned_uncertainty`); errors at apply are loud, naming both
the missing project object and the spec entry.

```python
import peteksim as ps

proj = ps.Project.load("Data/", settings=ps.LoadSettings(crs="...", aliases={"PHIT": "PORO"}))

# Declarative structure + settings (names, not objects).
hz = ps.Horizons(
    ps.hz("TopReservoir", tie="TopReservoir"),
    ps.hz("BaseReservoir"),
    zones=["Reservoir"],
    ties=ps.TieSettings(method="convergent"),
    gridding=ps.Gridding(collapse=True),
)
lay   = ps.Layering(nk=8)
con   = ps.Contacts({"Reservoir": dict(goc=2700.0, fwl=2750.0)})
props = ps.Props(
    ps.Prop("PORO", net_only=True,
            propagate=ps.Propagate(variogram=ps.variogram("spherical", 800.0), seed=1)),
    ps.Prop("NTG",
            propagate=ps.Propagate(variogram=ps.variogram("spherical", 800.0), seed=2,
                                   trend=ps.collocated("TopReservoir", corr=0.4))),
)

# The explicit application moments.
geom  = proj.grid_geometry(cell=(50.0, 50.0), orient=0)
grid  = geom.build(hz, layering=lay, collapse_negative=True)
model = grid.model(props, con, fluid="oil", fvf=1.30, gas_fvf=0.005, wells=proj.wells())
mc    = model.zoned_uncertainty(ps.Mc(porosity=0.02, contacts=5.0, n=10_000, seed=42))

# Scenarios = derived specs: same geometry, N specs → N models.
deep = con.replace("Reservoir", goc=2700.0, fwl=2780.0)
model_b = grid.model(props, deep, fluid="oil", fvf=1.30)
```

Every spec ships the **conformance battery** (`to_dict`/`from_dict`, value
`==`/`hash`, `.replace`, table `repr`); a scenario round-trips through
`ps.spec_from_dict(spec.to_dict())`. `ps.AssetSpec` bundles a whole scenario into
one durable value. See `dev-docs/plans/api-v2-implementation.md`.

## The staged model build — the v1 eight-call chain (deprecated)

> **Deprecated** (window: two minors) in favour of the declarative API v2 above.
> The chain keeps working and emits a `DeprecationWarning`; new code should use
> `proj.grid_geometry(...).build(ps.Horizons(...))`.

Eight staged calls: **load → framework (tied to well tops) → grid → properties
→ model → summary → uncertainty → tornado.** All SI/metric (depths **m**
positive-down, results **Sm³**, reported in **MSm³**).

```python
import peteksim as ps

# 1. INGEST — walk a Petrel-export tree; petekio loaders do the per-file work.
proj = ps.Project.load("Data/", crs="...", aliases={"PHIT": "PORO"})
proj.inventory()                         # what loaded + what was skipped-with-reason

# 2. FRAMEWORK — declare horizons + outline; hard-tie to well-top picks.
fw = proj.framework(horizons=["TopReservoir", "BaseReservoir"],
                    outline="outline", tie_to_tops=True)
fw.tie_report()                          # per-horizon-per-well residuals (loud on failure)
fw.set_zones({"Reservoir": ("TopReservoir", "BaseReservoir")})
fw.set_layering({"Reservoir": ps.layers(n=8)})
grid = fw.build_grid()

# 3. PROPERTIES — one at a time: upscale (visible, QC-able) then propagate (SGS).
por = grid.property("PORO")
por.upscale(proj.wells(), method="arithmetic"); por.qc()
por.propagate(ps.gaussian(ps.spherical(range_m=800), seed=1))
ntg = grid.property("NTG"); ntg.upscale(proj.wells())
ntg.propagate(ps.gaussian(ps.spherical(range_m=800), seed=2),
              trend=ps.collocated(proj.surface("TopReservoir"), corr=0.4))

# 4. MODEL + SUMMARY — two contacts (gas cap + oil rim) from the tops picks.
model = grid.model(contacts=dict(goc=proj.tops.pick("GOC"),
                                 fwl=proj.tops.pick("FWL")),
                   fluid="oil", fvf=1.25, gas_fvf=0.005)
model.summary()                          # STOIIP / GIIP [MSm³], GRV [mcm]

# 5. UNCERTAINTY — level shifts on the modelled cubes + contact pick-spread.
mc = model.uncertainty(porosity=ps.level_shift(sd=0.02),
                       net_to_gross=ps.level_shift(sd=0.03),
                       water_saturation=ps.normal(0.30, 0.03).clamped(0.05, 0.6),
                       contacts=ps.pick_spread(sd_m=5), n=10_000, seed=42)
mc.stoiip                                # {p90, p50, p10, mean, *_msm3, samples}
mc.tornado()                             # ranked input swings
field = ps.aggregate([mc], correlation="independent")
```

Runnable end-to-end on synthesized data: **`examples/staged_build.py`** (with
its synthetic-tree generator `examples/synthetic_tree.py`).

### Multi-zone stacks — per-zone population, per-zone MC, well ties

A multi-horizon stack (`fw.set_zonation([...])`) unlocks the per-zone surface:
each zone gets its own layering + contacts, and optionally its own priors and
property pipeline. Monte Carlo is then **per-zone** — a contactless zone
contributes GRV with zero hydrocarbon; per-zone and total P-curves are both
reachable (all SI: Sm³/MSm³/bcm; contacts positive-down `*_depth_m`).

```python
fw.set_zonation([                                 # one entry per horizon gap, top→down
    dict(zone="Z1", below_horizon="H1", conformity="proportional", nk=6, contacts=None),
    dict(zone="Z2", below_horizon="H2", nk=8, contacts={"owc": 2530.0}),
    dict(zone="Z3", below_horizon="H3", nk=6, contacts={"goc": 2600.0, "fwl": 2660.0}),
])
fw.set_zone_priors("Z1", porosity=0.14, net_to_gross=0.4, water_saturation=0.5)  # a shale
fw.set_well_ties([{"id": "99/9-1", "x": 4.31e5, "y": 6.52e6,
                   "tops": {"H1": 2510.0, "H2": 2528.0}}])   # engine per-horizon ties
grid = fw.build_grid()
grid.property("PORO", zone="Z2").upscale(proj.wells())        # a zone-scoped pipeline
grid.property("PORO", zone="Z2").propagate(ps.gaussian(ps.spherical(range_m=1200), seed=3))
model = grid.model(fluid="oil", fvf=1.30, gas_fvf=0.005, wells=proj.wells())

model.in_place_by_zone()          # {"zones": [...per-zone volumes...], "total": {...}}
model.well_tie_residuals()        # [{well, horizon, measured_depth_m, model_depth_m, residual_m}]

mc = model.zoned_uncertainty(     # per-zone contact draws + per-zone level shifts
    contacts=ps.pick_spread(sd_m=4.0), goc=ps.pick_spread(sd_m=3.0),
    porosity=ps.level_shift(sd=0.01),
    zones={"Z2": {"contacts": ps.pick_spread(sd_m=6.0), "porosity": ps.level_shift(sd=0.02)}},
    n=10_000, seed=42, workers=4)
mc.total["stoiip"]                # {p90, p50, p10, mean, *_msm3, samples}
mc.zones                          # [{zone, stoiip:{...}, giip:{...}, two_contact}, ...]
mc.distribution_bundle(zone="Z2") # a chart bundle for model.view(charts=[...]) / save_view
```

## The analytic box model — a quick estimate

All inputs are **SI/metric** (`decision_si_units_standard`): area in **km²**,
lengths/depths in **m** (positive down), FVF in Rm³/Sm³. Results are **Sm³**
(with `summary_msm3` / `summary_bcm` reporting scales); GRV is in **mcm**
(10⁶ m³). Imperial is opt-in conversion on your side, never a default.

```python
import peteksim

# A box model with Monte Carlo uncertainty on area + height:
m = peteksim.run_box_model(
    area_km2=(0.32, 0.4, 0.52),         # km²: (min, mode, max) triangular, or a constant
    gross_height_m={"normal": [15, 1.5]}, # m: tagged dict — normal / lognormal / uniform / triangular
    porosity=0.25, net_to_gross=0.8, water_saturation=0.3, fvf=1.25,
    fluid="oil", contact_m=2743,        # m: required — the base of the hydrocarbon column
)
print(m)                                 # P90 / P50 / P10 / mean / deterministic [Sm³]
print(m.summary_msm3)                    # the same percentiles in MSm³ (gas: summary_bcm)
print(len(m.samples))                    # the full per-realization in-place vector [Sm³]
m.view()                                 # opens the viewer (background server; returns at once)

# Each volumetric input accepts:
#   • a number                     -> a deterministic constant
#   • (min, mode, max)             -> triangular (shorthand)
#   • {"normal":    [mean, sd]}    -> normal
#   • {"lognormal": [mu, sigma]}   -> lognormal (log-space parameters)
#   • {"uniform":   [lo, hi]}      -> uniform
#   • {"triangular":[lo, mode, hi]}-> triangular

# ...or a structured model with relief, built in code (km², m):
sm = peteksim.Model(0.4, 15.0, ni=24, nj=24, nk=8,
                      top_m=1500, contact_m=1510.5)
sm.add_control(12, 12, 1489)             # a structural high (depth in m)
sm.view()
```

`./view.sh` builds the extension and opens the viewer in one step
(`./view.sh --box` for the Monte Carlo model). See `examples/build_and_view.py`.

### The viewer — Map · Intersection · Volume

`model.view()` opens a tabbed, bundle-driven inspection viewer in the browser:

- **Map** — areal rasters (horizon depth / property zone-average / k-slice) with
  outline, contact subcrop masks, well markers, pan/zoom + hover; draw a fence
  line or click a well to cut a section.
- **Intersection** — the vertical cross-section (per-layer property fills, horizon
  + contact traces, bore-path overlay, vertical-exaggeration slider).
- **Volume** — the corner-point mesh (three.js): property colouring, threshold
  slider, zone toggles, i/j/k clip planes, orbit.

`view()` is **non-blocking** (a background local server prints its URL and returns;
`view(block=True)` for the old hold-until-Ctrl-C behaviour). `model.save_view("m.html")`
writes **one self-contained HTML file** that opens straight off `file://` — no
server, no network, all data + JS inlined (confidential-data safe). The bundle
accessors `model.map_bundle(...)` / `intersection_bundle(...)` / `volume_bundle(...)`
return the JSON dicts directly. Full guide: **`VIEWER.md`**.

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
across the repo seam via path deps (no cycles); the facade is a thin
orchestration over these seams:

```
../petekStatic/crates/*   geomodel + static-uncertainty — srs-grid · srs-gridder ·
                          srs-wireframe · srs-petro · srs-data · srs-volumetrics ·
                          srs-uncertainty · srs-model · petekstatic-error
../petekTools             petektools — gridding/kriging/SGS kernels + samplers + units
petekio                   DATA layer — readers, ModelInputs, the Distribution DTO
```

## Develop

```sh
make build    # cargo build the workspace
make test     # cargo test
make lint     # clippy (warnings = errors) + rustfmt check
make gate     # fmt + lint + build + test
make view     # build a model and open the viewer
```

### The acceptance gate — R6, the pre-stamp gate

`crates/srs-py/tests/test_acceptance.py` is the **end-to-end acceptance suite**
(testing-doctrine R6): the whole chain — generated tree → `Project.load` →
framework → `build_grid` → staged upscale/propagate → `set_zonation` +
`set_well_ties` → `grid.model` → zoned MC → every bundle kind → `save_view` +
served session — run on synth asset v2, with the payload invariants asserted
(section `layer_tops_l != layer_tops_r` on dipping cells in both spec kinds,
non-empty volume shells, `outline == frame` extent, `wells[].ties` populated,
`wells_logs` lanes byte-valid, `horizon_traces` present + the pinch-out
NaN-gapped) and the planted truths recovered (rho, per-zone net PORO, contacts,
zero-spread zoned MC == deterministic, conservation, deviated tops).

**This is the standing gate the coordinator runs before stamping a cross-repo
task.** One documented entry point:

```sh
make acceptance        # rebuild the wheel, then the fast gate + opt-in spill/render legs
make acceptance-gate   # just the fast gate (assumes a fresh wheel), ~4 s on the 21-node asset
# or directly:
.venv-srs/bin/python -m pytest crates/srs-py/tests/test_acceptance.py -m acceptance -q
```

Three legs (pytest markers): `acceptance` (the fast per-wave gate, target < ~5 min;
~4 s at the default size), `acceptance_spill` (opt-in — forces petekStatic's
`MemoryBudget` out-of-core via `grid.model(memory_budget_bytes=...)`), and
`acceptance_render` (opt-in — a headless-Chromium Playwright round-trip of the
`save_view` export; skips cleanly when node/playwright/chromium are absent).

Conventions in [CLAUDE.md](CLAUDE.md).

## Licensing

petekSim is licensed under the **Business Source License 1.1** — see
[LICENSE](LICENSE). Non-production use is freely granted; production use is
permitted by the Additional Use Grant except as a competing commercial
"as-a-service" offering of the Licensed Work's functionality. Each released
version converts to the **Change License (Apache-2.0)** four years after its
first publication. For alternative licensing, contact kkollsg@gmail.com. (The
`{VERSION}` / Change Date parameters are filled in at each release cut.)
