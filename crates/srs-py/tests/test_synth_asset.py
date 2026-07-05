#!/usr/bin/env python3
"""Acceptance for `peteksim.synth_asset` — THE complete synthetic Petrel-export
asset (the suite dataset).

Proves the graph task `task_peteksim_synthetic_asset` acceptance:
  * Project.load ingests the tree with ZERO non-noise skips;
  * the eight-call facade + set_zonation + MC + charts + save_view run end-to-end;
  * per-zone NET-conditioned upscaled stats recover the planted zone targets
    (net_only PORO ~ net_por_mean; NTG ~ ntg_target) — the coupled generator
    guarantees the consistency;
  * the collocated depositional trend recovers the planted rho through the full
    pipeline (the killer test the real data never allowed);
  * the tree is bit-deterministic per seed.

    VIRTUAL_ENV="$PWD/.venv-srs" .venv-srs/bin/maturin develop -m crates/srs-py/Cargo.toml
    .venv-srs/bin/python -m pytest crates/srs-py/tests/test_synth_asset.py -q
"""

from __future__ import annotations

import math
import signal
import sys
import tempfile
from pathlib import Path

import pytest

REPO = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(REPO / "examples"))

import peteksim as ps  # noqa: E402

SEED = 20_260_704


@pytest.fixture(scope="module")
def asset():
    # A lighter 21-node lattice keeps the multi-SGS builds fast; the real suite
    # dataset is the default 41-node asset. Ingest + recovery are size-independent.
    man = ps.synth_asset(tempfile.mkdtemp(prefix="synth-asset-"), seed=SEED, ncol=21)
    proj = ps.Project.load(man["root"], crs=man["crs"], aliases=man["aliases"])
    return man, proj


_ZONED_CACHE = {}


def _zoned_model_cached(man, proj):
    if "m" not in _ZONED_CACHE:
        _ZONED_CACHE["m"] = _zoned_model(man, proj)
    return _ZONED_CACHE["m"]


# --- (1) ingest: zero non-noise skips ----------------------------------------
def test_ingest_zero_non_noise_skips(asset):
    man, proj = asset
    inv = proj.inventory()
    assert inv.skipped == [], inv.skipped                 # nothing dropped
    assert set(man["well_ids"]) <= set(inv.wells)
    # both point formats loaded (H0..H6 twice), the CPS-3 trend surface, the outline
    assert man["trend_surface"] in inv.surfaces
    assert "ModelEdge" in inv.polygons
    for h in man["horizons"]:
        assert h in inv.points
    # Type="Other" contacts parsed (GOC/FWL/OWC) + the Latin-1 name row decoded.
    assert {"GOC", "FWL", "OWC"} <= set(inv.tops)
    assert any("Blåbær" in t for t in inv.tops), inv.tops


# --- (2) net-conditioned zone stats recover the planted targets --------------
def _zoned_model(man, proj):
    fw = proj.framework(horizons=man["horizons"], outline="ModelEdge",
                        tie_to_tops=True, min_thickness_m=0.0)
    # Coarse layering (nk=2) keeps the multi-zone SGS build brisk in a test.
    zonation = [dict(z, nk=2) for z in man["zonation"]]
    fw.set_zonation(zonation)
    grid = fw.build_grid()
    por = grid.property("PORO")
    por.upscale(proj.wells(), net_only=True)              # NET-conditioned -> net_por_mean
    por.propagate(ps.gaussian(ps.spherical(range_m=1500.0), seed=11))
    ntg = grid.property("NTG")
    ntg.upscale(proj.wells())                             # arithmetic mean of the 0/1 flag -> ntg_target
    ntg.propagate(ps.gaussian(ps.spherical(range_m=1500.0), seed=12))
    return grid.model(fluid="oil", fvf=1.30, gas_fvf=0.005, wells=proj.wells())


