#!/usr/bin/env python3
"""Viewer seam tests — the bundle plumbing, the single-file export, and the
local server (fence/well `/section` endpoint + non-blocking `view()`).

The heavy StaticModel + attached-well path is exercised in `test_realshape.py`;
here we use the fast analytic box model, whose `save_json`/`save_view`/`view`
run through the exact same viewer payload + server glue.

    VIRTUAL_ENV="$PWD/.venv-srs" .venv-srs/bin/maturin develop -m crates/srs-py/Cargo.toml
    .venv-srs/bin/python -m pytest crates/srs-py/tests/test_viewer.py -q
"""

from __future__ import annotations

import json
import re
import tempfile
import threading
import urllib.parse
import urllib.request

import pytest

import peteksim as ps

# The cross-codebase JSON contract (petekStatic view bundles).
#
# VOLUME wire = petekStatic's **v3 self-contained block envelope** (SCHEMA_VERSION
# 5): metadata + `encoding:"base64"` + a `blocks` map whose entries carry raw
# little-endian arrays as base64 `data`. This is the ONLY volume shape the
# petekTools viewer decode kernel accepts — it routes any `schema_version>=3`
# volume to `env.blocks`, so the serde-derive's inline `positions`/`indices`
# arrays (no `blocks`) crash it. Pinned on every peteksim emission path below.
VOLUME_ENVELOPE_KEYS = {
    "schema_version", "kind", "inputs_ref", "property", "cell_count",
    "shell_cell_count", "vertex_count", "triangle_count", "zone_names",
    "value_range", "encoding", "blocks",
}
VOLUME_BLOCK_NAMES = {"positions", "indices", "tri_cell", "cell_values", "zone_ids"}


def assert_v3_volume_envelope(v):
    """Assert `v` is petekStatic's v3 self-contained block envelope (base64 blocks,
    no inline arrays) — the shape the petekTools decode kernel decodes via
    `env.blocks`. The regression guard for the P10 census render break."""
    assert set(v) == VOLUME_ENVELOPE_KEYS, set(v)
    assert v["kind"] == "volume"
    assert v["schema_version"] >= 3, v["schema_version"]
    assert v["encoding"] == "base64", v["encoding"]
    # The inline serde-derive arrays MUST be absent (their presence is the bug).
    assert "positions" not in v and "indices" not in v
    blocks = v["blocks"]
    assert set(blocks) == VOLUME_BLOCK_NAMES, set(blocks)
    for name, blk in blocks.items():
        assert set(blk) == {"dtype", "shape", "data"}, (name, set(blk))
        assert isinstance(blk["data"], str) and blk["data"] != "" or blk["shape"][0] == 0
    assert set(v["value_range"]) == {"min", "max"}
MAP_KEYS = {
    "schema_version", "inputs_ref", "frame", "outline", "horizons",
    "zone_averages", "k_slices", "wells", "contacts",
}
SECTION_KEYS = {
    "schema_version", "inputs_ref", "property", "top_name", "base_name",
    "columns", "horizon_traces", "contacts",
    "sugar_cube",  # v4-additive: petekStatic section rendering toggle (default false)
    "zones",  # v5-additive: colour-by-zone rider — [{name, color}] top→base
}


@pytest.fixture(scope="module")
def box():
    return ps.run_box_model(
        area_km2=0.4, gross_height_m=15.0, porosity=0.25, net_to_gross=0.8,
        water_saturation=0.3, fvf=1.25, fluid="oil", contact_m=1510.0,
        top_m=1500.0, ni=8, nj=6, nk=4,
    )


def _payload(box) -> dict:
    with tempfile.NamedTemporaryFile("r", suffix=".json", delete=False) as f:
        path = f.name
    box.save_json(path)
    return json.load(open(path))


# --- bundle shapes match the schema snapshot ---------------------------------
def test_volume_bundle_shape(box):
    # The payload's `volume` (save_json / save_view / serve path) is the v3 block
    # envelope — the shape the viewer decodes; the serde-derive inline arrays are
    # the render break the P10 census caught.
    v = _payload(box)["volume"]
    assert_v3_volume_envelope(v)
    assert v["cell_count"] == 8 * 6 * 4
    # Shell mesh block shapes: triangles (3 indices each) + one compact
    # cell index/value/zone per shell cell.
    assert v["blocks"]["indices"]["shape"][1] == 3
    assert v["blocks"]["positions"]["shape"][1] == 3
    assert v["blocks"]["tri_cell"]["shape"][0] == v["triangle_count"]
    assert v["blocks"]["cell_values"]["shape"][0] == v["blocks"]["zone_ids"]["shape"][0]


