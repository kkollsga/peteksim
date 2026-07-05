#!/usr/bin/env python3
"""THE end-to-end acceptance suite — testing-doctrine **R6** (the round-trip rule),
homed in the product repo (peteksim), coordinator-tracked (task_suite_acceptance_suite).

A cross-repo feature is NOT done when its per-repo tests pass; it is done when this
suite passes on the canonical-strength synthetic asset (v2). This is the standing
**pre-stamp gate** the coordinator runs before stamping any cross-repo task, and the
front half of task_suite_perf_validation.

Entry points
------------
    make -C petekSim acceptance          # the fast gate (`-m acceptance`) + spill + render
    .venv-srs/bin/python -m pytest crates/srs-py/tests/test_acceptance.py -m acceptance -q

Legs (pytest markers)
---------------------
  * ``acceptance``        — the fast per-wave gate: the full chain on synth asset v2
                            (world-georef, deviated wells, tops-only H3b, pinch-out),
                            EVERY bundle kind, zoned MC, payload invariants + planted
                            truths. Target < ~5 min at the default (21-node) size.
  * ``acceptance_spill``  — opt-in: the same chain forced out-of-core through the
                            facade's ``memory_budget_bytes=`` (petekStatic MemoryBudget).
  * ``acceptance_render`` — opt-in: a Playwright browser-render round-trip of the
                            save_view export; skips cleanly without node/playwright.

Invariant inventory (each asserted below)
-----------------------------------------
FULL CHAIN
  generated tree → Project.load (zero non-noise skips) → framework (all mapped +
  tops-only H3b) → build_grid → staged upscale/propagate (+ a zone-scoped pipe and a
  collocated trend) → set_zonation + set_well_ties → grid.model → zoned MC → every
  bundle kind → save_view + served session.
PAYLOAD INVARIANTS (the escapes catalogue)
  * section ``layer_tops_l != layer_tops_r`` on dipping cells for BOTH Polyline and
    AlongBore specs on the world-georef model (the 3× frame + trapezoid-flat escapes);
  * edge arrays flattened == centroid where absent (inactive layer ⇒ l == r == centroid
    == null) — the sugar-cube-flatten proxy (peteksim does not expose the toggle yet);
  * volume shell non-empty with a plausible triangle count (in-core here; spilled leg
    below);
  * map outline extent == frame extent (world frame, within the half-cell centroid inset);
  * ``wells[].ties`` populated for tied bores;
  * ``wells_logs`` lanes byte-valid vs petektools' reference encoder;
  * ``horizon_traces`` present + the section NaN-gaps the Z5 pinch-out (east columns
    truncate, west full).
PLANTED-TRUTH RECOVERIES
  * rho (collocated depositional trend);
  * per-zone net-conditioned PORO means;
  * the contact plan (per-zone HC zero/nonzero pattern incl. contactless);
  * zero-spread zoned MC == in_place_by_zone per zone;
  * conservation: total == sum(zones);
  * deviated tops land at the trajectory (x, y), vertical tops at the wellhead.
"""

from __future__ import annotations

import base64
import json
import math
import os
import shutil
import struct
import subprocess
import sys
import tempfile
import urllib.parse
import urllib.request
from pathlib import Path

import pytest

REPO = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(REPO / "examples"))

import peteksim as ps  # noqa: E402
from petektools.viewer._wells import encode_lane as ref_encode_lane  # noqa: E402

SEED = 20_260_704
NCOL = 21  # the fast default-leg lattice (ingest + recovery are size-independent)
NAN_F32 = struct.pack("<I", 0x7FC00000)

# The fast per-wave gate. The spill/render legs carry their own opt-in markers, so
# they are NOT selected by `-m acceptance` (marks are additive — a module-level
# `acceptance` would wrongly pull them in).
acceptance = pytest.mark.acceptance


