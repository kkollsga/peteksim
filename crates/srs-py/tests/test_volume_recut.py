#!/usr/bin/env python3
"""Live `/volume` re-cut endpoint — the server-side threshold recut that mirrors
the `/section` provider (coordinator wiring 2026-07-04).

A served session's `/volume?property=..&cutoff=..` calls peteksim's
`_volume_provider` -> `model._volume_json` -> `StaticModel::volume_bundle_thresholded`
and returns a v3 exterior-shell envelope. Re-cutting at a mid-range cutoff exposes
interior faces, so the recut shell differs from the full-set shell.

    VIRTUAL_ENV="$PWD/.venv-srs" .venv-srs/bin/maturin develop -m crates/srs-py/Cargo.toml
    .venv-srs/bin/python -m pytest crates/srs-py/tests/test_volume_recut.py -q
"""

from __future__ import annotations

import json
import sys
import tempfile
import threading
import urllib.request
from pathlib import Path

import pytest

REPO = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(REPO / "examples"))

import peteksim as ps  # noqa: E402
from synthetic_tree import build_tree  # noqa: E402


@pytest.fixture(scope="module")
def model():
    tree = build_tree(tempfile.mkdtemp(prefix="volrecut-test-"))
    proj = ps.Project.load(str(tree), crs="SYNTHETIC")
    fw = proj.framework(horizons=["TopReservoir", "BaseReservoir"], outline="outline")
    grid = fw.build_grid()
    por = grid.property("PORO")
    por.upscale(proj.wells())
    # The synthetic wells don't condition every simulated layer; opt into the
    # (structureless) mean-fill for the data-less layer (petekStatic's default is a
    # loud named error).
    por.propagate(ps.gaussian(ps.spherical(range_m=800.0), seed=1),
                  allow_mean_fill=True)
    return grid.model(contacts={"fwl": 2035.0}, fluid="oil", fvf=1.30)


def _get_json(url: str):
    with urllib.request.urlopen(url, timeout=10) as r:  # noqa: S310 (localhost only)
        assert r.status == 200
        return json.loads(r.read())


def test_direct_thresholded_volume_differs_from_full(model):
    full = json.loads(model._volume_json(property="PORO"))
    assert full["schema_version"] >= 3
    lo, hi = full["value_range"]["min"], full["value_range"]["max"]
    cut = json.loads(
        model._volume_json(property="PORO", cutoff=(lo + hi) / 2.0, keep_above=True)
    )
    assert cut["schema_version"] >= 3
    # Both are v3 self-contained block envelopes (base64 blocks, no inline arrays) —
    # the shape the petekTools viewer decode kernel reads via `env.blocks`.
    assert cut["encoding"] == "base64" and "positions" not in cut
    assert set(cut["blocks"]) == {"positions", "indices", "tri_cell", "cell_values", "zone_ids"}
    # `cell_count` is the fixed full-grid extent; the EXTERIOR SHELL is what a re-cut
    # rebuilds — dropping cells exposes new interior faces, so the shell
    # vertex/triangle counts change.
    assert cut["triangle_count"] != full["triangle_count"], "shell unchanged by recut"
    assert cut["vertex_count"] != full["vertex_count"]


def test_served_volume_endpoint_recut(model):
    # A real served session: GET /volume with a cutoff routes through
    # _volume_provider -> model._volume_json and returns a valid v3 envelope whose
    # shell differs from the full-set shell. The served payload body is irrelevant
    # to the /volume endpoint (it calls the provider), so a minimal one suffices.
    httpd, url = ps._make_server(model, "{}", 0)
    threading.Thread(target=httpd.serve_forever, daemon=True).start()
    try:
        full = _get_json(f"{url}/volume?property=PORO")
        assert full["schema_version"] >= 3
        mid = (full["value_range"]["min"] + full["value_range"]["max"]) / 2.0
        cut = _get_json(f"{url}/volume?property=PORO&cutoff={mid}&keep_above=true")
        assert cut["schema_version"] >= 3 and cut["encoding"] == "base64"
        assert "blocks" in cut and cut["triangle_count"] != full["triangle_count"], "shell unchanged"
        # Below-cutoff complement re-cut is also a valid, distinct shell.
        below = _get_json(f"{url}/volume?property=PORO&cutoff={mid}&keep_above=false")
        assert below["schema_version"] >= 3 and "blocks" in below
        assert below["triangle_count"] != full["triangle_count"]
    finally:
        httpd.shutdown()
        httpd.server_close()
