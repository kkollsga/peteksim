#!/usr/bin/env python3
"""Generate a tiny **synthetic** Petrel-export tree for the staged-build smoke.

> **The complete suite dataset is now ``peteksim.synth_asset``** (this module's
> format-writing knowledge, grown up: one seeded call emits a full multi-format
> Petrel export tree — CPS-3/EarthVision/IRAP surfaces, CPS-3 polygons, vendor-
> mnemonic LAS, Type="Other" + Latin-1 tops — that ``Project.load`` ingests with
> zero non-noise skips). Prefer it for new work:
>
>     import peteksim as ps
>     m = ps.synth_asset("/tmp/asset", seed=20260704)
>
> The hand-authored builders below stay as the **targeted regression fixtures**
> the F1–F8 / net-mask / dome tests pin to exact content (`build_tree`,
> `build_net_mask_tree`, `build_real_shape_tree`, and `dome_demo.build_dome_tree`).

No confidential data — every surface/log/pick is hand-authored to format spec
(the same shapes petekio's own fixtures use). The tree it writes::

    <root>/
      surfaces/   TopReservoir.irap  BaseReservoir.irap   (negative-down subsea
                  elevation z, m — petekio's documented surface convention)
      polygons/   outline.geojson                          (field footprint)
      wells/      A1/{A1.wellpath,A1.las}  A2/{...}         (vertical wells + logs)
      tops/       picks.tops                                (Petrel well tops)

Geometry (SI, metric): a 21x21 node lattice, 100 m spacing, origin (0,0); a
gentle dome Top (crest ~2000 m at centre) with a constant 40 m gross to Base.
Two wells carry PORO/NTG logs over the reservoir and Horizon picks for the two
horizons plus GOC/FWL contacts (written as Horizon-typed picks, the only kind
petekio's tops loader distributes — a deliberate synthesis choice).

Run standalone to drop a tree somewhere:  python synthetic_tree.py /tmp/tree
"""

from __future__ import annotations

import math
from pathlib import Path

try:  # re-export the complete composer so `from synthetic_tree import synth_asset` works
    from peteksim.synth_asset import synth_asset  # noqa: F401
except ImportError:  # peteksim not importable when reading this file in isolation
    synth_asset = None

# --- lattice + structure -----------------------------------------------------
NCOL = NROW = 21
INC = 100.0
XORI = YORI = 0.0
KB = 25.0  # kelly bushing (m above MSL)
CREST_M = 2000.0
GROSS_M = 40.0
DOME = 3.0e-5  # curvature: +60 m at the corners

# Contacts (TVDSS, positive-down, m)
GOC_M = 2015.0
FWL_M = 2035.0

WELLS = {
    "A1": (600.0, 700.0, 0.225),   # (x, y, mean PORO)
    "A2": (1400.0, 1300.0, 0.205),
}


def top_depth(x: float, y: float) -> float:
    """The Top Reservoir TVDSS at (x, y) — a dome, deeper away from centre."""
    cx = XORI + INC * (NCOL - 1) / 2.0
    cy = YORI + INC * (NROW - 1) / 2.0
    return CREST_M + DOME * ((x - cx) ** 2 + (y - cy) ** 2)


def base_depth(x: float, y: float) -> float:
    return top_depth(x, y) + GROSS_M


# --- IRAP classic surface writer --------------------------------------------
def _write_irap(path: Path, fn) -> None:
    xmax = XORI + INC * (NCOL - 1)
    ymax = YORI + INC * (NROW - 1)
    lines = [
        f"-996 {NROW} {INC} {INC}",
        f"{XORI} {xmax} {YORI} {ymax}",
        f"{NCOL} 0 {XORI} {YORI}",
        "0 0 0 0 0 0 0",
    ]
    # Column-major, x-fastest: k = i + j*ncol.
    vals = []
    for j in range(NROW):
        for i in range(NCOL):
            x = XORI + i * INC
            y = YORI + j * INC
            vals.append(f"{fn(x, y):.4f}")
    # Six per line (as petekio's fixtures do).
    for k in range(0, len(vals), 6):
        lines.append(" ".join(vals[k : k + 6]))
    path.write_text("\n".join(lines) + "\n")