# =============================================================================
# The full chain — built ONCE, shared read-only across the payload/plant tests.
# =============================================================================
@pytest.fixture(scope="module")
def chain():
    """generated tree → Project.load → framework (+ tops-only H3b) → build_grid →
    staged upscale/propagate (PORO net-conditioned, NTG, + a Z2 zone-scoped pipe) →
    set_zonation + set_well_ties → grid.model. The world-georef, deviated-well,
    pinch-out, per-zone-contact model every invariant runs on."""
    root = tempfile.mkdtemp(prefix="acceptance-")
    man = ps.synth_asset(root, seed=SEED, ncol=NCOL)
    proj = ps.Project.load(man["root"], crs=man["crs"], aliases=man["aliases"])

    inv = proj.inventory()
    assert inv.skipped == [], inv.skipped  # zero non-noise skips (ingest gate)

    fw = proj.framework(horizons=man["horizons"], outline="ModelEdge",
                        tie_to_tops=True, min_thickness_m=0.0)
    fw.set_zonation([dict(z, nk=2) for z in man["zonation"]])
    # Engine well ties keyed on each real bore head (residuals attach to shown bores).
    heads = proj.wells().heads()
    measured = {h: 2000.0 + 12.0 * k for k, h in enumerate(man["horizons"])}
    fw.set_well_ties([{"id": wid, "x": x, "y": y, "tops": dict(measured)}
                      for (wid, x, y) in heads])
    grid = fw.build_grid()

    # Whole-field PORO (net-conditioned) + NTG, plus a Z2 zone-scoped PORO pipe (the
    # contact-bearing zone) so the zoned-MC realizes that zone from its own cube.
    por = grid.property("PORO")
    por.upscale(proj.wells(), net_only=True)
    por.propagate(ps.gaussian(ps.spherical(range_m=1500.0), seed=11))
    ntg = grid.property("NTG")
    ntg.upscale(proj.wells())
    ntg.propagate(ps.gaussian(ps.spherical(range_m=1500.0), seed=12))
    z2 = grid.property("PORO", zone="Z2")
    z2.upscale(proj.wells(), net_only=True)  # net-conditioned like the whole-field pipe
    z2.propagate(ps.gaussian(ps.spherical(range_m=1500.0), seed=13))

    model = grid.model(fluid="oil", fvf=1.30, gas_fvf=0.005, wells=proj.wells())
    return man, proj, model


def _finite(v) -> bool:
    return isinstance(v, (int, float)) and v == v  # not None, not NaN


def _pearson(a, b):
    n = len(a)
    if n == 0:
        return 0.0
    ma, mb = sum(a) / n, sum(b) / n
    cov = sum((x - ma) * (y - mb) for x, y in zip(a, b))
    va = math.sqrt(sum((x - ma) ** 2 for x in a))
    vb = math.sqrt(sum((y - mb) ** 2 for y in b))
    return cov / (va * vb) if va > 0 and vb > 0 else 0.0


# =============================================================================
# (1) THE FULL CHAIN runs end-to-end, every bundle kind, save_view + served.
# =============================================================================
@acceptance
def test_full_chain_every_bundle_save_view_and_served(chain, tmp_path):
    man, _proj, model = chain
    # every bundle kind is produced and non-empty
    vol = model.volume_bundle(property="PORO")
    mp = model.map_bundle(property="PORO")
    f = mp["frame"]
    x0, y0 = f["origin_x"], f["origin_y"]
    x1 = x0 + (f["ncol"] - 1) * f["spacing_x"]
    y1 = y0 + (f["nrow"] - 1) * f["spacing_y"]
    line = model.intersection_bundle(line=[[x0, y0], [x1, y1]], property="PORO")
    bore = model.intersection_bundle(well=model.well_ids()[0], property="PORO")
    wells = model.wells_bundle()
    # v3 block envelope (base64 blocks) — the shape the viewer decodes.
    assert vol["cell_count"] > 0 and vol["encoding"] == "base64" and vol["triangle_count"] > 0
    assert f["ncol"] > 1 and mp["horizons"] and mp["zone_averages"]
    assert line["columns"] and bore["columns"]
    assert wells is not None and wells["kind"] == "wells_logs" and wells["wells"]

    # zoned MC + charts (the analytics half)
    mc = model.zoned_uncertainty(contacts=ps.pick_spread(sd_m=4.0),
                                 goc=ps.pick_spread(sd_m=3.0),
                                 porosity=ps.level_shift(sd=0.01), n=96, seed=42)
    charts = [mc.distribution_bundle(), mc.distribution_bundle(zone="Z4")]

    # save_view: ONE self-contained HTML with every geometry tab + the charts.
    html = tmp_path / "acceptance_view.html"
    model.save_view(str(html), property="PORO", charts=charts)
    text = html.read_text()
    assert html.stat().st_size > 0
    assert "window.PETEK_VIEWER_PAYLOAD=" in text and "<script src=" not in text
    assert '"kind":"wells_logs"' in text  # the wells tab rode into the frozen payload

    # served session: /model.json carries the payload; /section + /volume recut live.
    url = model.view(open_browser=False, port=0)
    with urllib.request.urlopen(url + "/model.json", timeout=10) as r:
        payload = json.loads(r.read().decode("utf-8"))
    assert payload["schema_version"] == 2 and payload["wells_logs"]["wells"]
    q = "line=" + urllib.parse.quote(json.dumps([[x0, y0], [x1, y1]]))
    with urllib.request.urlopen(url + "/section?" + q, timeout=10) as r:
        sec = json.loads(r.read())
    assert sec["columns"], "served /section produced no columns"
    full = json.loads(model._volume_json(property="PORO"))
    mid = (full["value_range"]["min"] + full["value_range"]["max"]) / 2.0
    with urllib.request.urlopen(
        url + f"/volume?property=PORO&cutoff={mid}&keep_above=true", timeout=10
    ) as r:
        cut = json.loads(r.read())
    # Both are v3 block envelopes; the re-cut shell exposes different geometry.
    assert cut["schema_version"] >= 3 and cut["encoding"] == "base64"
    assert "positions" not in cut and cut["triangle_count"] != full["triangle_count"]