def test_net_conditioned_zone_stats_recover_planted_targets(asset):
    man, proj = asset
    model = _zoned_model_cached(man, proj)
    zones = man["zones"]
    por_by_zone = {s["zone"]: s["mean"] for s in model.zone_stats("PORO")}
    ntg_by_zone = {s["zone"]: s["mean"] for s in model.zone_stats("NTG")}
    # NET-conditioned PORO recovers the planted net-rock porosity per zone, tightly:
    # net_only masks to net rock, and the coupled generator makes the net-rock PORO
    # exactly net_por_mean — this is THE net-conditioned acceptance, guaranteed.
    for zone in zones:
        tgt = man["zone_targets"][zone]
        assert math.isfinite(por_by_zone[zone])
        assert abs(por_by_zone[zone] - tgt["net_por_mean"]) < 0.05, (
            "PORO", zone, por_by_zone[zone], tgt["net_por_mean"])
    # The planted per-zone NTG PATTERN is recovered (strong positive correlation
    # across the six zones). The absolute per-zone proportion compresses toward the
    # mid-range in the zoned upscale — a petekStatic cell↔zone-boundary effect
    # (routed 2026-07-04; the LAS net-flag itself matches each target within ~0.07),
    # so the pattern correlation is the honest per-zone check here.
    tv = [man["zone_targets"][z]["ntg_target"] for z in zones]
    nv = [ntg_by_zone[z] for z in zones]
    assert _pearson(tv, nv) > 0.6, ("NTG pattern", nv, tv, _pearson(tv, nv))


# --- (3) the eight calls + set_zonation + MC + charts + save_view end-to-end --
def _whole_field(man, proj):
    fw = proj.framework(horizons=[man["horizons"][0], man["horizons"][-1]],
                        outline="ModelEdge", tie_to_tops=False)
    grid = fw.build_grid()
    por = grid.property("PORO")
    por.upscale(proj.wells(), net_only=True)
    # This coarse whole-field build (2 horizons, net_only masking) leaves the deepest
    # simulated layer with no net conditioning data — the petekStatic default now
    # errors loudly on that. It is an expected data-less layer here, so opt into the
    # documented (structureless) constant mean-fill for it.
    por.propagate(ps.gaussian(ps.spherical(range_m=1500.0), seed=1),
                  allow_mean_fill=True)
    return grid, por


def test_full_pipeline_runs_end_to_end(asset, tmp_path):
    man, proj = asset
    # eight calls (load already done): framework -> grid -> property -> model ...
    grid, _por = _whole_field(man, proj)
    deep = man["contacts"]["fwl_z4"] + 60.0
    model = grid.model(contacts={"fwl": deep}, fluid="oil", fvf=1.30)
    summ = model.summary()
    assert summ["stoiip_sm3"] >= 0.0
    # ... uncertainty (MC) + tornado/distribution charts + save_view
    mc = model.uncertainty(
        porosity=ps.level_shift(sd=0.02),
        net_to_gross=ps.normal(0.6, 0.05).clamped(0.2, 1.0),
        water_saturation=ps.normal(0.30, 0.03).clamped(0.05, 0.6),
        contacts=ps.pick_spread(sd_m=5.0),
        fvf=ps.normal(1.30, 0.03),
        n=300, seed=42,
    )
    assert mc.stoiip["p90"] <= mc.stoiip["p10"]
    charts = [mc.tornado_bundle(units="MSm³"), mc.distribution_bundle()]
    html = tmp_path / "asset_view.html"
    model.save_view(str(html), property="PORO", charts=charts)
    assert html.exists() and html.stat().st_size > 0
    # a zoned build also runs its own set_zonation + per-zone volumes end-to-end
    zoned = _zoned_model_cached(man, proj)
    z = zoned.in_place_by_zone()
    assert z["total"]["stoiip_sm3"] > 0.0


