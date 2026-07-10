# petekSim — locked public API (`peteksim`)

> **This file is the contract.** The `peteksim` wheel must expose exactly these
> names and signatures (arguments, defaults, return shapes). Bodies are the
> implementer's; the *surface* is fixed. Changing a signature here requires
> sign-off (petekSuite coordinator + any downstream consumer for a cross-library seam) and an
> edit to this file — the code must never silently drift from it. See
> [SPEC.md](SPEC.md) for the design constitution.

**Rust is canonical; the Python surface mirrors it.** The compute lives in the Rust
core (`peteksim._core`, over the petekStatic/petekIO/petekTools crates); this
document specifies the **Python facade** — the product surface. Project loading is
owned by `petekio`, and static model construction is owned by `petekstatic`.

**Conventions:**
- **SI / metric everywhere** (`decision_si_units_standard`): areas km², lengths /
  depths **metres, positive-down**, volumes Sm³ (reported MSm³ oil / bcm gas), GRV
  mcm (10⁶ m³), FVF dimensionless Rm³/Sm³. Imperial is caller-side, never a default.
- **A spec holds NAMES, resolved at apply.** A spec value is declarative and
  project-independent: it references horizons / surfaces / picks / properties by
  name, and those names are resolved against a loaded project only at the apply
  moment (`geom.build`, `grid.model`, `model.zoned_uncertainty`). Resolution errors
  are loud and name **both** the missing project object and the spec entry.
- **Value semantics.** Every spec supports `to_dict()` / `from_dict()` /
  `ps.spec_from_dict()`, value `==` + `hash`, `.replace(...)` derivation, and a
  domain-table `repr`. A scenario is a savable, diffable file.
- **`import peteksim as ps`** throughout.

---

## Module

```python
ps.version() -> str                 # the peteksim/crate version string
```

**Exceptions** (both `from peteksim import ...`):

```python
ps.NotYetSupported(NotImplementedError)   # a spec field serializes but the engine
                                          # capability has not landed — raised loudly
                                          # at apply, naming the carrying task
ps.ApplyError(ValueError)                 # a spec could not resolve against the
                                          # project (missing name / illegal combo);
                                          # the message names object AND spec entry
```

## Project Ownership

petekSim does not expose `Project` or `LoadSettings`. Load project trees with
`petekio.Project.import_data(..., settings=petekio.ImportSettings(...))`, then build
static grids/properties/volumes through `petekstatic`. petekSim consumes completed
static/dynamic products and provides simulation/appraisal workflows.

## Structure specs

```python
ps.Horizons(*rows: HorizonRow, zones=None, ties=None, gridding=None) -> Horizons
    # The ordered stratigraphic column (top->down) + the zones between horizons;
    # zone i sits between rows[i] and rows[i+1]. .replace("H1"|glob, surface=...)
    # derives a changed column.
ps.hz(name: str, surface: str | None = None, tie: str | None = None,
      sd: float = 0.0, vgm: tuple[str, float] | None = None) -> HorizonRow
    # One horizon. `surface` defaults to `name` (a loaded point-set -> Scatter, a
    # loaded grid -> Mapped). `tie` names the pick set (defaults to `name`).
    # `sd`(m) + `vgm`=(model, range) declare the structural-uncertainty field
    # (applied through the zoned MC path).

ps.Subzones(mapping: dict[str, Split] | None = None) -> Subzones   # per-zone splits
ps.splits(*entries, conformity: str = "proportional") -> Split
    # each entry: a name, or (name, dict(surface=, tie=)).
    # conformity: "proportional" | "follow_top" | "follow_base".
ps.zone(name: str, color: str | None = None) -> ZoneColor          # a zone's colour

ps.Layering(dz: float | None = None, nk: int | None = None,
            min_cell: float | None = None) -> Layering
    # Layer allocation; dz XOR nk set the default. .replace("Z*", dz=0.5) adds a
    # per-glob override. min_cell(m) = sub-threshold cell-collapse floor.

ps.Contacts(mapping: dict[str, dict[str, float]] | None = None) -> Contacts
    # Per-zone fluid contacts by glob: {"Z4": dict(goc=.., fwl=..), "Z2": dict(owc=..)}.
    # A zone with no matching entry is contactless. .replace("Z4", fwl=...) derives.
```

## Settings specs (the HOW objects)

```python
ps.TieSettings(method: str = "convergent", radius_m: float | None = None) -> TieSettings
    # method: "convergent" (control-replacement) | "radius" (tie-locality; radius_m).

ps.Gridding(fidelity_m=None, extrapolation: Extrapolation | None = None,
            collapse: bool = True, min_cell: float | None = None) -> Gridding
ps.decay_to_flat(range_m: float) -> Extrapolation
ps.flat() -> Extrapolation
ps.nearest() -> Extrapolation

ps.Run(memory_budget: int | None = None, workers: int = 0) -> Run
    # Run resources. memory_budget (BYTES) forwards to the engine out-of-core
    # switch (loud spill, never an OOM kill); workers shards the MC realize loop.

ps.ViewSettings(property=None, open_browser: bool = True,
                port: int = 0, block: bool = False) -> ViewSettings
```

