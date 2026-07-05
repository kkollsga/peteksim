#!/usr/bin/env python3
"""Real-data-shape regression for the eight-call facade (final-validation F1–F7).

Drives the full sequence against a **synthetic** tree shaped like the real
Petrel export the validation could not run: split `Wells/Paths/` + `Wells/Logs/`
dirs, UTM-magnitude coordinates, deviated single-bore wells, scattered
`.IrapClassicPoints` horizons, `Type="Other"` fluid contacts, and a net-sand
trend grid. No confidential data — every file is hand-authored to format spec by
`examples/synthetic_tree.build_real_shape_tree`.

    VIRTUAL_ENV="$PWD/.venv-srs" .venv-srs/bin/maturin develop -m crates/srs-py/Cargo.toml
    .venv-srs/bin/python -m pytest crates/srs-py/tests/test_realshape.py -q
"""

from __future__ import annotations

import sys
import tempfile
from pathlib import Path

import pytest

REPO = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(REPO / "examples"))

import peteksim as ps  # noqa: E402
from synthetic_tree import (  # noqa: E402
    RS_FWL_M,
    RS_GOC_M,
    build_real_shape_tree,
)

ALIASES = {"PHIE_2025": "PORO", "NTG_PhieLam": "NTG"}


@pytest.fixture(scope="module")
def tree() -> Path:
    return build_real_shape_tree(tempfile.mkdtemp(prefix="realshape-"))


@pytest.fixture(scope="module")
def proj(tree):
    return ps.Project.load(str(tree), crs="ED50/UTM31N", aliases=ALIASES)


# --- F1: split Paths/Logs layout -> real wells, not bogus Logs/Paths ---------
def test_f1_split_dir_wells(proj):
    inv = proj.inventory()
    # No `Logs`/`Paths` dir-name wells from the split layout. Inventory is
    # BORE-level (matching proj.wells()): the multi-sidetrack well contributes its
    # A + B bores; the single-bore well its plain id.
    assert set(inv.wells) == {"99_9-1 A", "99_9-1 B", "99_8-1"}, inv.wells
    assert set(inv.wells) == set(proj.wells().ids()), inv.wells
    assert "Logs" not in inv.wells and "Paths" not in inv.wells
    assert inv.skipped == [], inv.skipped  # nothing silently dropped


# --- R-a: proj.wells() is BORE-level — one entry per positioned bore ----------
def test_ra_bore_level_wells(proj):
    # 99_9-1 is multi-sidetrack (bores A + B); 99_8-1 is single-bore (main). So
    # proj.wells() yields three positioned bores under bore-qualified ids.
    ids = set(proj.wells().ids())
    assert ids == {"99_9-1 A", "99_9-1 B", "99_8-1"}, ids
    assert len(proj.wells()) == 3


# --- F2: scattered-point horizons load + reach framework ---------------------
def test_f2_point_horizons_load_as_points(proj):
    inv = proj.inventory()
    # The horizons arrive as scattered point-sets (not grid surfaces). framework()
    # grids them onto the build lattice — exercised by the downstream `model`
    # fixture, which cannot build without gridded horizons.
    assert {"Top Reservoir", "Base Reservoir"} <= set(inv.points), inv.points
    assert set(inv.surfaces) == {"NetSandTrend"}, inv.surfaces


# --- F3: Type="Other" fluid contacts surface through pick() ------------------
def test_f3_other_contacts_pick(proj):
    inv = proj.inventory()
    assert {"GOC", "FWL"} <= set(inv.tops), inv.tops
    assert abs(proj.tops.pick("GOC").level_m - RS_GOC_M) < 1e-6
    assert abs(proj.tops.pick("FWL").level_m - RS_FWL_M) < 1e-6


# --- F8: LAS 3.0 core files leave an inventory merge-trace -------------------
def test_f8_core_files_traced(proj):
    inv = proj.inventory()
    merged = dict(inv.merged)  # {filename: bore_id}
    # The multi-sidetrack well's core file merged into bore A (a named bore); the
    # single-bore well's core file merged into its main bore (no bore suffix).
    assert merged.get("99_9-1_A_core.las") == "99_9-1 A", inv.merged
    assert merged.get("99_8-1_A_core.las") == "99_8-1", inv.merged
    # Core files are merged, never skipped — the merge no longer vanishes them.
    assert not any("core" in p.lower() for p, _ in inv.skipped), inv.skipped


