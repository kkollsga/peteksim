#!/usr/bin/env python3
"""Tests for the staged model-build facade (`Project.load` → … →
`model.uncertainty`), driven against a synthesized Petrel-export tree.

Run under the project venv (build the wheel first with maturin):

    VIRTUAL_ENV="$PWD/.venv-srs" .venv-srs/bin/maturin develop -m crates/srs-py/Cargo.toml
    .venv-srs/bin/python -m pytest crates/srs-py/tests/test_facade.py -q

No confidential data — the tree is hand-authored to format spec by
`examples/synthetic_tree.py`.
"""

from __future__ import annotations

import math
import os
import sys
import tempfile
import threading
import time
from pathlib import Path

import pytest

REPO = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(REPO / "examples"))

import peteksim as ps  # noqa: E402
from synthetic_tree import (  # noqa: E402
    NM_AQUIFER_SW,
    NM_NET_SW,
    build_net_mask_tree,
    build_tree,
)


@pytest.fixture(scope="module")
def tree() -> Path:
    return build_tree(tempfile.mkdtemp(prefix="facade-test-"))


@pytest.fixture(scope="module")
def proj(tree):
    return ps.Project.load(str(tree), crs="SYNTHETIC")


# --- (1) ingest --------------------------------------------------------------
def test_project_load_inventory(proj):
    inv = proj.inventory()
    assert set(inv.surfaces) == {"TopReservoir", "BaseReservoir"}
    assert set(inv.wells) == {"A1", "A2"}
    assert {"GOC", "FWL", "TopReservoir", "BaseReservoir"} <= set(inv.tops)
    assert inv.polygons == ["outline"]
    assert inv.skipped == [], inv.skipped  # nothing silently dropped


def test_load_rejects_non_directory():
    with pytest.raises(ValueError):
        ps.Project.load("/no/such/tree/here")


def test_wells_handle(proj):
    wells = proj.wells()
    assert len(wells) == 2
    assert set(wells.ids()) == {"A1", "A2"}


def test_tops_pick(proj):
    goc = proj.tops.pick("GOC")
    assert goc.name == "GOC"
    assert len(goc.picks) == 2
    assert abs(goc.level_m - 2015.0) < 1e-6  # both wells pick the flat GOC
    assert goc.spread_m < 1e-6
    # Optional well filter restricts the aggregation to the listed well(s).
    one = proj.tops.pick("GOC", wells=["A1"])
    assert [w for w, _ in one.picks] == ["A1"]
    with pytest.raises(ValueError):
        proj.tops.pick("NoSuchSurface")


def test_surface_missing_raises(proj):
    with pytest.raises(ValueError):
        proj.surface("NoSuchSurface")


# --- (2) framework + ties ----------------------------------------------------
def test_framework_tie_report(proj):
    fw = proj.framework(horizons=["TopReservoir", "BaseReservoir"], outline="outline")
    tie = fw.tie_report()
    assert len(tie) == 4  # 2 horizons x 2 wells
    assert all(t["ok"] for t in tie)
    assert max(abs(t["residual_m"]) for t in tie) < 1e-6  # exact hard tie
    assert fw.tie_ok()
    # Z1 datum lock: the tree's .irap surfaces carry petekio's NEGATIVE-down
    # elevation; the facade converts onto the model's POSITIVE-down depth_m at
    # ingest (matching the pick path's -z). Both report columns must come out
    # positive-down at reservoir depth — if either negation were dropped, these
    # signs (and the exact-tie residuals above) would break loudly.
    assert all(t["surface_m"] > 1000.0 for t in tie), tie
    assert all(t["pick_m"] > 1000.0 for t in tie), tie


def test_framework_zones_layering(proj):
    fw = proj.framework(horizons=["TopReservoir", "BaseReservoir"], outline="outline")
    fw.set_zones({"Reservoir": ("TopReservoir", "BaseReservoir")})
    assert not fw.zones_limited()  # a single zone is within capability
    fw.set_layering({"Reservoir": ps.layers(n=6)})
    grid = fw.build_grid()
    assert grid.property_names() == []  # no pipelines yet


