"""The application driver — the explicit moments of the modelling API v2.

Specs are declarative values; THIS module is where they get resolved against a
loaded project and applied to the Rust ``peteksim._core`` engine. The moments:

    geom  = GridGeometry(project, extent=, cell=(dx, dy), orient=0)
    grid  = geom.build(hz, sz, lay, collapse_negative=True)
    model = grid.model(props, con, fluid=, fvf=, wells=)
    mc    = model.zoned_uncertainty(mc)   # auto-routes on is_zoned

Errors at apply are LOUD and name BOTH the missing project object AND the spec
entry. The v1 eight-call chain keeps working (``proj.framework(...)`` + the
``_core`` objects it returns) with a DeprecationWarning.
"""

from __future__ import annotations

import warnings
from typing import Any, Dict, List, Optional, Tuple

from . import _core
from .specs import (
    ApplyError,
    Contacts,
    Gridding,
    Horizons,
    Layering,
    Mc,
    NotYetSupported,
    Props,
    Run,
    Subzones,
    TieSettings,
    ViewSettings,
)
from .specs.props import CollocatedTrend

_DEPR = (
    "The v1 eight-call model-build surface is deprecated (window: two minors) in "
    "favour of the declarative modelling API v2 — see peteksim modelling-api-v2. "
)
_DEFAULT_OUTLINE = object()


def _both(spec_val, legacy: Dict[str, Any], call: str) -> None:
    """Loud spec-XOR-legacy-kwargs guard (testing-doctrine R7.5)."""
    given = [k for k, v in legacy.items() if v is not None]
    if spec_val is not None and given:
        raise ApplyError(
            f"{call}: pass EITHER a spec OR the legacy kwargs {given}, not both")


# --- collocated (dual: name-spec vs eager v1 Surface) -----------------------

def collocated(surface, corr: float, as_depth: bool = False):
    """A collocated-cokriging trend. ``surface`` a **name** (str) → a name-holding
    ``CollocatedTrend`` spec resolved at apply (the v2 pattern). ``surface`` a
    ``_core.Surface`` → the v1 eager trend (deprecated: bind by name instead)."""
    if isinstance(surface, str):
        return CollocatedTrend(surface=surface, corr=float(corr), as_depth=bool(as_depth))
    warnings.warn(
        _DEPR + "ps.collocated(surface_object, ...) eagerly binds a project object; "
        "pass the surface NAME (a str) to defer resolution to apply time.",
        DeprecationWarning, stacklevel=2)
    return _core.collocated(surface, corr, as_depth)


def _cell_size(cell) -> float:
    if isinstance(cell, (tuple, list)):
        if len(cell) != 2:
            raise ValueError(f"cell must be a scalar or (dx, dy), got {cell!r}")
        dx, dy = float(cell[0]), float(cell[1])
        if dx != dy:
            raise NotYetSupported(
                f"anisotropic cells (dx={dx} != dy={dy}) are not yet supported — "
                f"task_suite_grid_rotation; pass an isotropic cell for now")
        return dx
    return float(cell)


class GridGeometry:
    """The areal geometry (cell size + extent + orientation), pre-``build``."""

    def __init__(self, project: Any, cell, extent, orient: float):
        if orient not in (0, 0.0):
            raise NotYetSupported(
                f"grid_geometry(orient={orient}) — rotation is not yet supported "
                f"(task_suite_grid_rotation); pass orient=0")
        self.project = project
        self.cell_size_m = _cell_size(cell)
        self.extent = extent
        self.orient = orient

    def build(self, horizons: Horizons, subzones: Optional[Subzones] = None,
              layering: Optional[Layering] = None, collapse_negative: bool = True,
              outline=_DEFAULT_OUTLINE, min_thickness_m: float = 0.0,
              ties: Optional[TieSettings] = None,
              gridding: Optional[Gridding] = None) -> "Grid":
        """Freeze the geometry + structure specs into a ``Grid`` builder. Resolves
        names early (loud on a missing horizon), defers the engine build to
        ``grid.model`` (so contacts + props are known in one construction)."""
        if not isinstance(horizons, Horizons):
            raise TypeError("geom.build(hz, ...): hz must be a ps.Horizons spec")
        _validate_horizons(self.project._inner, horizons)
        resolved_outline = None if outline is _DEFAULT_OUTLINE else outline
        return Grid(self, horizons, subzones, layering, bool(collapse_negative),
                    resolved_outline, float(min_thickness_m),
                    ties or horizons.ties, gridding or horizons.gridding)


