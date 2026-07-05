"""The complete synthetic asset — ONE seeded call emits a full fake Petrel-export
tree, structurally indistinguishable from a real export, that ``peteksim.Project.load``
ingests with zero non-noise skips.

``synth_asset(root, seed=...)`` composes the **petektools** synthetic-data
generators (dome + isochore build-down + rho-correlated depositional trend +
seeded well placement + MIXED vertical/deviated trajectories + surface picks +
the COUPLED por/ntg petro generator) and writes the result in the REAL Petrel
export formats (IRAP classic points + EarthVision grid + CPS-3 grid surfaces,
CPS-3 line polygons, LAS 2.0 vendor-mnemonic comp-logs, `.wellpath` trajectories,
an extensionless Petrel well-tops file with Type="Other" contacts + a Latin-1
name row). It is deterministic per seed and returns a manifest of the *planted
truth* so a consumer can check what the pipeline recovers.

    import peteksim as ps
    m = ps.synth_asset("/tmp/asset", seed=20260704)
    proj = ps.Project.load(m["root"], crs=m["crs"], aliases=m["aliases"])

This is THE suite dataset — all suite validations / demos / ingest tests run on
it; ``examples/synthetic_tree.py`` is a thin call into the format writers here.
No confidential data: an arbitrary fictional study area, fictional 99/x-y ids,
every value generated from the seed.

Asset **v2** (2026-07-04, ``asset_version == 2``) makes the tree structurally
isomorphic to the canonical real model the testing doctrine derives from
(`petekSuite/dev-docs/designs/testing-doctrine.md`): a MIXED well program (some
vertical, some deviated build-hold / build-hold-drop bores that cross several
100 m columns at reservoir depth — logs+tops follow the true trajectory x,y), a
TOPS-ONLY internal split horizon (well picks, NO mapped surface — the
conformal-drape case), per-zone contacts spanning a two-contact zone (GOC+FWL), a
single-contact zone (OWC) and CONTACTLESS zones (GRV only), a zone that genuinely
PINCHES OUT to sub-threshold/zero thickness across part of the extent (R5
degenerate-column food), a single fictional WORLD georef (431000/6521000) end to
end via the petektools ``Georef`` idiom, and a documented spill-forcing size
recipe. Every v1 call keeps working and every v1 manifest key is preserved; v2
adds new planted-truth keys alongside them.
"""

from __future__ import annotations

import math
from pathlib import Path

import petektools as pt

# --- asset version -----------------------------------------------------------
ASSET_VERSION = 2

# --- study area (arbitrary fictional UTM-magnitude window) --------------------
X0, Y0 = 431_000.0, 6_521_000.0  # the fictional WORLD georef origin (doctrine R1)
INC = 100.0
NCOL = NROW = 41                 # 4 km square
KB = 30.0                        # kelly bushing, m above MSL
TOP_DATUM = 2_000.0              # regional Top TVDSS at zero relief (positive-down)
DOME_RELIEF = 80.0               # crest-to-flank relief (crest is shallowest)
DOME_ASPECT = 1.6                # elongation
DOME_TILT = 12.0                 # gentle regional tilt (m across the extent)
LOG_STEP = 0.5                   # LAS sample spacing (m)
NET_CUTOFF = 0.10  # owner ruling = petektools DEFAULT_NET_CUTOFF; coupled generator derives net_flag = phie >= cutoff
RHO = 0.6                        # planted collocated correlation (the killer test recovers this)

# Vendor-suffixed mnemonics (exercise the load-time aliasing seam).
MNEM_POR, MNEM_NTG, MNEM_SW = "PHIE_2025", "NTG_PhieLam_2025", "SW_2025"
ALIASES = {MNEM_POR: "PORO", MNEM_NTG: "NTG", MNEM_SW: "SW"}

# Per-zone planted truth: (mean isochore, isochore variability, ntg_target,
# net_por_mean, net_por_std). Two zones carry hydrocarbon (see CONTACT_ZONES).
# ntg_target + net_por_mean are what the net-conditioned upscale must recover.
ZONE_TABLE = [
    # name  mean_iso  iso_sd  ntg    net_por  net_sd
    ("Z0", 15.0, 3.0, 0.35, 0.235, 0.035),
    ("Z1", 12.0, 2.5, 0.55, 0.255, 0.035),
    ("Z2", 18.0, 3.5, 0.70, 0.275, 0.035),   # single OWC (oil)
    ("Z3", 10.0, 2.5, 0.45, 0.240, 0.035),   # carries the tops-only split H3b
    ("Z4", 14.0, 3.0, 0.80, 0.290, 0.035),   # GOC + FWL (gas cap + oil rim)
    ("Z5", 11.0, 2.5, 0.30, 0.225, 0.035),   # PINCHES OUT to zero across the east
]
NONNET_POR_MEAN, NONNET_POR_SD = 0.050, 0.010
BED_SCALE_M, CORR_LEN_M = 1.2, 4.0
NZONE = len(ZONE_TABLE)
NHOR = NZONE + 1                 # H0..H6
HORIZONS = [f"H{k}" for k in range(NHOR)]
ZONES = [z[0] for z in ZONE_TABLE]

# --- v2: the tops-only internal split horizon --------------------------------
# One internal horizon that has well PICKS but NO mapped surface in the emitted
# tree — the conformal-drape case (petekStatic drapes it between its bounding
# mapped horizons). Placed inside Z3 at a fixed depth fraction between H3 and H4.
TOPS_ONLY_HORIZON = "H3b"
TOPS_ONLY_SPLIT = (3, 0.5)       # (upper mapped-horizon index, fraction toward the next)