# =============================================================================
# (2) PAYLOAD INVARIANTS — the escapes catalogue.
# =============================================================================
def _edge_invariants(bundle):
    """Return (max|l-r| over finite cells, dipping-cell count > 0.1 m, nan_aligned).
    ``nan_aligned`` = wherever the centroid layer_top/base is null (inactive layer),
    the matching left/right edge value is null too (the flatten-when-absent proxy)."""
    maxlr = 0.0
    dipping = 0
    nan_aligned = True
    for c in bundle["columns"]:
        edges = ((c["layer_tops_l"], c["layer_tops_r"], c["layer_tops"]),
                 (c["layer_bases_l"], c["layer_bases_r"], c["layer_bases"]))
        for lo, hi, cen in edges:
            for k in range(len(cen)):
                cen_absent = cen[k] is None or cen[k] != cen[k]
                l_absent = lo[k] is None or lo[k] != lo[k]
                r_absent = hi[k] is None or hi[k] != hi[k]
                if cen_absent:
                    if not (l_absent and r_absent):
                        nan_aligned = False
                    continue
                if l_absent or r_absent:
                    continue
                d = abs(lo[k] - hi[k])
                maxlr = max(maxlr, d)
                if d > 0.1:
                    dipping += 1
    return maxlr, dipping, nan_aligned


@acceptance
def test_section_edges_differ_on_dipping_cells_polyline(chain):
    # THE motivating escape: trapezoid edges flat on the real model. On the
    # world-georef dome a Polyline section's cells dip within the column, so the
    # left/right fence-edge depths differ decisively (not a flat "sugar cube").
    _man, _proj, model = chain
    mp = model.map_bundle(property="PORO")["frame"]
    x0, y0 = mp["origin_x"], mp["origin_y"]
    x1 = x0 + (mp["ncol"] - 1) * mp["spacing_x"]
    y1 = y0 + (mp["nrow"] - 1) * mp["spacing_y"]
    sec = model.intersection_bundle(line=[[x0, y0], [x1, y1]], property="PORO")
    assert sec["sugar_cube"] is False  # the trapezoid (edge-carrying) path
    maxlr, dipping, nan_aligned = _edge_invariants(sec)
    assert maxlr > 1.0, ("Polyline layer edges are flat (l==r) — trapezoid escape", maxlr)
    assert dipping > 0, "no dipping cells found on the dome section"
    assert nan_aligned, "inactive-layer edges must be null where the centroid is null"


@acceptance
def test_section_edges_differ_on_dipping_cells_along_bore(chain):
    # Same trapezoid invariant on the AlongBore spec — the deviated-well path the
    # synth asset's vertical-only v1 never exercised (the AlongBore edge escape).
    man, _proj, model = chain
    deviated = [w["id"] for w in man["well_program"] if w["profile"] != "vertical"]
    assert deviated, "the canonical asset must carry deviated bores"
    sec = model.intersection_bundle(well=deviated[0], property="PORO")
    assert sec["sugar_cube"] is False
    maxlr, dipping, nan_aligned = _edge_invariants(sec)
    assert maxlr > 1.0, ("AlongBore layer edges are flat (l==r)", maxlr)
    assert dipping > 0, "no dipping cells along the deviated bore"
    assert nan_aligned