def _validate_horizons(proj_core, hz: Horizons) -> None:
    """Every horizon must resolve to a loaded surface OR a tops pick — loud,
    naming BOTH the project (no such object) AND the spec entry."""
    for r in hz.rows:
        surf = r.resolved_surface()
        ok_surface = _surface_loaded(proj_core, surf)
        ok_tops = _tops_pick(proj_core, r.resolved_tie()) is not None
        if not (ok_surface or ok_tops):
            raise ApplyError(
                f"horizon {r.name!r} (Horizons spec) resolves to no loaded surface "
                f"{surf!r} and no tops pick {r.resolved_tie()!r} in the project")
    for z in hz.zones:
        if z not in hz.zones:  # unreachable, kept for symmetry
            raise ApplyError(f"zone {z!r} not declared in Horizons.zones")


def _surface_loaded(proj_core, name: str) -> bool:
    try:
        proj_core.surface(name)
        return True
    except Exception:
        return False


def _tops_pick(proj_core, name: str):
    try:
        return proj_core.tops.pick(name)
    except Exception:
        return None


class Grid:
    """The frozen framework + structure specs — the v2 ``grid`` builder. The whole
    ``_core`` chain (framework → zonation → build → property population → model)
    runs lazily in :meth:`model`, so ``con``/``props`` are known in one shot and a
    derived spec rebuilds deterministically."""

    def __init__(self, geom: GridGeometry, horizons, subzones, layering,
                 collapse_negative, outline, min_thickness_m, ties, gridding):
        self.geom = geom
        self.horizons = horizons
        self.subzones = subzones
        self.layering = layering
        self.collapse_negative = collapse_negative
        self.outline = outline
        self.min_thickness_m = min_thickness_m
        self.ties = ties
        self.gridding = gridding

    def model(self, props: Optional[Props] = None, con=None, *, fluid: str = "oil",
              fvf: float = 1.25, gas_fvf: Optional[float] = None, wells=None,
              run: Optional[Run] = None, sugar_cube: bool = False) -> "Model":
        """Execute the build with the property + contact specs, returning a
        populated :class:`Model`. Zoned (Horizons.zones present) → contacts fold
        into the zonation; non-zoned → contacts go to the model."""
        proj_core = self.geom.project._inner
        hz = self.horizons
        zoned = bool(hz.zones)
        _guard_structural_uncertainty(hz, zoned)
        _warn_forward_declared(self.gridding, self.ties)

        collapse_below_m = self.gridding.min_cell if self.gridding else None
        if self.layering is not None and self.layering.min_cell is not None:
            collapse_below_m = self.layering.min_cell
        tie_to_tops = self.ties is not None
        fw = proj_core.framework(
            hz.horizon_names(), outline=self.outline, tie_to_tops=tie_to_tops,
            min_thickness_m=self.min_thickness_m, cell_size_m=self.geom.cell_size_m,
            collapse_below_m=collapse_below_m)

        model_contacts = None
        if zoned:
            fw.set_zonation(_build_zonation(hz, self.subzones, self.layering, con))
            if self.ties is not None:
                ties = _build_well_ties(proj_core, hz)
                if ties:
                    fw.set_well_ties(ties)
        else:
            if self.layering is not None:
                fw.set_layering(_layers_from(self.layering))
            model_contacts = _flat_contacts(con)

        grid = fw.build_grid()
        if props is not None:
            _apply_props(grid, proj_core, props)

        wells_core = wells if wells is not None else proj_core.wells()
        mb = run.memory_budget if run is not None else None
        inner = grid.model(contacts=model_contacts, fluid=fluid, fvf=fvf,
                           gas_fvf=gas_fvf, wells=wells_core,
                           memory_budget_bytes=mb, sugar_cube=sugar_cube)
        return Model(inner, self.geom.project, structural=_structural_rows(hz))


def _guard_structural_uncertainty(hz: Horizons, zoned: bool) -> None:
    """Structural uncertainty (``Horizons`` ``sd``/``vgm``) is wired through the
    ZONED (stack) MC path — the top row perturbs the top-surface depth, deeper
    rows the zone isochores (``decision_structural_uncertainty_isochore``). A
    NON-zoned model's flat MC uses the ``McInputs`` surface, which has no
    structural hook yet — loud until that path is RealizationDraw-backed."""
    if zoned:
        return
    bad = [r.name for r in hz.rows if r.has_uncertainty()]
    if bad:
        raise NotYetSupported(
            f"structural uncertainty on horizon(s) {bad} (Horizons sd/vgm) is applied "
            f"through the zoned/stack MC path, but this model is not zoned (no "
            f"Horizons.zones) — the flat MC surface has no structural hook yet. Declare "
            f"zones, or drop sd/vgm to build the flat model.")


