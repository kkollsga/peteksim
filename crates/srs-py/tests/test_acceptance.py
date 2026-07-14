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
from dataclasses import dataclass
from pathlib import Path

import pytest

import petekio as pio
import peteksim as ps
from petektools.viewer._wells import encode_lane


SEED = 20_260_704
NCOL = 11
NAN_F32 = struct.pack("<I", 0x7FC00000)
acceptance = pytest.mark.acceptance
acceptance_spill = pytest.mark.acceptance_spill


@dataclass
class CurrentSeam:
    manifest: dict
    project: object
    exact: dict | None
    wells_logs: dict
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


def _exact(chain: CurrentSeam) -> dict:
    if chain.exact is None:
        pytest.skip(
            "requires the exact-source viewer-schema-acceptance wheel; "
            "see CONTRIBUTING.md"
        )
    return chain.exact


@pytest.fixture(scope="module")
def chain(tmp_path_factory) -> CurrentSeam:
    root = tmp_path_factory.mktemp("current-seam")
    manifest = ps.synth_asset(root, seed=SEED, n_wells=8, ncol=NCOL)
    project = pio.Project.import_data(
        manifest["root"], crs=manifest["crs"], aliases=manifest["aliases"]
    )
    # The exact-source test wheel exposes one coherent typed StaticModel
    # composition root. petekStatic's public Python Grid workflow currently has
    # no lowering/export into that Rust type, so the already-imported IO project
    # enters honestly as explicit provenance rather than a false object hand-off.
    import peteksim._core as core

    helper = getattr(core, "_viewer_acceptance", None)
    inputs_ref = f"petekio-synth:{Path(manifest['root']).name}:seed-{SEED}"
    exact = helper(inputs_ref) if helper is not None else None
    payload = exact["payload"] if exact is not None else {}
    line = (
        [[column["x"], column["y"]] for column in payload["sections"][1]["columns"]]
        if exact is not None
        else []
    )
    first_well = next(iter(project.wells.values())).raw
    wells_logs = first_well.view(serve=False, tops=True).bundle()
    return CurrentSeam(
        manifest,
        project,
        exact,
        wells_logs,
        payload,
        line,
    )


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
    exact = _exact(chain)
    inv = chain.project.inventory()
    assert inv["counts"]["surfaces"] and inv["counts"]["wells"] == 8
    assert exact["inputs_ref"].startswith("petekio-synth:")
    refs = {
        chain.payload["map"]["inputs_ref"],
        chain.payload["sections"][0]["inputs_ref"],
        chain.payload["volume"]["inputs_ref"],
    }
    assert refs == {exact["inputs_ref"]}
    assert chain.payload["map"]["horizons"] and chain.payload["sections"][0]["columns"]
    assert chain.payload["volume"]["encoding"] == "base64"
    assert chain.payload["map"]["wells"]

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
def test_section_edges_differ_on_dipping_cells_polyline(chain):
    section = _exact(chain)["payload"]["sections"][1]
    max_delta, count, aligned = _edge_invariants(section)
    assert section["sugar_cube"] is False
    assert max_delta > 1.0 and count > 0 and aligned


@acceptance
def test_section_edges_differ_on_dipping_cells_along_bore(chain):
    section = _exact(chain)["payload"]["sections"][0]
    max_delta, count, aligned = _edge_invariants(section)
    assert max_delta > 0.1 and count > 0 and aligned
    path_z = [column["path_z"] for column in section["columns"]]
    assert all(value is not None for value in path_z)
    assert path_z[0] < path_z[-1]


@acceptance
def test_sugar_cube_flattens_section_edges_to_centroid(chain, tmp_path):
    exact = _exact(chain)
    dipping = exact["payload"]["sections"][1]
    sugar = exact["sugar_section"]
    assert dipping["sugar_cube"] is False and _edge_invariants(dipping)[0] > 0.1
    assert sugar["sugar_cube"] is True
    assert _edge_invariants(sugar) == (0.0, 0, True)
    payload = dict(exact["payload"])
    payload["sections"] = [sugar]
    payload["section_labels"] = ["Sugar cube"]
    ps._save_view(str(tmp_path / "sugar.html"), json.dumps(payload))


@acceptance
def test_volume_shell_non_empty_plausible_triangles(chain):
    _exact(chain)
    volume = chain.payload["volume"]
    assert volume["cell_count"] > 0 and volume["triangle_count"] > 100
    assert volume["encoding"] == "base64" and "positions" not in volume
    assert volume["blocks"]["indices"]["shape"] == [volume["triangle_count"], 3]


@acceptance
def test_map_outline_extent_equals_frame_extent(chain):
    _exact(chain)
    frame = chain.payload["map"]["frame"]
    theta = math.radians(frame["rotation_deg"])
    cos_t, sin_t = math.cos(theta), math.sin(theta)

    def world(fi, fj):
        u = fi * frame["spacing_x"]
        v = fj * frame["spacing_y"] * (-1.0 if frame["yflip"] else 1.0)
        return [
            frame["origin_x"] + cos_t * u - sin_t * v,
            frame["origin_y"] + sin_t * u + cos_t * v,
        ]

    expected = [world(i, j) for i, j in (
        (-0.5, -0.5), (frame["ncol"] - 0.5, -0.5),
        (frame["ncol"] - 0.5, frame["nrow"] - 0.5),
        (-0.5, frame["nrow"] - 0.5),
    )]
    outline = chain.payload["map"]["outline"][0]
    for actual, wanted in zip(outline[:4], expected):
        assert math.dist(actual, wanted) < 1e-8


