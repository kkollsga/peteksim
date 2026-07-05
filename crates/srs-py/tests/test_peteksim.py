#!/usr/bin/env python3
"""End-to-end functional test for the petekSim Python binding (`peteksim`).

Run it directly (no pytest needed) with the project venv's interpreter:

    # build/refresh the extension first (sets VIRTUAL_ENV so maturin targets the venv):
    VIRTUAL_ENV="$PWD/.venv-srs" .venv-srs/bin/maturin develop -m crates/srs-py/Cargo.toml
    # then run the tests:
    .venv-srs/bin/python crates/srs-py/tests/test_peteksim.py

NOTE: the user's shell aliases `python`, so always call `.venv-srs/bin/python`
explicitly. The module is named `peteksim` (import peteksim).

To *see* a model, build one and call `model.view()` (see
examples/build_and_view.py) — that opens the three.js viewer in a browser. This
script tests the compute + export path (no rendering).

Units (SI standard): area km², lengths/depths m (positive down), FVF Rm³/Sm³;
results in Sm³, GRV in mcm (10⁶ m³).

Exit code is 0 if all checks pass, 1 otherwise.
"""

import math
import sys
import traceback

try:
    import peteksim
except ImportError:
    sys.exit(
        "Could not import `peteksim`. Build it first:\n"
        '  VIRTUAL_ENV="$PWD/.venv-srs" .venv-srs/bin/maturin develop '
        "-m crates/srs-py/Cargo.toml"
    )

# Convention reminders for readers of the output:
#   P90 = low/conservative (exceeded 90% of the time), P10 = high. P90 < P50 < P10.

PASS, FAIL = 0, 0

# Exact conversion factors (documented in petektools::units) for the parity test.
M2_PER_ACRE = 4046.8564224  # 43 560 ft² × 0.3048²
M_PER_FT = 0.3048
SM3_PER_STB = 0.158987294928
FT3_PER_BBL = 5.614583333333333


def check(name, fn):
    global PASS, FAIL
    try:
        fn()
        PASS += 1
        print(f"  PASS  {name}")
    except Exception as e:  # noqa: BLE001 - test harness wants every failure
        FAIL += 1
        print(f"  FAIL  {name}: {e}")
        traceback.print_exc()


def approx(a, b, rel=1e-6, abs_=0.0):
    assert math.isclose(a, b, rel_tol=rel, abs_tol=abs_), f"{a} != {b}"


# --- Reference oil case: 0.4 km², 15 m, phi 0.25, NTG 0.8, Sw 0.3, Boi 1.25 ---
def base_oil(**kw):
    args = dict(
        area_km2=0.4,
        gross_height_m=15.0,
        porosity=0.25,
        net_to_gross=0.8,
        water_saturation=0.3,
        fvf=1.25,
        fluid="oil",
        top_m=1500.0,
        contact_m=2743.0,  # below base -> full column
        realizations=20000,
        seed=1,
    )
    args.update(kw)
    return peteksim.run_box_model(**args)


# ----------------------------------------------------------------------------- tests
def test_version():
    v = peteksim.version()
    assert isinstance(v, str) and len(v) > 0, v


def test_deterministic_constant_has_no_spread():
    # All-constant inputs -> the P-curve collapses onto the deterministic value.
    r = base_oil()
    approx(r.p90, r.p50, rel=1e-9)
    approx(r.p50, r.p10, rel=1e-9)
    approx(r.p50, r.deterministic_in_place, rel=1e-9)


def test_known_ooip_value():
    # HCPV = area·height·NTG·phi·(1-Sw) [m³]; OOIP = HCPV/Boi [Sm³].
    r = base_oil()
    hcpv = 0.4e6 * 15.0 * 0.8 * 0.25 * (1.0 - 0.3)  # 840 000 m³
    ooip = hcpv / 1.25  # 672 000 Sm³
    approx(r.deterministic_in_place, ooip, rel=1e-6)


def test_grv_full_column():
    # Whole 0.4 km² × 15 m box is above the contact -> 6.0 mcm.
    r = base_oil()
    approx(r.grv_mcm, 6.0, rel=1e-6)