def test_layers_conformity_styles():
    # ps.layers style validation: Follow styles require dz_m; a bad style is loud.
    ps.layers(dz_m=1.0, style="follow_top")
    ps.layers(dz_m=1.0, style="follow_base")
    ps.layers(n=8)  # proportional default
    with pytest.raises(ValueError):
        ps.layers(n=8, style="follow_top")  # Follow needs dz_m
    with pytest.raises(ValueError):
        ps.layers(dz_m=1.0, style="nonsense")


def test_follow_top_conformity_builds_and_sections(proj):
    # follow_top drape builds, its model summarises, and an along-bore section
    # renders (NaN-marked truncated layers guarded downstream). model.warnings()
    # is the additive surface for any LayersTruncated/LayerCountCapped advisory.
    fw = proj.framework(horizons=["TopReservoir", "BaseReservoir"], outline="outline")
    fw.set_layering(ps.layers(dz_m=2.0, style="follow_top"))
    grid = fw.build_grid()
    por = grid.property("PORO")
    por.upscale(proj.wells())
    # The synthetic wells don't condition every simulated layer here; opt into the
    # (structureless) mean-fill for the data-less layer (petekStatic's default is a
    # loud named error).
    por.propagate(ps.gaussian(ps.spherical(range_m=800.0), seed=1),
                  allow_mean_fill=True)
    model = grid.model(
        contacts=dict(goc=proj.tops.pick("GOC"), fwl=proj.tops.pick("FWL")),
        fluid="oil",
        fvf=1.25,
        gas_fvf=0.005,
        wells=proj.wells(),
    )
    assert model.summary()["grv_mcm"] > 0.0
    assert isinstance(model.warnings(), list)  # advisories surface (may be empty)
    # An along-bore section renders for a positioned bore.
    wid = model.well_ids()[0]
    sec = model.intersection_bundle(well=wid)
    assert sec["columns"]  # non-empty section


# --- (3-4) grid + properties -------------------------------------------------
def _grid(proj):
    fw = proj.framework(horizons=["TopReservoir", "BaseReservoir"], outline="outline")
    return fw.build_grid()


def test_property_upscale_qc(proj):
    grid = _grid(proj)
    por = grid.property("PORO")
    n = por.upscale(proj.wells(), method="arithmetic")
    assert n > 0
    qc = por.qc()
    assert qc["property"] == "PORO"
    assert qc["conditioned_cells"] > 0
    assert 0.15 < qc["upscaled_mean"] < 0.30  # near the log level
    por.propagate(ps.gaussian(ps.spherical(range_m=800.0), seed=1))


def test_upscale_bad_method_raises(proj):
    grid = _grid(proj)
    with pytest.raises(ValueError):
        grid.property("PORO").upscale(proj.wells(), method="bogus")


def test_net_only_upscale_shifts_to_net_mean():
    # A well whose section is half net rock (SW 0.24) and half aquifer (SW 0.72):
    # all-sample conditioning averages to ~0.48; net_only masks to NTG>0.5 and the
    # conditioned SW shifts to the net mean ~0.24.
    tree = build_net_mask_tree(tempfile.mkdtemp(prefix="netmask-"))
    proj = ps.Project.load(str(tree), crs="SYNTHETIC")
    all_sw = (NM_NET_SW + NM_AQUIFER_SW) / 2.0  # ~0.48

    def conditioned_sw(net_only):
        grid = proj.framework(
            horizons=["TopReservoir", "BaseReservoir"], outline="outline"
        ).build_grid()
        sw = grid.property("SW")
        sw.upscale(proj.wells(), net_only=net_only)
        return sw.qc()["log_mean"]

    full = conditioned_sw(False)
    net = conditioned_sw(True)
    assert abs(full - all_sw) < 0.03, full          # all samples -> ~0.48
    assert abs(net - NM_NET_SW) < 0.03, net         # net rock only -> ~0.24
    assert net < full - 0.15, (net, full)           # decisively shifted to net


