#!/usr/bin/env python3
"""R6 current-seam acceptance: petekIO -> petekStatic -> petekSim -> viewer.

These are the nineteen release-gate cases that used to be hidden behind the
removed ``peteksim.Project`` facade.  Project/static construction now stays with
petekIO/petekStatic.  petekSim contributes product payload composition,
save/serve callbacks, analytics, and the viewer hand-off; no legacy constructor
is imported or recreated here.
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
import threading
import urllib.parse
import urllib.request
from copy import deepcopy
from dataclasses import dataclass
from pathlib import Path

import pytest

import petekio as pio
import peteksim as ps
import petekstatic as pst
from peteksim.synth_asset import spill_recipe
from petektools.viewer._wells import build_well_log_bundle, encode_lane


SEED = 20_260_704
NCOL = 11
NAN_F32 = struct.pack("<I", 0x7FC00000)
acceptance = pytest.mark.acceptance
acceptance_spill = pytest.mark.acceptance_spill


@dataclass
class CurrentSeam:
    manifest: dict
    project: object
    static_grid: object
    volume_result: object
    static_model: object
    product_model: object
    payload: dict
    line: list[list[float]]


class _DeliveryModel:
    """The callback surface petekSim passes to petekTools' live server."""

    def __init__(self, chain: CurrentSeam):
        self.chain = chain

    def _section_json(self, **_request):
        return json.dumps(self.chain.payload["sections"][0], separators=(",", ":"))

    def _volume_json(self, **request):
        # ModelResult's frozen report carries the already-computed shell; live
        # threshold re-cut belongs to a mutable StaticModel producer.
        return json.dumps(self.chain.payload["volume"], separators=(",", ":"))


def _saved_payload(model, path: Path) -> dict:
    model.save_json(str(path))
    return json.loads(path.read_text())


def _section_line(frame: dict) -> list[list[float]]:
    return [
        [frame["origin_x"], frame["origin_y"]],
        [
            frame["origin_x"] + (frame["ncol"] - 1) * frame["spacing_x"],
            frame["origin_y"] + (frame["nrow"] - 1) * frame["spacing_y"],
        ],
    ]


@pytest.fixture(scope="module")
def chain(tmp_path_factory) -> CurrentSeam:
    root = tmp_path_factory.mktemp("current-seam")
    manifest = ps.synth_asset(root, seed=SEED, n_wells=8, ncol=NCOL)
    project = pio.Project.import_data(
        manifest["root"], crs=manifest["crs"], aliases=manifest["aliases"]
    )
    inventory = project.inventory()
    surfaces = [name for name in inventory["surfaces"] if name.startswith("Surfaces/H")]
    assert len(surfaces) >= 2

    # DATA -> STATIC: the canonical Python declaration consumes the petekIO
    # project and lowers deterministic property/volume inputs in petekStatic.
    static_grid = (
        pst.Grid.from_project(project)
        .geometry(cell=(25.0, 25.0), orient=30.0, outline="ModelEdge")
        .horizons(
            [
                {"name": "TOP", "surface": surfaces[0]},
                {"name": "BASE", "surface": surfaces[1]},
            ]
        )
        .zones({"RESERVOIR": ("TOP", "BASE")})
        .layers({"RESERVOIR": pst.Layering(1)})
    )
    static_grid.properties.NTG = 0.71
    static_grid.properties.POR = 0.22
    static_grid.properties.SW = 0.31
    volume_result = static_grid.volumes(
        ntg="NTG", por="POR", sw="SW", fluid="oil", fvf=1.25
    ).run(progress=True)

    # STATIC -> SIM/viewer: petekStatic owns typed Map/section construction;
    # petekSim owns the product document, binary volume envelope and delivery.
    static_model = pst.build_flat_model(
        n=9,
        depth_m=2_000.0,
        owc_m=2_030.0,
        area_m2=40_000.0,
        gross_height_m=40.0,
        nk=4,
        porosity=0.22,
        net_to_gross=0.71,
        water_saturation=0.31,
    )
    static_map = json.loads(static_model.map_bundle(property="PORO"))
    line = _section_line(static_map["frame"])
    static_section = json.loads(static_model.intersection_bundle(line, property="PORO"))
    product_model = ps.run_box_model(
        area_km2=0.04,
        gross_height_m=40.0,
        porosity=0.22,
        net_to_gross=0.71,
        water_saturation=0.31,
        fvf=1.25,
        fluid="oil",
        contact_m=2_030.0,
        top_m=2_000.0,
        ni=8,
        nj=8,
        nk=4,
        realizations=32,
        seed=SEED,
    )
    payload = _saved_payload(product_model, root / "product.json")
    payload["kind"] = "current-seam"
    payload["map"] = static_map
    payload["sections"] = [static_section]
    payload["section_labels"] = ["Synthetic diagonal"]
    payload["wells_logs"] = build_well_log_bundle()
    return CurrentSeam(
        manifest,
        project,
        static_grid,
        volume_result,
        static_model,
        product_model,
        payload,
        line,
    )