@pytest.fixture(scope="module")
def sugar_chain():
    """A second build of the canonical asset with ``grid.model(sugar_cube=True)`` —
    the flat-box section rendering (petekStatic ``with_sugar_cube``). Minimal: the
    sugar-cube toggle is geometry-only (no property population needed to exercise it)."""
    root = tempfile.mkdtemp(prefix="acceptance-sugar-")
    man = ps.synth_asset(root, seed=SEED, ncol=NCOL)
    proj = ps.Project.load(man["root"], crs=man["crs"], aliases=man["aliases"])
    fw = proj.framework(horizons=man["horizons"], outline="ModelEdge",
                        tie_to_tops=True, min_thickness_m=0.0)
    fw.set_zonation([dict(z, nk=2) for z in man["zonation"]])
    grid = fw.build_grid()
    model = grid.model(fluid="oil", fvf=1.30, gas_fvf=0.005,
                       wells=proj.wells(), sugar_cube=True)
    return man, model


@acceptance
def test_sugar_cube_flattens_section_edges_to_centroid(sugar_chain):
    # The DEFERRED assertion, now live: peteksim exposes grid.model(sugar_cube=True),
    # so the section's layer edges must collapse to the centroid (flat "sugar cube")
    # — the inverse of the default trapezoid path (test_section_edges_differ_*).
    _man, model = sugar_chain
    mp = model.map_bundle(property="PORO")["frame"]
    x0, y0 = mp["origin_x"], mp["origin_y"]
    x1 = x0 + (mp["ncol"] - 1) * mp["spacing_x"]
    y1 = y0 + (mp["nrow"] - 1) * mp["spacing_y"]
    sec = model.intersection_bundle(line=[[x0, y0], [x1, y1]], property="PORO")
    assert sec["sugar_cube"] is True  # the flat-box (edge-collapsed) path
    # Every finite cell's left/right edges equal the centroid trace (no dip).
    flattened = 0
    for c in sec["columns"]:
        for lo, hi, cen in ((c["layer_tops_l"], c["layer_tops_r"], c["layer_tops"]),
                            (c["layer_bases_l"], c["layer_bases_r"], c["layer_bases"])):
            for k in range(len(cen)):
                if cen[k] is None or cen[k] != cen[k]:
                    continue
                assert lo[k] == cen[k] == hi[k], ("sugar-cube edge not flat", lo[k], cen[k], hi[k])
                flattened += 1
    assert flattened > 0, "no finite section cells to check flattening on"
    # And the flatten-when-absent invariant still holds (inactive layer ⇒ all null).
    _maxlr, _dip, nan_aligned = _edge_invariants(sec)
    assert nan_aligned


@acceptance
def test_volume_shell_non_empty_plausible_triangles(chain):
    # volume bundle empty on spilled models was an escape; in-core it must be a
    # non-empty, index-consistent shell mesh with a plausible triangle count.
    _man, _proj, model = chain
    v = model.volume_bundle(property="PORO")
    # v3 block envelope: metadata + base64 blocks (no inline arrays).
    assert v["cell_count"] > 0 and v["encoding"] == "base64"
    assert "positions" not in v and "indices" not in v
    assert set(v["blocks"]) == {"positions", "indices", "tri_cell", "cell_values", "zone_ids"}
    assert v["blocks"]["indices"]["shape"] == [v["triangle_count"], 3]
    assert v["blocks"]["tri_cell"]["shape"][0] == v["triangle_count"]
    tris = v["triangle_count"]
    # a closed shell over a multi-thousand-cell grid has hundreds+ of triangles.
    assert tris > 100, tris
    assert set(v["value_range"]) == {"min", "max"}


