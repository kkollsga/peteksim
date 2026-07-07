"""Property specs — the per-property population chain as a declarative value.

``Prop`` (one cube's upscale + propagate recipe) + ``Props`` (the set applied at
``grid.model(props=)``). A ``Variogram``/``Propagate`` value describes the SGS;
``CollocatedTrend`` holds the steering surface's NAME (resolved at apply), fixing
the eager-binding defect the api-consistency contract calls out.
"""

from __future__ import annotations

import dataclasses
import warnings
from typing import Any, Dict, Optional, Tuple

from .base import ApplyError, Spec, render_table, spec


_PROPERTY_WORKFLOW_DEPR = (
    "peteksim.Prop/Props are a legacy petekSim property spec surface. The "
    "canonical property workflow now lives in petekStatic; use "
    "peteksim.upscale(...).sgs(...), peteksim.distributions.from_logs(), "
    "peteksim.Var, peteksim.Grid, and peteksim.PropertyPipelineSpec (or import "
    "the same names from petekstatic)."
)


# --- collocated trend (name-holding) ----------------------------------------

@spec
@dataclasses.dataclass(frozen=True, eq=False, repr=False)
class CollocatedTrend(Spec):
    """A collocated-cokriging trend that holds the steering surface's NAME
    (``ps.collocated("depo_trend", corr=0.6)``), resolved against the project at
    apply. ``as_depth=False`` reads the surface as a trend in its own units;
    ``True`` flips a structural elevation to positive-down depth."""

    _tag = "CollocatedTrend"
    surface: str = ""
    corr: float = 0.0
    as_depth: bool = False

    def resolve(self, core, project_core):
        """Build the ``_core`` Trend by resolving ``surface`` on the project."""
        try:
            surf = project_core.surface(self.surface)
        except Exception as e:  # loud: name BOTH the missing object and the spec
            raise ApplyError(
                f"collocated trend surface {self.surface!r} not loaded in the project "
                f"(CollocatedTrend spec) — {e}")
        return core.collocated(surf, self.corr, self.as_depth)


def collocated_name(surface: str, corr: float, as_depth: bool = False) -> CollocatedTrend:
    return CollocatedTrend(surface=surface, corr=float(corr), as_depth=bool(as_depth))


# --- variogram / propagation ------------------------------------------------

@spec
@dataclasses.dataclass(frozen=True, eq=False, repr=False)
class Variogram(Spec):
    """A serializable variogram descriptor (``ps.variogram("spherical",
    range_m=1500)``) — built into a ``_core`` Vgm (petekTools' VariogramSpec) at
    apply. The single-home kernel stays downstream; this is the durable value."""

    _tag = "Variogram"
    model: str = "spherical"
    range_m: float = 0.0
    sill: float = 1.0
    nugget: float = 0.0

    def resolve(self, core):
        builders = {
            "spherical": core.spherical,
            "exponential": core.exponential,
            "gaussian": core.gaussian_vgm,
        }
        b = builders.get(self.model)
        if b is None:
            raise ApplyError(f"unknown variogram model {self.model!r} (Variogram spec)")
        return b(self.range_m, self.sill, self.nugget)


def variogram(model: str, range_m: float, sill: float = 1.0, nugget: float = 0.0) -> Variogram:
    return Variogram(model=model, range_m=float(range_m), sill=float(sill), nugget=float(nugget))


@spec
@dataclasses.dataclass(frozen=True, eq=False, repr=False)
class Propagate(Spec):
    """An SGS propagation recipe: a ``Variogram`` + ``seed`` + optional search
    (``max_neighbours``/``radius_m``) + optional collocated ``trend`` +
    ``mode`` (``"level_shift"`` | ``"resimulate"``) + ``allow_mean_fill``."""

    _tag = "Propagate"
    variogram: Variogram = dataclasses.field(default_factory=Variogram)
    seed: int = 1
    max_neighbours: Optional[int] = None
    radius_m: Optional[float] = None
    trend: Optional[CollocatedTrend] = None
    mode: str = "level_shift"
    allow_mean_fill: bool = False


# --- property ---------------------------------------------------------------

@spec
@dataclasses.dataclass(frozen=True, eq=False, repr=False)
class Prop(Spec):
    """One property cube's population recipe (``ps.Prop("PORO", net_only=True,
    propagate=...)``). ``zone`` scopes the pipe to one zone of the stack.
    ``upscale`` conditions from the wells; ``propagate`` runs the SGS."""

    _tag = "Prop"
    name: str = ""
    zone: Optional[str] = None
    upscale_method: str = "arithmetic"
    net_only: bool = False
    net_cutoff: float = 0.5
    propagate: Optional[Propagate] = None


def Prop_factory(name: str, zone: Optional[str] = None, upscale_method: str = "arithmetic",
                 net_only: bool = False, net_cutoff: float = 0.5,
                 propagate: Optional[Propagate] = None) -> Prop:
    warnings.warn(_PROPERTY_WORKFLOW_DEPR, DeprecationWarning, stacklevel=2)
    return Prop(name=name, zone=zone, upscale_method=upscale_method,
                net_only=net_only, net_cutoff=net_cutoff, propagate=propagate)


@spec
@dataclasses.dataclass(frozen=True, eq=False, repr=False)
class Props(Spec):
    """The set of property recipes applied at ``grid.model(props=)``."""

    _tag = "Props"
    items: Tuple[Prop, ...] = ()

    def replace(self, *args: Any, **changes: Any) -> "Props":
        if not args:
            return dataclasses.replace(self, **changes)
        target = args[0]
        new = tuple(dataclasses.replace(p, **changes) if p.name == target else p
                    for p in self.items)
        return dataclasses.replace(self, items=new)

    def __repr__(self) -> str:
        rows = []
        for p in self.items:
            prop = p.propagate
            rows.append([p.name, p.zone or "-", p.upscale_method,
                         "net" if p.net_only else "all",
                         (prop.variogram.model if prop else "-"),
                         (prop.trend.surface if prop and prop.trend else "-"),
                         (prop.mode if prop else "-")])
        return render_table(
            "Props", ["cube", "zone", "upscale", "cond", "vgm", "trend", "mode"], rows)


def Props_factory(*items: Prop) -> Props:
    warnings.warn(_PROPERTY_WORKFLOW_DEPR, DeprecationWarning, stacklevel=2)
    return Props(items=tuple(items))