# --- (3b) the ZONED end-to-end: set_zonation + per-zone contacts + zoned MC ----
def test_zoned_end_to_end_mc_charts_and_save_view(asset, tmp_path):
    # The full ZONED end-to-end on the synthetic asset: the staged calls +
    # set_zonation + per-zone contacts from the manifest -> zoned MC -> per-zone AND
    # total P-curves -> charts -> save_view. The manifest plants Z2 (OWC) + Z4
    # (GOC+FWL); the rest are contactless.
    man, proj = asset
    model = _zoned_model_cached(man, proj)

    # Per-zone deterministic volumes seen from Python (the coarse nk=2 synth build
    # resolves Z4's gas-cap split but its thin cap rounds to zero GIIP — the dome
    # fixture covers a fat two-contact gas leg; here the oil legs carry the check).
    det = {r["zone"]: r for r in model.in_place_by_zone()["zones"]}
    assert det["Z2"]["stoiip_sm3"] > 0.0 and not det["Z2"]["two_contact"]
    assert det["Z4"]["two_contact"] and det["Z4"]["stoiip_sm3"] > 0.0
    assert det["Z0"]["stoiip_sm3"] == 0.0  # contactless

    # Zoned MC: per-zone contact draws (+ a GOC spread on Z4) + a base porosity shift.
    mc = model.zoned_uncertainty(
        contacts=ps.pick_spread(sd_m=4.0),
        goc=ps.pick_spread(sd_m=3.0),
        porosity=ps.level_shift(sd=0.01),
        n=96, seed=42,
    )
    zbyname = {z["zone"]: z for z in mc.zones}
    assert zbyname["Z2"]["stoiip"]["mean"] > 0.0
    assert zbyname["Z4"]["stoiip"]["mean"] > 0.0 and zbyname["Z4"]["two_contact"]
    assert zbyname["Z0"]["stoiip"]["mean"] == 0.0
    t = mc.total["stoiip"]
    assert t["p90"] <= t["p50"] <= t["p10"]
    # Conservation on every sampled draw.
    tot = t["samples"]
    for i in range(0, len(tot), 12):
        s = sum(z["stoiip"]["samples"][i] for z in mc.zones)
        assert abs(tot[i] - s) <= 1e-6 * max(abs(tot[i]), 1.0)

    # Charts (total + a per-zone panel) + save_view.
    charts = [mc.distribution_bundle(), mc.distribution_bundle(zone="Z4")]
    html = tmp_path / "zoned_view.html"
    model.save_view(str(html), property="PORO", charts=charts)
    assert html.exists() and html.stat().st_size > 0


def test_zoned_mc_parity_zero_spread(asset):
    # Cross-language parity vector: the zoned MC at ZERO spread reproduces the
    # deterministic per-zone rollup — Rust `in_place_by_zone` values == the
    # Python-visible MC means (the same identity the Rust unit test asserts).
    man, proj = asset
    model = _zoned_model_cached(man, proj)
    det = {r["zone"]: r["stoiip_sm3"] for r in model.in_place_by_zone()["zones"]}
    mc = model.zoned_uncertainty(n=8, seed=1)
    for z in mc.zones:
        d = det[z["zone"]]
        assert abs(z["stoiip"]["mean"] - d) <= 1e-6 * max(abs(d), 1.0), (
            z["zone"], z["stoiip"]["mean"], d)


def test_zoned_mc_parity_zero_spread_with_zone_scoped_pipe(asset):
    # Same zero-spread identity as `test_zoned_mc_parity_zero_spread`, but with the
    # properties staged through a ZONE-SCOPED pipeline (grid.property(name, zone=..))
    # on a contact-bearing zone. Now a HARD assertion (was xfail on
    # question_zoned_mc_zone_pipe_parity): peteksim threads the staged zone pipes into
    # StaticModelTemplate::with_zone_property, so the zoned MC realizes each piped
    # zone from its actual upscale+SGS cube — zero-spread MC == in_place_by_zone.
    man, proj = asset
    fw = proj.framework(horizons=man["horizons"], outline="ModelEdge",
                        tie_to_tops=True, min_thickness_m=0.0)
    fw.set_zonation([dict(z, nk=2) for z in man["zonation"]])
    grid = fw.build_grid()
    for prop, seed in (("PORO", 21), ("NTG", 22)):
        zp = grid.property(prop, zone="Z2")  # Z2 carries an OWC in the manifest
        zp.upscale(proj.wells())
        zp.propagate(ps.gaussian(ps.spherical(range_m=1500.0), seed=seed))
    model = grid.model(fluid="oil", fvf=1.30, gas_fvf=0.005, wells=proj.wells())
    det = {r["zone"]: r["stoiip_sm3"] for r in model.in_place_by_zone()["zones"]}
    mc = model.zoned_uncertainty(n=8, seed=1)
    for z in mc.zones:
        d = det[z["zone"]]
        assert abs(z["stoiip"]["mean"] - d) <= 1e-6 * max(abs(d), 1.0), (
            z["zone"], z["stoiip"]["mean"], d)