# --- v2: the pinch-out zone --------------------------------------------------
# Z5's isochore is multiplied by an areal ramp that falls from full thickness
# (west) through a sub-threshold band to EXACTLY zero across the eastern part of
# the extent — genuine degenerate columns (R5 food) the collapse/order-repair
# machinery must survive.
PINCH_ZONE_INDEX = 5
PINCH_FULL_FRAC = 0.55           # i/(ncol-1) below this: full thickness
PINCH_SUB_FRAC = 0.70            # [SUB, ZERO): a SUB-THRESHOLD band (thin, non-zero)
PINCH_ZERO_FRAC = 0.80           # i/(ncol-1) at/above this: exactly zero
PINCH_SUB_MULT = 0.03            # isochore multiplier at the top of the sub-threshold band
PINCH_SUBTHRESHOLD_M = 0.5       # thickness under this counts as a sub-threshold column

# --- v2: mixed well program --------------------------------------------------
# The last N_DEVIATED wells are directional; the rest vertical (v1 behaviour). A
# deviated bore kicks off ABOVE the reservoir, builds toward the field centre, and
# holds through the reservoir so it crosses several 100 m columns at depth. Each
# entry: (profile, hold_incl_deg, build_rate_deg_per_30m, final_incl_deg_or_None).
DEVIATED_PROGRAM = [
    ("build_hold", 68.0, 4.0, None),        # a strongly-deviated producer (high angle in-reservoir)
    ("build_hold_drop", 64.0, 4.0, 40.0),   # an S-well (build, hold, drop back)
    ("build_hold", 60.0, 4.0, None),        # a gentler deviated bore
]


# =============================================================================
# World frame (petektools Georef idiom — one fictional world frame, end to end)
# =============================================================================
def _georef() -> "pt.Georef":
    """The single fictional WORLD frame the whole asset lives in (doctrine R1).
    The georeference *is* the lattice; every surface/trend/polygon/pick/trajectory
    is placed against this one origin, so any frame mixing downstream fails loudly."""
    return pt.Georef(X0, Y0)


def _lattice() -> "pt.Lattice":
    return _georef().lattice(INC, INC, NCOL, NROW)


def _vgm(range_m: float) -> "pt.Variogram":
    return pt.Variogram("spherical", 0.0, 1.0, range_m)


# =============================================================================
# Structure (petektools: dome + isochore build-down) + trend
# =============================================================================
def _pinch_ramp(i: int) -> float:
    """The areal pinch multiplier for column ``i``: full thickness west of
    PINCH_FULL_FRAC; a linear taper to a thin PINCH_SUB_MULT across
    [FULL, SUB); a SUB-THRESHOLD band (thin, non-zero — collapse food) across
    [SUB, ZERO); and EXACTLY zero east of PINCH_ZERO_FRAC (degenerate columns)."""
    u = i / (NCOL - 1)
    if u <= PINCH_FULL_FRAC:
        return 1.0
    if u >= PINCH_ZERO_FRAC:
        return 0.0
    if u < PINCH_SUB_FRAC:
        t = (u - PINCH_FULL_FRAC) / (PINCH_SUB_FRAC - PINCH_FULL_FRAC)
        return 1.0 + t * (PINCH_SUB_MULT - 1.0)
    return PINCH_SUB_MULT * (PINCH_ZERO_FRAC - u) / (PINCH_ZERO_FRAC - PINCH_SUB_FRAC)


def _build_surfaces(seed: int) -> list[list[list[float]]]:
    """The NHOR horizon TVDSS fields (field[col][row], positive-down), top→down by
    construction: a dome Top + clamped isochores built DOWN, so no crossing. Z5's
    isochore is pinched to zero across the east (a genuine degenerate zone)."""
    lat = _lattice()
    relief = pt.synth_dome_surface(
        lat, DOME_RELIEF, DOME_ASPECT, DOME_TILT, 0.03 * DOME_RELIEF, _vgm(1500.0), seed
    )
    # Top TVDSS = datum - relief (crest = max relief = shallowest).
    top = [[TOP_DATUM - relief[i][j] for j in range(NROW)] for i in range(NCOL)]
    surfaces = [top]
    for k, (_n, mean, sd, *_r) in enumerate(ZONE_TABLE):
        iso = pt.synth_isochore(lat, mean, sd, _vgm(1200.0), seed + 101 + k)
        if k == PINCH_ZONE_INDEX:
            iso = [[iso[i][j] * _pinch_ramp(i) for j in range(NROW)] for i in range(NCOL)]
        prev = surfaces[-1]
        nxt = [[prev[i][j] + max(iso[i][j], 0.0) for j in range(NROW)] for i in range(NCOL)]
        surfaces.append(nxt)
    return surfaces


def _drape_surface(surfaces: list, split: tuple) -> list[list[float]]:
    """The tops-only split horizon as a per-node conformal drape a fraction ``f``
    of the way from mapped horizon ``a`` down to ``a+1`` (NO mapped surface emitted;
    only picks are written)."""
    a, f = split
    up, dn = surfaces[a], surfaces[a + 1]
    return [[up[i][j] + f * (dn[i][j] - up[i][j]) for j in range(NROW)] for i in range(NCOL)]