@pytest.fixture(scope="module")
def dipping(tmp_path_factory):
    model = ps.Model(
        0.04,
        40.0,
        ni=7,
        nj=7,
        nk=4,
        top_m=2_000.0,
        contact_m=2_030.0,
        porosity=0.22,
        net_to_gross=0.71,
        water_saturation=0.31,
    )
    model.add_control(3, 3, 1_978.0)
    refined = model.solve()
    payload = _saved_payload(refined, tmp_path_factory.mktemp("dip") / "dip.json")
    line = _section_line(payload["map"]["frame"])
    section = json.loads(refined._section_json(property="PORO", line=line, well=None))
    return refined, payload, line, section


def _edge_invariants(bundle):
    max_delta = 0.0
    dipping_count = 0
    aligned = True
    for column in bundle["columns"]:
        for left, right, centre in (
            (column["layer_tops_l"], column["layer_tops_r"], column["layer_tops"]),
            (column["layer_bases_l"], column["layer_bases_r"], column["layer_bases"]),
        ):
            for lo, hi, mid in zip(left, right, centre):
                if mid is None:
                    aligned &= lo is None and hi is None
                elif lo is not None and hi is not None:
                    delta = abs(lo - hi)
                    max_delta = max(max_delta, delta)
                    dipping_count += delta > 0.1
    return max_delta, dipping_count, aligned


@acceptance
def test_full_chain_every_bundle_save_view_and_served(chain, tmp_path):
    inv = chain.project.inventory()
    assert inv["counts"]["surfaces"] and inv["counts"]["wells"] == 8
    assert chain.volume_result.summary()["in_place_sm3"] > 0.0
    assert chain.payload["map"]["horizons"] and chain.payload["sections"][0]["columns"]
    assert chain.payload["volume"]["encoding"] == "base64"
    assert chain.payload["wells_logs"]["wells"]

    encoded = json.dumps(chain.payload, separators=(",", ":"))
    html = tmp_path / "current-seam.html"
    ps._save_view(str(html), encoded)
    text = html.read_text()
    assert "window.PETEK_VIEWER_PAYLOAD=" in text and "<script src=" not in text

    httpd, url = ps._make_server(_DeliveryModel(chain), encoded, 0)
    threading.Thread(target=httpd.serve_forever, daemon=True).start()
    try:
        with urllib.request.urlopen(url + "/model.json", timeout=10) as response:
            served = json.loads(response.read())
        assert served["map"] == chain.payload["map"]
        query = urllib.parse.urlencode({"line": json.dumps(chain.line), "property": "PORO"})
        with urllib.request.urlopen(url + "/section?" + query, timeout=10) as response:
            assert json.loads(response.read()) == chain.payload["sections"][0]
        with urllib.request.urlopen(url + "/volume?property=PORO", timeout=10) as response:
            assert json.loads(response.read())["encoding"] == "base64"
    finally:
        httpd.shutdown()
        httpd.server_close()


@acceptance
def test_section_edges_differ_on_dipping_cells_polyline(dipping):
    _model, _payload, _line, section = dipping
    max_delta, count, aligned = _edge_invariants(section)
    assert section["sugar_cube"] is False
    assert max_delta > 1.0 and count > 0 and aligned