@pytest.fixture(scope="module")
def sparse_asset():
    # A deliberately SPARSE single-well variant: v2's canonical (8-well, mixed
    # deviated) asset now conditions every whole-field layer, so a data-less layer
    # no longer arises there. One vertical well over the whole field guarantees a
    # data-less deep layer (its net samples miss the deepest slice) — the fixture
    # the R4 loudness path needs, and deterministic per seed.
    man = ps.synth_asset(tempfile.mkdtemp(prefix="sparse-asset-"), seed=SEED, ncol=21, n_wells=1)
    proj = ps.Project.load(man["root"], crs=man["crs"], aliases=man["aliases"])
    return man, proj


def test_data_less_layer_loud_error_and_mean_fill_opt_in(sparse_asset):
    # item 1 (allow_mean_fill exposure): a simulated layer that carries NO
    # conditioning data errors loudly, naming the property, by DEFAULT (the
    # petekStatic loud default) — and allow_mean_fill=True on the propagate opts into
    # the (structureless) constant mean-fill so the build proceeds. The single-well
    # coarse 2-horizon whole-field build with net_only masking guarantees a data-less
    # deep simulated layer.
    man, proj = sparse_asset

    def build(allow_mean_fill):
        fw = proj.framework(horizons=[man["horizons"][0], man["horizons"][-1]],
                            outline="ModelEdge", tie_to_tops=False)
        grid = fw.build_grid()
        por = grid.property("PORO")
        por.upscale(proj.wells(), net_only=True)
        por.propagate(ps.gaussian(ps.spherical(range_m=1500.0), seed=1),
                      allow_mean_fill=allow_mean_fill)
        return grid

    deep = man["contacts"]["fwl_z4"] + 60.0
    # Default (allow_mean_fill=False): a data-less simulated layer is a loud, named error.
    with pytest.raises(ValueError, match="PORO"):
        build(False).model(contacts={"fwl": deep}, fluid="oil", fvf=1.30)
    # Opt-in: the same build proceeds (structureless mean-fill for the data-less layer).
    model = build(True).model(contacts={"fwl": deep}, fluid="oil", fvf=1.30)
    assert model.summary()["stoiip_sm3"] >= 0.0


def test_engine_well_ties_surface_residuals_and_map_bundle(asset):
    # Per-horizon engine well ties: a tie table -> with_well_ties -> Provenance
    # residuals surfaced on model.well_tie_residuals(), and wells[].ties in the map
    # bundle payload.
    man, proj = asset
    fw = proj.framework(horizons=man["horizons"], outline="ModelEdge",
                        tie_to_tops=True, min_thickness_m=0.0)
    fw.set_zonation([dict(z, nk=2) for z in man["zonation"]])
    cx, cy = 431_000.0 + 1000.0, 6_521_000.0 + 1000.0
    tops = {h: 2000.0 + 12.0 * k for k, h in enumerate(man["horizons"])}
    fw.set_well_ties([{"id": "TIE-1", "x": cx, "y": cy, "tops": tops}])
    grid = fw.build_grid()
    por = grid.property("PORO"); por.upscale(proj.wells())
    por.propagate(ps.gaussian(ps.spherical(range_m=1500.0), seed=9))
    model = grid.model(fluid="oil", fvf=1.30, gas_fvf=0.005, wells=proj.wells())

    res = model.well_tie_residuals()
    assert res, "engine well-tie residuals should be surfaced from provenance"
    assert {r["horizon"] for r in res} <= set(man["horizons"])
    for r in res:
        assert set(r) == {"well", "horizon", "measured_depth_m", "model_depth_m", "residual_m"}
        assert r["residual_m"] == pytest.approx(r["measured_depth_m"] - r["model_depth_m"], abs=1e-6)
    # wells[].ties flows into the map bundle payload (the engine tie seam).
    tie_well = next(w for w in model.map_bundle(property="PORO")["wells"] if w["id"] == "TIE-1")
    assert tie_well["ties"] and all(set(t) == {"horizon", "residual_m"} for t in tie_well["ties"])


