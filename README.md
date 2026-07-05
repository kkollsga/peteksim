# petekSim

A fast field/discovery **appraisal toolkit** — a pure-Rust reservoir core with
thin Python bindings (`peteksim`). `peteksim` is the single Python-facing facade
over the whole subsurface-modelling stack (ingest → geomodel → volumetrics →
uncertainty): from a Petrel export to a STOIIP P-curve + tornado in a handful of
calls.

**Why:** a geoscientist should get from data to a defensible in-place P-curve
without wiring loaders, gridders, geostatistics and a Monte-Carlo loop together by
hand. petekSim presents the whole stack as declarative **specs** applied at a few
explicit moments — the compute lives in Rust; you describe *what* you want.

Everything is **SI/metric** (`decision_si_units_standard`): areas in km²,
lengths/depths in metres (positive-down), volumes in Sm³ (reported in MSm³ for oil,
bcm for gas), GRV in mcm (10⁶ m³), FVF as dimensionless Rm³/Sm³. Imperial is opt-in
conversion on your side, never a default.

## Documentation

The canonical docs for the whole petek family live on the **petekSuite site**
— peteksim's pages there:

- **[Library guide](https://peteksuite.readthedocs.io/en/latest/libraries/peteksim/)** — the peteksim guide.
- **Tutorials** — [Simulation & uncertainty](https://peteksuite.readthedocs.io/en/latest/tutorials/simulation-uncertainty/) · [Static model build (flagship)](https://peteksuite.readthedocs.io/en/latest/tutorials/static-model-build/) (driven through the `peteksim` facade).
- **[Notebooks](https://peteksuite.readthedocs.io/en/latest/notebooks/)** — executed examples: [full workflow](https://peteksuite.readthedocs.io/en/latest/notebooks/peteksim/01_full_workflow/) · [scenarios & uncertainty](https://peteksuite.readthedocs.io/en/latest/notebooks/peteksim/02_scenarios_uncertainty/).

## Install

```sh
pip install peteksim        # the whole stack behind one facade (Python 3.10+)
```

The wheel pulls its family dependencies (petektools) automatically. Rust
consumers: `cargo add peteksim`.

### Building from source (contributors)

```sh
python3 -m venv .venv-srs
.venv-srs/bin/pip install petektools    # the horizontal viewer/toolkit wheel
VIRTUAL_ENV="$PWD/.venv-srs" .venv-srs/bin/maturin develop -m crates/srs-py/Cargo.toml
```

## First volumes — a STOIIP P-curve in a handful of calls

The primary surface is the **declarative spec API**. A **spec** is an immutable
value that says WHAT (`Horizons`, `Subzones`, `Layering`, `Contacts`, `Props`,
`Mc`) or HOW (`TieSettings`, `Gridding`, `Run`); it holds **names**, not project
objects, resolved at apply time — so a spec is project-independent, reusable across
re-exports and synthetic assets, serializes to/from a dict (a scenario is a savable
file), compares by value, derives with `.replace()`, and pretty-prints as its
domain table. **Applications are explicit moments** (`geom.build`, `grid.model`,
`model.zoned_uncertainty`); errors at apply are loud, naming both the missing
project object and the spec entry.

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

mc.total["stoiip"]   # {p90, p50, p10, mean, *_msm3, samples} — the P-curve
mc.zones             # per-zone breakdown (a contactless zone contributes zero HC)
```

**Scenarios are derived specs** — same geometry, N specs → N models:

```python
deep    = con.replace("Reservoir", goc=2700.0, fwl=2780.0)
model_b = grid.model(props, deep, fluid="oil", fvf=1.30)
```

Every spec ships value semantics (`to_dict`/`from_dict`, `==`/`hash`, `.replace`,
table `repr`); a scenario round-trips through `ps.spec_from_dict(spec.to_dict())`,
and `ps.AssetSpec` bundles a whole scenario (load + structure + props + mc) into one
durable value.

### Multi-zone stacks

A multi-horizon stack (declare more `zones` between more `ps.hz` rows) unlocks
per-zone layering + contacts, optional per-zone property pipelines, and **per-zone
Monte Carlo** — a contactless zone contributes GRV with zero hydrocarbon; per-zone
and total P-curves are both reachable. `model.in_place_by_zone()`,
`model.zone_stats("PORO")` and `model.well_tie_residuals()` report the breakdown.

### Run resources + out-of-core

Pass a `ps.Run` to carry the run resources — `workers` shards the MC realize loop,
`memory_budget` (bytes) forwards to the engine's out-of-core switch (a
larger-than-memory model spills to disk with a loud notice, never an OOM kill):

```python
model = grid.model(props, con, run=ps.Run(memory_budget=8 * 1024**3, workers=4))
```

## The analytic box model — a quick estimate

Before a full project, a box model gives a first P-curve with Monte-Carlo on the
volumetric inputs (all SI: area km², depths m positive-down, FVF Rm³/Sm³):

```python
import peteksim

m = peteksim.run_box_model(
    area_km2=(0.32, 0.4, 0.52),           # (min, mode, max) triangular, or a constant
    gross_height_m={"normal": [15, 1.5]}, # tagged dict: normal / lognormal / uniform / triangular
    porosity=0.25, net_to_gross=0.8, water_saturation=0.3, fvf=1.25,
    fluid="oil", contact_m=2743,          # required — the base of the hydrocarbon column
)
print(m)                                  # P90 / P50 / P10 / mean / deterministic [Sm³]
print(m.summary_msm3)                     # the same percentiles in MSm³ (gas: summary_bcm)
print(len(m.samples))                     # the full per-realization in-place vector [Sm³]
m.view()                                  # opens the viewer (background server; returns at once)

# ...or a structured box with real relief, built in code (km², m):
sm = peteksim.Model(0.4, 15.0, ni=24, nj=24, nk=8, top_m=1500, contact_m=1510.5)
sm.add_control(12, 12, 1489)              # a structural high (depth in m)
sm.view()
```

Each volumetric input accepts a number (constant), a `(min, mode, max)` triangular,
or a tagged dict — `{"normal": [mean, sd]}`, `{"lognormal": [mu, sigma]}`,
`{"uniform": [lo, hi]}`, `{"triangular": [lo, mode, hi]}`.

## The viewer — Map · Intersection · Volume

`model.view()` opens a tabbed, bundle-driven inspection viewer in the browser:

- **Map** — areal rasters (horizon depth / property zone-average / k-slice) with
  outline, contact subcrop masks, well markers, pan/zoom + hover; draw a fence line
  or click a well to cut a section.
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

`./view.sh` builds the extension and opens the viewer in one step
(`./view.sh --box` for the Monte Carlo box model). See `examples/build_and_view.py`.

## Migrating from v1

Earlier versions used an eight-call staged chain (`proj.framework(...)` →
`set_zones` → `build_grid` → per-property `upscale`/`propagate` → `grid.model` →
`uncertainty` → `tornado`). It is **deprecated** (window: two minors) in favour of
the declarative API above — it keeps working and emits a `DeprecationWarning`.
Replace `proj.framework(horizons=[...])` with
`proj.grid_geometry(...).build(ps.Horizons(ps.hz(...), zones=[...]))`, and the
per-property `upscale`/`propagate` calls with a `ps.Props(ps.Prop(...))` spec passed
to `grid.model(props=...)`. The runnable staged example is `examples/staged_build.py`.

## Licensing

petekSim is licensed under the **Business Source License 1.1** — see
[LICENSE](LICENSE). Non-production use is freely granted; production use is
permitted by the Additional Use Grant except as a competing commercial
"as-a-service" offering of the Licensed Work's functionality. Each released version
converts to the **Change License (Apache-2.0)** four years after its first
publication. For alternative licensing, contact kkollsga@gmail.com.

## Contributing

Building petekSim itself — the crate workspace, the build/test gates, the acceptance
suite, and the planning-graph/inbox workflow — is documented in
[CONTRIBUTING.md](CONTRIBUTING.md). Design and architecture live in
[SPEC.md](SPEC.md); the locked public API is [API.md](API.md).