def _ntg_pattern() -> list[list[float]]:
    """An areal [0,1] 'zone NTG pattern' the depositional trend is rho-correlated
    with — a smooth west→east improving-sand gradient with a crest bump."""
    field = [[0.0] * NROW for _ in range(NCOL)]
    for i in range(NCOL):
        for j in range(NROW):
            u, v = i / (NCOL - 1), j / (NROW - 1)
            bump = 0.15 * math.exp(-(((u - 0.5) / 0.35) ** 2 + ((v - 0.5) / 0.35) ** 2))
            field[i][j] = min(1.0, max(0.0, 0.30 + 0.45 * u + bump))
    return field


def _build_trend(seed: int) -> list[list[float]]:
    """The depositional trend map: a [0,1] field correlated with the NTG pattern
    at RHO (petektools collocated cokriging generation)."""
    return pt.synth_trend_map(_lattice(), _vgm(1000.0), seed + 7, correlate_with=(_ntg_pattern(), RHO))


def _sample(field: list[list[float]], x: float, y: float) -> float:
    i = min(max(round((x - X0) / INC), 0), NCOL - 1)
    j = min(max(round((y - Y0) / INC), 0), NROW - 1)
    return field[i][j]


def _crest(surface: list[list[float]]) -> float:
    return min(min(col) for col in surface)


# =============================================================================
# Format writers (REAL Petrel export formats; petekio round-trips them)
# =============================================================================
def _fmt(v: float) -> str:
    return f"{v:.4f}"


def _write_irap_grid(path: Path, field: list[list[float]], *, negate: bool) -> None:
    """IRAP classic grid (petekio SURFACE, NO y-flip: yori=ymin, column-major
    x-fastest). `negate` writes negative-down elevation (a depth surface); a value
    grid (depositional trend) is written verbatim. Unlike CPS-3 this keeps
    yori=ymin so a value trend maps 1:1 for collocated cokriging + `value_at`."""
    xmax, ymax = X0 + (NCOL - 1) * INC, Y0 + (NROW - 1) * INC
    lines = [
        f"-996 {NROW} {INC} {INC}",
        f"{X0} {xmax} {Y0} {ymax}",
        f"{NCOL} 0 {X0} {Y0}",
        "0 0 0 0 0 0 0",
    ]
    vals = []
    for j in range(NROW):
        for i in range(NCOL):
            v = field[i][j]
            vals.append(-v if negate else v)
    for k in range(0, len(vals), 6):
        lines.append(" ".join(_fmt(v) for v in vals[k : k + 6]))
    path.write_text("\n".join(lines) + "\n")


def _write_irap_points(path: Path, surface: list[list[float]]) -> None:
    """Scattered `.IrapClassicPoints` (plain `x y z`, NEGATIVE-down elevation)."""
    rows = []
    for i in range(NCOL):
        for j in range(NROW):
            x, y = X0 + i * INC, Y0 + j * INC
            rows.append(f"{x:.3f} {y:.3f} {-surface[i][j]:.4f}")
    path.write_text("\n".join(rows) + "\n")


def _write_earthvision_grid(path: Path, surface: list[list[float]]) -> None:
    """EarthVision grid (petekio loads it as scattered points; NEGATIVE-down z).
    A `#` directive header carries the EARTHVISION marker so the reader classifies
    it even before the first data row."""
    head = [
        "# EarthVision Grid export (synthetic)",
        f"# GRID_SIZE: {NCOL} x {NROW}",
        "# GRID_SPACE: node-centred",
        "# Null_value: 1.0e30",
    ]
    rows = []
    for i in range(NCOL):
        for j in range(NROW):
            x, y = X0 + i * INC, Y0 + j * INC
            rows.append(f"{x:.3f} {y:.3f} {-surface[i][j]:.4f}")
    path.write_text("\n".join(head + rows) + "\n")


def _write_cps3_grid(path: Path, field: list[list[float]], *, negate: bool) -> None:
    """CPS-3 ASCII grid (petekio SURFACE). Row-major, row 0 = north (ymax) stepping
    south, col 0 = west (xmin). `negate` writes negative-down elevation (horizons);
    a value trend is written verbatim."""
    xmin, xmax = X0, X0 + (NCOL - 1) * INC
    ymin, ymax = Y0, Y0 + (NROW - 1) * INC
    vals = []
    for r in range(NROW):            # r=0 -> ymax (north)
        jrow = NROW - 1 - r
        for c in range(NCOL):        # c -> x (west→east)
            v = field[c][jrow]
            vals.append(-v if negate else v)
    lines = [
        "FSASCI 0 1 0 5 1.0E+30",
        f"FSLIMI {xmin} {xmax} {ymin} {ymax} -1.0E+04 1.0E+04",
        f"FSNROW {NROW} {NCOL}",
        f"FSXINC {INC} {INC}",
        "->",
    ]
    for k in range(0, len(vals), 6):
        lines.append(" ".join(_fmt(v) for v in vals[k : k + 6]))
    path.write_text("\n".join(lines) + "\n")


def _write_cps3_lines(path: Path, rings: list[list[list[float]]]) -> None:
    """CPS-3 line polygons: each ring after a `-> n` block, vertices `x y z`."""
    lines = ["FFLINE 0 1 0", "# CPS-3 line set (synthetic)"]
    for n, ring in enumerate(rings, 1):
        lines.append(f"-> {n}")
        for pt_xy in ring:
            lines.append(f"{pt_xy[0]:.3f} {pt_xy[1]:.3f} 0.0000")
    path.write_text("\n".join(lines) + "\n")