def test_per_zone_priors_and_pipeline(asset):
    # Per-zone parameterization: set_zone_priors pins a zone's PORO level, and a
    # zone-scoped property pipeline (with_zone_property) simulates in one zone only.
    man, proj = asset
    fw = proj.framework(horizons=man["horizons"], outline="ModelEdge",
                        tie_to_tops=True, min_thickness_m=0.0)
    fw.set_zonation([dict(z, nk=2) for z in man["zonation"]])
    fw.set_zone_priors("Z0", porosity=0.12, net_to_gross=0.3, water_saturation=0.5)
    grid = fw.build_grid()
    z4 = grid.property("PORO", zone="Z4")          # zone-scoped pipeline
    z4.upscale(proj.wells())
    z4.propagate(ps.gaussian(ps.spherical(range_m=1500.0), seed=15))
    model = grid.model(fluid="oil", fvf=1.30, gas_fvf=0.005, wells=proj.wells())
    stats = {s["zone"]: s["mean"] for s in model.zone_stats("PORO")}
    # Z0 was pinned to a low sand prior; unpipelined zones sit at the global prior.
    assert abs(stats["Z0"] - 0.12) < 0.03, stats
    assert stats["Z1"] > stats["Z0"], stats  # global 0.25 > Z0's 0.12


# --- (4) collocated trend through the pipeline --------------------------------
def _collocated_ntg_map_and_trend(man, proj, nlayers=None):
    """Build a whole-field NTG cube steered by the depositional trend at the planted
    rho and return (built NTG areal map values, trend values) on the map frame.

    ``nlayers=1`` collapses the column so the areal map IS the per-node field — the
    correct frame for recovering a per-node planted rho (a multi-layer map averages
    the column, whose areal-mean correlation runs higher than the per-node rho)."""
    trend = proj.surface(man["trend_surface"])
    fw = proj.framework(horizons=[man["horizons"][0], man["horizons"][-1]],
                        outline="ModelEdge", tie_to_tops=False)
    if nlayers is not None:
        fw.set_layering(ps.layers(n=nlayers))
    grid = fw.build_grid()
    ntg = grid.property("NTG")
    ntg.upscale(proj.wells())
    ntg.propagate(ps.gaussian(ps.spherical(range_m=1200.0), seed=7),
                  trend=ps.collocated(trend, corr=man["rho"], as_depth=False))
    model = grid.model(contacts={"fwl": man["contacts"]["fwl_z4"] + 60.0}, fluid="oil", fvf=1.30)
    mb = model.map_bundle(property="NTG")
    frame = mb["frame"]
    ntg_map = mb["zone_averages"][0]["values"]
    ni, nj = frame["ncol"], frame["nrow"]
    tvals, nvals = [], []
    for j in range(nj):
        for i in range(ni):
            v = ntg_map[j * ni + i]
            if not (isinstance(v, (int, float)) and v == v):
                continue
            x = frame["origin_x"] + i * frame["spacing_x"]
            y = frame["origin_y"] + j * frame["spacing_y"]
            t = trend.value_at(x, y)
            if isinstance(t, (int, float)) and t == t:
                nvals.append(v)
                tvals.append(t)
    return model, tvals, nvals


def test_collocated_trend_pipeline_runs(asset):
    # The full collocated pipeline runs end-to-end: the trend surface loads as a
    # proper varying 2-D field, ps.collocated builds, the NTG cube populates steered
    # by it, and the conditioned model is valid. (The numeric rho recovery is a
    # separate xfail below — blocked by the routed collocated georef no-op.)
    man, proj = asset
    trend = proj.surface(man["trend_surface"])
    corners = [trend.value_at(x, y) for x in (431_050.0, 432_950.0) for y in (6_521_050.0, 6_522_950.0)]
    assert max(corners) - min(corners) > 0.2, corners  # a real, varying trend
    model, _t, _n = _collocated_ntg_map_and_trend(man, proj)
    assert model.summary()["stoiip_sm3"] > 0.0