def test_si_matches_old_imperial_case():
    # PARITY PROOF (decision_si_units_standard): the old imperial reference case
    # (100 acres, 50 ft, top 5000 ft, contact 9000 ft) expressed in SI yields the
    # SAME physical answer — the Sm³ result converts back to the identical STB
    # the old imperial path produced.
    r = base_oil(
        area_km2=100.0 * M2_PER_ACRE / 1e6,  # 0.40468564224 km²
        gross_height_m=50.0 * M_PER_FT,  # 15.24 m
        top_m=5000.0 * M_PER_FT,  # 1524 m
        contact_m=9000.0 * M_PER_FT,  # 2743.2 m -> full column
    )
    # Old imperial path: HCPV [ft³] -> rb -> /Boi = STB.
    hcpv_ft3 = 100.0 * 43560.0 * 50.0 * 0.8 * 0.25 * (1.0 - 0.3)
    old_stb = (hcpv_ft3 / FT3_PER_BBL) / 1.25
    new_stb = r.deterministic_in_place / SM3_PER_STB
    approx(new_stb, old_stb, rel=1e-9)
    # And the petekStatic-proven twin case (0.4 km² × 50 m): OOIP = 2.24 MSm³
    # = 14 089 176.1258 STB.
    r2 = base_oil(gross_height_m=50.0)
    approx(r2.deterministic_in_place, 2.24e6, rel=1e-9)
    approx(r2.deterministic_in_place / SM3_PER_STB, 14_089_176.1258, rel=1e-9)


def test_dist_scaling_is_exact():
    # The km²→m² distribution rescale is exact: doubling every triangular
    # parameter doubles every percentile bit-for-bit (same seed -> same u draws).
    a = base_oil(area_km2=(0.32, 0.4, 0.52), seed=11)
    b = base_oil(area_km2=(0.64, 0.8, 1.04), seed=11)
    approx(b.p50, 2.0 * a.p50, rel=1e-12)
    approx(b.p10, 2.0 * a.p10, rel=1e-12)


def test_triangular_percentile_ordering():
    r = base_oil(area_km2=(0.32, 0.4, 0.52), gross_height_m=(12, 15, 20))
    assert r.p90 < r.p50 < r.p10, (r.p90, r.p50, r.p10)
    assert r.realizations == 20000


def test_swanson_mean_within_band():
    r = base_oil(area_km2=(0.32, 0.4, 0.52), gross_height_m=(12, 15, 20))
    assert r.p90 < r.mean < r.p10, (r.p90, r.mean, r.p10)


def test_summary_msm3_and_bcm_scales():
    r = base_oil(area_km2=(0.32, 0.4, 0.52))
    ms = r.summary_msm3
    bc = r.summary_bcm
    for key in ("p90", "p50", "p10", "mean"):
        approx(ms[key], getattr(r, key) / 1e6, rel=1e-12)
        approx(bc[key], getattr(r, key) / 1e9, rel=1e-12)


def test_reproducible_same_seed():
    a = base_oil(area_km2=(0.32, 0.4, 0.52), seed=42)
    b = base_oil(area_km2=(0.32, 0.4, 0.52), seed=42)
    approx(a.p50, b.p50, rel=0.0, abs_=0.0)
    approx(a.p10, b.p10, rel=0.0, abs_=0.0)


def test_different_seed_differs():
    a = base_oil(area_km2=(0.32, 0.4, 0.52), seed=1)
    b = base_oil(area_km2=(0.32, 0.4, 0.52), seed=2)
    assert a.p50 != b.p50


def test_area_scales_in_place():
    # Constant inputs: doubling area doubles in-place.
    r1 = base_oil(area_km2=0.4)
    r2 = base_oil(area_km2=0.8)
    approx(r2.deterministic_in_place, 2.0 * r1.deterministic_in_place, rel=1e-9)


def test_contact_above_reservoir_gives_zero():
    # Contact shallower than the top -> nothing in the hydrocarbon column.
    r = base_oil(contact_m=1200.0)
    approx(r.deterministic_in_place, 0.0, abs_=1e-9)
    approx(r.grv_mcm, 0.0, abs_=1e-9)


def test_partial_contact_halves_column():
    # Top 1500 m, height 15 -> base 1515; contact at 1507.5 ~ half the column.
    full = base_oil(contact_m=2743.0)
    half = base_oil(contact_m=1507.5, nk=50)  # fine layering for a clean cut
    assert 0.45 < half.deterministic_in_place / full.deterministic_in_place < 0.55, (
        half.deterministic_in_place / full.deterministic_in_place
    )