# --- F4 + F6: tie at the deviated pick's own (x, y), not the shared head -----
def test_f4_f6_deviated_tie(proj):
    fw = proj.framework(
        horizons=["Top Reservoir", "Base Reservoir"], outline="outline", tie_to_tops=True
    )
    tie = fw.tie_report()
    # One row per (horizon, bore): 3 positioned bores x 2 horizons = 6 (R-a — the
    # multi-sidetrack well's A and B bores each tie independently).
    assert len(tie) == 6 and all(t["ok"] for t in tie), tie
    # Tied at the pick's own XY on a dipping surface -> surface ≈ pick (small
    # residual). Wellhead-tying would sample the surface at the head, off by tens
    # of metres — and would collapse A/B onto one node (the F4 last-well-wins bug).
    assert all(abs(t["residual_m"]) < 2.0 for t in tie), tie
    assert all(abs(t["surface_m"] - t["pick_m"]) < 2.0 for t in tie)
    # The two bores of ONE well (A: 18°, B: 32°) drift to different eastings, so
    # the east-dipping surface ties them at DIFFERENT depths — the decisive proof
    # that each bore positions its own pick (R-a), not the shared wellhead (F4).
    top = {t["well"]: t["surface_m"] for t in tie if t["horizon"] == "Top Reservoir"}
    assert abs(top["99_9-1 A"] - top["99_9-1 B"]) > 10.0, top


# --- F5 + PHIE alias: upscale conditions cells at real UTM coordinates -------
def test_f5_upscale_conditions_at_utm(proj):
    fw = proj.framework(
        horizons=["Top Reservoir", "Base Reservoir"], outline="outline", tie_to_tops=False
    )
    fw.set_layering(ps.layers(n=10))
    grid = fw.build_grid()
    # property("PHIE") aliases to the canonical PORO cube (API note).
    phie = grid.property("PHIE")
    n = phie.upscale(proj.wells())
    assert n > 0
    qc = phie.qc()
    assert qc["conditioned_cells"] > 0, "F5: 0 cells conditioned at UTM coords"
    assert qc["property"] == "PORO"  # PHIE -> PORO alias
    assert 0.15 < qc["upscaled_mean"] < 0.30


# --- F7 + full downstream: collocated net-sand trend, model, MC --------------
@pytest.fixture(scope="module")
def model(proj):
    """A conditioned two-contact model — built once (the point-set gridding +
    build is the expensive step) and shared read-only across the downstream tests."""
    fw = proj.framework(
        horizons=["Top Reservoir", "Base Reservoir"], outline="outline", tie_to_tops=False
    )
    fw.set_layering(ps.layers(n=10))
    grid = fw.build_grid()
    phie = grid.property("PHIE")
    phie.upscale(proj.wells())
    phie.propagate(ps.gaussian(ps.spherical(range_m=1200.0), seed=1))
    ntg = grid.property("NTG")
    ntg.upscale(proj.wells())
    # F7: a net-sand trend in [0,1] as a VALUE trend (as_depth=False default) —
    # previously rejected ("trend multiplier must be non-negative").
    ntg.propagate(
        ps.gaussian(ps.spherical(range_m=1200.0), seed=2),
        trend=ps.collocated(proj.surface("NetSandTrend"), corr=0.3),
    )
    return grid.model(
        contacts=dict(goc=proj.tops.pick("GOC"), fwl=proj.tops.pick("FWL")),
        fluid="oil",
        fvf=1.30,
        gas_fvf=0.005,
        # Attach the positioned bores so the viewer can draw well markers and cut
        # along-bore sections (intersection_bundle(well=..)).
        wells=proj.wells(),
    )


def test_f7_collocated_and_conditioned_model(model):
    # PHIE alias landed the canonical PORO cube (not a silent non-canonical one).
    assert "PORO" in model.property_names()
    s = model.summary()
    assert s["two_contact"] is True
    assert s["stoiip_sm3"] > 0.0 and s["grv_mcm"] > 0.0


def test_downstream_uncertainty(model):
    mc = model.uncertainty(
        porosity=ps.level_shift(0.02),
        net_to_gross=ps.level_shift(0.03),
        contacts=ps.pick_spread(5.0),
        n=400,
        seed=42,
    )
    st = mc.stoiip
    assert st["p90"] < st["p50"] < st["p10"] and st["p90"] > 0.0