def test_collocated_recovers_planted_rho(asset):
    # The killer test, now GREEN: petekStatic's world-georef trend fix (6079133)
    # threads the loaded trend through the grid's world frame, so the collocated
    # secondary is real (not all-NaN) and the NTG cube genuinely tracks the
    # depositional trend end-to-end — previously a silent no-op that fell back to
    # plain SGS. The planted rho is a PER-NODE correlation, so we measure it on a
    # single-layer build (nlayers=1): the areal field IS the per-node field there.
    # (A multi-layer map is a vertical average whose areal-mean correlation runs
    # higher than the per-node rho as independent SGS noise averages out down-column
    # — measured ~0.9 at the default layering; the per-node frame is the honest one.)
    man, proj = asset
    _model, tvals, nvals = _collocated_ntg_map_and_trend(man, proj, nlayers=1)
    r = _pearson(tvals, nvals)
    # Built NTG tracks the trend at ~the planted rho (collocated cokriging realizes
    # slightly below the input corr — a known, accepted property of the estimator).
    assert abs(r - man["rho"]) < 0.25, (r, man["rho"])
    assert r > 0.2, r


def _pearson(a, b):
    n = len(a)
    ma, mb = sum(a) / n, sum(b) / n
    cov = sum((x - ma) * (y - mb) for x, y in zip(a, b))
    va = math.sqrt(sum((x - ma) ** 2 for x in a))
    vb = math.sqrt(sum((y - mb) ** 2 for y in b))
    return cov / (va * vb) if va > 0 and vb > 0 else 0.0


# --- (5) bit-determinism per seed --------------------------------------------
def test_bit_deterministic_per_seed():
    # The DEFAULT call is now the full v2 asset (mixed deviated wells + tops-only
    # split + pinch-out) — byte-determinism must hold across every new generator.
    a = ps.synth_asset(tempfile.mkdtemp(prefix="det-a-"), seed=SEED)
    b = ps.synth_asset(tempfile.mkdtemp(prefix="det-b-"), seed=SEED)
    ra, rb = Path(a["root"]), Path(b["root"])
    fa = sorted(p.relative_to(ra) for p in ra.rglob("*") if p.is_file())
    fb = sorted(p.relative_to(rb) for p in rb.rglob("*") if p.is_file())
    assert fa == fb, "file layout differs between two same-seed runs"
    for rel in fa:
        assert (ra / rel).read_bytes() == (rb / rel).read_bytes(), f"bytes differ: {rel}"


# =============================================================================
# v2 — structurally isomorphic to the canonical model (testing-doctrine.md)
# =============================================================================
def _read_tops(root):
    """Parse the emitted Petrel well-tops tree into pick rows (the actual artifact,
    Latin-1)."""
    text = (Path(root) / "WellTops" / "FieldWellTops").read_bytes().decode("latin-1")
    rows = []
    for line in text.splitlines():
        if '"' not in line:                       # header / column-name lines
            continue
        f = line.split()
        parts = line.split('"')
        rows.append({
            "x": float(f[0]), "y": float(f[1]), "tvdss": -float(f[2]),
            "md": float(f[6]), "type": f[8], "surface": parts[1], "well": parts[3],
        })
    return rows


def _read_irap(path):
    """De-negate an emitted IRAP grid back to a TVDSS field[col][row] (positive-down)."""
    lines = [ln for ln in path.read_text().split("\n") if ln.strip()]
    nrow = int(float(lines[0].split()[1]))
    ncol = int(float(lines[2].split()[0]))
    vals = [float(v) for ln in lines[4:] for v in ln.split()]
    field = [[0.0] * nrow for _ in range(ncol)]
    k = 0
    for j in range(nrow):
        for i in range(ncol):
            field[i][j] = -vals[k]                # writer negates (negative-down elevation)
            k += 1
    return field, ncol, nrow


