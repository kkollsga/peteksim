#!/usr/bin/env python3
"""Round-trip tests for the multi-zone horizon-stack facade wrap
(`fw.set_zonation(...)` → `model.in_place_by_zone()` / `zone_stats()`).

Driven against the SYNTHETIC procedural dome-stack tree from
`examples/dome_demo.py` — an 11-horizon / 10-zone stack with one tops-only
internal horizon (H5), a single-OWC oil zone (Z2), a two-contact gas+oil zone
(Z4), and contactless zones. No confidential data.

    VIRTUAL_ENV="$PWD/.venv-srs" .venv-srs/bin/maturin develop -m crates/srs-py/Cargo.toml
    .venv-srs/bin/python -m pytest crates/srs-py/tests/test_zonation.py -q
"""

from __future__ import annotations

import sys
import tempfile
from pathlib import Path

import pytest

REPO = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(REPO / "examples"))

import peteksim as ps  # noqa: E402
from dome_demo import HORIZONS, TOPS_ONLY_IDX, build_dome_tree, zonation_for  # noqa: E402

# A small lattice keeps the 10-zone build fast while still exercising every path.
NCOL = 13


@pytest.fixture(scope="module")
def tree() -> Path:
    return build_dome_tree(tempfile.mkdtemp(prefix="zonation-test-"), ncol=NCOL)


def _zoned_model(tree: Path, collapse_below_m: float | None = None):
    proj = ps.Project.load(str(tree), crs="SYNTHETIC / UTM zone 00N")
    fw = proj.framework(
        horizons=HORIZONS,
        outline="outline",
        tie_to_tops=True,
        min_thickness_m=0.0,
        collapse_below_m=collapse_below_m,
    )
    # Coarse layering (nk=2 / dz=5 m) keeps the 10-zone build fast in tests.
    fw.set_zonation(zonation_for(NCOL, nk=2, dz_m=5.0))
    grid = fw.build_grid()
    por = grid.property("PORO")
    por.upscale(proj.wells())
    # The coarse synthetic wells don't condition every simulated layer of this
    # 10-zone stack; opt into the (structureless) mean-fill for the data-less layers
    # (petekStatic's default is a loud named error).
    por.propagate(ps.gaussian(ps.spherical(range_m=1500.0), seed=7),
                  allow_mean_fill=True)
    return grid.model(fluid="oil", fvf=1.30, gas_fvf=0.005, wells=proj.wells())


@pytest.fixture(scope="module")
def model(tree: Path):
    # One zoned build reused across the round-trip assertions (the build is the
    # expensive part; the collapse-warning test builds its own).
    return _zoned_model(tree)


def test_tops_only_horizon_has_no_surface_but_builds(tree: Path, model):
    # H5 is tops-only: no mapped surface file exists, yet the stack builds through
    # the facade (the facade maps its well picks to node-lattice Picks).
    proj = ps.Project.load(str(tree))
    assert HORIZONS[TOPS_ONLY_IDX] not in proj.inventory().surfaces
    assert model.is_zoned()


def test_ten_zone_stack_round_trips_and_conserves(model):
    z = model.in_place_by_zone()
    assert len(z["zones"]) == 10
    assert [row["zone"] for row in z["zones"]] == [f"Z{i}" for i in range(10)]

    # Conservation: the total is the summed rollup of the per-zone volumes.
    sum_stoiip = sum(r["stoiip_sm3"] for r in z["zones"])
    sum_giip = sum(r["giip_sm3"] for r in z["zones"])
    sum_grv = sum(r["grv_mcm"] for r in z["zones"])
    assert z["total"]["stoiip_sm3"] == pytest.approx(sum_stoiip, rel=1e-9)
    assert z["total"]["giip_sm3"] == pytest.approx(sum_giip, rel=1e-9)
    assert z["total"]["grv_mcm"] == pytest.approx(sum_grv, rel=1e-9)