# --- tops.pick(name, wells=[...]) — optional bore/well filter ----------------
def test_tops_pick_wells_filter(proj):
    all_picks = proj.tops.pick("Top Reservoir")
    assert len(all_picks.picks) == 3, all_picks.picks  # A, B, 99_8-1

    # A listed WELL id selects all its bores.
    one_well = proj.tops.pick("Top Reservoir", wells=["99_9-1"])
    assert {w for w, _ in one_well.picks} == {"99_9-1 A", "99_9-1 B"}, one_well.picks

    # A bore-qualified id selects exactly that bore.
    bore = proj.tops.pick("Top Reservoir", wells=["99_9-1 A"])
    assert len(bore.picks) == 1 and bore.picks[0][0] == "99_9-1 A", bore.picks

    single = proj.tops.pick("Top Reservoir", wells=["99_8-1"])
    assert len(single.picks) == 1 and single.picks[0][0] == "99_8-1", single.picks

    # The two bores tie the east-dipping surface at different depths, so the
    # filtered aggregation's level differs from another well's — proof the filter
    # actually restricts the aggregation, not just the returned rows.
    assert abs(one_well.level_m - single.level_m) > 1.0

    # An empty filter is the same as no filter; a non-matching filter raises.
    assert proj.tops.pick("Top Reservoir", wells=[]).picks == all_picks.picks
    with pytest.raises(ValueError):
        proj.tops.pick("Top Reservoir", wells=["nope"])


# --- min_thickness_m: thin-margin crossing point-horizons build cleanly -------
def test_min_thickness_repairs_thin_crossing(proj):
    # The real point-horizon case: a thin-margin crossing pair (`Top Thin` /
    # `Base Thin`) where the base crosses ABOVE the top over most of the extent.
    # DEFAULT (no min_thickness_m) errors under the crossing guard, loudly.
    fw = proj.framework(
        horizons=["Top Thin", "Base Thin"], outline="outline", tie_to_tops=False
    )
    grid = fw.build_grid()
    with pytest.raises(ValueError, match="cross"):
        grid.model(contacts=dict(fwl=2100.0), fluid="oil", fvf=1.30)

    # min_thickness_m=2.0: the SAME build now succeeds (base pulled to top+2 m at
    # the crossing columns) and surfaces the repair as a loud, non-swallowed
    # warning on the Python result.
    fw2 = proj.framework(
        horizons=["Top Thin", "Base Thin"],
        outline="outline",
        tie_to_tops=False,
        min_thickness_m=2.0,
    )
    model = fw2.build_grid().model(contacts=dict(fwl=2100.0), fluid="oil", fvf=1.30)
    thin = [w for w in model.warnings() if w["kind"] == "thin_columns_repaired"]
    assert thin, model.warnings()
    assert thin[0]["columns"] > 0, thin  # the 100-300 crossing nodes repaired
    assert thin[0]["worst_m"] < 0.0, thin  # negative separation = a true crossing


