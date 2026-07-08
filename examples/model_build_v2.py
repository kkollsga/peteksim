#!/usr/bin/env python3
"""Current suite modelling shape on the canonical synthetic asset.

petekSim creates the synthetic export and remains the appraisal/product layer.
petekIO imports the raw project tree. petekStatic owns static grid declaration,
property setup, log-upscale recipes, and volumetrics.

No confidential data is used or produced.
"""

from __future__ import annotations

import sys
import tempfile

import petekio as pio
import peteksim as ps
import petekstatic as pst


def main(root: str | None = None) -> int:
    root = root or tempfile.mkdtemp(prefix="model-v2-")
    manifest = ps.synth_asset(root, seed=20260704, n_wells=4)
    print(f"peteksim {ps.version()} synthetic export: {manifest['root']}\n")

    project = pio.Project.import_data(
        manifest["root"],
        settings=pio.ImportSettings(
            crs=manifest["crs"],
            aliases=manifest["aliases"],
        ),
    )
    print("Project.import_data:", project.inventory()["counts"])
    print("project.surfaces:", project.surfaces)
    print("project.wells.logs:", project.wells.logs)

    grid = (
        pst.Grid.from_project(project)
        .geometry(cell=(50.0, 50.0), orient=0.0, outline="ModelEdge")
        .horizons(
            [
                {
                    "name": "Top reservoir",
                    "surface": manifest["horizons"][0],
                    "well top": "FieldWellTops/H0",
                    "zone": "Reservoir",
                },
                {
                    "name": "Base reservoir",
                    "surface": manifest["horizons"][-1],
                    "well top": "FieldWellTops/H6",
                },
            ],
            well_tie={"influence_radius": 800},
        )
        .layers({"Reservoir": pst.Layering(n=2)})
    )

    p = grid.properties
    p.ntg = 0.80
    p.por = p.ntg * 0.85
    p.sw = 0.20

    result = grid.volumes(ntg="ntg", por="por", sw="sw", fluid="oil", fvf=1.30).run()
    summary = result.summary()
    print(
        "\nstatic volumes:",
        f"GRV={summary['grv_m3']:.0f} m3",
        f"HCPV={summary['hcpv_m3']:.0f} m3",
        f"OOIP={summary['ooip_sm3']:.0f} Sm3",
    )

    logs = project.wells.logs
    vgm = pst.Var("spherical", major=1500, minor=700, vertical=20, azimuth=35)
    recipe = pst.upscale(logs.PORO(logs.NTG > 0.50)).sgs(
        distribution=pst.distributions.from_logs(),
        variogram=vgm,
        seed=12,
    )
    spec = recipe.lower("PORO_NET", project=project)
    print(
        "log-upscale recipe:",
        spec.property,
        f"{len(spec.well_logs or ())} wells",
        spec.variogram,
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1] if len(sys.argv) > 1 else None))