# --- GeoJSON outline ---------------------------------------------------------
def _write_outline(path: Path) -> None:
    x1 = XORI + INC * (NCOL - 1)
    y1 = YORI + INC * (NROW - 1)
    ring = [[XORI, YORI], [x1, YORI], [x1, y1], [XORI, y1], [XORI, YORI]]
    ring_s = ", ".join(f"[{x}, {y}]" for x, y in ring)
    path.write_text(
        '{"type": "FeatureCollection", "features": [{"type": "Feature", '
        '"properties": {"name": "outline"}, "geometry": {"type": "Polygon", '
        f'"coordinates": [[{ring_s}]]}}}}]}}\n'
    )


# --- vertical wellpath -------------------------------------------------------
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
    # Survey stations every 250 m, plus an explicit station at TD so the
    # trajectory spans every pick MD (vertical: TVD == MD, zero inclination).
    mds = [m * 250.0 for m in range(int(td_md // 250.0) + 1)]
    if mds[-1] < td_md:
        mds.append(td_md)
    rows = [f"{md} {x} {y} {-md} {md} 0 0 0 0 0 0" for md in mds]
    path.write_text("\n".join(header + rows) + "\n")


# --- LAS 2.0 log -------------------------------------------------------------
def _write_las(path: Path, well: str, x: float, y: float, mean_poro: float) -> None:
    top_md = top_depth(x, y) + KB
    base_md = base_depth(x, y) + KB
    step = 5.0
    n = int((base_md - top_md) / step) + 1
    rows = []
    for k in range(n):
        md = top_md + k * step
        frac = k / max(n - 1, 1)
        poro = mean_poro + 0.02 * math.sin(frac * math.pi)  # gentle profile
        ntg = 0.78 - 0.10 * frac
        rows.append(f"{md:.2f} {poro:.4f} {ntg:.4f}")
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
        "~ASCII",
        *rows,
    ]
    path.write_text("\n".join(body) + "\n")


# --- Petrel well tops --------------------------------------------------------
def _write_tops(path: Path) -> None:
    head = [
        "# Petrel well tops",
        "VERSION 2",
        "BEGIN HEADER",
        "X", "Y", "Z", "TWT", "TWT2", "age", "MD", "PVD", "Type", "Surface", "Well",
        "END HEADER",
    ]
    rows = []
    for well, (x, y, _poro) in WELLS.items():
        picks = {
            "TopReservoir": top_depth(x, y),
            "BaseReservoir": base_depth(x, y),
            "GOC": GOC_M,
            "FWL": FWL_M,
        }
        for surface, tvdss in picks.items():
            md = tvdss + KB
            z = -tvdss  # negative-down elevation
            rows.append(
                f"{x} {y} {z:.2f} -999 -999 -999 {md:.4f} {z:.2f} "
                f'Horizon "{surface}" "{well}"'
            )
    path.write_text("\n".join(head + rows) + "\n")


def build_tree(root: str | Path) -> Path:
    """Write the whole synthetic tree under `root` and return its path."""
    root = Path(root)
    (root / "surfaces").mkdir(parents=True, exist_ok=True)
    (root / "polygons").mkdir(parents=True, exist_ok=True)
    (root / "tops").mkdir(parents=True, exist_ok=True)
    # Surfaces carry NEGATIVE-down subsea elevation (petekio's convention, like a
    # real Petrel export); top_depth/base_depth are positive-down TVDSS helpers,
    # so negate at the writer — the same flip the tops writer does (z = -tvdss).
    _write_irap(root / "surfaces" / "TopReservoir.irap", lambda x, y: -top_depth(x, y))
    _write_irap(root / "surfaces" / "BaseReservoir.irap", lambda x, y: -base_depth(x, y))
    _write_outline(root / "polygons" / "outline.geojson")
    for well, (x, y, poro) in WELLS.items():
        wdir = root / "wells" / well
        wdir.mkdir(parents=True, exist_ok=True)
        td_md = base_depth(x, y) + KB + 50.0
        _write_wellpath(wdir / f"{well}.wellpath", x, y, td_md)
        _write_las(wdir / f"{well}.las", well, x, y, poro)
    _write_tops(root / "tops" / "picks.tops")
    return root


# --- net-mask tree (net_only upscale regression) -----------------------------
# One vertical well over the reservoir whose section splits cleanly into net
# rock (top half: NTG 0.80, SW 0.24 — hydrocarbon-bearing) and non-net aquifer
# (bottom half: NTG 0.20, SW 0.72). All-sample conditioning averages the two
# (SW ~ 0.48); net_only masks to NTG > 0.5, so the conditioned SW ~ 0.24.
NM_NET_SW = 0.24
NM_AQUIFER_SW = 0.72
NM_NET_NTG = 0.80
NM_AQUIFER_NTG = 0.20


def _write_las_net(path: Path, well: str, x: float, y: float) -> None:
    """A LAS with SW + NTG curves; the top half of the section is net rock, the
    bottom half is non-net aquifer (a hard NTG split across 0.5)."""
    top_md = top_depth(x, y) + KB
    base_md = base_depth(x, y) + KB
    step = 2.0
    n = int((base_md - top_md) / step) + 1
    rows = []
    for k in range(n):
        md = top_md + k * step
        net = k < n // 2  # top half net, bottom half aquifer
        sw = NM_NET_SW if net else NM_AQUIFER_SW
        ntg = NM_NET_NTG if net else NM_AQUIFER_NTG
        rows.append(f"{md:.2f} {sw:.4f} {ntg:.4f}")
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
        " SW.v/v : Water saturation",
        " NTG.v/v : Net to gross",
        "~ASCII",
        *rows,
    ]
    path.write_text("\n".join(body) + "\n")


def build_net_mask_tree(root: str | Path) -> Path:
    """A minimal tree with one well whose log splits net/non-net for the
    `net_only` upscale regression (surfaces + outline + the N1 well)."""
    root = Path(root)
    (root / "surfaces").mkdir(parents=True, exist_ok=True)
    (root / "polygons").mkdir(parents=True, exist_ok=True)
    _write_irap(root / "surfaces" / "TopReservoir.irap", lambda x, y: -top_depth(x, y))
    _write_irap(root / "surfaces" / "BaseReservoir.irap", lambda x, y: -base_depth(x, y))
    _write_outline(root / "polygons" / "outline.geojson")
    x, y = 600.0, 700.0
    wdir = root / "wells" / "N1"
    wdir.mkdir(parents=True, exist_ok=True)
    _write_wellpath(wdir / "N1.wellpath", x, y, base_depth(x, y) + KB + 50.0)
    _write_las_net(wdir / "N1.las", "N1", x, y)
    return root


# =============================================================================
# Real-shape synthetic tree (final-validation F1–F7 regression)
# -----------------------------------------------------------------------------
# Still 100 % synthetic (hand-authored to format spec), but mimics the shape the
# real Petrel export has, which the eight-call facade could not run:
#   - split well layout: wells/Paths/<id>.wellpath + wells/Logs/<id>.las   (F1)
#   - UTM-magnitude coordinates (6–7 digit x/y)                             (F5)
#   - DEVIATED wells (single bore, lateral drift with depth)               (F6, F4)
#   - horizons as scattered .IrapClassicPoints (not grids)                 (F2)
#   - fluid contacts as Petrel Type="Other" rows                          (F3)
#   - a net-sand trend grid in [0,1] for ps.collocated                     (F7)
#   - vendor log mnemonics (PHIE_2025 → PORO alias)                        (API)
# =============================================================================

import math as _math

# UTM-magnitude georeference (ED50/UTM31N-like) + a gently east-dipping reservoir.
RS_X0, RS_Y0 = 431_000.0, 6_521_000.0
RS_DX = RS_DY = 3_000.0
RS_KB = 30.0
RS_INC_DEG = 18.0
RS_TOP0 = 2_000.0     # Top TVDSS at x = RS_X0
RS_DIP = 0.04         # m of depth per m of easting (dips east)
RS_GROSS = 40.0
RS_GOC_M = 2_035.0    # fluid contacts (TVDSS), Type="Other"
RS_FWL_M = 2_065.0

# A thin-margin CROSSING horizon pair (the real point-horizon case). The base is
# authored only `RS_THIN_MARGIN` m below the top at the west edge and narrows
# `RS_THIN_WEDGE` m per m of easting, so over most of the 3 km extent the base
# crosses ABOVE the top — the ~100-300-crossing-node structure that errors under
# the crossing guard and needs `min_thickness_m` to build (blocking wire).
RS_THIN_MARGIN = 3.0
RS_THIN_WEDGE = 0.012

# Wells in the split Paths/Logs layout. `99_9-1` is a **multi-sidetrack** well —
# one wellhead, two deviated bores A/B at different inclinations (so different
# bottomhole eastings on the east-dipping reservoir), each its own comp-log +
# bore-suffixed tops; petekio labels the bores from the two `99_9-1_A/_B.wellpath`
# stems and its top-level accessors then RAISE until a bore is chosen (the R-a
# path). `99_8-1` is single-bore (one wellpath -> petekio's main bore). Both
# exercise id-from-filename discovery (`99_9-1_A` -> well `99_9-1`). Each carries
# a LAS 3.0 core file merged into its first bore (the F8 inventory trace).
RS_WELLS = {
    "99_9-1": {
        "head": (431_800.0, 6_522_000.0),
        "poro": 0.225,
        "bores": {"A": 18.0, "B": 32.0},  # bore label -> inclination (deg), drifts east
    },
    "99_8-1": {
        "head": (432_600.0, 6_522_600.0),
        "poro": 0.205,
        "bores": {"A": 20.0},  # one wellpath -> petekio main bore
    },
}


def _rs_plane(x: float, d0: float) -> float:
    """A horizon TVDSS at easting `x` for a plane at `d0` over RS_X0 (dips east)."""
    return d0 + RS_DIP * (x - RS_X0)


def _rs_solve_pick(hx: float, d0: float, inc_deg: float) -> tuple[float, float, float]:
    """Where a constant-inclination `inc_deg` well (from KB at head `hx`) meets the
    plane `d0 + RS_DIP·(x-RS_X0)`: returns (pick_md, pick_tvdss, bottomhole_x).

    Trajectory: TVD-from-KB = md·cos(i), easting drift = md·sin(i); the facade's
    pick depth = TVD_col − KB, so at the pick MD `md·cos(i) = tvdss + KB`. A more
    inclined bore drifts further east and (dipping east) picks deeper — so two
    bores of one well tie the surface at different depths (F4/R-a per-bore proof).
    """
    i = _math.radians(inc_deg)
    tan_i = _math.tan(i)
    a = hx - RS_X0
    pick = (d0 + RS_DIP * a + RS_DIP * RS_KB * tan_i) / (1.0 - RS_DIP * tan_i)
    md = (pick + RS_KB) / _math.cos(i)
    bottom_x = hx + md * _math.sin(i)
    return md, pick, bottom_x


def _write_irap_points(path: Path, d0: float) -> None:
    """A scattered .IrapClassicPoints horizon (plain `x y z`, NEGATIVE-down
    elevation) sampling the dipping plane on a jittered grid (F2 — the facade
    grids these to the build lattice)."""
    rng = 12345
    rows = []
    n = 22
    for a in range(n):
        for b in range(n):
            # Deterministic pseudo-jitter so the scatter is irregular but stable.
            rng = (1103515245 * rng + 12345) & 0x7FFFFFFF
            jx = (rng / 0x7FFFFFFF - 0.5) * (RS_DX / n)
            rng = (1103515245 * rng + 12345) & 0x7FFFFFFF
            jy = (rng / 0x7FFFFFFF - 0.5) * (RS_DY / n)
            x = RS_X0 + a * (RS_DX / (n - 1)) + jx
            y = RS_Y0 + b * (RS_DY / (n - 1)) + jy
            z = -_rs_plane(x, d0)  # negative-down elevation
            rows.append(f"{x:.3f} {y:.3f} {z:.4f}")
    path.write_text("\n".join(rows) + "\n")


def _write_thin_crossing_points(path: Path, is_base: bool) -> None:
    """A scattered .IrapClassicPoints horizon of a thin-margin CROSSING pair. The
    top is the plane at `RS_TOP0`; the base sits `RS_THIN_MARGIN` m below at the
    west edge but narrows `RS_THIN_WEDGE` m/m east, so it crosses above the top
    over most of the extent — thin/crossing margins the crossing guard rejects and
    `min_thickness_m` repairs (NEGATIVE-down elevation, like the other horizons)."""
    rng = 6789
    rows = []
    n = 22
    for a in range(n):
        for b in range(n):
            rng = (1103515245 * rng + 12345) & 0x7FFFFFFF
            jx = (rng / 0x7FFFFFFF - 0.5) * (RS_DX / n)
            rng = (1103515245 * rng + 12345) & 0x7FFFFFFF
            jy = (rng / 0x7FFFFFFF - 0.5) * (RS_DY / n)
            x = RS_X0 + a * (RS_DX / (n - 1)) + jx
            y = RS_Y0 + b * (RS_DY / (n - 1)) + jy
            top = _rs_plane(x, RS_TOP0)
            tvdss = top + RS_THIN_MARGIN - RS_THIN_WEDGE * (x - RS_X0) if is_base else top
            rows.append(f"{x:.3f} {y:.3f} {-tvdss:.4f}")  # negative-down elevation
    path.write_text("\n".join(rows) + "\n")


def _write_net_sand_trend(path: Path) -> None:
    """A net-sand trend as an IRAP-classic GRID with values in [0.3, 0.7] rising
    east — a value trend (NOT a depth) for ps.collocated(as_depth=False) (F7)."""
    ncol = nrow = 21
    inc = RS_DX / (ncol - 1)
    xmax = RS_X0 + inc * (ncol - 1)
    ymax = RS_Y0 + inc * (nrow - 1)
    lines = [
        f"-996 {nrow} {inc} {inc}",
        f"{RS_X0} {xmax} {RS_Y0} {ymax}",
        f"{ncol} 0 {RS_X0} {RS_Y0}",
        "0 0 0 0 0 0 0",
    ]
    vals = []
    for j in range(nrow):
        for i in range(ncol):
            frac = i / (ncol - 1)
            vals.append(f"{0.3 + 0.4 * frac:.4f}")  # [0.3, 0.7]
    for k in range(0, len(vals), 6):
        lines.append(" ".join(vals[k : k + 6]))
    path.write_text("\n".join(lines) + "\n")


def _write_deviated_wellpath(
    path: Path, hx: float, hy: float, td_md: float, inc_deg: float
) -> None:
    i = _math.radians(inc_deg)
    cos_i, sin_i = _math.cos(i), _math.sin(i)
    header = [
        "# WELL TRACE (synthetic, deviated)",
        f"# WELL HEAD X-COORDINATE: {hx} (m)",
        f"# WELL HEAD Y-COORDINATE: {hy} (m)",
        f"# WELL DATUM (KB, Kelly bushing, from MSL): {RS_KB} (m)",
        "# CRS: SYNTHETIC / ED50 UTM zone 31N",
        "# MD AND TVD ARE REFERENCED AT WELL DATUM",
        "==========",
        "MD X Y Z TVD DX DY AZIM_TN INCL DLS AZIM_GN",
    ]
    mds = [m * 50.0 for m in range(int(td_md // 50.0) + 1)]
    if mds[-1] < td_md:
        mds.append(td_md)
    rows = []
    for md in mds:
        tvd = md * cos_i  # TVD from KB
        x = hx + md * sin_i  # drifts east
        rows.append(f"{md:.3f} {x:.3f} {hy:.3f} {-tvd:.3f} {tvd:.3f} 0 0 90 {inc_deg} 0 90")
    path.write_text("\n".join(header + rows) + "\n")


def _write_core_las(path: Path, well: str, top_md: float, base_md: float) -> None:
    """A **LAS 3.0** core log (delimited sections + a `~Core_Data` block) with a
    core-porosity curve. The filename carries 'core', so petekio tags the curves
    `LogKind::Core` and merges them into the bore whose label appears in the name;
    the facade lists the file in `inventory().merged` (F8) rather than vanishing
    it. A coarse sample every 10 m over the reservoir section."""
    step = 10.0
    n = max(int((base_md - top_md) / step) + 1, 2)
    rows = [
        f"{top_md + k * step:.2f} {0.21 + 0.01 * _math.sin(k):.4f}" for k in range(n)
    ]
    body = [
        "~Version",
        " VERS. 3.0 : CWLS LOG ASCII STANDARD - VERSION 3.0",
        " WRAP. NO  : ONE LINE PER DEPTH STEP",
        " DLM. SPACE : DELIMITING CHARACTER",
        "~Well",
        " NULL. -999.25 : NULL VALUE",
        f" WELL. {well} : WELL NAME",
        "~Core_Definition",
        " DEPTH.M : Core measured depth",
        " CPOR.v/v : Core porosity",
        "~Core_Data | Core_Definition",
        *rows,
    ]
    path.write_text("\n".join(body) + "\n")


def _write_las_utm(path: Path, well: str, top_md: float, base_md: float, mean_poro: float) -> None:
    step = 5.0
    n = max(int((base_md - top_md) / step) + 1, 2)
    rows = []
    for k in range(n):
        md = top_md + k * step
        frac = k / max(n - 1, 1)
        poro = mean_poro + 0.02 * _math.sin(frac * _math.pi)
        ntg = 0.62 - 0.12 * frac
        rows.append(f"{md:.2f} {poro:.4f} {ntg:.4f}")
    body = [
        "~Version",
        " VERS. 2.0 : CWLS LOG ASCII STANDARD - VERSION 2.0",
        " WRAP. NO  : ONE LINE PER DEPTH STEP",
        "~Well",
        f" STRT.M {top_md:.2f} : START DEPTH",
        f" STOP.M {base_md:.2f} : STOP DEPTH",
        f" STEP.M {step} : STEP",
        " NULL. -999.25 : NULL VALUE",
        f" KB.M {RS_KB} : KELLY BUSHING",
        f" WELL. {well} : WELL NAME",
        "~Curve",
        " DEPT.M : Measured depth",
        " PHIE_2025.v/v : Effective porosity (vendor mnemonic)",
        " NTG_PhieLam.v/v : Net to gross",
        "~ASCII",
        *rows,
    ]
    path.write_text("\n".join(body) + "\n")


def _write_tops_other(path: Path) -> None:
    """Petrel well tops: stratigraphic (Type='Horizon') picks per well PLUS the
    fluid contacts as Type='Other' rows (GOC/FWL) — the F3 shape."""
    head = [
        "# Petrel well tops",
        "VERSION 2",
        "BEGIN HEADER",
        "X", "Y", "Z", "TWT", "TWT2", "age", "MD", "PVD", "Type", "Surface", "Well",
        "END HEADER",
    ]
    rows = []
    for well, spec in RS_WELLS.items():
        hx, hy = spec["head"]
        base_id = well.replace("_", "/")  # Petrel-style id ("99_9-1" -> "99/9-1")
        # Bore-suffixed stratigraphic picks: one Top/Base per bore, at that bore's
        # own MD/TVDSS (petekio routes "99/9-1 B" -> well 99/9-1, bore B).
        for bore, inc in spec["bores"].items():
            rec_well = f"{base_id} {bore}"  # e.g. "99/9-1 A"
            top_md, top_tvdss, _bx = _rs_solve_pick(hx, RS_TOP0, inc)
            base_md, base_tvdss, _bx2 = _rs_solve_pick(hx, RS_TOP0 + RS_GROSS, inc)
            for surface, md, tvdss in (
                ("Top Reservoir", top_md, top_tvdss),
                ("Base Reservoir", base_md, base_tvdss),
            ):
                z = -tvdss
                rows.append(
                    f"{hx} {hy} {z:.2f} -999 -999 -999 {md:.4f} {z:.2f} "
                    f'Horizon "{surface}" "{rec_well}"'
                )
        # Field-wide fluid contacts: Type="Other" (dropped by petekio's tops
        # loader; the facade parses them). One GOC/FWL per well; Z is negative-down
        # elevation -> depth = -Z.
        first_bore = next(iter(spec["bores"]))
        rec_well = f"{base_id} {first_bore}"
        for surface, tvdss in [("GOC", RS_GOC_M), ("FWL", RS_FWL_M)]:
            z = -tvdss
            rows.append(
                f"{hx} {hy} {z:.2f} -999 -999 -999 {tvdss + RS_KB:.4f} {z:.2f} "
                f'Other "{surface}" "{rec_well}"'
            )
    path.write_text("\n".join(head + rows) + "\n")


def build_real_shape_tree(root: str | Path) -> Path:
    """Write the real-shape synthetic Petrel-export tree under `root` (F1–F7)."""
    root = Path(root)
    for sub in ("surfaces", "polygons", "tops", "wells/Paths", "wells/Logs"):
        (root / sub).mkdir(parents=True, exist_ok=True)
    # Horizons as scattered point-sets (F2); a net-sand trend as a grid (F7).
    _write_irap_points(root / "surfaces" / "Top Reservoir.IrapClassicPoints", RS_TOP0)
    _write_irap_points(root / "surfaces" / "Base Reservoir.IrapClassicPoints", RS_TOP0 + RS_GROSS)
    # A thin-margin CROSSING pair (blocking `min_thickness_m` wire).
    _write_thin_crossing_points(root / "surfaces" / "Top Thin.IrapClassicPoints", is_base=False)
    _write_thin_crossing_points(root / "surfaces" / "Base Thin.IrapClassicPoints", is_base=True)
    _write_net_sand_trend(root / "surfaces" / "NetSandTrend.irap")
    # Outline over the extent (UTM ring).
    x1, y1 = RS_X0 + RS_DX, RS_Y0 + RS_DY
    ring = [[RS_X0, RS_Y0], [x1, RS_Y0], [x1, y1], [RS_X0, y1], [RS_X0, RS_Y0]]
    ring_s = ", ".join(f"[{x}, {y}]" for x, y in ring)
    (root / "polygons" / "outline.geojson").write_text(
        '{"type": "FeatureCollection", "features": [{"type": "Feature", '
        '"properties": {"name": "outline"}, "geometry": {"type": "Polygon", '
        f'"coordinates": [[{ring_s}]]}}}}]}}\n'
    )
    # Split well layout: surveys under Paths/, logs under Logs/ (F1 + F6). A
    # multi-sidetrack well writes one `<id>_<bore>.wellpath` + `.las` per bore.
    for well, spec in RS_WELLS.items():
        hx, hy = spec["head"]
        poro = spec["poro"]
        for bore, inc in spec["bores"].items():
            top_md, _t, _bx = _rs_solve_pick(hx, RS_TOP0, inc)
            base_md, _b, _bx2 = _rs_solve_pick(hx, RS_TOP0 + RS_GROSS, inc)
            name = f"{well}_{bore}"
            _write_deviated_wellpath(
                root / "wells" / "Paths" / f"{name}.wellpath", hx, hy, base_md + 60.0, inc
            )
            _write_las_utm(root / "wells" / "Logs" / f"{name}.las", name, top_md, base_md, poro)
        # F8: a LAS 3.0 core file merged into the first bore (leaves a merge trace).
        first_bore, first_inc = next(iter(spec["bores"].items()))
        c_top, _ct, _ = _rs_solve_pick(hx, RS_TOP0, first_inc)
        c_base, _cb, _ = _rs_solve_pick(hx, RS_TOP0 + RS_GROSS, first_inc)
        _write_core_las(
            root / "wells" / "Logs" / f"{well}_{first_bore}_core.las",
            f"{well}_{first_bore}",
            c_top,
            c_base,
        )
    # Real Petrel tops export ships with NO extension (name carries "Tops").
    _write_tops_other(root / "tops" / "FieldTops")
    return root


if __name__ == "__main__":
    import sys

    dest = sys.argv[1] if len(sys.argv) > 1 else "./synthetic_tree"
    shape = sys.argv[2] if len(sys.argv) > 2 else "simple"
    out = build_real_shape_tree(dest) if shape == "real" else build_tree(dest)
    print(f"synthetic Petrel-export tree ({shape}) written to {out}")