def _write_wellpath(path: Path, traj: dict, kb: float) -> None:
    """Petrel `.wellpath` positioned survey. petekio reads MD/X/Y (cols 1-3), TVD
    (col 5, positioned depth ⇒ subsea z = TVD − KB), INCL (col 9), AZIM_GN (col 11);
    a deviated bore carries the drift in X/Y/TVD + real INCL/AZIM."""
    header = [
        "# WELL TRACE (synthetic)",
        f"# WELL HEAD X-COORDINATE: {traj['x'][0]:.3f} (m)",
        f"# WELL HEAD Y-COORDINATE: {traj['y'][0]:.3f} (m)",
        f"# WELL DATUM (KB, Kelly bushing, from MSL): {kb} (m)",
        "# CRS: SYNTHETIC / ED50 UTM zone 31N",
        "# MD AND TVD ARE REFERENCED AT WELL DATUM",
        "==========",
        "MD X Y Z TVD DX DY AZIM_TN INCL DLS AZIM_GN",
    ]
    rows = []
    n = len(traj["md"])
    incl = traj.get("incl", [0.0] * n)
    azim = traj.get("azim", [0.0] * n)
    for k in range(n):
        # cols: MD X Y Z TVD DX DY AZIM_TN INCL DLS AZIM_GN
        rows.append(
            f"{traj['md'][k]:.3f} {traj['x'][k]:.3f} {traj['y'][k]:.3f} "
            f"{traj['z'][k]:.3f} {traj['tvd'][k]:.3f} 0 0 "
            f"{azim[k]:.3f} {incl[k]:.3f} 0 {azim[k]:.3f}"
        )
    path.write_text("\n".join(header + rows) + "\n")


def _write_las(path: Path, well: str, md: list[float], por: list[float],
               ntg: list[float], sw: list[float]) -> None:
    """LAS 2.0 comp-log with VENDOR-suffixed mnemonics (PHIE_2025 / NTG.._2025 /
    SW_2025) so the loader's aliasing is exercised."""
    body = [
        "~Version",
        " VERS. 2.0 : CWLS LOG ASCII STANDARD - VERSION 2.0",
        " WRAP. NO  : ONE LINE PER DEPTH STEP",
        "~Well",
        f" STRT.M {md[0]:.3f} : START DEPTH",
        f" STOP.M {md[-1]:.3f} : STOP DEPTH",
        f" STEP.M {LOG_STEP} : STEP",
        " NULL. -999.25 : NULL VALUE",
        f" KB.M {KB} : KELLY BUSHING",
        f" WELL. {well} : WELL NAME",
        "~Curve",
        " DEPT.M : Measured depth",
        f" {MNEM_POR}.v/v : Effective porosity (vendor mnemonic)",
        f" {MNEM_NTG}.v/v : Net to gross (net flag)",
        f" {MNEM_SW}.v/v : Water saturation",
        "~ASCII",
    ]
    for k in range(len(md)):
        body.append(f"{md[k]:.3f} {por[k]:.4f} {ntg[k]:.4f} {sw[k]:.4f}")
    path.write_text("\n".join(body) + "\n")


def _write_tops(path: Path, horizon_picks: list, contact_rows: list, latin1_row: str) -> None:
    """Petrel well-tops export: Type="Horizon" stratigraphic picks (distributed by
    petekio) + Type="Other" GOC/FWL contact rows (parsed facade-side) + one Latin-1
    name row. Extensionless filename carries 'Tops' so the walker routes it. Written
    Latin-1 so the high-byte name survives."""
    head = [
        "# Petrel well tops (synthetic)",
        "VERSION 2",
        "BEGIN HEADER",
        "X", "Y", "Z", "TWT", "TWT2", "age", "MD", "PVD", "Type", "Surface", "Well",
        "END HEADER",
    ]
    rows = []
    for (x, y, tvdss, md, surface, well) in horizon_picks:
        z = -tvdss
        rows.append(f"{x:.3f} {y:.3f} {z:.2f} -999 -999 -999 {md:.4f} {z:.2f} "
                    f'Horizon "{surface}" "{well}"')
    for (x, y, tvdss, md, surface, well) in contact_rows:
        z = -tvdss
        rows.append(f"{x:.3f} {y:.3f} {z:.2f} -999 -999 -999 {md:.4f} {z:.2f} "
                    f'Other "{surface}" "{well}"')
    # One decorative Type="Other" row with a Latin-1 name (exercises the decode).
    rows.append(f'{X0:.3f} {Y0:.3f} 0.00 -999 -999 -999 0.0000 0.00 Other "{latin1_row}" "field"')
    path.write_bytes(("\n".join(head + rows) + "\n").encode("latin-1"))


# =============================================================================
# Wells + logs (petektools: place_wells + trajectories + petro curves)
# =============================================================================
def _petro_specs() -> list["pt.PetroZoneSpec"]:
    return [
        pt.PetroZoneSpec(ntg, npm, nsd, NONNET_POR_MEAN, NONNET_POR_SD,
                         BED_SCALE_M, CORR_LEN_M, net_cutoff=NET_CUTOFF)
        for (_n, _m, _s, ntg, npm, nsd) in ZONE_TABLE
    ]