@acceptance
def test_map_outline_extent_equals_frame_extent(chain):
    # outline == frame extent (a frame-mixing escape): the ModelEdge outline and the
    # raster frame share the WORLD frame. The raster frame is the column-centroid
    # frame (inset half a cell inside the node-lattice outline), so their extents
    # agree within one cell spacing and the frame sits inside the outline.
    _man, _proj, model = chain
    m = model.map_bundle(property="PORO")
    f = m["frame"]
    assert f["origin_x"] > 400_000 and f["origin_y"] > 6_000_000, f  # R1 world frame
    fx0, fy0 = f["origin_x"], f["origin_y"]
    fx1 = fx0 + (f["ncol"] - 1) * f["spacing_x"]
    fy1 = fy0 + (f["nrow"] - 1) * f["spacing_y"]
    ring = m["outline"][0]
    oxs = [p[0] for p in ring]
    oys = [p[1] for p in ring]
    sx, sy = f["spacing_x"], f["spacing_y"]
    assert abs(min(oxs) - fx0) <= sx and abs(max(oxs) - fx1) <= sx, (min(oxs), max(oxs), fx0, fx1)
    assert abs(min(oys) - fy0) <= sy and abs(max(oys) - fy1) <= sy, (min(oys), max(oys), fy0, fy1)
    # the centroid frame is contained within the node-lattice outline extent.
    assert min(oxs) <= fx0 and fx1 <= max(oxs)
    assert min(oys) <= fy0 and fy1 <= max(oys)


@acceptance
def test_wells_ties_populated_for_tied_bores(chain):
    # wells[].ties populated (the engine tie seam). The map bundle's wells carry the
    # per-horizon tie residuals for the bores that were tied.
    _man, _proj, model = chain
    res_ids = {r["well"] for r in model.well_tie_residuals()}
    assert res_ids, "engine well-tie residuals must be surfaced"
    tied = [w for w in model.map_bundle(property="PORO")["wells"] if w.get("ties")]
    assert tied, "no wells carry ties in the map bundle"
    for w in tied:
        assert w["id"] in res_ids
        for t in w["ties"]:
            assert set(t) == {"horizon", "residual_m"}


@acceptance
def test_wells_logs_lanes_byte_valid_vs_reference_encoder(chain):
    # wells_logs lanes byte-valid vs petektools' reference encoder — the wire is
    # byte-identical to the reference `encode_lane` (LE f32, base64, NaN=0x7FC00000).
    _man, _proj, model = chain
    b = model.wells_bundle()
    assert b["kind"] == "wells_logs" and b["schema_version"] == 4
    w = b["wells"][0]
    saw_nan = False
    for lane in (w["md_m"], w["tvd_m"], *(c["values"] for c in w["curves"])):
        raw = base64.b64decode(lane["data"])
        n = lane["shape"][0]
        assert len(raw) == 4 * n
        vals = list(struct.unpack(f"<{n}f", raw))
        for i, v in enumerate(vals):
            if v != v:  # NaN sample must be the canonical quiet NaN
                saw_nan = True
                assert raw[4 * i:4 * i + 4] == NAN_F32
        # re-encoding the same values through the reference reproduces the bytes.
        assert ref_encode_lane(vals)["data"] == lane["data"]
    up = next(c for c in w["curves"] if c["mnemonic"] == "PHIE_UP")
    up_vals = struct.unpack(
        f"<{up['values']['shape'][0]}f", base64.b64decode(up["values"]["data"])
    )
    assert any(v != v for v in up_vals) or saw_nan, "the NaN=null lane path must be exercised"


@acceptance
def test_horizon_traces_present_and_pinch_out_nan_gapped(chain):
    # horizon_traces present + NaN-gapped correctly over the Z5 pinch-out. A full
    # W→E section carries the interior-horizon depth traces (present, parallel to the
    # columns) AND NaN-gaps the pinch: the eastern (pinched-to-zero) columns truncate
    # their deepest Z5 cells to null while the western columns stay full (R5 food
    # flowing through to the section payload).
    man, _proj, model = chain
    f = model.map_bundle(property="PORO")["frame"]
    x0 = f["origin_x"]
    x1 = f["origin_x"] + (f["ncol"] - 1) * f["spacing_x"]
    ymid = f["origin_y"] + (f["nrow"] - 1) * f["spacing_y"] / 2
    sec = model.intersection_bundle(line=[[x0, ymid], [x1, ymid]], property="PORO")
    traces = sec["horizon_traces"]
    assert traces, "interior horizon traces must be present on the multi-zone section"
    ncols = len(sec["columns"])
    mapped = set(man["horizons"])
    for h in traces:
        assert h["name"] in mapped
        assert len(h["depths"]) == ncols  # parallel to the columns
    # NaN-gap: western columns full (no truncated layers), eastern columns truncate.
    def truncated(col):
        return sum(1 for v in col["layer_tops"] if v is None or v != v)
    west = sec["columns"][0]
    east = sec["columns"][-1]
    assert truncated(west) == 0, "west (full-thickness) column should not truncate"
    assert truncated(east) > 0, "east (pinched) column must NaN-gap its Z5 cells"


