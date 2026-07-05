#!/usr/bin/env python3
"""The modelling API v2 headline: declarative specs applied at explicit moments,
from a (synthetic) Petrel export to a per-zone STOIIP P-curve — on the canonical
synthetic asset.

    VIRTUAL_ENV="$PWD/.venv-srs" .venv-srs/bin/maturin develop -m crates/srs-py/Cargo.toml
    .venv-srs/bin/python examples/model_build_v2.py

A spec says WHAT/HOW and holds NAMES (resolved at apply); applications are the
explicit moments geom.build / grid.model / model.zoned_uncertainty. No
confidential data is used or produced.
"""

from __future__ import annotations

import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import peteksim as ps  # noqa: E402


def main(root: str | None = None) -> int:
    root = root or tempfile.mkdtemp(prefix="model-v2-")
    man = ps.synth_asset(root)
    print(f"peteksim {ps.version()} — modelling API v2 over {man['root']}\n")

    # INGEST — a LoadSettings spec is a value (crs + alias canonicalisation).
    proj = ps.Project.load(
        man["root"], settings=ps.LoadSettings(crs=man["crs"], aliases=man["aliases"]))
    print("Project.load  ", proj.inventory())

    # DECLARE — structure + settings as names, resolved at apply.
    hz = ps.Horizons(
        *[ps.hz(h) for h in man["horizons"]],
        zones=man["zones"],
        ties=ps.TieSettings(method="convergent"),
        gridding=ps.Gridding(collapse=True),
    )
    lay = ps.Layering(nk=2)
    con = ps.Contacts({z["zone"]: dict(z["contacts"])
                       for z in man["zonation"] if z["contacts"]})
    props = ps.Props(
        ps.Prop("PORO", net_only=True,
                propagate=ps.Propagate(variogram=ps.variogram("spherical", 1500.0), seed=11)),
        ps.Prop("NTG",
                propagate=ps.Propagate(variogram=ps.variogram("spherical", 1500.0), seed=12)),
    )
    print("\nHorizons spec (a value — prints as its stratigraphic column):")
    print(hz)

    # APPLY — the explicit moments.
    geom = proj.grid_geometry(cell=(50.0, 50.0), orient=0)
    grid = geom.build(hz, layering=lay, collapse_negative=True, min_thickness_m=0.0)
    model = grid.model(props, con, fluid="oil", fvf=1.30, gas_fvf=0.005, wells=proj.wells())
    print("\nmodel:", model, " is_zoned:", model.is_zoned())

    for r in model.in_place_by_zone()["zones"]:
        print(f"  {r['zone']:5s}  STOIIP={r['stoiip_sm3']/1e6:8.3f} MSm³  "
              f"two_contact={r['two_contact']}")

    # MC — one Mc spec, auto-routed to the zoned run.
    mc = model.zoned_uncertainty(ps.Mc(porosity=0.01, contacts=4.0, goc=3.0, n=2000, seed=42))
    t = mc.total["stoiip"]
    print(f"\nfield STOIIP P90/P50/P10 = "
          f"{t['p90_msm3']:.3f} / {t['p50_msm3']:.3f} / {t['p10_msm3']:.3f} MSm³")

    # SCENARIO — a derived spec (deeper Z4 FWL): same geometry, a new model.
    deeper = con.replace("Z4", goc=man["contacts"]["goc_z4"],
                         fwl=man["contacts"]["fwl_z4"] + 30.0)
    model_b = grid.model(props, deeper, fluid="oil", fvf=1.30, gas_fvf=0.005)
    a = model.in_place_by_zone()["total"]["stoiip_sm3"] / 1e6
    b = model_b.in_place_by_zone()["total"]["stoiip_sm3"] / 1e6
    print(f"\nscenario derivation: base total {a:.3f} MSm³ → deeper-FWL {b:.3f} MSm³")

    # A scenario is a savable file: round-trip the whole asset spec through a dict.
    asset = ps.AssetSpec(name="demo", load=ps.LoadSettings(crs=man["crs"]),
                         horizons=hz, layering=lay, contacts=con, props=props)
    assert ps.spec_from_dict(asset.to_dict()) == asset
    print("\nAssetSpec round-trips through to_dict/from_dict (durable scenario).")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1] if len(sys.argv) > 1 else None))
