#!/usr/bin/env python3
"""Acceptance for the peteksim `wells_logs` producer — the viewer's **Wells** tab
fed from a LOADED PROJECT + its populated model (task_peteksim_wells_payload).

Proves the model-context producer slice of the well-correlation seam
(`petekSuite/dev-docs/designs/well-log-bundle-seam.md`, codified in
`petektools/viewer/SCHEMA.md`):

  * `model.wells_bundle()` assembles a `WellLogBundle` (kind `wells_logs`, v4)
    from the synthetic asset — per-bore md/tvd lanes, RAW + UPSCALED curves,
    framework tops (top→down) + zone bands, and tie residuals from
    `Provenance.well_ties` (`model.well_tie_residuals()`);
  * the wire matches the petektools REFERENCE FIXTURE structure (same keys, same
    v3 f32 base64 lane encoding — the encoder is byte-identical to the fixture's
    `encode_lane`, NaN = the canonical 0x7FC00000);
  * the Wells tab lights up both in a SERVED session (payload `/model.json`) and
    in a self-contained EXPORT (`save_view`).

    VIRTUAL_ENV="$PWD/.venv-srs" .venv-srs/bin/maturin develop -m crates/srs-py/Cargo.toml
    .venv-srs/bin/python -m pytest crates/srs-py/tests/test_wells_payload.py -q
"""

from __future__ import annotations

import base64
import json
import struct
import sys
import tempfile
import urllib.request
from pathlib import Path

import pytest

pytestmark = pytest.mark.skip(
    reason="legacy petekSim Project facade removed; petekIO owns project loading"
)

REPO = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(REPO / "examples"))

import peteksim as ps  # noqa: E402
from petektools.viewer._wells import (  # noqa: E402  (the reference fixture)
    build_well_log_bundle as ref_build,
    encode_lane as ref_encode_lane,
)

SEED = 20_260_704
NAN_F32 = struct.pack("<I", 0x7FC00000)


@pytest.fixture(scope="module")
def model_with_wells():
    man = ps.synth_asset(tempfile.mkdtemp(prefix="wells-payload-"), seed=SEED, ncol=21)
    proj = ps.Project.load(man["root"], crs=man["crs"], aliases=man["aliases"])
    fw = proj.framework(horizons=man["horizons"], outline="ModelEdge",
                        tie_to_tops=True, min_thickness_m=0.0)
    fw.set_zonation([dict(z, nk=2) for z in man["zonation"]])
    # Engine well ties keyed on each REAL bore head (so the residuals attach to the
    # wells the Wells tab shows) — a small measured-top ramp per horizon.
    heads = proj.wells().heads()
    measured = {h: 2000.0 + 12.0 * k for k, h in enumerate(man["horizons"])}
    fw.set_well_ties([{"id": wid, "x": x, "y": y, "tops": dict(measured)}
                      for (wid, x, y) in heads])
    grid = fw.build_grid()
    por = grid.property("PORO")
    por.upscale(proj.wells(), net_only=True)
    por.propagate(ps.gaussian(ps.spherical(range_m=1500.0), seed=11))
    ntg = grid.property("NTG")
    ntg.upscale(proj.wells())
    ntg.propagate(ps.gaussian(ps.spherical(range_m=1500.0), seed=12))
    model = grid.model(fluid="oil", fvf=1.30, gas_fvf=0.005, wells=proj.wells())
    return man, model


def _lane_floats(lane: dict) -> list[float]:
    raw = base64.b64decode(lane["data"])
    n = lane["shape"][0]
    assert len(raw) == 4 * n, (len(raw), n)
    return list(struct.unpack(f"<{n}f", raw))