def test_gas_case():
    r = base_oil(fluid="gas", fvf=0.004)
    # GIIP = HCPV / Bgi [Sm³] -> much larger magnitude than the oil Sm³.
    hcpv = 0.4e6 * 15.0 * 0.8 * 0.25 * (1.0 - 0.3)
    approx(r.deterministic_in_place, hcpv / 0.004, rel=1e-6)
    assert r.fluid == "gas"


def test_invalid_fluid_raises():
    try:
        base_oil(fluid="water")
        raise AssertionError("expected ValueError for bad fluid")
    except ValueError:
        pass


def test_invalid_triangle_raises():
    try:
        base_oil(area_km2=(0.52, 0.4, 0.32))  # min > max
        raise AssertionError("expected ValueError for bad triangle")
    except ValueError:
        pass


def test_repr_is_informative():
    r = base_oil()
    s = repr(r)
    assert "ModelResult" in s and "P50" in s, s


def test_save_json_writes_viewer_payload():
    # After the mesh retirement, save_json writes the bundle-driven viewer payload
    # (map + volume bundles + summary), not the old {summary, mesh}.
    import json
    import tempfile

    r = base_oil()
    with tempfile.NamedTemporaryFile("r", suffix=".json", delete=False) as f:
        path = f.name
    r.save_json(path)
    payload = json.load(open(path))
    assert payload["schema_version"] == 2  # top-level payload contract (since `charts`)
    assert {"volume", "map", "summary", "property", "properties"} <= set(payload), payload.keys()
    approx(payload["summary"]["p50"], r.p50, rel=1e-9)
    approx(payload["summary"]["grv_mcm"], r.grv_mcm, rel=1e-9)
    # The mesh rides petekStatic's VolumeBundle, serialized as the **v3
    # self-contained block envelope** (base64 blocks) — the shape the petekTools
    # decode kernel reads; inline serde-derive arrays crash it (P10 census).
    v = payload["volume"]
    assert v["kind"] == "volume" and v["encoding"] == "base64"
    assert "positions" not in v and "indices" not in v
    assert set(v["blocks"]) == {"positions", "indices", "tri_cell", "cell_values", "zone_ids"}
    assert v["blocks"]["tri_cell"]["shape"][0] == v["triangle_count"]


def test_structured_model_builds_and_responds_to_controls():
    # Build a structured model in code; a crestal high raises in-place when the
    # contact is mid-column.
    def make():
        return peteksim.Model(
            0.4, 15.0, ni=12, nj=12, nk=6,
            top_m=1500.0, contact_m=1507.5,
            porosity=0.25, net_to_gross=0.8, water_saturation=0.3,
            fvf=1.25, fluid="oil",
        )

    flat = make().solve()
    assert flat.in_place > 0.0 and flat.controls == 4

    high = make()
    high.add_control(6, 6, 1494.0)
    crest = high.solve()
    assert crest.controls == 5
    assert crest.in_place > flat.in_place, (flat.in_place, crest.in_place)


def test_model_save_json_has_null_percentiles():
    import json
    import tempfile

    m = peteksim.Model(0.4, 15.0, ni=8, nj=8, nk=4, contact_m=2743.0)
    with tempfile.NamedTemporaryFile(suffix=".json", delete=False) as f:
        path = f.name
    m.save_json(path)
    summary = json.load(open(path))["summary"]
    # A deterministic model has no Monte Carlo percentiles.
    assert summary["p50"] is None and summary["deterministic_in_place"] > 0.0


def test_model_without_contact_raises_at_construction():
    # A non-finite contact (including the missing default) can't define a
    # hydrocarbon column and is now rejected up front, not at solve().
    try:
        peteksim.Model(0.4, 15.0)  # no contact_m
        raise AssertionError("expected ValueError for missing contact_m")
    except ValueError:
        pass


def test_model_repr_is_informative():
    m = peteksim.Model(0.4, 15.0, ni=12, nj=12, nk=6, contact_m=2743.0)
    s = repr(m)
    assert "Model" in s and "contact_m" in s and "controls=4" in s, s
    m.add_control(6, 6, 1494.0)
    assert "controls=5" in repr(m), repr(m)


