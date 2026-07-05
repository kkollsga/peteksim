#!/usr/bin/env python3
"""Build a petekSim model **in code**, then view it.

    .venv-srs/bin/python examples/build_and_view.py          # structured model
    .venv-srs/bin/python examples/build_and_view.py --box     # box Monte Carlo model

Each builds a model and calls `model.view()`, which opens a three.js render in
your browser and serves until you press Ctrl-C. (`python` is shell-aliased, so
call the venv interpreter explicitly.)
"""

import sys

import peteksim


def build_structured():
    """A structured grid with real 3D relief: add a crestal high in code."""
    m = peteksim.Model(
        0.4, 15.0,                  # area (km²), gross height (m)
        ni=24, nj=24, nk=8,
        top_m=1500.0,               # depths in m, positive down
        contact_m=1510.5,           # mid-column -> a dome-shaped trapped cap
        porosity=0.25, net_to_gross=0.8, water_saturation=0.3,
        fvf=1.25, fluid="oil",      # FVF in Rm³/Sm³
    )
    m.add_control(12, 12, 1489.0)   # a structural high at the crest (m)
    print(m.solve())                # in-place [Sm³], GRV [mcm]
    return m


def build_box():
    """A box model with Monte Carlo uncertainty on area + height."""
    m = peteksim.run_box_model(
        area_km2=(0.32, 0.4, 0.52),     # km²
        gross_height_m=(12, 15, 20),    # m
        porosity=0.25, net_to_gross=0.8, water_saturation=0.3, fvf=1.25,
        fluid="oil", top_m=1500.0, contact_m=2743.0,
        ni=12, nj=12, nk=6, realizations=20000, seed=1,
    )
    print(m)                            # percentiles in Sm³ (see m.summary_msm3)
    return m


if __name__ == "__main__":
    model = build_box() if "--box" in sys.argv else build_structured()
    model.view()   # opens the browser; Ctrl-C to stop