# =============================================================================
# (3) PLANTED-TRUTH RECOVERIES.
# =============================================================================
@acceptance
def test_per_zone_net_conditioned_poro_recovers_targets(chain):
    # per-zone net-conditioned PORO means recover the planted net-rock porosity.
    man, _proj, model = chain
    por = {s["zone"]: s["mean"] for s in model.zone_stats("PORO")}
    for zone in man["zones"]:
        tgt = man["zone_targets"][zone]["net_por_mean"]
        assert _finite(por[zone])
        assert abs(por[zone] - tgt) < 0.05, ("PORO", zone, por[zone], tgt)


@acceptance
def test_contact_plan_hc_pattern(chain):
    # the contact plan: per-zone HC zero/nonzero pattern incl. the contactless zones.
    # Z2 single OWC (oil, nonzero, not two-contact); Z4 two-contact (GOC+FWL, nonzero);
    # every contactless zone rolls up exactly zero HC.
    man, _proj, model = chain
    det = {r["zone"]: r for r in model.in_place_by_zone()["zones"]}
    plan = man["contact_plan"]
    for zone, spec in plan.items():
        row = det[zone]
        if spec["type"] == "single":
            assert row["stoiip_sm3"] > 0.0 and not row["two_contact"], (zone, row)
        elif spec["type"] == "two_contact":
            assert row["two_contact"] and row["stoiip_sm3"] > 0.0, (zone, row)
        else:  # contactless
            assert row["stoiip_sm3"] == 0.0, (zone, row)


@acceptance
def test_zero_spread_zoned_mc_equals_deterministic_and_conserves(chain):
    # zero-spread zoned MC == in_place_by_zone per zone (the canonical planted-truth
    # instance) AND conservation: total == sum(zones) on every draw. The Z2 zone-scoped
    # pipe makes this the zone-pipe parity path too.
    _man, _proj, model = chain
    det = {r["zone"]: r["stoiip_sm3"] for r in model.in_place_by_zone()["zones"]}
    mc = model.zoned_uncertainty(n=8, seed=1)  # zero spread
    for z in mc.zones:
        d = det[z["zone"]]
        assert abs(z["stoiip"]["mean"] - d) <= 1e-6 * max(abs(d), 1.0), (
            z["zone"], z["stoiip"]["mean"], d)
    # conservation on every sampled draw of a spread MC.
    smc = model.zoned_uncertainty(contacts=ps.pick_spread(sd_m=4.0), n=64, seed=7)
    tot = smc.total["stoiip"]["samples"]
    for i in range(0, len(tot), 8):
        s = sum(z["stoiip"]["samples"][i] for z in smc.zones)
        assert abs(tot[i] - s) <= 1e-6 * max(abs(tot[i]), 1.0), (i, tot[i], s)


@acceptance
def test_deviated_tops_at_trajectory_vertical_at_wellhead(chain):
    # deviated tops land at the trajectory (x, y); vertical tops at the wellhead.
    man, _proj, _model = chain
    rows = _read_tops(man["root"])
    heads = {w["id"]: (w["x"], w["y"]) for w in man["well_program"]}
    dev = {w["id"] for w in man["well_program"] if w["profile"] != "vertical"}
    vert = {w["id"] for w in man["well_program"] if w["profile"] == "vertical"}
    off = {}
    for r in rows:
        if r["type"] != "Horizon" or r["surface"] not in man["horizons"]:
            continue
        hx, hy = heads[r["well"]]
        off.setdefault(r["well"], []).append(math.hypot(r["x"] - hx, r["y"] - hy))
    for wid in dev:
        assert max(off[wid]) > 100.0, (wid, off[wid])   # deep picks far from head
    for wid in vert:
        assert max(off[wid]) < 1.0, (wid, off[wid])     # picks sit at the head


