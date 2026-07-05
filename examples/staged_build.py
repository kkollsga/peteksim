#!/usr/bin/env python3
"""The staged model-build headline: eight calls from a Petrel export to a
STOIIP P-curve + tornado, on **synthetic** data.

    VIRTUAL_ENV="$PWD/.venv-srs" .venv-srs/bin/maturin develop -m crates/srs-py/Cargo.toml
    .venv-srs/bin/python examples/staged_build.py

It writes a tiny synthetic Petrel-export tree (see ``synthetic_tree.py``), then
runs the exact eight-call sequence from the design contract against it — through
the built ``peteksim`` wheel — producing a P-curve and tornado in MSm³. No
confidential data is used or produced.
"""

from __future__ import annotations

import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import peteksim as ps  # noqa: E402
from synthetic_tree import build_tree  # noqa: E402


def main(tree_root: str | None = None) -> int:
    if tree_root is None:
        tree_root = tempfile.mkdtemp(prefix="facade-smoke-")
    tree = build_tree(tree_root)
    print(f"peteksim {ps.version()} — staged build over {tree}\n")

    # (1) INGEST — walk the export tree, classify + load every file.
    proj = ps.Project.load(str(tree), crs="SYNTHETIC / UTM zone 00N")
    inv = proj.inventory()
    print("1. Project.load           ", inv)
    print("     surfaces:", inv.surfaces, " wells:", inv.wells, " tops:", inv.tops)
    if inv.skipped:
        print("     skipped:", inv.skipped)

    # (2) FRAMEWORK — declare horizons + outline, hard-tie to well tops.
    fw = proj.framework(
        horizons=["TopReservoir", "BaseReservoir"],
        outline="outline",
        tie_to_tops=True,
    )
    tie = fw.tie_report()
    worst = max((abs(t["residual_m"]) for t in tie if t["ok"]), default=0.0)
    print(f"\n2. framework(tie_to_tops)  {len(tie)} tie rows, worst |residual| = {worst:.3f} m")
    for t in tie:
        flag = "" if t["ok"] else "  <-- FAILED"
        print(f"     {t['horizon']:<14} {t['well']:<4} "
              f"surface={t['surface_m']:.2f}  pick={t['pick_m']:.2f}  "
              f"resid={t['residual_m']:.3f}{flag}")

    # (3) ZONES / LAYERING / GRID — single-ZoneTable capability (documented).
    fw.set_zones({"Reservoir": ("TopReservoir", "BaseReservoir")})
    fw.set_layering({"Reservoir": ps.layers(n=8)})
    grid = fw.build_grid()
    print("\n3. build_grid              cubes so far:", grid.property_names())

    # (4) PROPERTIES — one at a time: upscale (visible/QC) then propagate (SGS).
    por = grid.property("PORO")
    n_por = por.upscale(proj.wells(), method="arithmetic")
    qc = por.qc()
    print(f"\n4. property('PORO')        upscaled {n_por} samples; "
          f"conditioned {qc['conditioned_cells']} cells, "
          f"log_mean={qc['log_mean']:.4f} -> upscaled_mean={qc['upscaled_mean']:.4f}")
    por.propagate(ps.gaussian(ps.spherical(range_m=800.0), seed=1))

    ntg = grid.property("NTG")
    ntg.upscale(proj.wells())
    # A *structural* trend: steer NTG by depth (as_depth=True flips the surface's
    # negative-down elevation to positive-down depth), so +0.4 = NTG increases
    # basinward. (For a net-sand/amplitude trend grid, use the default as_depth=False
    # and pass the trend in its own units.)
    ntg.propagate(
        ps.gaussian(ps.spherical(range_m=800.0), seed=2),
        trend=ps.collocated(proj.surface("TopReservoir"), corr=0.4, as_depth=True),
    )
    print("   property('NTG')         propagated with a collocated trend")

    # (5) MODEL — two contacts (gas cap + oil rim) from the tops picks.
    model = grid.model(
        contacts=dict(goc=proj.tops.pick("GOC"), fwl=proj.tops.pick("FWL")),
        fluid="oil",
        fvf=1.25,
        gas_fvf=0.005,
    )
    # (6) SUMMARY — deterministic volumes.
    s = model.summary()
    print(f"\n5-6. model.summary()       cubes={model.property_names()}")
    print(f"     STOIIP={s['stoiip_msm3']:.3f} MSm3   GIIP={s['giip_msm3']:.3f} MSm3"
          f"   GRV={s['grv_mcm']:.2f} mcm   two_contact={s['two_contact']}")

    # (7) UNCERTAINTY — level shifts on the modelled cubes + contact spread.
    mc = model.uncertainty(
        porosity=ps.level_shift(sd=0.02),
        net_to_gross=ps.level_shift(sd=0.03),
        water_saturation=ps.normal(0.30, 0.03).clamped(0.05, 0.6),
        contacts=ps.pick_spread(sd_m=5.0),
        fvf=ps.normal(1.25, 0.03),
        gas_fvf=ps.normal(0.005, 0.0003),
        n=5000,
        seed=42,
    )
    st = mc.stoiip
    print(f"\n7. uncertainty(n=5000)     {mc!r}")
    print(f"     STOIIP  P90={st['p90_msm3']:.3f}  P50={st['p50_msm3']:.3f}  "
          f"P10={st['p10_msm3']:.3f}  mean={st['mean_msm3']:.3f} MSm3  "
          f"(n={len(st['samples'])})")

    # (8) TORNADO + field aggregation.
    print("\n8. tornado() (top drivers, MSm3 swing):")
    for bar in mc.tornado()[:6]:
        print(f"     {bar['input']:<16} swing={bar['swing'] / 1e6:.3f} "
              f"[{bar['out_lo'] / 1e6:.3f} .. {bar['out_hi'] / 1e6:.3f}]")
    field = ps.aggregate([mc], correlation="independent")
    print(f"\n   aggregate([mc])         field P90/P50/P10 = "
          f"{field['p90_msm3']:.3f}/{field['p50_msm3']:.3f}/{field['p10_msm3']:.3f} MSm3")

    # Sanity for a smoke: an ordered, positive P-curve + successful ties.
    assert st["p90"] < st["p50"] < st["p10"], (st["p90"], st["p50"], st["p10"])
    assert st["p90"] > 0.0
    assert all(t["ok"] for t in tie), "a well-top tie failed"
    print("\nOK — staged build produced an ordered, positive STOIIP P-curve.")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1] if len(sys.argv) > 1 else None))
