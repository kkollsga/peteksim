#!/usr/bin/env python3
"""The API-V2 acceptance leg — testing-doctrine **R7** (workflow-shape lock).

The peteksim acceptance suite additionally locks the WORKFLOW SHAPE: this leg
runs the ratified canonical sketch (modelling-api-v2.md) VERBATIM on synth asset
v2 — geometry → Horizons/Subzones/Layering → build → Props/Contacts → model → Mc
→ view — recovers the planted truths through the NEW surface, pins the
application-moment signatures, and asserts scenario derivation (two derived specs
→ two deterministic, differing models).

Runs under ``-m acceptance`` (the standing pre-stamp gate), beside the v1 R6 leg
in ``test_acceptance.py`` — same planted truths, the declarative surface.
"""

from __future__ import annotations

import inspect
import tempfile

import pytest

pytestmark = pytest.mark.skip(
    reason="legacy petekSim Project facade removed; petekIO owns project loading"
)

import peteksim as ps

SEED = 20_260_704
NCOL = 21
acceptance = pytest.mark.acceptance


def _specs(man):
    """The v2 specs mirroring the v1 acceptance chain (same planted truths)."""
    hz = ps.Horizons(
        *[ps.hz(h) for h in man["horizons"]],
        zones=man["zones"],
        ties=ps.TieSettings(method="convergent"),
        gridding=ps.Gridding(collapse=True),
    )
    con = ps.Contacts({z["zone"]: dict(z["contacts"])
                       for z in man["zonation"] if z["contacts"]})
    lay = ps.Layering(nk=2)

    def vg(seed):
        return ps.Propagate(variogram=ps.variogram("spherical", 1500.0), seed=seed)

    props = ps.Props(
        ps.Prop("PORO", net_only=True, propagate=vg(11)),
        ps.Prop("NTG", propagate=vg(12)),
        ps.Prop("PORO", zone="Z2", net_only=True, propagate=vg(13)),
    )
    return hz, con, lay, props


@pytest.fixture(scope="module")
def chain_v2():
    """The full v2 chain: LoadSettings → grid_geometry → build(hz, lay) →
    model(props, con) — the world-georef, deviated-well, pinch-out, per-zone-
    contact model, built through the declarative surface."""
    root = tempfile.mkdtemp(prefix="acceptance-v2-")
    man = ps.synth_asset(root, seed=SEED, ncol=NCOL)
    proj = ps.Project.load(man["root"],
                           settings=ps.LoadSettings(crs=man["crs"], aliases=man["aliases"]))
    hz, con, lay, props = _specs(man)
    geom = proj.grid_geometry(cell=(50.0, 50.0), orient=0.0)
    grid = geom.build(hz, layering=lay, collapse_negative=True, min_thickness_m=0.0)
    model = grid.model(props, con, fluid="oil", fvf=1.30, gas_fvf=0.005)
    return man, proj, model


# =============================================================================
# (1) THE CANONICAL SKETCH runs verbatim + produces every bundle kind.
# =============================================================================
@acceptance
def test_canonical_sketch_runs_verbatim_and_produces_bundles(chain_v2, tmp_path):
    man, _proj, model = chain_v2
    assert model.is_zoned()
    # every bundle kind produced + non-empty (the workflow reaches the viewer)
    vol = model.volume_bundle(property="PORO")
    mp = model.map_bundle(property="PORO")
    # v3 block envelope (base64 blocks) — the shape the viewer decodes.
    assert vol["cell_count"] > 0 and vol["encoding"] == "base64" and vol["triangle_count"] > 0
    assert mp["frame"]["ncol"] > 1 and mp["zone_averages"]
    f = mp["frame"]
    x0, y0 = f["origin_x"], f["origin_y"]
    x1 = x0 + (f["ncol"] - 1) * f["spacing_x"]
    y1 = y0 + (f["nrow"] - 1) * f["spacing_y"]
    line = model.intersection_bundle(line=[[x0, y0], [x1, y1]], property="PORO")
    assert line["columns"]
    # Mc via the unifying spec → charts → save_view (the analytics half).
    mc = model.zoned_uncertainty(ps.Mc(contacts=4.0, goc=3.0, porosity=0.01,
                                       n=96, seed=42))
    charts = [ps.Distribution().bundle(mc), ps.Distribution(zone="Z4").bundle(mc)]
    html = tmp_path / "acceptance_v2.html"
    model.save_view(str(html), property="PORO", charts=charts)
    assert html.stat().st_size > 0
    assert "window.PETEK_VIEWER_PAYLOAD=" in html.read_text()