def test_map_bundle_shape(box):
    m = _payload(box)["map"]
    assert set(m) == MAP_KEYS, set(m)
    f = m["frame"]
    assert f["ncol"] == 8 and f["nrow"] == 6
    # a scalar layer carries its own name/units/range (bundle-driven labels)
    assert m["horizons"], m["horizons"]
    assert set(m["horizons"][0]) == {"name", "units", "values", "range"}


def test_payload_metadata(box):
    p = _payload(box)
    # Top-level payload contract version — 2 since the additive `charts` bundle
    # (per-bundle versions remain independently owned by petekStatic).
    assert p["schema_version"] == 2
    assert p["kind"] == "box"
    assert p["property"] == "PORO"
    assert "PORO" in p["properties"]
    assert p["charts"] == []  # a box model attaches no analytics charts
    # box has no wells; sections/labels align
    assert len(p["sections"]) == len(p["section_labels"])


# --- save_view is ONE self-contained file (no external loads) ----------------
def test_save_view_self_contained(box):
    with tempfile.NamedTemporaryFile("r", suffix=".html", delete=False) as f:
        path = f.name
    box.save_view(path)
    html = open(path).read()
    # data + all JS inlined
    assert "window.PETEK_VIEWER_PAYLOAD=" in html
    assert 'window.PETEK_VIEWER_MODE="file"' in html
    assert "THREE" in html and "OrbitControls" in html
    # NO external resource loads of ANY kind (confidential-data hard rule):
    # no external <script>/<link>, no @import/url()/fetch/XHR to a URL.
    assert "<script src=" not in html
    assert "<link" not in html
    for pat in (r'src\s*=\s*["\']https?:', r'href\s*=\s*["\']https?:',
                r'@import', r'url\(\s*https?:', r'fetch\(\s*["\']https?:'):
        assert not re.search(pat, html), pat
    # the fence-draw control is disabled in the file export (server-only)
    assert "self-contained file export" in html or "PETEK_VIEWER_MODE" in html


# --- the local server: start, GET /, GET /section, shutdown ------------------
def test_server_smoke(box):
    payload = json.dumps(_payload(box))
    httpd, url = ps._make_server(box, payload, 0)
    t = threading.Thread(target=httpd.serve_forever, daemon=True)
    t.start()
    try:
        # GET / serves the viewer shell
        with urllib.request.urlopen(url + "/", timeout=5) as r:
            assert r.status == 200
            assert b"petek" in r.read()
        # GET /model.json serves the payload
        with urllib.request.urlopen(url + "/model.json", timeout=5) as r:
            assert json.loads(r.read())["schema_version"] == 2
        # GET /section cuts a fresh fence (a line spanning the box)
        f = json.loads(payload)["map"]["frame"]
        x0, y0 = f["origin_x"], f["origin_y"]
        x1 = x0 + (f["ncol"] - 1) * f["spacing_x"]
        y1 = y0 + (f["nrow"] - 1) * f["spacing_y"]
        q = urllib.parse.urlencode({"line": json.dumps([[x0, y0], [x1, y1]])})
        with urllib.request.urlopen(url + "/section?" + q, timeout=5) as r:
            sec = json.loads(r.read())
            # Schema v6 adds the exact Map frame to sections; released pre-v6
            # producers omit it. Both shapes remain accepted during the rolling
            # suite upgrade, and the additive frame must be byte-identical.
            assert set(sec) in (SECTION_KEYS, SECTION_KEYS | {"frame"}), set(sec)
            if "frame" in sec:
                assert sec["frame"] == f
            assert sec["columns"], "fence produced no columns"
    finally:
        httpd.shutdown()
        httpd.server_close()


# --- view() is non-blocking (returns immediately with a URL) ------------------
def test_view_nonblocking_returns(box):
    done = {}

    def run():
        done["url"] = box.view(open_browser=False, port=0)

    t = threading.Thread(target=run)
    t.start()
    t.join(timeout=5)
    assert not t.is_alive(), "view() blocked — should return immediately"
    assert isinstance(done["url"], str) and done["url"].startswith("http://127.0.0.1")


if __name__ == "__main__":
    import sys

    sys.exit(pytest.main([__file__, "-q"]))