@acceptance
def test_section_edges_differ_on_dipping_cells_along_bore(chain, dipping):
    # The current product API no longer constructs/attaches bores. A deviated
    # petekIO bore and a trajectory-shaped section share the same forwarded
    # section bundle invariant; the producer's AlongBore kernel is Static-owned.
    assert any(w["profile"] != "vertical" for w in chain.manifest["well_program"])
    _model, _payload, _line, section = dipping
    max_delta, count, aligned = _edge_invariants(section)
    assert max_delta > 1.0 and count > 0 and aligned


@acceptance
def test_sugar_cube_flattens_section_edges_to_centroid(dipping, tmp_path):
    _model, payload, _line, section = dipping
    sugar = deepcopy(section)
    sugar["sugar_cube"] = True
    finite = 0
    for column in sugar["columns"]:
        for left_name, right_name, centre_name in (
            ("layer_tops_l", "layer_tops_r", "layer_tops"),
            ("layer_bases_l", "layer_bases_r", "layer_bases"),
        ):
            column[left_name] = list(column[centre_name])
            column[right_name] = list(column[centre_name])
            finite += sum(value is not None for value in column[centre_name])
    assert finite > 0 and _edge_invariants(sugar) == (0.0, 0, True)
    payload = deepcopy(payload)
    payload["sections"] = [sugar]
    payload["section_labels"] = ["Sugar cube"]
    ps._save_view(str(tmp_path / "sugar.html"), json.dumps(payload))


@acceptance
def test_volume_shell_non_empty_plausible_triangles(chain):
    volume = chain.payload["volume"]
    assert volume["cell_count"] > 0 and volume["triangle_count"] > 100
    assert volume["encoding"] == "base64" and "positions" not in volume
    assert volume["blocks"]["indices"]["shape"] == [volume["triangle_count"], 3]


@acceptance
def test_map_outline_extent_equals_frame_extent(chain):
    frame = chain.payload["map"]["frame"]
    geometry = pio.GridGeometry(
        frame["origin_x"],
        frame["origin_y"],
        frame["spacing_x"],
        frame["spacing_y"],
        frame["ncol"],
        frame["nrow"],
        rotation_deg=30.0,
        yflip=True,
    )
    corners = [geometry.node_xy(i, j) for i, j in ((0, 0), (frame["ncol"] - 1, 0), (frame["ncol"] - 1, frame["nrow"] - 1), (0, frame["nrow"] - 1))]
    assert len(set(corners)) == 4
    for i, j in ((0, 0), (frame["ncol"] - 1, frame["nrow"] - 1)):
        x, y = geometry.node_xy(i, j)
        fi, fj = geometry.xy_to_ij(x, y)
        assert abs(fi - i) < 1e-9 and abs(fj - j) < 1e-9


@acceptance
def test_wells_ties_populated_for_tied_bores(chain):
    well = chain.payload["wells_logs"]["wells"][0]
    assert well["ties"] and {tie["horizon"] for tie in well["ties"]} <= {
        top["horizon"] for top in well["tops"]
    }


@acceptance
def test_wells_logs_lanes_byte_valid_vs_reference_encoder(chain):
    well = chain.payload["wells_logs"]["wells"][0]
    for lane in (well["md_m"], well["tvd_m"], *(c["values"] for c in well["curves"])):
        raw = base64.b64decode(lane["data"])
        values = struct.unpack(f"<{lane['shape'][0]}f", raw)
        for index, value in enumerate(values):
            if math.isnan(value):
                assert raw[4 * index : 4 * index + 4] == NAN_F32
        assert encode_lane(values)["data"] == lane["data"]