def _well_log(surfaces, well_x, well_y, specs, seed):
    """Assemble one VERTICAL well's continuous PHIE/NTG/SW arrays (top→down) from the
    coupled per-zone petro generator. NTG is the derived net flag (0/1) so net_only
    upscale at NET_CUTOFF conditions PORO to the planted net-rock porosity."""
    md, por, ntg, sw = [], [], [], []
    for k, spec in enumerate(specs):
        top = _sample(surfaces[k], well_x, well_y)
        base = _sample(surfaces[k + 1], well_x, well_y)
        thick = max(base - top, LOG_STEP)
        n = max(int(thick / LOG_STEP), 2)
        curves = pt.synth_petro_curves(spec, LOG_STEP, n, seed + 31 * k)
        phie, flag = curves["phie"], curves["net_flag"]
        for s in range(n):
            tvdss = top + s * LOG_STEP
            md.append(tvdss + KB)
            por.append(phie[s])
            ntg.append(flag[s])                 # 0/1 net flag == the NTG curve
            sw.append(0.30 - 0.08 * flag[s])    # net rock a touch lower Sw
    return md, por, ntg, sw


def _traj_at_md(traj: dict, md: float) -> tuple:
    """Interpolate (x, y, tvd) at measured depth ``md`` along a station trajectory."""
    mds = traj["md"]
    if md <= mds[0]:
        return traj["x"][0], traj["y"][0], traj["tvd"][0]
    if md >= mds[-1]:
        return traj["x"][-1], traj["y"][-1], traj["tvd"][-1]
    # linear scan (trajectories are short); stations are monotone in md
    for k in range(1, len(mds)):
        if md <= mds[k]:
            t = (md - mds[k - 1]) / (mds[k] - mds[k - 1])
            x = traj["x"][k - 1] + t * (traj["x"][k] - traj["x"][k - 1])
            y = traj["y"][k - 1] + t * (traj["y"][k] - traj["y"][k - 1])
            tvd = traj["tvd"][k - 1] + t * (traj["tvd"][k] - traj["tvd"][k - 1])
            return x, y, tvd
    return traj["x"][-1], traj["y"][-1], traj["tvd"][-1]


def _bore_crossing(traj: dict, surface: list, _lat) -> tuple | None:
    """Where the bore first crosses ``surface`` (top→down): the (x, y, tvdss, md) at
    the sign change of ``tvdss − surface(x, y)``. ``None`` if it never crosses.
    Uses the surface value at the bore's OWN (x, y) — a deviated pick is at the
    trajectory position, not the wellhead."""
    xs, ys, tvds, mds = traj["x"], traj["y"], traj["tvd"], traj["md"]
    prev = None
    for k in range(len(mds)):
        tvdss = tvds[k] - KB
        f = tvdss - _sample(surface, xs[k], ys[k])
        if prev is not None and prev[0] < 0.0 <= f:
            pf, px, py, ptv, pmd = prev
            t = -pf / (f - pf) if f != pf else 0.0
            return (px + t * (xs[k] - px), py + t * (ys[k] - py),
                    (ptv + t * (tvdss - ptv)), pmd + t * (mds[k] - pmd))
        prev = (f, xs[k], ys[k], tvdss, mds[k])
    return None


def _well_log_deviated(surfaces, traj, specs, seed):
    """A DEVIATED well's log, sampled at LOG_STEP MD along the true bore. Each sample
    is classified into the zone spanning its (x, y, tvdss); per-zone porosity/net
    flags come from the same coupled generators as the vertical path, so the planted
    per-zone targets are preserved. Returns (md, por, ntg, sw) or empty if the bore
    never enters the reservoir."""
    top0 = _bore_crossing(traj, surfaces[0], None)
    base = _bore_crossing(traj, surfaces[-1], None)
    if top0 is None:
        return [], [], [], []
    md_start = top0[3]
    md_end = base[3] if base is not None else traj["md"][-1]
    if md_end <= md_start + LOG_STEP:
        return [], [], [], []
    n_steps = int((md_end - md_start) / LOG_STEP)
    samples = []                       # (md, zone_index)
    for s in range(n_steps + 1):
        md = md_start + s * LOG_STEP
        x, y, tvd = _traj_at_md(traj, md)
        tvdss = tvd - KB
        z = NZONE - 1
        for k in range(NZONE):
            if _sample(surfaces[k], x, y) <= tvdss < _sample(surfaces[k + 1], x, y):
                z = k
                break
            if tvdss < _sample(surfaces[0], x, y):
                z = 0
                break
        samples.append((md, z))
    counts = [0] * NZONE
    for _md, z in samples:
        counts[z] += 1
    curves, cursor = {}, [0] * NZONE
    for k in range(NZONE):
        if counts[k] > 0:
            curves[k] = pt.synth_petro_curves(specs[k], LOG_STEP, counts[k], seed + 31 * k)
    md_out, por, ntg, sw = [], [], [], []
    for md, z in samples:
        c = curves[z]
        i = cursor[z]
        cursor[z] += 1
        phie, flag = c["phie"][i], c["net_flag"][i]
        md_out.append(md)
        por.append(phie)
        ntg.append(flag)
        sw.append(0.30 - 0.08 * flag)
    return md_out, por, ntg, sw


def _well_profiles(n_wells: int) -> list:
    """Deterministic per-well program: the last ``min(3, n_wells-4)`` wells are
    directional (build-hold / build-hold-drop), the rest vertical."""
    n_dev = min(len(DEVIATED_PROGRAM), max(0, n_wells - 4))
    profiles = ["vertical"] * (n_wells - n_dev)
    profiles += [DEVIATED_PROGRAM[i] for i in range(n_dev)]
    return profiles


def _azimuth_to_center(wx: float, wy: float) -> float:
    """Grid-north azimuth (deg, N=0 E=90) from the wellhead toward the field centre,
    so a deviated bore drifts INWARD and stays inside the modelled extent."""
    cx = X0 + (NCOL - 1) * INC / 2.0
    cy = Y0 + (NROW - 1) * INC / 2.0
    return math.degrees(math.atan2(cx - wx, cy - wy)) % 360.0