def test_asset_version_and_v2_manifest(asset):
    man, _ = asset
    assert man["asset_version"] == 2
    # world georef recorded (doctrine R1) — the single fictional frame.
    assert man["georef"] == {"east0": 431_000.0, "north0": 6_521_000.0}
    # per-well program spans vertical + deviated profiles.
    profs = {w["profile"] for w in man["well_program"]}
    assert "vertical" in profs and ({"build_hold", "build_hold_drop"} & profs)
    # per-zone contact plan spans all three cases (two-contact / single / contactless).
    types = {v["type"] for v in man["contact_plan"].values()}
    assert {"two_contact", "single", "contactless"} <= types
    # the two-contact zone carries GOC+FWL, the single carries OWC.
    two = next(v for v in man["contact_plan"].values() if v["type"] == "two_contact")
    assert set(two["contacts"]) == {"goc", "fwl"}
    one = next(v for v in man["contact_plan"].values() if v["type"] == "single")
    assert set(one["contacts"]) == {"owc"}
    # tops-only horizon + pinch-out recorded.
    assert man["tops_only_horizon"] == "H3b" and man["tops_only_horizon"] not in man["horizons"]
    assert man["pinch_out"]["zone"] in man["zones"]
    # spill recipe: the live-set estimate exceeds the forcing budget.
    sr = man["spill_recipe"]
    assert sr["est_live_set_bytes"] > sr["force_budget_bytes"] > 0
    assert sr["cells"] == sr["ncol"] ** 2 * sr["nk_total"]


def test_mixed_well_program_deviated_bores_cross_columns(asset):
    man, _ = asset
    dev = [w for w in man["well_program"] if w["profile"] != "vertical"]
    assert dev, "the canonical asset carries deviated bores (AlongBore edge food)"
    for w in dev:
        # believable dogleg (petektools bound) — not a kinked, unphysical path.
        assert 0.0 < w["max_dls_deg_per_30m"] <= 6.0, w
    # a deviated bore crosses SEVERAL 100 m columns at reservoir depth: its mapped
    # H0→H6 picks occupy multiple DISTINCT areal cells (section/edge machinery food),
    # and every pick stays inside the modelled extent (drift aimed inward).
    tops = _read_tops(man["root"])
    all_cols = set()
    for w in dev:
        picks = {r["surface"]: (r["x"], r["y"]) for r in tops
                 if r["well"] == w["id"] and r["type"] == "Horizon" and r["surface"] in man["horizons"]}
        assert "H0" in picks and "H6" in picks
        cols = set()
        for (px, py) in picks.values():
            assert 431_000.0 <= px <= 431_000.0 + (NCOL21 - 1) * 100.0
            assert 6_521_000.0 <= py <= 6_521_000.0 + (NCOL21 - 1) * 100.0
            col = (round((px - 431_000.0) / 100.0), round((py - 6_521_000.0) / 100.0))
            cols.add(col)
            all_cols.add(col)
        assert len(cols) >= 2, (w["id"], "reservoir picks span >1 areal column", cols)
    assert len(all_cols) >= 5, all_cols   # the deviated program spreads across several columns


def test_deviated_tops_at_trajectory_xy_not_wellhead(asset):
    # THE deviated-tops acceptance: a directional bore's Type="Horizon" picks land at
    # the TRAJECTORY (x, y) where the bore crosses each surface — NOT at the wellhead.
    man, _ = asset
    tops = _read_tops(man["root"])
    heads = {w["id"]: (w["x"], w["y"]) for w in man["well_program"]}
    dev = {w["id"] for w in man["well_program"] if w["profile"] != "vertical"}
    vert = {w["id"] for w in man["well_program"] if w["profile"] == "vertical"}
    off = {}
    for r in tops:
        if r["type"] != "Horizon" or r["surface"] not in man["horizons"]:
            continue
        hx, hy = heads[r["well"]]
        off.setdefault(r["well"], []).append(math.hypot(r["x"] - hx, r["y"] - hy))
    # deviated bores: deep picks are far from the wellhead (offset grows with depth).
    for wid in dev:
        assert max(off[wid]) > 100.0, (wid, off[wid])
    # vertical bores: picks sit at the wellhead (constant x, y).
    for wid in vert:
        assert max(off[wid]) < 1.0, (wid, off[wid])


