# The viewer — moved to petekTools

The bundle renderer is now petekTools' horizontal **`petektools.viewer`** unit
(owner ruling `decision_viewer_home_petektools`, 2026-07-04): it serves all layers
(petekStatic, petekIO, peteksim), so it lives in the toolkit, not the product. The
full viewer reference (tabs, modes, the generic render schema, design system, the
three.js re-vendoring notes) now lives with the unit:

- `../petekTools/VIEWER.md` — the viewer guide
- `../petekTools/python/petektools/viewer/SCHEMA.md` — the generic render schema

## peteksim's side (what stays here)

peteksim is a **consumer**. `model.view()` / `model.save_view(path)` still work
exactly as before — they compose the render payload in Rust from petekStatic's
typed view bundles (`map_bundle` / `intersection_bundle` / `volume_bundle`,
`SCHEMA_VERSION=1`), then hand it to `petektools.viewer`:

| call | what you get |
|---|---|
| `model.view()` | non-blocking background local server; returns the URL. Live fence-draw / click-a-well hit the `/section` endpoint, routed to `model._section_json`. |
| `model.view(block=True)` | blocking serve until `Ctrl-C`. |
| `model.save_view("model.html")` | one self-contained HTML file (`file://`). |

`view()` / `save_view()` exist on every model surface (`run_box_model(...)`
results, the structured `Model` / its `Refined` solve, and the staged-facade
`StaticModel`). `property=` picks the render cube; on the `StaticModel` path,
`lines=[[[x,y],…], …]` pre-computes extra fences and `wells=proj.wells()` attaches
bore tracks. For UTM data the map/section frame is the world (UTM) frame (the
`with_georef` seam), so the raster overlays the world outline/wells and a world
fence / bore trajectory sections correctly.

## Well correlation (the Wells tab)

On the staged-facade `StaticModel` path, attaching bores (`grid.model(...,
wells=proj.wells())`) also lights up the viewer's **Wells** tab: a `wells_logs`
bundle (viewer `SCHEMA.md` § WellLogBundle, `schema_version` 4) assembled in Rust
from the loaded project + the populated model. Per bore it carries `md_m`/`tvd_m`
lanes (v3 f32 base64 blocks, `NaN`=`0x7FC00000`), the **raw** logs (`PHIE` with a
net cutoff line + fill, `NTG`, `SW`, plus a `FACIES` net strip) **and** the
**upscaled** cube curves (`PHIE_UP`/`NTG_UP`/`SW_UP` — the blocky cell value
sampled back along the bore, one per populated cube) as separate tracks, the model
framework `tops[]` (top→down) + `zones[]` bands, and the per-horizon tie residuals
from `Provenance.well_ties` (`model.well_tie_residuals()`). The bundle rides both
`view()` (served) and `save_view` (self-contained); the flatten-on-pick transform
is viewer-side (the payload carries picks, not transforms).

| call | what you get |
|---|---|
| `model.wells_bundle()` | the `wells_logs` dict (per-bore lanes + raw/upscaled curves + tops/zones/ties), or `None` if no bore carries a correlatable curve — the exact payload the Wells tab renders. |
| `proj.wells().heads()` | `(id, x, y)` per positioned bore (wellhead world coord; no model build needed) — e.g. to seed a `fw.set_well_ties(...)` table on each real bore. |

The raw log names are canonical (petekio normalizes at load; `PHIE` = the effective
porosity cube's `PORO`), curve colour is identity **by track** so `PHIE` reads one
colour across every well, and the PHIE net cutoff defaults to the producer's
net-conditioning value.

## Analytics charts (the Charts tab)

peteksim maps MC results + positioned logs onto petekTools' generic **`charts`**
payload (viewer `SCHEMA.md` § ChartBundle). Strictly **render-only**: the tornado
pivots, the histogram bins, the exceedance points and the regression coefficients
are all computed **here** (deterministically, in Rust) and shipped in the payload;
the viewer fits and bins nothing. Volumes report in **MSm³** (oil) / **bcm** (gas)
via petekTools units.

| call | what you get |
|---|---|
| `mc.tornado_bundle(base=None, units="MSm³", fold_count=8)` | a **tornado** bundle from the ranked bars. `base` defaults to the STOIIP P50 (the all-P50 working point); pass `model.summary()["stoiip_msm3"]` for the deterministic anchor. Pivots are the P90/P10 percentiles, so the payload carries the **inner** band only (no min/max outer span). |
| `mc.distribution_bundle(gas=False, name=None)` | a **distribution** bundle (histogram + exceedance CDF + P90/P50/P10 in the reservoir convention) from the kept realization vectors. Binning is the deterministic 1/2/5·10ⁿ rule, in the payload. `gas=True` → the GIIP leg in `bcm`. |
| `proj.crossplot_bundle(x, y, wells=None, color_by="well", x_log=False, y_log=False, regression=False)` | a **scatter** (crossplot) bundle from positioned logs — samples paired on the x-curve's MDs. `color_by` is `"well"` / `"zone"` (categorical identity) or any log mnemonic (a continuous ramp). The optional least-squares trend is fit here (in the axes' log/linear space); only the line + coefficients ship. |

Attach the bundles to a model view, or open a pure-analytics session:

| call | what you get |
|---|---|
| `model.view(charts=[mc.tornado_bundle(), mc.distribution_bundle(), proj.crossplot_bundle(...)])` | the geometry tabs **plus** a Charts tab carrying the attached bundles. `save_view(path, charts=[...])` bakes them into the self-contained file. |
| `mc.view()` / `mc.save_view(path)` | a **pure-analytics** session — the Charts tab only (defaults to tornado + the STOIIP distribution), the geometry tabs show their empty state. Pass `charts=[...]` to choose the bundles. |

Overlay a **per-structure + field-aggregate** distribution with
`ps.distribution_bundle([mc_a, mc_b, …], aggregate=ps.aggregate([mc_a, mc_b, …]),
names=[…])` — one series per structure plus a `"Field"` series from the aggregate's
kept samples, in one histogram/CDF.