def test_contactless_zone_has_zero_hydrocarbon(model):
    rows = {r["zone"]: r for r in model.in_place_by_zone()["zones"]}
    # Z0/Z1/... are contactless (no goc/owc) → gross bulk, zero hydrocarbon.
    z0 = rows["Z0"]
    assert not z0["two_contact"]
    assert z0["grv_mcm"] > 0.0
    assert z0["stoiip_sm3"] == 0.0 and z0["giip_sm3"] == 0.0


def test_single_and_two_contact_zones_hold_hydrocarbon(model):
    rows = {r["zone"]: r for r in model.in_place_by_zone()["zones"]}
    # Z2 is a single-OWC oil zone; Z4 splits into a gas cap + oil rim.
    assert rows["Z2"]["stoiip_sm3"] > 0.0 and not rows["Z2"]["two_contact"]
    assert rows["Z4"]["two_contact"]
    assert rows["Z4"]["stoiip_sm3"] > 0.0 and rows["Z4"]["giip_sm3"] > 0.0


def test_zone_stats_shape_and_values(model):
    stats = model.zone_stats("PORO")
    assert len(stats) == 10
    for s in stats:
        assert set(s) == {"zone", "count", "mean", "min", "max"}
        assert s["count"] > 0  # every zone has active, conditioned cells
        assert 0.0 < s["min"] <= s["mean"] <= s["max"] < 1.0


def test_zoned_summary_matches_zoned_total(model):
    s = model.summary()
    total = model.in_place_by_zone()["total"]
    # The zoned summary reads the per-zone rollup (whole-model in_place would mix
    # contacts across zones).
    assert s["stoiip_sm3"] == pytest.approx(total["stoiip_sm3"], rel=1e-9)
    assert s["giip_sm3"] == pytest.approx(total["giip_sm3"], rel=1e-9)
    assert s["two_contact"] is True  # Z4 splits


def test_collapse_below_m_surfaces_a_warning(tree: Path):
    # An aggressive cell-collapse floor merges sub-threshold cells → CellsCollapsed.
    model = _zoned_model(tree, collapse_below_m=3.0)
    kinds = {w["kind"] for w in model.warnings()}
    assert "cells_collapsed" in kinds


def test_whole_model_uncertainty_routes_to_zoned(model):
    # The whole-model uncertainty() would mix contacts across zones — it points the
    # caller at the per-zone driver instead.
    with pytest.raises(ValueError, match="multi-zone stack"):
        model.uncertainty(porosity=ps.normal(0.25, 0.02), n=100, seed=1)


def test_zoned_uncertainty_per_zone_and_total(model):
    # Stack-aware MC: per-zone contact draws roll up into per-zone AND total P-curves.
    mc = model.zoned_uncertainty(contacts=ps.pick_spread(sd_m=3.0), n=64, seed=3)
    zones = mc.zones
    assert len(zones) == 10
    assert [z["zone"] for z in zones] == [f"Z{i}" for i in range(10)]

    # Contactless zones (Z0, Z1, ...) contribute zero hydrocarbon on every draw.
    z0 = next(z for z in zones if z["zone"] == "Z0")
    assert z0["stoiip"]["mean"] == 0.0 and z0["stoiip"]["p50"] == 0.0
    assert not z0["two_contact"]

    # Z2 (single OWC) + Z4 (GOC+FWL, two-contact) hold hydrocarbon.
    z2 = next(z for z in zones if z["zone"] == "Z2")
    z4 = next(z for z in zones if z["zone"] == "Z4")
    assert z2["stoiip"]["mean"] > 0.0 and not z2["two_contact"]
    assert z4["stoiip"]["mean"] > 0.0 and z4["two_contact"] and z4["giip"]["mean"] > 0.0

    # The total P-curve is ordered (reservoir exceedance convention).
    t = mc.total["stoiip"]
    assert t["p90"] <= t["p50"] <= t["p10"]

    # Conservation: the total conserves the per-zone legs on every sampled draw.
    tot = mc.total["stoiip"]["samples"]
    n = len(tot)
    assert n == 64
    for i in range(0, n, 8):
        s = sum(z["stoiip"]["samples"][i] for z in zones)
        assert abs(tot[i] - s) <= 1e-6 * max(abs(tot[i]), 1.0)