def test_property_phie_aliases_to_poro(proj):
    # API note: property("PHIE") populates the canonical PORO cube the volumetrics
    # + MC read, not a silent non-canonical cube.
    grid = _grid(proj)
    phie = grid.property("PHIE")
    phie.upscale(proj.wells())
    assert phie.qc()["property"] == "PORO"


def test_propagate_resimulate_marker(proj):
    # ps.resimulate() drives propagate(resimulate=...) (the resimulate MC mode).
    grid = _grid(proj)
    por = grid.property("PORO")
    por.upscale(proj.wells())
    por.propagate(ps.gaussian(ps.spherical(range_m=800.0), seed=1), resimulate=ps.resimulate())


def test_collocated_as_depth_and_value(proj):
    # F7: a structural trend (as_depth=True) and a value trend (default) both build.
    surf = proj.surface("TopReservoir")
    ps.collocated(surf, corr=0.4, as_depth=True)
    ps.collocated(surf, corr=0.3)  # value trend (as_depth=False default)


# --- (5-6) model + summary ---------------------------------------------------
def _model(proj):
    grid = _grid(proj)
    por = grid.property("PORO")
    por.upscale(proj.wells())
    # The synthetic wells don't condition every simulated layer; opt into the
    # (structureless) mean-fill for the data-less layers (petekStatic's default is a
    # loud named error).
    por.propagate(ps.gaussian(ps.spherical(range_m=800.0), seed=1),
                  allow_mean_fill=True)
    # Model NTG too, so an NTG level_shift routes as a per-draw shift (a level
    # shift only makes sense on a modelled cube; unmodelled priors take an
    # absolute distribution like ps.normal(0.30, ...)).
    ntg = grid.property("NTG")
    ntg.upscale(proj.wells())
    ntg.propagate(ps.gaussian(ps.spherical(range_m=800.0), seed=2),
                  allow_mean_fill=True)
    return grid.model(
        contacts=dict(goc=proj.tops.pick("GOC"), fwl=proj.tops.pick("FWL")),
        fluid="oil",
        fvf=1.25,
        gas_fvf=0.005,
    )


def test_model_summary(proj):
    model = _model(proj)
    s = model.summary()
    assert s["two_contact"] is True
    assert s["stoiip_sm3"] > 0.0
    assert s["giip_sm3"] > 0.0
    assert s["grv_mcm"] > 0.0
    # MSm3 convenience matches the Sm3 field.
    assert math.isclose(s["stoiip_msm3"], s["stoiip_sm3"] / 1e6, rel_tol=1e-12)
    assert "PORO" in model.property_names()


def test_model_needs_a_lower_contact(proj):
    grid = _grid(proj)
    with pytest.raises(ValueError):
        grid.model(contacts=dict(goc=2015.0))  # no fwl/owc


# --- (7-8) uncertainty + tornado + aggregate --------------------------------
def test_uncertainty_pcurve(proj):
    model = _model(proj)
    mc = model.uncertainty(
        porosity=ps.level_shift(sd=0.02),
        net_to_gross=ps.level_shift(sd=0.03),
        water_saturation=ps.normal(0.30, 0.03).clamped(0.05, 0.6),
        contacts=ps.pick_spread(sd_m=5.0),
        fvf=ps.normal(1.25, 0.03),
        gas_fvf=ps.normal(0.005, 0.0003),
        n=800,
        seed=42,
    )
    st = mc.stoiip
    assert st["p90"] < st["p50"] < st["p10"]
    assert st["p90"] > 0.0
    assert len(st["samples"]) == 800
    # ~10% of draws below P90 (low), by construction of the exceedance convention.
    below = sum(1 for x in st["samples"] if x < st["p90"]) / len(st["samples"])
    assert 0.05 < below < 0.16, below

    bars = mc.tornado()
    assert bars, "tornado has bars"
    names = [b["input"] for b in bars]
    assert "contact_depth_m" in names
    # bars are pre-sorted by swing, descending
    swings = [b["swing"] for b in bars]
    assert swings == sorted(swings, reverse=True)


