#!/usr/bin/env python3
"""The dome demo — the suite's demo-face for the multi-zone horizon stack.

Ports the petekStatic multi-zone acceptance fixture's *recipe* onto the peteksim
facade path: a procedural 4-way dip closure (an elliptical dome) + a stacked
sequence of clamped isochores built DOWN from the top (so pinch-outs are natural),
a handful of vertical wells carrying residual tops + first-cut zone-target logs,
and one **tops-only** internal horizon (well picks, no mapped surface). All data
is SYNTHETIC and seeded — an arbitrary fictional study area, no dataset content.

It then drives the eight-call facade with ``fw.set_zonation([...])`` to build the
zoned StaticModel, reads per-zone volumes (``in_place_by_zone``) + per-zone stats
(``zone_stats``), runs zoned Monte Carlo for the P-curve/distribution charts, and
writes a self-contained viewer HTML the Playwright harness screenshots (volume /
crest section / map).

    VIRTUAL_ENV="$PWD/.venv-srs" .venv-srs/bin/maturin develop -m crates/srs-py/Cargo.toml
    .venv-srs/bin/python examples/dome_demo.py            # build + save_view HTML

The build factory is importable (``build_dome_tree`` / ``zonation_for``) so the
pytest round-trip reuses the exact same synthetic stack.
"""

from __future__ import annotations

import math
import random
import sys
import tempfile
from pathlib import Path

# --- study area + procedural stack (mirrors the engine fixture SHAPE) ---------
INC = 100.0
# A fictional study-area window (NOT the vicinity used elsewhere in the repo).
ORIGIN_X = 700_000.0
ORIGIN_Y = 7_100_000.0
KB = 25.0  # kelly bushing (m above MSL)
BASE_DEPTH = 2000.0  # regional level (TVDSS, positive-down)
CREST_RELIEF = 80.0  # crest-to-spill relief of the top closure

# 10 interval (isochore) targets: (mean thickness m, sd m). Index 8 is thin +
# high-variance so its clamped isochore pinches out in places (truncation).
INTERVALS = [
    (12.0, 3.0),
    (13.0, 3.0),
    (9.0, 2.5),
    (14.0, 3.5),
    (12.0, 3.0),
    (11.0, 3.0),
    (14.0, 3.5),
    (7.0, 2.0),
    (5.0, 4.0),  # pinch-prone
    (12.0, 3.0),
]
N_HORIZON = len(INTERVALS) + 1  # 11 horizons -> 10 zones
TOPS_ONLY_IDX = 5  # H5 is an untied internal split (well picks, no surface)
SEED = 20_260_704

HORIZONS = [f"H{k}" for k in range(N_HORIZON)]
ZONES = [f"Z{z}" for z in range(N_HORIZON - 1)]


def _top_trend(u: float, v: float) -> float:
    """The noiseless top surface: a 4-way dip closure (an elongated ~5:3 elliptical
    dome, crest interior) + a gentle regional tilt. ``u, v`` are 0..1 fractions."""
    tilt = 8.0 * u + 4.0 * v  # ~12 m across, << relief
    ex, ey = (u - 0.5) / 0.32, (v - 0.5) / 0.19
    dome = -CREST_RELIEF * math.exp(-(ex * ex + ey * ey))
    return BASE_DEPTH + tilt + dome


def _smooth_field(ncol: int, amp: float, seed: int) -> list[list[float]]:
    """A deterministic, spatially smooth field (a few seeded sinusoids) — stands in
    for the fixture's SGS-correlated noise without a numeric dependency. Zero-mean,
    amplitude ~``amp``."""
    rng = random.Random(seed)
    comps = [
        (rng.uniform(0.4, 1.8), rng.uniform(0.4, 1.8), rng.uniform(0.0, 2 * math.pi), rng.uniform(-1.0, 1.0))
        for _ in range(5)
    ]
    out = [[0.0] * ncol for _ in range(ncol)]
    for j in range(ncol):
        for i in range(ncol):
            u, v = i / (ncol - 1), j / (ncol - 1)
            s = sum(w * math.sin(2 * math.pi * (fx * u + fy * v) + ph) for (fx, fy, ph, w) in comps)
            out[j][i] = amp * s / len(comps)
    return out