# =============================================================================
# (2) PLANTED-TRUTH RECOVERY through the new surface (mirrors the v1 leg).
# =============================================================================
@acceptance
def test_v2_per_zone_net_conditioned_poro_recovers_targets(chain_v2):
    man, _proj, model = chain_v2
    por = {s["zone"]: s["mean"] for s in model.zone_stats("PORO")}
    for zone in man["zones"]:
        tgt = man["zone_targets"][zone]["net_por_mean"]
        assert isinstance(por[zone], float) and por[zone] == por[zone]
        assert abs(por[zone] - tgt) < 0.05, ("PORO", zone, por[zone], tgt)


@acceptance
def test_v2_contact_plan_hc_pattern(chain_v2):
    man, _proj, model = chain_v2
    det = {r["zone"]: r for r in model.in_place_by_zone()["zones"]}
    for zone, spec in man["contact_plan"].items():
        row = det[zone]
        if spec["type"] == "single":
            assert row["stoiip_sm3"] > 0.0 and not row["two_contact"], (zone, row)
        elif spec["type"] == "two_contact":
            assert row["two_contact"] and row["stoiip_sm3"] > 0.0, (zone, row)
        else:
            assert row["stoiip_sm3"] == 0.0, (zone, row)


@acceptance
def test_v2_zero_spread_mc_equals_deterministic_and_conserves(chain_v2):
    _man, _proj, model = chain_v2
    det = {r["zone"]: r["stoiip_sm3"] for r in model.in_place_by_zone()["zones"]}
    mc = model.zoned_uncertainty(ps.Mc(n=8, seed=1))  # zero spread
    for z in mc.zones:
        d = det[z["zone"]]
        assert abs(z["stoiip"]["mean"] - d) <= 1e-6 * max(abs(d), 1.0), (
            z["zone"], z["stoiip"]["mean"], d)
    # conservation on every sampled draw of a spread MC.
    smc = model.zoned_uncertainty(ps.Mc(contacts=4.0, n=64, seed=7))
    tot = smc.total["stoiip"]["samples"]
    for i in range(0, len(tot), 8):
        s = sum(z["stoiip"]["samples"][i] for z in smc.zones)
        assert abs(tot[i] - s) <= 1e-6 * max(abs(tot[i]), 1.0), (i, tot[i], s)


# =============================================================================
# (3) APPLICATION-MOMENT SIGNATURES pinned.
# =============================================================================
@acceptance
def test_application_moment_signatures_pinned():
    def params(fn):
        return set(inspect.signature(fn).parameters)

    assert {"cell", "extent", "orient"} <= params(ps.Project.grid_geometry)
    from peteksim.apply import GridGeometry, Grid, Model
    assert {"horizons", "subzones", "layering", "collapse_negative"} <= params(GridGeometry.build)
    assert {"props", "con", "fluid", "fvf", "gas_fvf", "wells", "run"} <= params(Grid.model)
    assert "mc" in params(Model.zoned_uncertainty) and "mc" in params(Model.uncertainty)


# =============================================================================
# (4) SCENARIO DERIVATION — two derived specs → two deterministic, differing models.
# =============================================================================
def _build(proj, hz, lay, con, props):
    geom = proj.grid_geometry(cell=(50.0, 50.0), orient=0.0)
    grid = geom.build(hz, layering=lay, collapse_negative=True, min_thickness_m=0.0)
    return grid.model(props, con, fluid="oil", fvf=1.30, gas_fvf=0.005)


@acceptance
def test_scenario_derivation_contacts_differ_and_deterministic(chain_v2):
    # Two derived Contacts scenarios (a deeper Z2 OWC) → two models whose Z2 STOIIP
    # differs decisively; each rebuild from the SAME spec is bit-deterministic.
    man, proj, _model = chain_v2
    hz, con, lay, props = _specs(man)
    base_owc = man["contacts"]["owc_z2"]
    con_a = con
    con_b = con.replace("Z2", owc=base_owc + 40.0)  # deeper OWC ⇒ more Z2 oil
    assert con_a != con_b and con.for_zone("Z2")["owc"] == base_owc  # original intact

    a = _build(proj, hz, lay, con_a, props)
    b = _build(proj, hz, lay, con_b, props)
    z2 = lambda m: {r["zone"]: r["stoiip_sm3"] for r in m.in_place_by_zone()["zones"]}["Z2"]
    assert z2(b) > z2(a) * 1.0 and abs(z2(b) - z2(a)) > 1.0, (z2(a), z2(b))
    # determinism: rebuild scenario A → identical Z2 STOIIP.
    a2 = _build(proj, hz, lay, con_a, props)
    assert z2(a2) == z2(a)