# --- viewer: the real-shape model exports bundles + a self-contained save_view -
def test_view_bundles_from_real_shape(model):
    import re
    import tempfile

    # VOLUME bundle (dict path): petekStatic's shell mesh as the v3 self-contained
    # block envelope (base64 blocks) — the shape the petekTools decode kernel reads;
    # inline serde-derive arrays would crash it (P10 census render break).
    v = model.volume_bundle()
    assert v["cell_count"] > 0
    assert v["kind"] == "volume" and v["encoding"] == "base64"
    assert "positions" not in v and "indices" not in v
    assert set(v["blocks"]) == {"positions", "indices", "tri_cell", "cell_values", "zone_ids"}
    assert v["blocks"]["tri_cell"]["shape"][0] == v["triangle_count"]
    assert "PORO" in {v["property"], *model.property_names()}

    # MAP bundle: structural surfaces + per-property zone-average maps on one frame.
    m = model.map_bundle()
    f = m["frame"]
    assert f["ncol"] > 1 and f["nrow"] > 1
    assert len(m["horizons"]) == 2 and m["zone_averages"]

    # FRAME IS WORLD (UTM), not local. The `.with_georef(...)` seam (grid.model +
    # the MC template) labels the column lattice with its world frame, so the map
    # frame origin is the real UTM easting/northing (RS_X0/RS_Y0 ≈ 431000/6521000)
    # and the world outline falls inside the frame extent (raster overlays outline).
    assert f["origin_x"] > 400_000 and f["origin_y"] > 6_000_000, (f["origin_x"], f["origin_y"])
    xmin, ymin = f["origin_x"], f["origin_y"]
    xmax = f["origin_x"] + (f["ncol"] - 1) * f["spacing_x"]
    ymax = f["origin_y"] + (f["nrow"] - 1) * f["spacing_y"]
    ring = m["outline"][0]
    assert all(xmin <= px <= xmax and ymin <= py <= ymax for px, py in ring), (ring, (xmin, xmax, ymin, ymax))

    # INTERSECTION along a line in the MODEL FRAME (the frame the section marches).
    x0 = f["origin_x"]
    y0 = f["origin_y"] + (f["nrow"] - 1) * f["spacing_y"] / 2
    x1 = f["origin_x"] + (f["ncol"] - 1) * f["spacing_x"]
    line = model.intersection_bundle(line=[[x0, y0], [x1, y0]])
    assert line["columns"], "line section produced no columns"
    assert line["property"] == "PORO"
    # cells carry per-layer tops/bases and a property value (bundle-driven fills)
    col = line["columns"][0]
    assert len(col["layer_tops"]) == len(col["layer_bases"]) == len(col["values"])

    # The three positioned bores attach as viewer well tracks + section requests.
    ids = model.well_ids()
    assert set(ids) == {"99_9-1 A", "99_9-1 B", "99_8-1"}, ids
    # A bore section is produced and returns the contract shape. With the world
    # georeference now wired (grid.model `.with_georef`), the UTM bore trajectory
    # traces through the SAME xy↔ij as log registration, so the along-bore section
    # yields NON-EMPTY columns (previously zero under the local-frame seam bug —
    # fixed engine-side in petekStatic, wired here).
    bore = model.intersection_bundle(well=ids[0])
    assert set(bore) == {
        "schema_version", "inputs_ref", "property", "top_name", "base_name",
        "columns", "horizon_traces", "contacts",
        "sugar_cube",  # v4-additive: petekStatic section rendering toggle (default false)
        "zones",  # v5-additive: colour-by-zone rider — [{name, color}] top→base
    }, set(bore)
    assert bore["columns"], "UTM bore section produced no columns (world frame not wired?)"

    # save_view: ONE self-contained HTML from the real-shape model. Sanity-scan it.
    with tempfile.NamedTemporaryFile("r", suffix=".html", delete=False) as fh:
        path = fh.name
    model.save_view(path)
    html = open(path).read()
    assert "window.PETEK_VIEWER_PAYLOAD=" in html and 'PETEK_VIEWER_MODE="file"' in html
    assert "<script src=" not in html  # everything inlined
    for pat in (r'src\s*=\s*["\']https?:', r'href\s*=\s*["\']https?:', r'@import',
                r'url\(\s*https?:', r'fetch\(\s*["\']https?:'):
        assert not re.search(pat, html), pat
    # the pre-computed bore sections rode into the file (well markers + sections)
    assert '"kind":"static"' in html.replace(" ", "")


# --- analytics chart bundles (tornado / distribution / crossplot) ------------
# Render-only: peteksim plumbing maps MC results + logs onto petekTools' generic
# `charts` payload (viewer SCHEMA.md § ChartBundle); the viewer fits/bins nothing.
@pytest.fixture(scope="module")
def mc(model):
    return model.uncertainty(
        porosity=ps.level_shift(0.02),
        net_to_gross=ps.level_shift(0.03),
        contacts=ps.pick_spread(5.0),
        n=300,
        seed=7,
    )


def test_tornado_bundle_shape(mc):
    t = mc.tornado_bundle()
    assert t["mark"] == "tornado" and t["units"] == "MSm³"
    assert isinstance(t["base"], float) and t["fold_count"] == 8 and t["bars"]
    for b in t["bars"]:
        assert {"param", "in_lo", "in_hi", "out_lo", "out_hi", "swing"} <= set(b)
        assert "out_min" not in b  # inner-only: tornado pivots are P90/P10, no min/max span