def build_surfaces(ncol: int, seed: int = SEED) -> list[list[list[float]]]:
    """The 11 horizon node-depth fields (positive-down TVDSS), ordered top→down by
    construction: ``surf[k+1] = surf[k] + max(isochore_k, 0)`` — no crossing
    possible, realistic thickness variation, and pinch-outs."""
    top = [[_top_trend(i / (ncol - 1), j / (ncol - 1)) for i in range(ncol)] for j in range(ncol)]
    noise = _smooth_field(ncol, 0.04 * CREST_RELIEF, seed)
    top = [[top[j][i] + noise[j][i] for i in range(ncol)] for j in range(ncol)]
    surfaces = [top]
    for k, (mean, sd) in enumerate(INTERVALS):
        iso = _smooth_field(ncol, sd, seed + 101 + k)
        prev = surfaces[-1]
        nxt = [[prev[j][i] + max(mean + iso[j][i], 0.0) for i in range(ncol)] for j in range(ncol)]
        surfaces.append(nxt)
    return surfaces


# --- file writers ------------------------------------------------------------
def _write_irap(path: Path, depth: list[list[float]], ncol: int) -> None:
    """IRAP-classic grid of **negative-down elevation** (= -TVDSS), column-major
    x-fastest — the convention petekio's surface loader + the facade expect."""
    xmax = ORIGIN_X + INC * (ncol - 1)
    ymax = ORIGIN_Y + INC * (ncol - 1)
    lines = [
        f"-996 {ncol} {INC} {INC}",
        f"{ORIGIN_X} {xmax} {ORIGIN_Y} {ymax}",
        f"{ncol} 0 {ORIGIN_X} {ORIGIN_Y}",
        "0 0 0 0 0 0 0",
    ]
    vals = [f"{-depth[j][i]:.4f}" for j in range(ncol) for i in range(ncol)]
    for k in range(0, len(vals), 6):
        lines.append(" ".join(vals[k : k + 6]))
    path.write_text("\n".join(lines) + "\n")


def _write_outline(path: Path, ncol: int) -> None:
    x1 = ORIGIN_X + INC * (ncol - 1)
    y1 = ORIGIN_Y + INC * (ncol - 1)
    ring = [[ORIGIN_X, ORIGIN_Y], [x1, ORIGIN_Y], [x1, y1], [ORIGIN_X, y1], [ORIGIN_X, ORIGIN_Y]]
    ring_s = ", ".join(f"[{x}, {y}]" for x, y in ring)
    path.write_text(
        '{"type": "FeatureCollection", "features": [{"type": "Feature", '
        '"properties": {"name": "outline"}, "geometry": {"type": "Polygon", '
        f'"coordinates": [[{ring_s}]]}}}}]}}\n'
    )