# =============================================================================
# The composer
# =============================================================================
def spill_recipe(*, ncol: int = 61, n_cubes: int = 3, nk_per_zone: int = 14) -> dict:
    """The documented SPILL-FORCING recipe (doctrine R2 spilled cell of the matrix).

    Returns the live-set estimate for an ``ncol × ncol`` asset built with ~1 m
    proportional layering (``nk_per_zone`` layers over each of the six zones)
    carrying ``n_cubes`` property cubes, plus a ``force_budget_bytes`` set just
    below it. The estimate is petekStatic's OWN budget formula, read across the
    seam via ``peteksim._core.live_set_bytes`` (which calls
    ``srs_model::live_set_bytes``) — NOT a Python mirror of its
    ``WARM_FACTOR``/``ELEM_BYTES`` constants, so a change to those on the
    petekStatic side can never drift this recipe silently. The R6 acceptance suite
    builds the spill variant (``synth_asset(..., ncol=ncol)`` + fine layering) and
    forces the petekStatic ``MemoryBudget::bytes(force_budget_bytes)`` so the live
    set exceeds the budget and the pipeline SPILLS (out-of-core). Only the asset
    size + layering live here; the budget is applied by the build (see the note in
    the manifest)."""
    from ._core import live_set_bytes as _live_set_bytes

    nk_total = nk_per_zone * NZONE
    cells = ncol * ncol * nk_total
    est = int(_live_set_bytes(ncol, ncol, nk_total, n_cubes))
    return {
        "ncol": ncol,
        "nk_per_zone": nk_per_zone,
        "nk_total": nk_total,
        "n_cubes": n_cubes,
        "cells": cells,
        "est_live_set_bytes": est,
        # A budget just under the estimate forces the spilled path.
        "force_budget_bytes": int(est * 0.5),
        "recipe": (
            f"synth_asset(root, ncol={ncol}) built with ~1 m proportional layering "
            f"(~{nk_per_zone} layers/zone, {nk_total} total) + {n_cubes} property "
            f"cubes ⇒ live set ≈ {est / (1 << 20):.1f} MiB; force "
            f"MemoryBudget::bytes({int(est * 0.5)}) to spill."
        ),
    }