def test_distribution_bundle_shape(mc):
    d = mc.distribution_bundle()
    assert d["mark"] == "distribution" and d["units"] == "MSm³"
    s = d["series"][0]
    assert set(s) == {"name", "bins", "cdf", "markers"}
    # binning happens in the plumbing: every kept realization is counted
    assert sum(b["count"] for b in s["bins"]) == 300
    assert s["markers"]["p90"] <= s["markers"]["p50"] <= s["markers"]["p10"]
    exc = [p["exceedance"] for p in s["cdf"]]
    assert exc == sorted(exc, reverse=True)  # exceedance is monotone non-increasing


def test_gas_distribution_uses_bcm(mc):
    d = mc.distribution_bundle(gas=True)  # two-contact model -> a gas leg exists
    assert d["units"] == "bcm" and d["series"] and d["series"][0]["bins"]


def test_field_distribution_overlays_structures_and_aggregate(mc):
    # per-structure + field-aggregate overlay in one panel (multi-series).
    field = ps.aggregate([mc, mc], correlation="independent")
    d = ps.distribution_bundle([mc, mc], aggregate=field, names=["North", "South"])
    names = [s["name"] for s in d["series"]]
    assert names == ["North", "South", "Field"]
    assert all(s["bins"] and s["cdf"] for s in d["series"])


def test_crossplot_bundle_shape(proj):
    c = proj.crossplot_bundle(x="PORO", y="NTG", color_by="well", regression=True)
    assert c["mark"] == "scatter"
    assert c["x"]["name"] == "PORO" and c["y"]["name"] == "NTG"
    assert c["color_by"]["kind"] == "categorical" and c["groups"]
    assert c["points"] and {"x", "y", "c"} <= set(c["points"][0])
    for tr in c["trends"]:  # render-only: coefficients computed here, shipped in payload
        assert {"x0", "y0", "x1", "y1", "slope", "intercept", "r2", "equation"} <= set(tr)


def test_crossplot_log_axis(proj):
    c = proj.crossplot_bundle(x="PORO", y="NTG", y_log=True)
    assert c["y"]["log"] is True and c["mark"] == "scatter"


def _inlined_payload(html: str) -> dict:
    import json
    import re

    m = re.search(r"window\.PETEK_VIEWER_PAYLOAD=(.*?);window\.PETEK_VIEWER_MODE", html)
    return json.loads(m.group(1).replace("<\\/", "</"))


def test_model_view_includes_attached_charts(model, mc):
    # model.save_view(charts=[...]) bakes the analytics bundles into the payload.
    charts = [mc.tornado_bundle(base=model.summary()["stoiip_msm3"]), mc.distribution_bundle()]
    with tempfile.NamedTemporaryFile("r", suffix=".html", delete=False) as fh:
        path = fh.name
    model.save_view(path, charts=charts)
    payload = _inlined_payload(open(path).read())
    assert payload["schema_version"] == 2
    assert {c["mark"] for c in payload["charts"]} == {"tornado", "distribution"}
    # geometry tabs still present alongside the charts
    assert payload["volume"]["cell_count"] > 0 and payload["map"]["frame"]["ncol"] > 1


def test_mc_view_is_charts_only(mc):
    # mc.save_view() = a pure-analytics session: charts only, no map/volume.
    with tempfile.NamedTemporaryFile("r", suffix=".html", delete=False) as fh:
        path = fh.name
    mc.save_view(path)  # default charts = tornado + STOIIP distribution
    html = open(path).read()
    assert "window.PETEK_VIEWER_PAYLOAD=" in html and "<script src=" not in html
    payload = _inlined_payload(html)
    assert payload["kind"] == "mc"
    assert "volume" not in payload and "map" not in payload
    assert {c["mark"] for c in payload["charts"]} == {"tornado", "distribution"}


def test_mc_view_serves_nonblocking(mc):
    # the served viewer path (no section provider) returns a live URL immediately.
    url = mc.view(open_browser=False, port=0)
    assert url.startswith("http://127.0.0.1")


def test_resimulate_symbol():
    # ps.resimulate() exists (the symbol was previously missing). Its use in
    # propagate(resimulate=...) is exercised on the fast synthetic tree in
    # test_facade.py.
    assert ps.resimulate() is True


if __name__ == "__main__":
    sys.exit(pytest.main([__file__, "-q"]))