def _structural_rows(hz: Horizons):
    """The per-row ``(sd_m, vgm)`` structural-uncertainty descriptor in horizon
    order (row 0 = top surface; row k>=1 = zone k-1 isochore), or ``None`` when no
    row declares uncertainty (the common case → the engine sees no structural
    field, byte-identical to before). ``vgm`` is ``(model, range)`` or ``None``."""
    if not any(r.has_uncertainty() for r in hz.rows):
        return None
    return [(float(r.sd), tuple(r.vgm) if r.vgm is not None else None)
            for r in hz.rows]


_WARNED_FORWARD: set = set()


def _warn_forward_declared(gridding: Optional[Gridding], ties: Optional[TieSettings]) -> None:
    """One-time advisory for recorded-but-not-yet-wired settings (never silent)."""
    if gridding is not None:
        if (gridding.fidelity_m is not None or gridding.extrapolation is not None) \
                and "gridding" not in _WARNED_FORWARD:
            _WARNED_FORWARD.add("gridding")
            warnings.warn(
                "Gridding.fidelity_m/extrapolation are recorded but not yet honoured "
                "by the engine (wired when the structure-investigation branch merges).",
                UserWarning, stacklevel=3)
    if ties is not None and (ties.method == "radius" or ties.radius_m is not None) \
            and "ties" not in _WARNED_FORWARD:
        _WARNED_FORWARD.add("ties")
        warnings.warn(
            "TieSettings.method='radius'/radius_m are recorded; the engine currently "
            "ties horizons to tops picks (convergent). Radius locality lands with the "
            "structure-build investigation.", UserWarning, stacklevel=3)


def _build_zonation(hz: Horizons, sz: Optional[Subzones], lay: Optional[Layering],
                    con) -> List[Dict[str, Any]]:
    contacts = con if isinstance(con, Contacts) else None
    out: List[Dict[str, Any]] = []
    for zname in hz.zones:
        below = hz.below_horizon(zname)
        split = sz.get(zname) if sz is not None else None
        conformity = split.conformity if split is not None else "proportional"
        nk, dz = lay.for_zone(zname) if lay is not None else (1, None)
        entry: Dict[str, Any] = {"zone": zname, "below_horizon": below,
                                 "conformity": conformity}
        if dz is not None:
            entry["dz_m"] = dz
        else:
            entry["nk"] = nk if nk is not None else 1
        entry["contacts"] = contacts.for_zone(zname) if contacts is not None else None
        out.append(entry)
    return out


def _build_well_ties(proj_core, hz: Horizons) -> List[Dict[str, Any]]:
    """Best-effort engine well-tie table from the tie pick sets (populates the
    model's well-tie residuals). Skips a horizon with no pick."""
    try:
        heads = {wid: (x, y) for (wid, x, y) in proj_core.wells().heads()}
    except Exception:
        return []
    per_well: Dict[str, Dict[str, float]] = {}
    for r in hz.rows:
        pick = _tops_pick(proj_core, r.resolved_tie())
        if pick is None:
            continue
        for well_id, depth in pick.picks:
            per_well.setdefault(well_id, {})[r.name] = depth
    ties = []
    for well_id, tops in per_well.items():
        if well_id in heads and tops:
            x, y = heads[well_id]
            ties.append({"id": well_id, "x": x, "y": y, "tops": tops})
    return ties


def _layers_from(lay: Layering):
    nk, dz = lay.for_zone("*")
    if dz is not None:
        return _core.layers(dz_m=dz)
    return _core.layers(n=nk if nk is not None else 1)


def _flat_contacts(con) -> Optional[Dict[str, float]]:
    if con is None:
        return None
    if isinstance(con, dict):
        return con
    if isinstance(con, Contacts):
        merged: Dict[str, float] = {}
        for _glob, kv in con.entries:
            merged.update(dict(kv))
        return merged or None
    raise ApplyError("con must be a ps.Contacts spec or a plain contacts dict")