def synth_asset(root, *, seed: int = 20_260_704, n_wells: int = 8, ncol: int = NCOL,
                surfaces_as_points: bool = False) -> dict:
    """Write the complete synthetic Petrel-export tree under ``root`` and return the
    planted-truth manifest. Deterministic per ``(seed, ncol, n_wells)``.

    ``ncol`` sizes the square node lattice (default 41 = a 4 km study area, THE
    suite dataset); a smaller value yields a lighter tree for fast tests. The tree
    is asset **v2**: a mixed vertical/deviated well program, a tops-only split
    horizon, per-zone contacts (two-contact / single / contactless), a pinching
    zone, and a single world georef — see the module docstring + ``spill_recipe``.

    ``surfaces_as_points=True`` emits each mapped horizon ONLY in the scattered
    point formats (IRAP classic points + EarthVision), skipping the pre-gridded IRAP
    / CPS-3 grid copies — so the loader classifies the horizons as **point-sets**
    and the framework routes them down petekStatic's ``from_scatter_stack``
    conditioning path (``HorizonSource::Scatter``), exactly as the canonical real
    model does. The default (``False``, the pre-gridded ``Mapped`` escape hatch) is
    unchanged. This is the fixture the scatter-conditioning perf/dedup work
    (``task_suite_scatter_perf``) drives to exercise the expensive per-horizon
    bilinear solve on the synthetic asset."""
    global NCOL, NROW  # noqa: PLW0603 — the module geometry the writers/generators read
    NCOL = NROW = ncol
    root = Path(root)
    surf_dir = root / "Surfaces"
    poly_dir = root / "Polygons"
    tops_dir = root / "WellTops"
    paths_dir = root / "Wells" / "Paths"
    logs_dir = root / "Wells" / "Logs"
    for d in (surf_dir, poly_dir, tops_dir, paths_dir, logs_dir):
        d.mkdir(parents=True, exist_ok=True)

    surfaces = _build_surfaces(seed)
    drape = _drape_surface(surfaces, TOPS_ONLY_SPLIT)   # tops-only split (NO surface emitted)
    trend = _build_trend(seed)
    specs = _petro_specs()

    # --- surfaces: each MAPPED horizon in real-export formats. The tops-only split
    # horizon (TOPS_ONLY_HORIZON) is deliberately NOT written here — only its well
    # picks are emitted, the conformal-drape case. By default the framework's mapped
    # surface is the IRAP grid (fast, yori=ymin maps 1:1), with scattered IRAP points
    # + EarthVision as alternate point representations + a `_cps3.CPS3grid` CPS-3
    # copy. With `surfaces_as_points` we emit ONLY the point formats (no grid copies),
    # so the horizons load as scattered point-sets and the framework conditions them
    # on petekStatic's `from_scatter_stack` path (the canonical real-model shape). ---
    for k, name in enumerate(HORIZONS):
        _write_irap_points(surf_dir / f"{name}.IrapClassicPoints", surfaces[k])
        _write_earthvision_grid(surf_dir / f"{name}.EarthVisionGrid", surfaces[k])
        if not surfaces_as_points:
            _write_irap_grid(surf_dir / f"{name}.irap", surfaces[k], negate=True)
            _write_cps3_grid(surf_dir / f"{name}_cps3.CPS3grid", surfaces[k], negate=True)
    # --- depositional trend as an IRAP grid surface (yori=ymin so collocated
    # cokriging + value_at map it 1:1). -----------------------------------------
    trend_name = "DepoTrend_NTG"
    _write_irap_grid(surf_dir / f"{trend_name}.irap", trend, negate=False)

    # --- outlines: study-area ModelEdge + a derived structure closure ---------
    lat = _lattice()
    edge = pt.study_area_outline(X0, Y0, X0 + (NCOL - 1) * INC, Y0 + (NROW - 1) * INC, 0.0, 1)
    _write_cps3_lines(poly_dir / "ModelEdge.CPS3lines", [edge])
    # The structure closure = the deepest contour of the Top that still closes
    # inside the study area (the 4-way dip closure's spill level).
    closure: list = []
    for frac in (0.65, 0.55, 0.45, 0.35, 0.25):
        ring = pt.closure_outline(surfaces[0], lat, _crest(surfaces[0]) + frac * DOME_RELIEF)
        if len(ring) >= 4:
            closure = ring
            break
    if closure:
        _write_cps3_lines(poly_dir / "StructureOutline.CPS3lines", [closure])

    # --- wells: seeded placement + MIXED vertical/deviated trajectory + logs ----
    heads = pt.place_wells(X0 + 400, Y0 + 400,
                           X0 + (NCOL - 1) * INC - 400, Y0 + (NROW - 1) * INC - 400,
                           n_wells, seed + 3)
    residual = pt.Sampler.uniform(-1.5, 1.5)
    profiles = _well_profiles(n_wells)
    well_ids, horizon_picks, well_program = [], [], []
    for w, (wx, wy) in enumerate(heads):
        wid = f"99_{w + 1}-1"                 # fictional Petrel id 99/{k}-1
        well_ids.append(wid)
        deepest = _sample(surfaces[-1], wx, wy)
        prof = profiles[w]
        if prof == "vertical":
            td = deepest + KB + 40.0
            traj = pt.synth_trajectory([wx, wy], KB, td, 50.0, seed + w)
            _write_wellpath(paths_dir / f"{wid}.wellpath", traj, KB)
            md, por, ntg, sw = _well_log(surfaces, wx, wy, specs, seed + 500 * (w + 1))
            # Vertical picks: existing residual-perturbed surface pick at the wellhead.
            for k in range(NHOR):
                tv = pt.tops_from_surface(surfaces[k], lat, [[wx, wy]], residual, seed + k)[0]
                if tv == tv:  # finite
                    horizon_picks.append((wx, wy, tv, tv + KB, HORIZONS[k], wid))
            dtv = _sample(drape, wx, wy)          # tops-only split pick (drape at wellhead)
            horizon_picks.append((wx, wy, dtv, dtv + KB, TOPS_ONLY_HORIZON, wid))
            well_program.append({"id": wid, "profile": "vertical", "x": wx, "y": wy})
        else:
            name, hold_incl, build_rate, final_incl = prof
            azim = _azimuth_to_center(wx, wy)
            # Kick off just above the reservoir so the build COMPLETES near the top
            # horizon — full inclination THROUGH the reservoir (crossing several 100 m
            # columns at depth) with minimal pre-reservoir drift, so the bore stays
            # inside the modelled extent.
            # Kickoff is a TVD above the reservoir top (vertical until then ⇒ MD==TVD),
            # so the build completes near the top horizon and the reservoir is traversed
            # at the full hold angle. TD is expressed in MD: below the kickoff the bore
            # is inclined, so MD gains faster than TVD — divide the vertical remainder by
            # cos(hold) (a generous overshoot) so the bore clears the reservoir base.
            cos_hold = math.cos(math.radians(hold_incl))
            build_len = hold_incl / build_rate * 30.0
            top_tvd = _sample(surfaces[0], wx, wy) + KB
            kickoff = max(200.0, top_tvd - build_len)
            base_tvd = deepest + KB + 40.0        # reservoir base (H6) + margin
            base_md = kickoff + (base_tvd - kickoff) / cos_hold + build_len  # MD past the base
            kwargs = dict(profile=name, md_step=15.0, kickoff_md=kickoff,
                          build_rate_deg_per_30m=build_rate, hold_incl_deg=hold_incl,
                          azimuth_deg=azim)
            if name == "build_hold_drop":
                # Drop starts BELOW the reservoir base so the whole reservoir is
                # traversed at the full hold angle; the drop tail rides past TD.
                kwargs["drop_start_md"] = base_md + 20.0
                kwargs["drop_rate_deg_per_30m"] = build_rate
                kwargs["final_incl_deg"] = final_incl
                td_md = base_md + 20.0 + (hold_incl - final_incl) / build_rate * 30.0 + 60.0
            else:
                td_md = base_md + 40.0
            traj = pt.synth_trajectory_profile([wx, wy], KB, td_md, seed=seed + w, **kwargs)
            _write_wellpath(paths_dir / f"{wid}.wellpath", traj, KB)
            md, por, ntg, sw = _well_log_deviated(surfaces, traj, specs, seed + 500 * (w + 1))
            # Deviated picks: crossing of each mapped horizon at the bore's OWN (x, y).
            for k in range(NHOR):
                c = _bore_crossing(traj, surfaces[k], lat)
                if c is not None:
                    horizon_picks.append((c[0], c[1], c[2], c[3] + KB, HORIZONS[k], wid))
            dc = _bore_crossing(traj, drape, lat)  # tops-only split at the trajectory
            if dc is not None:
                horizon_picks.append((dc[0], dc[1], dc[2], dc[3] + KB, TOPS_ONLY_HORIZON, wid))
            dls = pt.max_dogleg_severity(traj["md"], traj["incl"], traj["azim"])
            well_program.append({"id": wid, "profile": name, "x": wx, "y": wy,
                                 "kickoff_md": kickoff, "hold_incl_deg": hold_incl,
                                 "build_rate_deg_per_30m": build_rate,
                                 "azimuth_deg": azim, "max_dls_deg_per_30m": dls,
                                 **({"final_incl_deg": final_incl} if final_incl is not None else {})})
        if md:
            _write_las(logs_dir / f"{wid}.las", wid, md, por, ntg, sw)

    # --- contacts (Type="Other"): derive from the two hydrocarbon zones -------
    owc_z2 = _crest(surfaces[2]) + 0.45 * ZONE_TABLE[2][1]
    goc_z4 = _crest(surfaces[4]) + 0.20 * ZONE_TABLE[4][1]
    fwl_z4 = _crest(surfaces[4]) + 0.60 * ZONE_TABLE[4][1]
    wx0, wy0 = heads[0]
    contact_rows = [
        (wx0, wy0, owc_z2, owc_z2 + KB, "OWC", well_ids[0]),
        (wx0, wy0, goc_z4, goc_z4 + KB, "GOC", well_ids[0]),
        (wx0, wy0, fwl_z4, fwl_z4 + KB, "FWL", well_ids[0]),
    ]
    _write_tops(tops_dir / "FieldWellTops", horizon_picks, contact_rows, "Blåbær Sør")

    # --- zonation recipe (per-zone conformity + contacts) ---------------------
    zonation = []
    for z in range(NZONE):
        entry = {"zone": ZONES[z], "below_horizon": HORIZONS[z + 1],
                 "conformity": "proportional", "nk": max(int(round(ZONE_TABLE[z][1])), 1)}
        if z == 2:
            entry["contacts"] = {"owc": owc_z2}
        elif z == 4:
            entry["contacts"] = {"goc": goc_z4, "fwl": fwl_z4}
        else:
            entry["contacts"] = None
        zonation.append(entry)

    # --- the per-zone contact PLAN (planted truth: two-contact / single / none) --
    contact_plan = {}
    for z in range(NZONE):
        if z == 2:
            contact_plan[ZONES[z]] = {"type": "single", "contacts": {"owc": owc_z2}}
        elif z == 4:
            contact_plan[ZONES[z]] = {"type": "two_contact",
                                      "contacts": {"goc": goc_z4, "fwl": fwl_z4}}
        else:
            contact_plan[ZONES[z]] = {"type": "contactless", "contacts": None}

    return {
        "root": str(root),
        "asset_version": ASSET_VERSION,
        "seed": seed,
        "crs": "SYNTHETIC / ED50 UTM zone 31N",
        "aliases": dict(ALIASES),
        "horizons": list(HORIZONS),           # MAPPED horizons only
        # Whether the horizons were emitted as scattered point-sets (the
        # `from_scatter_stack` conditioning path) rather than pre-gridded surfaces.
        "surfaces_as_points": surfaces_as_points,
        "zones": list(ZONES),
        "well_ids": well_ids,
        "trend_surface": trend_name,
        "rho": RHO,
        "net_cutoff": NET_CUTOFF,
        # planted per-zone targets the net-conditioned upscale must recover:
        "zone_targets": {
            ZONE_TABLE[z][0]: {"ntg_target": ZONE_TABLE[z][3], "net_por_mean": ZONE_TABLE[z][4]}
            for z in range(NZONE)
        },
        "contacts": {"owc_z2": owc_z2, "goc_z4": goc_z4, "fwl_z4": fwl_z4},
        "zonation": zonation,
        # --- v2 planted truths --------------------------------------------------
        "georef": {"east0": X0, "north0": Y0},
        "well_program": well_program,          # per-well profile + params (vertical/deviated)
        "tops_only_horizon": TOPS_ONLY_HORIZON,  # picks only, NO mapped surface
        "contact_plan": contact_plan,          # per-zone: two_contact / single / contactless
        "pinch_out": {
            "zone": ZONES[PINCH_ZONE_INDEX],
            "below_horizon": HORIZONS[PINCH_ZONE_INDEX + 1],
            "full_frac": PINCH_FULL_FRAC,
            "zero_frac": PINCH_ZERO_FRAC,
            "subthreshold_m": PINCH_SUBTHRESHOLD_M,
            "note": "Z5 isochore ramps to sub-threshold then EXACTLY zero across "
                    "the eastern columns (i/(ncol-1) in [full_frac, zero_frac]) — "
                    "genuine degenerate columns for R5 collapse/order-repair.",
        },
        "spill_recipe": spill_recipe(),
    }


if __name__ == "__main__":
    import sys

    dest = sys.argv[1] if len(sys.argv) > 1 else "./synthetic_asset"
    man = synth_asset(dest)
    print(f"synthetic asset v{man['asset_version']} written to {man['root']} (seed {man['seed']})")
    print(f"  horizons={man['horizons']} zones={man['zones']} wells={man['well_ids']}")
    print(f"  program={[(w['id'], w['profile']) for w in man['well_program']]}")
    print(f"  tops_only={man['tops_only_horizon']} pinch={man['pinch_out']['zone']}")