# --- W15: tagged distribution dicts -----------------------------------------
def test_dist_dict_normal_and_lognormal_give_spread():
    # {"normal":[m,sd]} on porosity + {"lognormal":[mu,sigma]} on FVF -> spread.
    r = base_oil(porosity={"normal": [0.25, 0.02]}, fvf={"lognormal": [0.22, 0.05]})
    assert r.p90 < r.p50 < r.p10, (r.p90, r.p50, r.p10)


def test_dist_dict_uniform_and_triangular_give_spread():
    r = base_oil(
        area_km2={"uniform": [0.32, 0.52]},
        gross_height_m={"triangular": [12, 15, 20]},
    )
    assert r.p90 < r.p50 < r.p10, (r.p90, r.p50, r.p10)


def test_dist_dict_matches_tuple_shorthand():
    # {"triangular":[a,b,c]} is the explicit form of the (a,b,c) tuple shorthand.
    a = base_oil(area_km2=(0.32, 0.4, 0.52), seed=7)
    b = base_oil(area_km2={"triangular": [0.32, 0.4, 0.52]}, seed=7)
    approx(a.p50, b.p50, rel=0.0, abs_=0.0)
    approx(a.p10, b.p10, rel=0.0, abs_=0.0)


def test_dist_dict_unknown_tag_raises():
    try:
        base_oil(area_km2={"weibull": [1, 2]})
        raise AssertionError("expected ValueError for unknown distribution tag")
    except ValueError:
        pass


def test_dist_dict_wrong_arity_raises():
    for bad in ({"normal": [0.4]}, {"triangular": [0.32, 0.52]}, {"uniform": [1, 2, 3]}):
        try:
            base_oil(area_km2=bad)
            raise AssertionError(f"expected ValueError for wrong arity: {bad}")
        except ValueError:
            pass


def test_dist_dict_invalid_params_raise():
    # sd <= 0 / min >= max are rejected by the underlying distribution.
    try:
        base_oil(area_km2={"normal": [0.4, 0.0]})
        raise AssertionError("expected ValueError for sd=0")
    except ValueError:
        pass


# --- W17: per-realization sample vector --------------------------------------
def test_samples_expose_the_realization_vector():
    r = base_oil(area_km2=(0.32, 0.4, 0.52), gross_height_m=(12, 15, 20), realizations=5000)
    s = r.samples
    assert len(s) == 5000 == r.realizations
    assert all(x > 0.0 for x in s)
    # The sample IS the summary's basis: ~10% of draws fall below P90 (low).
    below_p90 = sum(1 for x in s if x < r.p90) / len(s)
    assert 0.07 < below_p90 < 0.13, below_p90
    assert min(s) <= r.p90 <= r.p50 <= r.p10 <= max(s)


def test_samples_degenerate_for_constant_inputs():
    r = base_oil()  # all-constant -> every realization is the same value
    s = r.samples
    assert len(s) == r.realizations
    approx(min(s), max(s), rel=1e-9)


# --- W16: run_box_model contact is required ----------------------------------
def test_run_box_model_without_contact_raises():
    # The silent contact_m=INFINITY default is gone: a missing contact
    # now fails fast (like Model.__new__), not a silent whole-box volume.
    try:
        peteksim.run_box_model(
            area_km2=0.4, gross_height_m=15.0, porosity=0.25,
            net_to_gross=0.8, water_saturation=0.3, fvf=1.25,
        )
        raise AssertionError("expected ValueError for missing contact_m")
    except ValueError:
        pass


def main():
    print(f"peteksim {peteksim.version()} — functional tests\n")

    # Show a representative run so the numbers are visible, not just asserted.
    demo = base_oil(area_km2=(0.32, 0.4, 0.52), gross_height_m=(12, 15, 20))
    print("Demo (oil, triangular area+height):")
    print(f"  {demo!r}")
    ms = demo.summary_msm3
    print(
        f"  P90={demo.p90:,.0f}  P50={demo.p50:,.0f}  P10={demo.p10:,.0f} Sm3"
        f"  ({ms['p90']:.3f}/{ms['p50']:.3f}/{ms['p10']:.3f} MSm3)"
        f"   GRV={demo.grv_mcm:,.2f} mcm\n"
    )

    for name, fn in sorted(globals().items()):
        if name.startswith("test_") and callable(fn):
            check(name[5:], fn)

    print(f"\n{PASS} passed, {FAIL} failed")
    sys.exit(1 if FAIL else 0)


if __name__ == "__main__":
    main()