def _write_wellpath(path: Path, x: float, y: float, td_md: float) -> None:
    header = [
        "# WELL TRACE (synthetic)",
        f"# WELL HEAD X-COORDINATE: {x} (m)",
        f"# WELL HEAD Y-COORDINATE: {y} (m)",
        f"# WELL DATUM (KB, Kelly bushing, from MSL): {KB} (m)",
        "# CRS: SYNTHETIC / UTM zone 00N",
        "# MD AND TVD ARE REFERENCED AT WELL DATUM",
        "==========",
        "MD X Y Z TVD DX DY AZIM_TN INCL DLS AZIM_GN",
    ]
    mds = [m * 250.0 for m in range(int(td_md // 250.0) + 1)]
    if mds[-1] < td_md:
        mds.append(td_md)
    rows = [f"{md} {x} {y} {-md} {md} 0 0 0 0 0 0" for md in mds]
    path.write_text("\n".join(header + rows) + "\n")


def _sample(field: list[list[float]], ncol: int, x: float, y: float) -> float:
    """Nearest-node value of a node field at world (x, y)."""
    i = min(max(round((x - ORIGIN_X) / INC), 0), ncol - 1)
    j = min(max(round((y - ORIGIN_Y) / INC), 0), ncol - 1)
    return field[j][i]


def _write_las(path: Path, well: str, top_tvd: float, base_tvd: float, mean_poro: float) -> None:
    """A first-cut zone-target log: PORO/NTG/SW over the reservoir, with a gentle
    profile (better rock mid-column) so the cubes + zone_stats read sensibly."""
    top_md, base_md = top_tvd + KB, base_tvd + KB
    step = 4.0
    n = max(int((base_md - top_md) / step) + 1, 2)
    rows = []
    for k in range(n):
        md = top_md + k * step
        frac = k / (n - 1)
        poro = mean_poro + 0.03 * math.sin(frac * math.pi)
        ntg = 0.85 - 0.20 * abs(frac - 0.4)
        sw = 0.22 + 0.10 * frac
        rows.append(f"{md:.2f} {poro:.4f} {max(ntg,0.0):.4f} {sw:.4f}")
    body = [
        "~Version",
        " VERS. 2.0 : CWLS LOG ASCII STANDARD - VERSION 2.0",
        " WRAP. NO  : ONE LINE PER DEPTH STEP",
        "~Well",
        f" STRT.M {top_md:.2f} : START DEPTH",
        f" STOP.M {base_md:.2f} : STOP DEPTH",
        f" STEP.M {step} : STEP",
        " NULL. -999.25 : NULL VALUE",
        f" KB.M {KB} : KELLY BUSHING",
        f" WELL. {well} : WELL NAME",
        "~Curve",
        " DEPT.M : Measured depth",
        " PORO.v/v : Porosity",
        " NTG.v/v : Net to gross",
        " SW.v/v : Water saturation",
        "~ASCII",
        *rows,
    ]
    path.write_text("\n".join(body) + "\n")


def _write_tops(path: Path, picks: list[tuple[float, float, float, str, str]]) -> None:
    """Petrel well-tops file: `(x, y, tvdss, surface, well)` rows, Type=Horizon."""
    head = [
        "# Petrel well tops",
        "VERSION 2",
        "BEGIN HEADER",
        "X", "Y", "Z", "TWT", "TWT2", "age", "MD", "PVD", "Type", "Surface", "Well",
        "END HEADER",
    ]
    rows = []
    for x, y, tvdss, surface, well in picks:
        md = tvdss + KB
        z = -tvdss
        rows.append(
            f"{x} {y} {z:.2f} -999 -999 -999 {md:.4f} {z:.2f} "
            f'Horizon "{surface}" "{well}"'
        )
    path.write_text("\n".join(head + rows) + "\n")


# --- well layout (crest + flanks) --------------------------------------------
def _well_layout(ncol: int) -> dict[str, tuple[float, float, float]]:
    """5 wells across the structure: (x, y, mean PORO). One near the crest (best
    rock), the rest on the flanks."""
    span = INC * (ncol - 1)
    frac = {
        "99/1-1": (0.50, 0.50, 0.26),  # crest
        "99/2-1": (0.30, 0.35, 0.22),
        "99/3-1": (0.70, 0.35, 0.23),
        "99/4-1": (0.35, 0.68, 0.21),
        "99/5-1": (0.68, 0.66, 0.22),
    }
    return {w: (ORIGIN_X + fx * span, ORIGIN_Y + fy * span, p) for w, (fx, fy, p) in frac.items()}


def build_dome_tree(root: str | Path, ncol: int = 41, seed: int = SEED) -> Path:
    """Write the synthetic dome-stack Petrel export tree; return its root path.

    - ``surfaces/`` — 10 mapped IRAP horizons (H5 is omitted — tops-only).
    - ``wells/`` — 5 vertical wells + PORO/NTG/SW logs.
    - ``tops/`` — Horizon picks: H5 for every well (with a small residual), plus H0
      + H8 residual picks (so tie residuals are non-trivial).
    """
    root = Path(root)
    (root / "surfaces").mkdir(parents=True, exist_ok=True)
    (root / "wells").mkdir(parents=True, exist_ok=True)
    (root / "tops").mkdir(parents=True, exist_ok=True)

    surfaces = build_surfaces(ncol, seed)
    for k, surf in enumerate(surfaces):
        if k == TOPS_ONLY_IDX:
            continue  # tops-only — no mapped surface
        _write_irap(root / "surfaces" / f"{HORIZONS[k]}.irap", surf, ncol)
    _write_outline(root / "surfaces" / "outline.geojson", ncol)

    wells = _well_layout(ncol)
    picks: list[tuple[float, float, float, str, str]] = []
    for well, (x, y, poro) in wells.items():
        wid = well.replace("/", "_")  # the id petekio derives from the wellpath stem
        wdir = root / "wells" / wid
        wdir.mkdir(parents=True, exist_ok=True)
        top_tvd = _sample(surfaces[0], ncol, x, y)
        base_tvd = _sample(surfaces[-1], ncol, x, y)
        _write_wellpath(wdir / f"{wid}.wellpath", x, y, base_tvd + KB + 40.0)
        _write_las(wdir / f"{wid}.las", well, top_tvd, base_tvd, poro)
        # Residual tops: H5 (tops-only, required) + H0/H8 (mapped, small mis-tie).
        # The Well column must match the wellpath-derived id (underscore form).
        picks.append((x, y, _sample(surfaces[TOPS_ONLY_IDX], ncol, x, y) + 0.8, "H5", wid))
        picks.append((x, y, _sample(surfaces[0], ncol, x, y) + 0.5, "H0", wid))
        picks.append((x, y, _sample(surfaces[8], ncol, x, y) - 0.6, "H8", wid))
    _write_tops(root / "tops" / "dome_tops.tops", picks)
    return root


def zonation_for(
    ncol: int = 41, seed: int = SEED, nk: int | None = None, dz_m: float | None = None
) -> list[dict]:
    """The 10-zone zonation for the dome stack — per-zone conformity + contacts,
    with contact depths derived from the generated surfaces so the two-contact zone
    genuinely splits into a gas cap + oil rim and the single-OWC zone rings the
    structure. Zones 2 (single OWC) and 4 (GOC + OWC) hold hydrocarbon; the rest
    are contactless. Returns the list of dicts for ``fw.set_zonation``.

    ``nk`` / ``dz_m`` override the per-zone layer allocation (a coarse override
    keeps the round-trip *tests* fast — the demo uses ~1 m layering)."""
    surfaces = build_surfaces(ncol, seed)

    def crest(k: int) -> float:
        return min(min(row) for row in surfaces[k])

    zones = []
    for z in range(N_HORIZON - 1):
        base_h = HORIZONS[z + 1]
        conformity = "follow_top" if z == 3 else "proportional"
        entry: dict = {"zone": ZONES[z], "below_horizon": base_h, "conformity": conformity}
        if conformity == "follow_top":
            entry["dz_m"] = dz_m if dz_m is not None else 1.0
        else:
            entry["nk"] = nk if nk is not None else max(int(round(INTERVALS[z][0])), 1)
        if z == 2:
            # Single OWC part-way down the zone top's relief → an oil ring.
            entry["contacts"] = {"owc": crest(2) + 40.0}
        elif z == 4:
            # A genuine gas-cap + oil-rim split near the crest of the zone.
            c = crest(4)
            entry["contacts"] = {"goc": c + 22.0, "fwl": c + 44.0}
        else:
            entry["contacts"] = None
        zones.append(entry)
    return zones


# --- the demo run ------------------------------------------------------------
def _build_zoned(proj, ncol: int):
    """The zoned build: framework over all 11 horizons (H5 tops-only) + set_zonation
    + PORO/NTG/SW property pipelines + build. Returns the zoned StaticModel."""
    import peteksim as ps

    fw = proj.framework(horizons=HORIZONS, outline="outline", tie_to_tops=True, min_thickness_m=0.0)
    fw.set_zonation(zonation_for(ncol))
    grid = fw.build_grid()
    # PORO is modelled (colours the views + drives zone_stats); NTG/SW stay at
    # priors — enough for the demo-face and keeps the 10-zone SGS build brisk.
    por = grid.property("PORO")
    por.upscale(proj.wells())
    por.propagate(ps.gaussian(ps.spherical(range_m=1500.0), seed=11))
    return grid.model(fluid="oil", fvf=1.30, gas_fvf=0.005, wells=proj.wells())


def _whole_field_mc(proj):
    """A whole-field (top→base, single-zone) build + MC for the flat tornado chart.
    The main zoned model uses ``zoned_uncertainty``; this helper keeps the demo's
    flat sensitivity chart available too. Returns the Uncertainty result."""
    import peteksim as ps

    surfaces = build_surfaces(41)
    deep = max(max(row) for row in surfaces[-1]) + 60.0
    fw = proj.framework(horizons=["H0", "H10"], outline="outline", tie_to_tops=False)
    grid = fw.build_grid()
    por = grid.property("PORO")
    por.upscale(proj.wells())
    por.propagate(ps.gaussian(ps.spherical(range_m=1500.0), seed=1))
    model = grid.model(contacts={"fwl": deep}, fluid="oil", fvf=1.30)
    # PORO is modelled (level-shift its pattern); NTG/SW stay at priors (draw the
    # value directly, clamped into range).
    return model.uncertainty(
        porosity=ps.level_shift(sd=0.02),
        net_to_gross=ps.normal(0.80, 0.04).clamped(0.4, 1.0),
        water_saturation=ps.normal(0.30, 0.03).clamped(0.05, 0.6),
        contacts=ps.pick_spread(sd_m=5.0),
        fvf=ps.normal(1.30, 0.03),
        n=2000,
        seed=42,
    )


def main() -> int:
    sys.path.insert(0, str(Path(__file__).resolve().parent))
    import peteksim as ps

    ncol = 41
    tree = build_dome_tree(tempfile.mkdtemp(prefix="dome-demo-"), ncol=ncol)
    print(f"peteksim {ps.version()} — dome demo over {tree}\n")

    proj = ps.Project.load(str(tree), crs="SYNTHETIC / UTM zone 00N")
    print("1. load                    ", proj.inventory())

    model = _build_zoned(proj, ncol)
    print(f"2. zoned build             zoned={model.is_zoned()} cubes={model.property_names()}")

    for w in model.warnings():
        print(f"     warning [{w['kind']}] {w['message']}")

    z = model.in_place_by_zone()
    print("\n3. in_place_by_zone (per-zone volumes):")
    for row in z["zones"]:
        tag = "GAS+OIL" if row["two_contact"] else ("OIL " if row["stoiip_msm3"] > 1e-6 else "----")
        print(f"     {row['zone']:<4} {tag:<8} GRV={row['grv_mcm']:9.2f} mcm  "
              f"STOIIP={row['stoiip_msm3']:8.3f} MSm3  GIIP={row['giip_bcm']:7.4f} bcm")
    t = z["total"]
    print(f"     {'TOTAL':<4} {'':<8} GRV={t['grv_mcm']:9.2f} mcm  "
          f"STOIIP={t['stoiip_msm3']:8.3f} MSm3  GIIP={t['giip_bcm']:7.4f} bcm")

    print("\n4. zone_stats('PORO'):")
    for s in model.zone_stats("PORO"):
        print(f"     {s['zone']:<4} n={s['count']:>5}  mean={s['mean']:.4f}  "
              f"[{s['min']:.4f} .. {s['max']:.4f}]")

    # Whole-field MC for the charts (a fresh project — the zoned grid is consumed).
    proj2 = ps.Project.load(str(tree), crs="SYNTHETIC / UTM zone 00N")
    mc = _whole_field_mc(proj2)
    print(f"\n5. whole-field MC          {mc!r}")

    # A crest section line (SW→NE through the dome centre) so the section tab shows
    # the zone stack + contacts.
    span = INC * (ncol - 1)
    crest_line = [
        [ORIGIN_X + 0.12 * span, ORIGIN_Y + 0.20 * span],
        [ORIGIN_X + 0.50 * span, ORIGIN_Y + 0.50 * span],
        [ORIGIN_X + 0.88 * span, ORIGIN_Y + 0.80 * span],
    ]
    charts = [mc.tornado_bundle(units="MSm³"), mc.distribution_bundle()]
    out = Path(__file__).resolve().parent.parent
    out = Path(tempfile.gettempdir()) / "peteksim-demo"
    out.mkdir(parents=True, exist_ok=True)
    html = out / "dome_view.html"
    model.save_view(str(html), property="PORO", lines=[crest_line], charts=charts)
    print(f"\n6. save_view               {html}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