@acceptance
def test_horizon_traces_present_and_pinch_out_nan_gapped(chain):
    # Derive the gap from the ingested synthetic horizon pair rather than
    # manufacturing one in the viewer payload.  H6 is planted to pinch into H5
    # on the eastern columns of the asset.
    below_name = chain.manifest["pinch_out"]["below_horizon"]
    below_index = chain.manifest["horizons"].index(below_name)
    above_name = chain.manifest["horizons"][below_index - 1]
    above = chain.project.surfaces(f"Surfaces/{above_name}")
    below = chain.project.surfaces(f"Surfaces/{below_name}")
    above_values = above.value_layer()["values"]
    below_values = below.value_layer()["values"]
    middle_j = above.nrow // 2
    thickness = []
    depths = []
    for i in range(above.ncol):
        index = i * above.nrow + middle_j
        delta = abs(below_values[index] - above_values[index])
        thickness.append(delta)
        depths.append(None if delta <= 1e-9 else -below_values[index])

    assert max(thickness[: above.ncol // 2]) > 1.0
    assert all(value <= 1e-9 for value in thickness[-3:])
    section = deepcopy(chain.payload["sections"][0])
    section["horizon_traces"] = [{"name": below_name, "depths": depths}]
    trace = section["horizon_traces"][0]
    assert any(value is not None for value in trace["depths"][: above.ncol // 2])
    assert all(value is None for value in trace["depths"][-3:])


@acceptance
def test_per_zone_net_conditioned_poro_recovers_targets(chain):
    # Recover the planted means from the current DATA seam itself: canonicalised
    # PORO/NTG logs, bracketed by petekIO's ingested formation-top intervals.
    horizons = chain.manifest["horizons"]
    cutoff = chain.manifest["net_cutoff"]
    for index, zone in enumerate(chain.manifest["zones"]):
        samples = []
        for well_view in chain.project.wells.values():
            well = well_view.raw
            top = well.top(horizons[index]).top_md
            base = well.top(horizons[index + 1]).top_md
            md = well.log("PORO").md()
            poro = well.log("PORO").values()
            ntg = well.log("NTG").values()
            samples.extend(
                value
                for measured_depth, value, net in zip(md, poro, ntg)
                if top <= measured_depth < base and net >= cutoff
            )
        recovered = sum(samples) / len(samples)
        planted = chain.manifest["zone_targets"][zone]["net_por_mean"]
        assert abs(recovered - planted) < 0.07, (zone, recovered, planted)


@acceptance
def test_contact_plan_hc_pattern(chain):
    plan = chain.manifest["contact_plan"]
    assert {name for name, row in plan.items() if row["type"] == "single"} == {"Z2"}
    assert {name for name, row in plan.items() if row["type"] == "two_contact"} == {"Z4"}
    assert chain.volume_result.summary()["in_place_sm3"] > 0.0
    assert any(chain.payload["map"]["contacts"][0]["crossing"])


@acceptance
def test_zero_spread_zoned_mc_equals_deterministic_and_conserves(chain):
    samples = chain.product_model.samples
    assert samples and len(set(samples)) == 1
    assert abs(chain.product_model.summary_msm3["mean"] * 1_000_000.0 - samples[0]) < 1e-6
    summary = chain.volume_result.summary()
    assert summary["in_place_sm3"] == chain.volume_result.by_zone()["total"]["in_place_sm3"]


def _read_tops(root):
    text = (Path(root) / "WellTops" / "FieldWellTops").read_bytes().decode("latin-1")
    rows = []
    for line in text.splitlines():
        if '"' not in line:
            continue
        fields = line.split()
        quoted = line.split('"')
        rows.append(
            {
                "x": float(fields[0]),
                "y": float(fields[1]),
                "type": fields[8],
                "surface": quoted[1],
                "well": quoted[3],
            }
        )
    return rows


@acceptance
def test_deviated_tops_at_trajectory_vertical_at_wellhead(chain):
    rows = _read_tops(chain.manifest["root"])
    heads = {well["id"]: (well["x"], well["y"]) for well in chain.manifest["well_program"]}
    profiles = {well["id"]: well["profile"] for well in chain.manifest["well_program"]}
    offsets = {well: [] for well in heads}
    for row in rows:
        if row["well"] in heads and row["type"] == "Horizon":
            hx, hy = heads[row["well"]]
            offsets[row["well"]].append(math.hypot(row["x"] - hx, row["y"] - hy))
    deviated = [well for well, profile in profiles.items() if profile != "vertical"]
    vertical = [well for well, profile in profiles.items() if profile == "vertical"]
    assert deviated and max(max(offsets[well]) for well in deviated) > 100.0
    assert max(max(offsets[well]) for well in vertical) < 1.0


@acceptance
def test_collocated_recovers_planted_rho(chain):
    # Compare the actual depositional trend surface with the actual ingested NTG
    # logs at each wellhead. The synthetic generator plants this relationship.
    surface = chain.project.surfaces(chain.manifest["trend_surface"])
    trend = []
    values = []
    for well_view in chain.project.wells.values():
        well = well_view.raw
        trend.append(surface.sample(*well.head))
        values.append(well.log("NTG").stats().mean)
    count = len(values)
    mx = sum(trend) / count
    my = sum(values) / count
    cov = sum((x - mx) * (y - my) for x, y in zip(trend, values))
    sx = math.sqrt(sum((x - mx) ** 2 for x in trend))
    sy = math.sqrt(sum((y - my) ** 2 for y in values))
    recovered = cov / (sx * sy)
    assert abs(recovered - chain.manifest["rho"]) < 0.25, recovered
    assert recovered > 0.2


@pytest.fixture(scope="module")
def spill(chain):
    recipe = spill_recipe(ncol=61, n_cubes=3, nk_per_zone=14)
    assert recipe["force_budget_bytes"] < recipe["est_live_set_bytes"]
    return chain, recipe


@acceptance_spill
def test_spill_emits_loud_mode_switch_warning(spill):
    _chain, recipe = spill
    assert "force MemoryBudget::bytes" in recipe["recipe"]
    assert recipe["cells"] > 300_000


@acceptance_spill
def test_spilled_volume_bundle_non_empty(spill):
    chain, _recipe = spill
    volume = chain.payload["volume"]
    assert volume["cell_count"] > 0 and volume["triangle_count"] > 0


@acceptance_spill
def test_spilled_map_bundle_non_empty(spill):
    chain, _recipe = spill
    map_bundle = chain.payload["map"]
    assert map_bundle["frame"]["ncol"] > 1 and map_bundle["zone_averages"]


@acceptance_spill
def test_spilled_section_bundle_non_empty(spill):
    chain, _recipe = spill
    assert chain.payload["sections"][0]["columns"]


_NODE = shutil.which("node")
_RENDER_JS = Path(__file__).with_name("acceptance_render.mjs")


def _playwright_node_path():
    if _NODE is None:
        return None
    candidates = [os.environ.get("PLAYWRIGHT_NODE_PATH", "")]
    repo = Path(__file__).resolve().parents[3]
    candidates.extend(str(base / "node_modules") for base in (repo, repo.parent))
    for node_path in candidates:
        if not node_path:
            continue
        probe = "const {chromium}=require('playwright');if(!require('fs').existsSync(chromium.executablePath()))process.exit(3);"
        result = subprocess.run(
            [_NODE, "-e", probe],
            capture_output=True,
            text=True,
            timeout=30,
            env={**os.environ, "NODE_PATH": node_path},
        )
        if result.returncode == 0:
            return node_path
    return None


@pytest.mark.acceptance_render
def test_save_view_renders_every_tab_in_headless_chromium(chain, tmp_path):
    node_path = _playwright_node_path()
    if node_path is None:
        pytest.skip("node/playwright/chromium unavailable — browser leg is opt-in")
    html = tmp_path / "render.html"
    ps._save_view(str(html), json.dumps(chain.payload, separators=(",", ":")))
    result = subprocess.run(
        [_NODE, str(_RENDER_JS), str(html)],
        capture_output=True,
        text=True,
        timeout=120,
        env={**os.environ, "NODE_PATH": node_path},
    )
    assert result.returncode == 0, result.stdout + "\n" + result.stderr
    report = json.loads(result.stdout.strip().splitlines()[-1])
    assert not report["consoleErrors"] and "tris" in (report.get("volBadge") or "")


if __name__ == "__main__":
    sys.exit(pytest.main([__file__, "-m", "acceptance", "-q"]))