def _apply_props(grid, proj_core, props: Props) -> None:
    for prop in props.items:
        p = grid.property(prop.name, zone=prop.zone)
        p.upscale(proj_core.wells(), method=prop.upscale_method,
                  net_only=prop.net_only, net_cutoff=prop.net_cutoff)
        pg = prop.propagate
        if pg is not None:
            vgm = pg.variogram.resolve(_core)
            gspec = _core.gaussian(vgm, pg.seed, pg.max_neighbours, pg.radius_m)
            trend = pg.trend.resolve(_core, proj_core) if pg.trend is not None else None
            p.propagate(gspec, trend=trend, resimulate=(pg.mode == "resimulate"),
                        allow_mean_fill=pg.allow_mean_fill)


# --- Model (wraps _core.StaticModel) ----------------------------------------

class Model:
    """A populated static model — the v2 model. Wraps ``_core.StaticModel``; every
    bundle/summary/view accessor forwards unchanged. ``uncertainty`` /
    ``zoned_uncertainty`` accept a ``ps.Mc`` spec (auto-routed) or the legacy
    kwargs (deprecated)."""

    def __init__(self, inner, project: Any, structural=None):
        self._inner = inner
        self._project = project
        # The Horizons structural-uncertainty descriptor (per-row sd/vgm), carried
        # from the build so the zoned MC can stamp it on every draw. None = no field.
        self._structural = structural

    def __getattr__(self, name):
        return getattr(self._inner, name)

    def mc(self, spec: Mc):
        """Run an ``Mc`` spec, auto-routing on ``is_zoned()``."""
        return self.zoned_uncertainty(spec) if self._inner.is_zoned() \
            else self.uncertainty(spec)

    def zoned_uncertainty(self, mc: Optional[Mc] = None, **legacy):
        if isinstance(mc, Mc):
            _both(mc, legacy, "model.zoned_uncertainty")
            return _run_zoned(self._inner, mc, self._structural)
        if mc is not None:
            raise TypeError("zoned_uncertainty(mc) expects a ps.Mc spec")
        if legacy:
            warnings.warn(_DEPR + "Pass a ps.Mc spec to zoned_uncertainty(...).",
                          DeprecationWarning, stacklevel=2)
        return self._inner.zoned_uncertainty(**legacy)

    def uncertainty(self, mc: Optional[Mc] = None, **legacy):
        if isinstance(mc, Mc):
            _both(mc, legacy, "model.uncertainty")
            return _run_flat(self._inner, mc)
        if mc is not None:
            raise TypeError("uncertainty(mc) expects a ps.Mc spec")
        if legacy:
            warnings.warn(_DEPR + "Pass a ps.Mc spec to uncertainty(...).",
                          DeprecationWarning, stacklevel=2)
        return self._inner.uncertainty(**legacy)

    def view(self, settings: Optional[ViewSettings] = None, **kwargs):
        if settings is not None:
            _both(settings, kwargs, "model.view")
            return self._inner.view(open_browser=settings.open_browser,
                                    port=settings.port, block=settings.block,
                                    property=settings.property)
        return self._inner.view(**kwargs)

    def __repr__(self) -> str:
        return f"Model(zoned={self._inner.is_zoned()}, props={self._inner.property_names()})"


def _resolve(u, core):
    return u.resolve(core) if u is not None else None


def _run_zoned(inner, mc: Mc, structural=None):
    per_zone = {z: {"contacts": m.contacts_sd_m, "goc": m.goc_sd_m,
                    "porosity": _resolve(m.porosity, _core),
                    "net_to_gross": _resolve(m.net_to_gross, _core),
                    "water_saturation": _resolve(m.water_saturation, _core)}
                for z, m in mc.per_zone}
    return inner.zoned_uncertainty(
        porosity=_resolve(mc.porosity, _core),
        net_to_gross=_resolve(mc.net_to_gross, _core),
        water_saturation=_resolve(mc.water_saturation, _core),
        contacts=mc.contacts_sd_m, goc=mc.goc_sd_m,
        fvf=_resolve(mc.fvf, _core), gas_fvf=_resolve(mc.gas_fvf, _core),
        zones=per_zone or None, structural=structural,
        n=mc.n, seed=mc.seed, workers=mc.settings.workers)


def _run_flat(inner, mc: Mc):
    return inner.uncertainty(
        porosity=_resolve(mc.porosity, _core),
        net_to_gross=_resolve(mc.net_to_gross, _core),
        water_saturation=_resolve(mc.water_saturation, _core),
        contacts=mc.contacts_sd_m, fvf=_resolve(mc.fvf, _core),
        gas_fvf=_resolve(mc.gas_fvf, _core), n=mc.n, seed=mc.seed,
        lo_pct=mc.settings.lo_pct, hi_pct=mc.settings.hi_pct)