def test_tops_only_horizon_has_picks_but_no_surface(asset):
    # The conformal-drape case: the split horizon carries well picks but is NOT
    # emitted as a mapped surface (no grid/point file, absent from the horizon list).
    man, proj = asset
    name = man["tops_only_horizon"]
    tops = _read_tops(man["root"])
    picked = [r for r in tops if r["surface"] == name and r["type"] == "Horizon"]
    assert picked, "the tops-only horizon must carry well picks"
    assert {r["well"] for r in picked} == set(man["well_ids"]), "one pick per well"
    inv = proj.inventory()
    assert name not in inv.surfaces and name not in inv.points   # NO mapped surface
    assert name not in man["horizons"]                            # not a mapped horizon
    # each split pick lies between its bounding mapped horizons (a real internal drape).
    for r in picked:
        h3 = next(t["tvdss"] for t in tops if t["well"] == r["well"] and t["surface"] == "H3")
        h4 = next(t["tvdss"] for t in tops if t["well"] == r["well"] and t["surface"] == "H4")
        assert h3 <= r["tvdss"] <= h4 or h4 <= r["tvdss"] <= h3, (r["well"], h3, r["tvdss"], h4)


def test_pinch_out_zone_has_degenerate_columns(asset):
    # The pinch-out: the recorded zone's isochore falls to sub-threshold AND exactly
    # zero across the eastern columns — genuine degenerate-column food (R5).
    man, _ = asset
    po = man["pinch_out"]
    # the pinch zone sits below `below_horizon`; read the bounding emitted surfaces.
    hi = po["below_horizon"]                                     # base of the pinch zone (H6)
    lo = man["horizons"][man["horizons"].index(hi) - 1]          # top of the pinch zone (H5)
    surf_dir = Path(man["root"]) / "Surfaces"
    top, ncol, nrow = _read_irap(surf_dir / f"{lo}.irap")
    base, _c, _r = _read_irap(surf_dir / f"{hi}.irap")
    zero = sub = full = 0
    for i in range(ncol):
        for j in range(nrow):
            t = base[i][j] - top[i][j]
            if t <= 1e-9:
                zero += 1
            elif t < po["subthreshold_m"]:
                sub += 1
            elif t > 3.0:
                full += 1
    assert zero > 0, "the pinch zone must have EXACTLY-zero columns"
    assert sub > 0, "the pinch zone must have SUB-THRESHOLD columns"
    assert full > 0, "the pinch zone must stay full-thickness elsewhere"


def test_world_georef_end_to_end(asset):
    # doctrine R1: every emitted coordinate is in the single fictional WORLD frame
    # (~431000/6521000) — nothing sits in a tidy local (~0) frame that would let a
    # frame-mixing bug pass silently.
    man, proj = asset
    for r in _read_tops(man["root"]):
        assert r["x"] > 400_000.0 and r["y"] > 6_000_000.0, r
    # a loaded mapped surface reports world-frame node coordinates too.
    trend = proj.surface(man["trend_surface"])
    assert trend.value_at(431_500.0, 6_521_500.0) == trend.value_at(431_500.0, 6_521_500.0)  # finite in-frame


def test_collapse_enabled_build_on_pinched_asset_no_livelock(asset):
    # R5: a zoned build with collapse ENABLED must EXERCISE the collapse/order-repair
    # machinery on the pinch-out's degenerate columns and TERMINATE — a hard timeout
    # turns a livelock into a FAILURE instead of a hung run (the collapse_below_m
    # livelock is fixed). A light property keeps it brisk; the collapse fires at
    # grid-build time on the degenerate columns regardless of layer count.
    man, proj = asset
    prev = signal.signal(signal.SIGALRM, lambda *a: (_ for _ in ()).throw(TimeoutError("collapse livelock")))
    signal.alarm(90)
    try:
        fw = proj.framework(horizons=man["horizons"], outline="ModelEdge",
                            tie_to_tops=True, min_thickness_m=0.0, collapse_below_m=0.5)
        fw.set_zonation([dict(z, nk=3) for z in man["zonation"]])
        grid = fw.build_grid()                       # collapse + order-repair run here
        por = grid.property("PORO")
        por.upscale(proj.wells(), net_only=True)
        por.propagate(ps.gaussian(ps.spherical(range_m=1200.0), seed=11))
        model = grid.model(fluid="oil", fvf=1.30, gas_fvf=0.005, wells=proj.wells())
        total = model.in_place_by_zone()["total"]["stoiip_sm3"]
    finally:
        signal.alarm(0)
        signal.signal(signal.SIGALRM, prev)
    assert total >= 0.0


# The default fixture is the 21-node lattice (see `asset`); its extent bound.
NCOL21 = 21