def test_uncertainty_reproducible(proj):
    model = _model(proj)
    kw = dict(porosity=ps.level_shift(sd=0.02), contacts=ps.pick_spread(sd_m=4.0), n=500, seed=7)
    a = model.uncertainty(**kw).stoiip
    b = model.uncertainty(**kw).stoiip
    assert a["p50"] == b["p50"] and a["p10"] == b["p10"]


def test_uncertainty_releases_the_gil(proj):
    """S1: the Monte Carlo runs off the GIL, so a concurrent Python thread keeps
    making progress while `uncertainty()` computes in Rust.

    Event-ordering check (not a wall-clock race): a worker thread runs a long
    `uncertainty()`; the main thread spins a pure-Python counter until the worker
    signals `done`. With the GIL released for the Rust compute the counter climbs
    into the millions; if the GIL were held the whole call, the main loop could
    not execute a single iteration until `uncertainty()` returned (counter ~0).
    """
    if (os.cpu_count() or 1) < 2:
        pytest.skip("needs >= 2 cores to observe concurrent GIL release")

    model = _model(proj)
    started = threading.Event()
    done = threading.Event()
    errors: list[BaseException] = []

    def worker():
        started.set()
        try:
            # ~500 draws — long enough (seconds) to dominate thread/setup overhead.
            model.uncertainty(
                porosity=ps.level_shift(sd=0.02),
                contacts=ps.pick_spread(sd_m=5.0),
                n=500,
                seed=11,
            )
        except BaseException as exc:  # noqa: BLE001 — re-raised on the main thread
            errors.append(exc)
        finally:
            done.set()

    t = threading.Thread(target=worker)
    t.start()
    assert started.wait(10.0), "worker never started"

    counter = 0
    deadline = time.perf_counter() + 30.0
    while not done.is_set() and time.perf_counter() < deadline:
        counter += 1

    t.join(30.0)
    assert not errors, errors
    assert not t.is_alive(), "worker did not finish"
    # Concurrent Python progress during the Rust MC proves the GIL was released.
    # Threshold is deliberately far below the millions a released-GIL run reaches,
    # yet far above the ~0 a GIL-held run would leave.
    assert counter > 50_000, counter


def test_aggregate_single_segment(proj):
    model = _model(proj)
    mc = model.uncertainty(porosity=ps.level_shift(sd=0.02), n=500, seed=3)
    field = ps.aggregate([mc], correlation="independent")
    # one segment: the field P50 equals the segment's own STOIIP P50.
    assert math.isclose(field["p50"], mc.stoiip["p50"], rel_tol=1e-9)


# --- distribution / spec builders -------------------------------------------
def test_distribution_builders_validate():
    ps.normal(0.2, 0.02)
    ps.lognormal(0.2, 0.05)
    ps.uniform(0.1, 0.3)
    ps.triangular(0.1, 0.2, 0.3)
    ps.truncated_normal(0.2, 0.02, 0.1, 0.3)
    ps.level_shift(0.02).clamped(-0.1, 0.1)
    with pytest.raises(ValueError):
        ps.normal(0.2, 0.0)  # sd must be > 0
    with pytest.raises(ValueError):
        ps.uniform(0.3, 0.1)  # lo < hi


def test_variogram_and_layers_builders():
    ps.spherical(range_m=500.0)
    ps.exponential(range_m=500.0, sill=1.0, nugget=0.1)
    ps.gaussian_vgm(range_m=500.0)
    ps.layers(n=8)
    ps.layers(dz_m=2.0)
    with pytest.raises(ValueError):
        ps.layers(n=8, dz_m=2.0)  # exactly one


def test_fit_variogram():
    # A small isotropic point set — just exercise the fit path.
    coords = [[float(i), float(j), 0.1 + 0.01 * (i + j)] for i in range(6) for j in range(6)]
    ps.fit_variogram(coords, model="spherical", lag=1.0, n_lags=5)


if __name__ == "__main__":
    sys.exit(pytest.main([__file__, "-q"]))