## Property specs

```python
ps.Props(*items: Prop) -> Props                         # the set applied at grid.model(props=)
ps.Prop(name: str, zone: str | None = None, upscale_method: str = "arithmetic",
        net_only: bool = False, net_cutoff: float = 0.5,
        propagate: Propagate | None = None) -> Prop
    # One cube's population: upscale (from wells) then propagate (SGS). `zone`
    # scopes the pipe to one zone of a stack.
ps.Propagate(variogram: Variogram = Variogram(), seed: int = 1,
             max_neighbours: int | None = None, radius_m: float | None = None,
             trend: CollocatedTrend | None = None, mode: str = "level_shift",
             allow_mean_fill: bool = False) -> Propagate
    # mode: "level_shift" | "resimulate".
ps.variogram(model: str, range_m: float, sill: float = 1.0,
             nugget: float = 0.0) -> Variogram
    # model: "spherical" | "exponential" | "gaussian".
ps.collocated(surface, corr: float, as_depth: bool = False)
    # surface a NAME (str) -> a CollocatedTrend spec resolved at apply (the v2 form).
    # surface a _core.Surface -> the v1 eager trend (DEPRECATED — bind by name).
```

## Monte-Carlo specs

```python
ps.Mc(porosity=None, net_to_gross=None, water_saturation=None,
      fvf=None, gas_fvf=None, contacts=None, goc=None,
      per_zone: dict[str, Mc] | None = None,
      settings: McSettings | None = None, n: int = 10_000, seed: int = 42) -> Mc
    # Property fields take a scalar sd / ps.shift(...) / ps.dist(...); contact
    # fields (contacts = lower FWL/OWC, goc) take a scalar sd_m / ps.pick_spread(...).
    # per_zone overrides by zone. Auto-routes to zoned_uncertainty on a zoned model,
    # else uncertainty.
ps.McSettings(lo_pct: float = 10.0, hi_pct: float = 90.0, workers: int = 0) -> McSettings
ps.shift(sd: float) -> Uncertain                        # a zero-mean level shift
ps.dist(kind: str, *params: float) -> Uncertain
    # kind: "level_shift"(sd) | "normal"(mean,sd) | "lognormal"(mean,sd) |
    #       "uniform"(lo,hi) | "triangular"(lo,mode,hi) |
    #       "truncated_normal"(mean,sd,lo,hi).

# Distribution builders (the _core samplers, for the v1 kwargs and Uncertain.resolve):
ps.normal(mean, sd);  ps.lognormal(mean, sd);  ps.uniform(lo, hi)
ps.triangular(lo, mode, hi);  ps.truncated_normal(mean, sd, lo, hi)
ps.level_shift(sd);  ps.pick_spread(sd_m)               # .clamped(lo, hi) on samplers
```

## Chart specs + the asset bundle

```python
ps.Tornado(base=None, units: str = "MSm³", fold_count: int = 8) -> Tornado
ps.Distribution(gas=False, zone=None, name=None) -> Distribution

ps.synth_asset(root, *, seed=20_260_704, n_wells=8, ncol=41,
               surfaces_as_points=False) -> dict
    # Compatibility shim over petektools.synth_asset. The returned manifest keeps
    # petekSim's `spill_recipe` key.
from peteksim.synth_asset import spill_recipe
spill_recipe(ncol=61, n_cubes=3, nk_per_zone=14) -> dict
    # petekSim-owned spill-forcing estimate; reads the petekStatic live-set seam.

ps.AssetSpec(name="", horizons=None, subzones=None, layering=None,
             contacts=None, ties=None, gridding=None, props=None, mc=None,
             run=None, view=None) -> AssetSpec
    # A whole modelling scenario as one durable value; every field is a spec, so
    # asset.to_dict() is a total, savable scenario file.
```

## The apply moments

```python
grid.model(props: Props | None = None, con: Contacts | dict | None = None, *,
           fluid: str = "oil", fvf: float = 1.25, gas_fvf: float | None = None,
           wells=None, run: Run | None = None, sugar_cube: bool = False) -> Model
    # Execute the build with the property + contact specs -> a populated Model.
    # Zoned (Horizons.zones present) -> contacts fold into the zonation; non-zoned
    # -> contacts go to the model. `run` carries memory_budget + workers.

model.zoned_uncertainty(mc: Mc | None = None, **legacy) -> ZonedUncertainty
model.uncertainty(mc: Mc | None = None, **legacy) -> Uncertainty
model.mc(spec: Mc)                       # auto-routes on model.is_zoned()
    # Pass EITHER a ps.Mc spec OR the legacy kwargs — not both. Legacy kwargs emit
    # a DeprecationWarning.
```