@acceptance
def test_collocated_recovers_planted_rho():
    # rho (collocated): a single-layer whole-field NTG cube steered by the depositional
    # trend recovers the planted per-node correlation through the full pipeline.
    root = tempfile.mkdtemp(prefix="acceptance-rho-")
    man = ps.synth_asset(root, seed=SEED, ncol=NCOL)
    proj = ps.Project.load(man["root"], crs=man["crs"], aliases=man["aliases"])
    trend = proj.surface(man["trend_surface"])
    fw = proj.framework(horizons=[man["horizons"][0], man["horizons"][-1]],
                        outline="ModelEdge", tie_to_tops=False)
    fw.set_layering(ps.layers(n=1))          # per-node frame (the planted rho is per-node)
    grid = fw.build_grid()
    ntg = grid.property("NTG")
    ntg.upscale(proj.wells())
    ntg.propagate(ps.gaussian(ps.spherical(range_m=1200.0), seed=7),
                  trend=ps.collocated(trend, corr=man["rho"], as_depth=False))
    model = grid.model(contacts={"fwl": man["contacts"]["fwl_z4"] + 60.0},
                       fluid="oil", fvf=1.30)
    mb = model.map_bundle(property="NTG")
    frame = mb["frame"]
    ntg_map = mb["zone_averages"][0]["values"]
    ni, nj = frame["ncol"], frame["nrow"]
    tvals, nvals = [], []
    for j in range(nj):
        for i in range(ni):
            v = ntg_map[j * ni + i]
            if not _finite(v):
                continue
            x = frame["origin_x"] + i * frame["spacing_x"]
            y = frame["origin_y"] + j * frame["spacing_y"]
            t = trend.value_at(x, y)
            if _finite(t):
                nvals.append(v)
                tvals.append(t)
    r = _pearson(tvals, nvals)
    assert abs(r - man["rho"]) < 0.25, (r, man["rho"])
    assert r > 0.2, r


def _read_tops(root):
    """Parse the emitted Petrel well-tops tree into pick rows (Latin-1)."""
    text = (Path(root) / "WellTops" / "FieldWellTops").read_bytes().decode("latin-1")
    rows = []
    for line in text.splitlines():
        if '"' not in line:
            continue
        f = line.split()
        parts = line.split('"')
        rows.append({"x": float(f[0]), "y": float(f[1]), "type": f[8],
                     "surface": parts[1], "well": parts[3]})
    return rows


# =============================================================================
# SPILL LEG (opt-in) — the same chain forced out-of-core through the facade's
# MemoryBudget knob. Volume works in-core-shaped at base HEAD; map/section are
# xfail'd against the pending petekStatic spilled-bundle fixes.
# =============================================================================
acceptance_spill = pytest.mark.acceptance_spill


@pytest.fixture(scope="module")
def spilled():
    """The R2 spilled cell of the mode matrix: the chain forced out-of-core through
    the facade's ``memory_budget_bytes=`` knob (petekStatic MemoryBudget), capturing
    the loud mode-switch advisory. Built once for the spill leg."""
    root = tempfile.mkdtemp(prefix="acceptance-spill-")
    man = ps.synth_asset(root, seed=SEED, ncol=NCOL)
    proj = ps.Project.load(man["root"], crs=man["crs"], aliases=man["aliases"])
    fw = proj.framework(horizons=man["horizons"], outline="ModelEdge",
                        tie_to_tops=True, min_thickness_m=0.0)
    fw.set_zonation([dict(z, nk=3) for z in man["zonation"]])
    grid = fw.build_grid()
    por = grid.property("PORO")
    por.upscale(proj.wells(), net_only=True)
    por.propagate(ps.gaussian(ps.spherical(range_m=1500.0), seed=11))
    # Force the out-of-core path with a tiny budget, capturing the loud mode-switch
    # advisory on stderr (fd-level, so the Rust eprintln is caught).
    r_fd, w_fd = os.pipe()
    saved = os.dup(2)
    os.dup2(w_fd, 2)
    os.close(w_fd)
    try:
        model = grid.model(fluid="oil", fvf=1.30, gas_fvf=0.005,
                           wells=proj.wells(), memory_budget_bytes=1024)
    finally:
        os.dup2(saved, 2)
        os.close(saved)
        stderr = os.read(r_fd, 1 << 16).decode(errors="replace")
        os.close(r_fd)
    return man, model, stderr


