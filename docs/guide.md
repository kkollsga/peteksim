# The petekSim guide

`peteksim` is a fast field/discovery **appraisal toolkit**: a pure-Rust reservoir
core with thin Python bindings. It is the single Python-facing **facade** over the
whole subsurface-modelling stack — ingest → geomodel → volumetrics → uncertainty →
viewer — so that a handful of calls carry you from a Petrel-style export to a
per-zone STOIIP P-curve, an uncertainty spread, and an interactive 3-D view.

This guide is a task-oriented tour of the product surface. It assumes the wheel is
built and importable (`import peteksim as ps`). Two runnable notebooks accompany
it: `examples/notebooks/01_full_workflow.ipynb` (asset → model → MC → view) and
`examples/notebooks/02_scenarios_uncertainty.ipynb` (scenario derivation +
structural uncertainty). Every example here runs on a **fully synthetic** asset —
`ps.synth_asset(...)` fabricates a fictional field, so nothing confidential is
touched.

## The shape of the product

peteksim gives you three things behind one import:

1. **A declarative model-build surface (API v2)** — you describe *what* to model
   as immutable spec values, then *apply* them at explicit moments.
2. **Uncertainty on top of the model** — a zoned Monte Carlo that turns one model
   into a per-zone and field P-curve, plus a flat tornado path for input ranking.
3. **A code-first viewer** — `model.view()` / `model.save_view(path)` render the
   grid, sections, wells, and analytics charts in the browser or a single HTML
   file.

There is also a one-call **analytic box model** for a back-of-the-envelope
estimate before you have a grid. It is covered at the end.

## A synthetic asset to model against

Everything downstream needs a project on disk. `ps.synth_asset` writes one — a
fictional multi-zone field with deviated wells, per-zone contacts, and a pinch-out
— and hands back a manifest describing it:

```python
import tempfile
import petekio as pio
import peteksim as ps

man  = ps.synth_asset(tempfile.mkdtemp(), seed=20260704, n_wells=8)
project = pio.Project.load(
    man["root"],
    settings=pio.LoadSettings(crs=man["crs"], aliases=man["aliases"]),
)
project.inventory()   # what loaded + what was skipped-with-reason
```

The manifest carries the names you feed the specs (`horizons`, `zones`,
`zonation`, `contacts`, `well_ids`, `net_cutoff`, `crs`, `aliases`, …). On real
data you would point `petekio.Project.load` at your own export folder.

## The declarative model build (API v2)

The primary model-build surface. A **spec** is a declarative, immutable value that
says **WHAT** (`Horizons`, `Subzones`, `Layering`, `Contacts`, `Props`, `Mc`) or
**HOW** (`TieSettings`, `Gridding`, `Run`). Crucially a spec holds **names**, not
project objects — the names resolve at apply time. That makes a spec:

- **project-independent and reusable** — the same `Horizons` value applies to a
  re-export or a synthetic asset;
- **serializable** — `spec.to_dict()` / `ps.spec_from_dict(...)` round-trip, so a
  scenario is a savable file;
- **value-comparable** — `==` and `hash` are by value;
- **derivable** — `.replace(...)` returns a new spec with one field changed;
- **self-describing** — `repr(spec)` pretty-prints its domain table.

You declare the pieces once:

```python
hz = ps.Horizons(
    *[ps.hz(h) for h in man["horizons"]],       # one row per horizon, top→down
    zones=man["zones"],                          # zones = horizons - 1
    ties=ps.TieSettings(method="convergent"),    # HOW the framework ties
    gridding=ps.Gridding(collapse=True),
)
lay = ps.Layering(nk=2)                          # layers per zone
con = ps.Contacts({z["zone"]: dict(z["contacts"])
                   for z in man["zonation"] if z["contacts"]})
props = ps.Props(
    ps.Prop("PORO", net_only=True,
            propagate=ps.Propagate(variogram=ps.variogram("spherical", 1500.0), seed=11)),
    ps.Prop("NTG",
            propagate=ps.Propagate(variogram=ps.variogram("spherical", 1500.0), seed=12)),
)
```

Project-backed structure/property application is owned by `petekstatic`, starting
from `pst.Grid.from_project(project)`. petekSim no longer exposes the project
application facade.

### Reading the model

Once built, the model answers volumetric questions directly. All results are
SI/metric — depths in **m** (positive down), volumes in **Sm³** (reported in
**MSm³**), GRV in **mcm** (10⁶ m³):

```python
model.is_zoned()                 # True for a multi-zone stack
model.summary()                  # {stoiip_sm3, stoiip_msm3, giip_*, grv_mcm, two_contact, ...}
model.zone_stats("PORO")         # per-zone [{zone, count, mean, min, max}, ...]

byz = model.in_place_by_zone()   # {"zones": [...], "total": {...}}
for r in byz["zones"]:
    print(r["zone"], r["stoiip_msm3"], "two_contact:", r["two_contact"])
```

A contactless zone contributes GRV with zero hydrocarbon; a two-contact zone
(gas cap + oil rim) is flagged `two_contact=True`.

## Zoned Monte Carlo and P-curves

Uncertainty rides on top of the built model. One `Mc` spec — porosity level-shift,
contact pick-spread, GOC spread, a draw count and a seed — routes automatically to
the **zoned** run and produces per-zone and field P-curves:

```python
mc = model.zoned_uncertainty(ps.Mc(porosity=0.01, contacts=4.0, goc=3.0,
                                    n=2000, seed=42))
t = mc.total["stoiip"]           # {p90_msm3, p50_msm3, p10_msm3, mean, samples, ...}
print(t["p90_msm3"], t["p50_msm3"], t["p10_msm3"])
mc.zones                         # [{zone, stoiip:{...}, giip:{...}, two_contact}, ...]
```

The percentiles follow the reservoir convention (**P90 low, P10 high**). The full
`samples` vector is in the payload, so you can histogram it yourself or hand it to
a chart bundle: `ps.Distribution(zone="Z4").bundle(mc)`.

**Structural uncertainty** is opt-in through the horizon spec: `ps.hz(name,
sd=12.0, vgm=("spherical", 2500.0))` on the **top** row plants a correlated
top-depth field, and on deeper rows plants isochore fields. Applied on a **zoned**
model it perturbs every MC draw and widens the in-place spread; on a **non-zoned**
model it raises `ps.NotYetSupported` (loud, never a silent no-op).

**Tornado** is the **flat** path only. It lives on the non-zoned `Uncertainty`
(`model.uncertainty(...)` then `mc.tornado_bundle()`), which ranks each input's
swing. The zoned MC has no tornado; the two helpers `ps.aggregate(...)` and
`ps.distribution_bundle(...)` operate on flat `Uncertainty` segments (e.g. to
overlay several structures plus a field aggregate in one distribution).

## Scenarios — derive a spec, re-apply

A scenario is not a new API; it is a **derived spec** applied to the same
geometry. Because specs compare and derive by value, you get N models from N specs
deterministically:

```python
deeper  = con.replace("Z4", goc=man["contacts"]["goc_z4"],
                       fwl=man["contacts"]["fwl_z4"] + 30.0)   # deeper oil-water contact
model_b = grid.model(props, deeper, fluid="oil", fvf=1.30, gas_fvf=0.005)

hz_drop = hz.replace(rows=hz.rows[:-1], zones=hz.zones[:-1])   # drop the deepest zone
lay_hi  = lay.replace("Z*", dz=5.0)                             # refine layering
```

`ps.AssetSpec(...)` bundles simulation/scenario settings — horizons, layering,
contacts, props — into one durable value that round-trips through
`to_dict()` / `ps.spec_from_dict(...)`, so a full scenario is a file you can
version, diff, and re-run.

## The viewer

The bundle renderer is petekTools' horizontal `petektools.viewer` unit; peteksim
is a **consumer**. It composes a typed render payload in Rust from the model's view
bundles and hands it to the viewer. Two entry points:

| call | what you get |
|---|---|
| `model.view()` | a **non-blocking** background local server; prints its URL and returns at once. Live fence-draw / click-a-well hit the `/section` endpoint. `view(block=True)` holds until `Ctrl-C`. |
| `model.save_view("model.html", property="PORO", charts=[...])` | **one self-contained HTML file** that opens straight off `file://` — no server, all data + JS inlined (safe to hand around). |

The viewer is tabbed:

- **Map** — areal rasters (horizon depth, property zone-average, k-slice) with the
  outline, contact subcrop masks, well markers, pan/zoom/hover; draw a fence or
  click a well to cut a section.
- **Intersection** — the vertical cross-section: per-layer property fills, horizon
  and contact traces, bore-path overlay, vertical-exaggeration slider.
- **Volume** — the corner-point mesh in three.js: property colouring, a threshold
  slider, zone toggles, i/j/k clip planes, orbit.
- **Wells** — when bores are attached (`grid.model(..., wells=proj.wells())`), a
  correlation panel with `md`/`tvd` lanes, raw + upscaled log curves, framework
  tops/zones, and per-horizon tie residuals.
- **Charts** — MC results as render-only bundles: the tornado pivots, histogram
  bins, exceedance CDF, and regression coefficients are all computed **here**
  (deterministically, in Rust) and shipped in the payload; the viewer fits and
  bins nothing.

The raw bundle accessors (`model.map_bundle(...)`,
`model.intersection_bundle(...)`, `model.volume_bundle(...)`) return the JSON dicts
directly if you want to inspect or post-process them.

## The analytic box model — a quick estimate

Before you have a grid, `ps.run_box_model(...)` gives a one-call volumetric
estimate with Monte Carlo on the uncertain inputs. Each volumetric input accepts a
number (constant), an `(min, mode, max)` triangular shorthand, or a tagged dict
(`{"normal": [mean, sd]}`, `lognormal`, `uniform`, `triangular`). Inputs are
SI/metric — area in **km²**, depths in **m** positive-down, FVF in Rm³/Sm³; results
are **Sm³** with `summary_msm3` / `summary_bcm` reporting scales. It returns
P90/P50/P10, a deterministic value, and the full per-realization sample vector, and
carries the same `view()` / `save_view()` surface as a full model.

## Where to go next

- `examples/notebooks/01_full_workflow.ipynb` — the full asset-to-view walk.
- `examples/notebooks/02_scenarios_uncertainty.ipynb` — scenario derivation and
  structural uncertainty, with plotted P-curves.
- `examples/model_build_v2.py` — the same v2 workflow as a runnable script.
- `README.md` — install/build, the deprecated v1 staged chain, and the acceptance
  gate.