def test_zoned_uncertainty_zero_spread_matches_deterministic(model):
    # Parity identity: with no spread every draw reproduces the deterministic build,
    # so each zone's MC mean == in_place_by_zone (the SAME identity the Rust unit
    # test `zoned_mc_zero_spread_matches_deterministic` asserts on its own fixture).
    det = {r["zone"]: r["stoiip_sm3"] for r in model.in_place_by_zone()["zones"]}
    mc = model.zoned_uncertainty(n=8, seed=1)
    for z in mc.zones:
        d = det[z["zone"]]
        assert abs(z["stoiip"]["mean"] - d) <= 1e-6 * max(abs(d), 1.0), (
            z["zone"], z["stoiip"]["mean"], d)
    tt = mc.total["stoiip"]["mean"]
    td = model.in_place_by_zone()["total"]["stoiip_sm3"]
    assert abs(tt - td) <= 1e-6 * max(abs(td), 1.0)


def test_zoned_uncertainty_parallel_matches_serial(model):
    # The sharded realize loop reproduces the serial per-zone/total means.
    serial = model.zoned_uncertainty(contacts=ps.pick_spread(sd_m=2.0), n=48, seed=5)
    par = model.zoned_uncertainty(contacts=ps.pick_spread(sd_m=2.0), n=48, seed=5, workers=4)
    assert par.total["stoiip"]["samples"] == serial.total["stoiip"]["samples"]


def test_in_place_by_zone_schema_snapshot(model):
    z = model.in_place_by_zone()
    assert set(z) == {"zones", "total"}
    expected = {
        "zone", "grv_mcm", "hcpv_m3", "stoiip_sm3", "stoiip_msm3",
        "giip_sm3", "giip_bcm", "two_contact",
    }
    assert set(z["total"]) == expected
    for row in z["zones"]:
        assert set(row) == expected


def test_contact_labels_are_unique_across_map_and_section(tree: Path, model):
    # A multi-zone stack holds a single-OWC zone (Z2) AND a GOC+FWL zone (Z4).
    # petekStatic's ContactKind has no FWL, so the lower contacts both collapse to
    # "OWC" — the payload must qualify the repeats so the viewer legend shows one
    # entry per distinct contact (no duplicate "OWC" rows) on BOTH tabs, with the
    # same labels so identity colours agree.
    import json

    mb = model.map_bundle(property="PORO")
    map_kinds = [c["kind"] for c in mb["contacts"]]
    assert len(map_kinds) == len(set(map_kinds)), map_kinds
    assert sum(k.startswith("OWC") for k in map_kinds) == 2  # both lower contacts
    assert "GOC" in map_kinds  # the single GOC is left unqualified

    # A crest fence section carries the same model-wide contacts, identically labelled.
    proj = ps.Project.load(str(tree), crs="SYNTHETIC / UTM zone 00N")
    span = 100.0 * (NCOL - 1)
    from dome_demo import ORIGIN_X, ORIGIN_Y  # noqa: PLC0415

    line = [
        [ORIGIN_X + 0.12 * span, ORIGIN_Y + 0.20 * span],
        [ORIGIN_X + 0.50 * span, ORIGIN_Y + 0.50 * span],
        [ORIGIN_X + 0.88 * span, ORIGIN_Y + 0.80 * span],
    ]
    del proj  # the fixture model already carries the section provider
    sj = json.loads(model._section_json(property="PORO", line=line, well=None))
    sec_kinds = [c["kind"] for c in sj["contacts"]]
    assert sec_kinds == map_kinds  # section + map labels match exactly