# --- (1) the bundle assembles from the loaded project + model ----------------
def test_wells_bundle_shape_and_content(model_with_wells):
    man, model = model_with_wells
    b = model.wells_bundle()
    assert b is not None, "a model with attached bores must carry a wells_logs bundle"
    assert b["kind"] == "wells_logs" and b["schema_version"] == 4
    assert b["flatten_default"] in man["horizons"]
    assert b["wells"], "wells populated from the loaded project"

    for w in b["wells"]:
        assert set(w) >= {"id", "display_name", "x", "y", "datum_m",
                          "md_m", "tvd_m", "curves", "tops", "zones"}
        n = w["md_m"]["shape"][0]
        assert n >= 2 and w["tvd_m"]["shape"][0] == n
        assert w["md_m"]["dtype"] == "f32" and set(w["md_m"]) == {"dtype", "shape", "data"}
        # every curve lane is sampled on md_m (same length)
        mnems = {c["mnemonic"] for c in w["curves"]}
        assert {"PHIE", "NTG", "FACIES"} <= mnems, mnems          # raw tracks
        assert {"PHIE_UP", "NTG_UP"} <= mnems, mnems              # upscaled tracks
        for c in w["curves"]:
            assert c["values"]["shape"][0] == n
            assert c["kind"] in ("continuous", "flag")
            if c["kind"] == "flag":
                assert set(c["codes"]) and all(k.isdigit() for k in c["codes"])
            if c["mnemonic"].startswith("PHIE"):
                assert c["cutoff"] > 0.0                          # PHIE cutoff line + fill
        # framework tops are top→down; zone bands are real intervals
        tvds = [t["tvd_m"] for t in w["tops"]]
        assert tvds == sorted(tvds) and len(tvds) >= 2
        assert w["zones"], "zone bands from the model"
        for z in w["zones"]:
            assert set(z) == {"name", "top_tvd_m", "base_tvd_m"}
            assert z["base_tvd_m"] > z["top_tvd_m"]

    # ties[] from Provenance.well_ties surface on the shown wells (real bore ids).
    tied = [w for w in b["wells"] if w.get("ties")]
    assert tied, "engine well-tie residuals should attach to the shown bores"
    res_ids = {r["well"] for r in model.well_tie_residuals()}
    for w in tied:
        assert w["id"] in res_ids
        for t in w["ties"]:
            assert set(t) == {"horizon", "residual_m"}


# --- (2) round-trip against the petektools reference fixture -----------------
def test_lane_encoding_is_byte_identical_to_the_reference(model_with_wells):
    _man, model = model_with_wells
    b = model.wells_bundle()
    w = b["wells"][0]

    # The v3 f32 lane encoder matches the reference fixture's `encode_lane`
    # byte-for-byte (LE f32, base64, NaN canonicalization) for the SAME values.
    for lane in (w["md_m"], w["tvd_m"], *(c["values"] for c in w["curves"])):
        vals = _lane_floats(lane)
        # NaN samples must be the canonical quiet-NaN 0x7FC00000 (viewer null).
        raw = base64.b64decode(lane["data"])
        for i, v in enumerate(vals):
            if v != v:  # NaN
                assert raw[4 * i:4 * i + 4] == NAN_F32
        # Re-encoding through the reference producer reproduces the exact bytes.
        assert ref_encode_lane(vals)["data"] == lane["data"]

    # An upscaled curve carries NaN gaps (log extends beyond the cube) — proves the
    # NaN=null path is exercised, not just finite samples.
    phie_up = next(c for c in w["curves"] if c["mnemonic"] == "PHIE_UP")
    up = _lane_floats(phie_up["values"])
    assert any(v != v for v in up), "upscaled curve should NaN-gap outside the cells"

    # Structural key parity: our LogWell / Curve / lane keys match the reference.
    ref = ref_build()
    rw = ref["wells"][0]
    assert set(b) == set(ref)                                   # bundle keys
    assert {"id", "x", "y", "datum_m", "md_m", "tvd_m", "curves", "tops", "zones"} <= set(rw)
    ref_curve_keys = set().union(*(set(c) for c in rw["curves"]))
    for c in w["curves"]:
        assert set(c) <= ref_curve_keys | {"display_name", "range", "cutoff", "codes"}


# --- (3) the Wells tab lights up: served session AND self-contained export ----
def test_wells_tab_in_served_session_and_export(model_with_wells, tmp_path):
    _man, model = model_with_wells

    # Self-contained export: the frozen HTML inlines the wells_logs payload.
    html = tmp_path / "wells_view.html"
    model.save_view(str(html))
    text = html.read_text()
    assert html.stat().st_size > 0
    assert '"wells_logs"' in text and '"kind":"wells_logs"' in text
    assert "PHIE_UP" in text  # the upscaled track rode into the frozen payload

    # Served session: the live server writes the same payload to /model.json.
    url = model.view(open_browser=False, port=0)
    try:
        with urllib.request.urlopen(url + "/model.json", timeout=5) as r:
            payload = json.loads(r.read().decode("utf-8"))
    finally:
        pass
    wl = payload["wells_logs"]
    assert wl["kind"] == "wells_logs" and wl["schema_version"] == 4
    assert wl["wells"] and {"PHIE", "PHIE_UP"} <= {
        c["mnemonic"] for c in wl["wells"][0]["curves"]
    }