@acceptance
def test_wells_ties_populated_for_tied_bores(chain):
    well = _exact(chain)["payload"]["map"]["wells"][0]
    assert well["id"] == "SYNTH-01"
    assert {tie["horizon"] for tie in well["ties"]} == {"H1", "H2"}
    assert well["tie_residual_m"] is not None


@acceptance
def test_wells_logs_lanes_byte_valid_vs_reference_encoder(chain):
    well = chain.wells_logs["wells"][0]
    for lane in (well["md_m"], well["tvd_m"], *(c["values"] for c in well["curves"])):
        raw = base64.b64decode(lane["data"])
        values = struct.unpack(f"<{lane['shape'][0]}f", raw)
        for index, value in enumerate(values):
            if math.isnan(value):
                assert raw[4 * index : 4 * index + 4] == NAN_F32
        assert encode_lane(values)["data"] == lane["data"]


@acceptance
def test_horizon_traces_present_and_pinch_out_nan_gapped(chain):
    section = _exact(chain)["payload"]["sections"][1]
    assert [trace["name"] for trace in section["horizon_traces"]] == ["H1", "H2"]
    assert all(
        len(trace["depths"]) == len(section["columns"])
        for trace in section["horizon_traces"]
    )
    # Producer contract: horizon polylines remain continuous; collapsed/pinched
    # layers carry the JSON null gaps.
    assert all(value is not None for trace in section["horizon_traces"] for value in trace["depths"])
    assert any(value is None for column in section["columns"] for value in column["layer_tops"])


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
    exact = _exact(chain)
    zones = exact["deterministic_zones"]["zones"]
    by_name = {zone["name"]: zone for zone in zones}
    assert by_name["CONTACTLESS"]["hcpv_m3"] == 0.0
    assert by_name["SINGLE"]["hcpv_m3"] > 0.0
    assert not by_name["SINGLE"]["has_gas_leg"]
    assert by_name["TWO_CONTACT"]["has_gas_leg"]
    assert by_name["TWO_CONTACT"]["has_oil_leg"]
    assert len(exact["payload"]["map"]["contacts"]) == 3


@acceptance
def test_zero_spread_zoned_mc_equals_deterministic_and_conserves(chain):
    exact = _exact(chain)
    deterministic = exact["deterministic_zones"]
    realized = exact["realized_zones"]
    for expected, actual in zip(deterministic["zones"], realized["zones"]):
        assert expected["name"] == actual["name"]
        assert math.isclose(expected["grv_m3"], actual["grv_m3"], rel_tol=1e-9)
        assert math.isclose(expected["hcpv_m3"], actual["hcpv_m3"], rel_tol=1e-9)
    for result in (deterministic, realized):
        assert math.isclose(
            result["total"]["grv_m3"],
            sum(zone["grv_m3"] for zone in result["zones"]), rel_tol=1e-9,
        )
        assert math.isclose(
            result["total"]["hcpv_m3"],
            sum(zone["hcpv_m3"] for zone in result["zones"]), rel_tol=1e-9,
        )


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
def spill():
    # The helper is compiled only into the coordinator's exact-source acceptance
    # wheel. Ordinary released-floor wheels remain standalone and skip this
    # opt-in leg rather than relabelling an in-core payload as spilled.
    import peteksim._core as core

    helper = getattr(core, "_spilled_viewer_acceptance", None)
    if helper is None:
        pytest.skip(
            "requires the exact-source viewer-schema-acceptance wheel; "
            "see CONTRIBUTING.md"
        )
    artifact = helper()
    assert artifact["is_spilled"] is True
    assert artifact["mode"] == "out-of-core"
    assert artifact["budget_bytes"] == 1_024
    assert artifact["estimate_bytes"] > artifact["budget_bytes"]
    return artifact


@acceptance_spill
def test_spill_emits_loud_mode_switch_warning(spill):
    assert "OUT-OF-CORE mode" in spill["notice"]
    assert "exceeds budget" in spill["notice"]
    assert "spilling geometry + cubes" in spill["notice"]
    assert spill["cells"] == 24 * 24 * 10


@acceptance_spill
def test_spilled_volume_bundle_non_empty(spill):
    volume = spill["volume"]
    assert volume["cell_count"] > 0 and volume["triangle_count"] > 0


@acceptance_spill
def test_spilled_map_bundle_non_empty(spill):
    map_bundle = spill["map"]
    assert map_bundle["frame"]["ncol"] > 1 and map_bundle["zone_averages"]


@acceptance_spill
def test_spilled_section_bundle_non_empty(spill):
    section = spill["section"]
    assert section["columns"]
    assert section["frame"] == spill["map"]["frame"]


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
    _exact(chain)
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