@acceptance_spill
def test_spill_emits_loud_mode_switch_warning(spilled):
    _man, _model, stderr = spilled
    assert "OUT-OF-CORE mode" in stderr and "spilling" in stderr, stderr


@acceptance_spill
def test_spilled_volume_bundle_non_empty(spilled):
    # volume bundle empty on spilled models was an escape; the spilled shell must be
    # non-empty + index-consistent. This already holds at base HEAD 9a846ae.
    _man, model, _err = spilled
    v = model.volume_bundle(property="PORO")
    # v3 block envelope: non-empty shell, block manifest present.
    assert v["cell_count"] > 0 and v["encoding"] == "base64" and v["triangle_count"] > 0
    assert v["blocks"]["tri_cell"]["shape"][0] == v["triangle_count"]


@acceptance_spill
def test_spilled_map_bundle_non_empty(spilled):
    # XFAIL vs base HEAD 9a846ae: a spilled model reports a 1×1 areal frame to the
    # view producers ("view frame needs an areal lattice of at least 2x2 columns,
    # got 1x1"). Fix is on an unmerged petekStatic branch — coordinator merge window.
    _man, model, _err = spilled
    try:
        m = model.map_bundle(property="PORO")
    except ValueError as e:
        pytest.xfail(f"spilled map bundle unsupported at petekStatic 9a846ae: {e}")
    assert m["frame"]["ncol"] > 1 and m["zone_averages"]


@acceptance_spill
def test_spilled_section_bundle_non_empty(spilled):
    # XFAIL vs base HEAD 9a846ae: same 1×1-areal-frame limitation as the map leg.
    _man, model, _err = spilled
    try:
        m = model.map_bundle(property="PORO")
        f = m["frame"]
        x0, y0 = f["origin_x"], f["origin_y"]
        x1 = x0 + (f["ncol"] - 1) * f["spacing_x"]
        sec = model.intersection_bundle(line=[[x0, y0], [x1, y0]], property="PORO")
    except ValueError as e:
        pytest.xfail(f"spilled section bundle unsupported at petekStatic 9a846ae: {e}")
    assert sec["columns"]


# =============================================================================
# RENDER LEG (opt-in) — the Playwright browser round-trip of save_view.
# =============================================================================
_NODE = shutil.which("node")
_RENDER_JS = Path(__file__).parent / "acceptance_render.mjs"


def _playwright_node_path():
    """A NODE_PATH under which `require('playwright')` resolves AND a chromium build
    exists, or None (the render leg then skips cleanly). Searches common install
    roots + a caller-provided PLAYWRIGHT_NODE_PATH."""
    if _NODE is None:
        return None
    candidates = [os.environ.get("PLAYWRIGHT_NODE_PATH", "")]
    for base in (REPO, REPO.parent):
        candidates.append(str(base / "node_modules"))
    for np in candidates:
        if not np:
            continue
        probe = ("const {chromium}=require('playwright');"
                 "if(!require('fs').existsSync(chromium.executablePath()))process.exit(3);")
        try:
            out = subprocess.run([_NODE, "-e", probe], capture_output=True,
                                 text=True, timeout=30, env={**os.environ, "NODE_PATH": np})
            if out.returncode == 0:
                return np
        except Exception:
            continue
    return None


@pytest.mark.acceptance_render
def test_save_view_renders_every_tab_in_headless_chromium(chain, tmp_path):
    node_path = _playwright_node_path()
    if node_path is None:
        pytest.skip("node/playwright/chromium unavailable — the browser render leg is opt-in")
    _man, _proj, model = chain
    mc = model.zoned_uncertainty(contacts=ps.pick_spread(sd_m=4.0), n=64, seed=3)
    html = tmp_path / "render_view.html"
    model.save_view(str(html), property="PORO", charts=[mc.distribution_bundle()])
    out = subprocess.run([_NODE, str(_RENDER_JS), str(html)], capture_output=True,
                         text=True, timeout=120, env={**os.environ, "NODE_PATH": node_path})
    assert out.returncode == 0, f"render failed (exit {out.returncode}): {out.stdout}\n{out.stderr}"
    result = json.loads(out.stdout.strip().splitlines()[-1])
    assert not result["consoleErrors"], result["consoleErrors"]
    assert "tris" in (result.get("volBadge") or ""), result


if __name__ == "__main__":
    sys.exit(pytest.main([__file__, "-m", "acceptance", "-q"]))