@acceptance
def test_scenario_derivation_hz_specs_differ_and_deterministic(chain_v2):
    # Two derived HORIZON specs (full column vs. the deepest zone dropped) → two
    # deterministic models with a different zone inventory / total GRV.
    man, proj, _model = chain_v2
    hz, con, lay, props = _specs(man)
    hz_full = hz
    # Drop the deepest zone AND its base horizon (the engine keeps horizons=zones+1).
    hz_drop = hz.replace(rows=hz.rows[:-1], zones=hz.zones[:-1])
    assert hz_drop != hz_full and hz_full.zones[-1] not in hz_drop.zones

    m_full = _build(proj, hz_full, lay, con, props)
    m_drop = _build(proj, hz_drop, lay, con, props)
    zones_full = [r["zone"] for r in m_full.in_place_by_zone()["zones"]]
    zones_drop = [r["zone"] for r in m_drop.in_place_by_zone()["zones"]]
    assert len(zones_drop) == len(zones_full) - 1
    grv = lambda m: m.in_place_by_zone()["total"]["grv_mcm"]
    assert grv(m_drop) < grv(m_full)  # a dropped zone ⇒ less gross rock
    # determinism: rebuild the dropped scenario → identical total GRV.
    m_drop2 = _build(proj, hz_drop, lay, con, props)
    assert grv(m_drop2) == grv(m_drop)


# =============================================================================
# (5) STRUCTURAL UNCERTAINTY — ps.hz(sd=, vgm=) wired onto the zoned MC draws
#     (task_petekstatic_structural_uncertainty, petekstatic 76c2532).
# =============================================================================
@acceptance
def test_structural_uncertainty_widens_inplace_spread_and_reproducible(chain_v2):
    """A ``ps.hz(sd=, vgm=)`` top-surface depth field + a zone isochore field
    perturb each zoned-MC draw: the total in-place spread WIDENS vs. the no-field
    control, the run is bit-reproducible per seed, and differs across seeds
    (planted-sensitivity sanity — not a full recovery)."""
    man, proj, _model = chain_v2
    _hz, con, lay, props = _specs(man)
    ties = ps.TieSettings(method="convergent")
    grd = ps.Gridding(collapse=True)

    def build(sd):
        if sd > 0:  # sd on the top row (top-depth field) + row 1 (zone-0 isochore)
            rows = [ps.hz(man["horizons"][0], sd=sd, vgm=("spherical", 2500.0)),
                    ps.hz(man["horizons"][1], sd=sd, vgm=("spherical", 2500.0))]
            rows += [ps.hz(h) for h in man["horizons"][2:]]
        else:
            rows = [ps.hz(h) for h in man["horizons"]]
        hz = ps.Horizons(*rows, zones=man["zones"], ties=ties, gridding=grd)
        geom = proj.grid_geometry(cell=(50.0, 50.0), orient=0.0)
        grid = geom.build(hz, layering=lay, collapse_negative=True, min_thickness_m=0.0)
        return grid.model(props, con, fluid="oil", fvf=1.30, gas_fvf=0.005)

    def spread(model, seed):
        mc = model.zoned_uncertainty(ps.Mc(contacts=4.0, n=128, seed=seed))
        s = mc.total["stoiip"]["samples"]
        return max(s) - min(s)

    control = build(0.0)     # no structural field
    structural = build(12.0)  # 12 m top-depth + zone-0 isochore field

    # bit-reproducible per seed (a pure function of the draw set)
    assert spread(structural, 42) == spread(structural, 42)
    # different seeds resample the field → a different draw set
    assert spread(structural, 42) != spread(structural, 7)
    # planted sensitivity: the structural perturbation widens the in-place spread
    assert spread(structural, 42) > spread(control, 42), (
        spread(structural, 42), spread(control, 42))


@acceptance
def test_structural_uncertainty_on_flat_model_is_loud(chain_v2):
    """On a NON-zoned model the flat MC surface has no structural hook yet — a
    ``ps.hz(sd=, vgm=)`` build raises ``NotYetSupported`` (never a silent no-op)."""
    man, proj, _man2 = chain_v2
    hz = ps.Horizons(ps.hz(man["horizons"][0], sd=5.0, vgm=("spherical", 2000.0)),
                     ps.hz(man["horizons"][-1]))  # no zones ⇒ flat model
    geom = proj.grid_geometry(cell=(50.0, 50.0), orient=0.0)
    with pytest.raises(ps.NotYetSupported):
        geom.build(hz, layering=ps.Layering(nk=2)).model()


if __name__ == "__main__":
    import sys
    sys.exit(pytest.main([__file__, "-m", "acceptance", "-q"]))