## The result surface — `Model`

```python
model.summary() -> dict                  # STOIIP / GIIP [MSm³], GRV [mcm]
model.in_place_by_zone() -> dict         # {"zones": [...], "total": {...}}
model.zone_stats(property: str) -> list  # per-zone count/mean/min/max
model.well_tie_residuals() -> list       # [{well, horizon, measured_depth_m,
                                         #   model_depth_m, residual_m}]
model.property_names() -> list[str]
model.is_zoned() -> bool
model.well_ids() -> list[str]
model.warnings() -> list

# viewer bundles + render:
model.map_bundle(property=None, k_slice=None) -> dict
model.intersection_bundle(line=None, well=None, property=None) -> dict
model.volume_bundle(property=None) -> dict
model.wells_bundle() -> dict
model.view(settings: ViewSettings | None = None, *, open_browser=True, port=0,
           block=False, property=None, lines=None, charts=None) -> str   # URL
model.save_view(path: str, property=None, lines=None, charts=None) -> None
```

## The result surface — uncertainty

```python
# flat (non-zoned) — model.uncertainty(...):
unc.stoiip -> dict                       # {p90, p50, p10, mean, *_msm3, samples}
unc.giip -> dict
unc.tornado() -> list                    # ranked input swings
unc.tornado_bundle(base=None, units="MSm³", fold_count=8) -> dict
unc.distribution_bundle(gas=False, name=None) -> dict
unc.view(open_browser=True, port=0, block=False, charts=None) -> str
unc.save_view(path, charts=None) -> None

# zoned — model.zoned_uncertainty(...):
zunc.total -> dict                       # {"stoiip": {..}, "giip": {..}, "two_contact": bool}
zunc.zones -> list                       # [{"zone", "stoiip", "giip", "two_contact"}, ...]
zunc.distribution_bundle(gas=False, zone=None, name=None) -> dict
```

## Spec value semantics (shared)

```python
spec.to_dict() -> dict                   # tagged with "spec"; JSON-able
Spec.from_dict(d) -> Spec                # per class
ps.spec_from_dict(d) -> Spec             # dispatch on the "spec" tag
ps.registered_specs() -> tuple[type, ...]
spec.replace(**changes) -> Spec          # collection specs also accept a leading
                                         # name/glob: hz.replace("H1", surface=...)
spec == other;  hash(spec);  repr(spec)  # value equality; domain-table repr
```

## The analytic box model

```python
ps.run_box_model(area_km2, gross_height_m, porosity, net_to_gross,
                 water_saturation, fvf, *, fluid="oil", top_m=0.0,
                 contact_m=math.inf, ni=10, nj=10, nk=5,
                 realizations=10_000, seed=1) -> ModelResult
    # Each volumetric input: a number (constant) | (min, mode, max) triangular |
    # {"normal"|"lognormal"|"uniform"|"triangular": [...]} tagged dict.
    # contact_m is REQUIRED and finite (a non-finite contact is a loud error).

m.samples -> list[float]                 # per-realization in-place [Sm³]
m.summary_msm3 -> dict                    # {p90, p50, p10, mean} in MSm³ (oil)
m.summary_bcm -> dict                     # the same in bcm (gas)
m.scaled_summary(per: float) -> dict
m.view(open_browser=True, port=0, block=False, property=None) -> str
m.save_view(path, property=None) -> None
m.save_json(path, property=None) -> None
repr(m)                                   # P90 / P50 / P10 / mean / deterministic [Sm³]

ps.Model(area_km2, gross_height_m, *, ni=20, nj=20, nk=8, top_m=1500.0,
         contact_m=math.inf, porosity=0.25, net_to_gross=0.8,
         water_saturation=0.3, fvf=1.25, fluid="oil")     # a structured box
sm.add_control(ip: int, jp: int, depth_m: float) -> None  # a structural high
sm.solve() -> Refined
sm.view(...) ; sm.save_view(...) ; sm.save_json(...)
```

## Aggregation + standalone charts

```python
ps.aggregate(segments, correlation: str = "independent") -> list[float]
    # correlation: "independent" | "comonotonic". Sum per-segment realization
    # vectors under an explicit dependence assumption.
ps.distribution_bundle(segments, aggregate=None, names=None, gas=False,
                       title=None) -> dict
```

## v1 (deprecated)

The v1 eight-call staged chain — `proj.framework(horizons=[...])` → `set_zones` /
`set_zonation` / `set_layering` / `set_well_ties` → `build_grid` → per-property
`grid.property(...).upscale(...).propagate(...)` → `grid.model(contacts=...)` →
`model.uncertainty(...)` / `model.zoned_uncertainty(...)` → `mc.tornado()` — remains
callable for a **two-minor** window and emits a `DeprecationWarning`. New code uses
the declarative v2 surface above. The runnable staged example is
`examples/staged_build.py`.
