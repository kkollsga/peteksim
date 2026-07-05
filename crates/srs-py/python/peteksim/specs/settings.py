"""Settings specs — the HOW objects: ``TieSettings``, ``Gridding`` (+ the
extrapolation policy), ``Run`` (resources), ``LoadSettings`` (ingest), and
``ViewSettings`` (render). Attached to a spec or an application call; per-row/
per-call overrides allowed.
"""

from __future__ import annotations

import dataclasses
from typing import Any, Dict, Optional, Tuple

from .base import Spec, spec


# --- extrapolation policy (a Gridding sub-value) ----------------------------

@spec
@dataclasses.dataclass(frozen=True, eq=False, repr=False)
class Extrapolation(Spec):
    """A gridding extrapolation policy — build with ``ps.decay_to_flat(range_m)``,
    ``ps.flat()``, or ``ps.nearest()``. Mirrors petekStatic's ExtrapolationPolicy;
    honoured once the structure-investigation gridding branch wires SolveOpts."""

    _tag = "Extrapolation"
    kind: str = "nearest"
    range_m: Optional[float] = None


def decay_to_flat(range_m: float) -> Extrapolation:
    """Extrapolate beyond data by decaying the gradient to flat over ``range_m``."""
    return Extrapolation(kind="decay_to_flat", range_m=float(range_m))


def flat() -> Extrapolation:
    """Hold the edge value flat beyond the data."""
    return Extrapolation(kind="flat")


def nearest() -> Extrapolation:
    """Nearest-node extrapolation beyond the data (the engine default)."""
    return Extrapolation(kind="nearest")


# --- gridding ---------------------------------------------------------------

@spec
@dataclasses.dataclass(frozen=True, eq=False, repr=False)
class Gridding(Spec):
    """Gridding settings (``ps.Gridding(fidelity_m=0.5,
    extrapolation=ps.decay_to_flat(2000))``). ``collapse`` folds
    negative/crossing isochores (the from_horizon_stack build-down); ``min_cell``
    (m) is the sub-threshold cell-collapse floor → ``collapse_below_m``.

    ``fidelity_m`` + ``extrapolation`` are forward-declared: recorded + serialized
    now, honoured when the structure-investigation branch wires SolveOpts (field
    names coordinated with its report). The output cell SIZE is
    ``grid_geometry(cell=..)``, not here."""

    _tag = "Gridding"
    fidelity_m: Optional[float] = None
    extrapolation: Optional[Extrapolation] = None
    collapse: bool = True
    min_cell: Optional[float] = None


def Gridding_factory(fidelity_m: Optional[float] = None,
                     extrapolation: Optional[Extrapolation] = None,
                     collapse: bool = True,
                     min_cell: Optional[float] = None) -> Gridding:
    return Gridding(fidelity_m=fidelity_m, extrapolation=extrapolation,
                    collapse=collapse, min_cell=min_cell)


# --- ties -------------------------------------------------------------------

@spec
@dataclasses.dataclass(frozen=True, eq=False, repr=False)
class TieSettings(Spec):
    """Well-tie settings (``ps.TieSettings(method="radius", radius_m=1500)``).
    ``method``: ``"radius"`` (tie-locality re-solve) | ``"convergent"`` (control-
    replacement re-solve). ``method``/``radius_m`` mirror petekStatic's TieSettings
    — recorded now; the current engine ties each mapped horizon to its tops picks
    (a hard control at the well node) and the tie residuals surface on the model."""

    _tag = "TieSettings"
    method: str = "convergent"
    radius_m: Optional[float] = None

    def __post_init__(self) -> None:
        if self.method not in ("radius", "convergent"):
            raise ValueError(
                f"TieSettings.method must be 'radius' or 'convergent', got {self.method!r}")


def TieSettings_factory(method: str = "convergent",
                        radius_m: Optional[float] = None) -> TieSettings:
    return TieSettings(method=method, radius_m=radius_m)


# --- run resources ----------------------------------------------------------

@spec
@dataclasses.dataclass(frozen=True, eq=False, repr=False)
class Run(Spec):
    """Run resources (``ps.Run(memory_budget=..., workers=4)``). ``memory_budget``
    (bytes) forwards to petekStatic's MemoryBudget (loud out-of-core switch, never
    an OOM kill); ``workers`` shards the MC realize loop."""

    _tag = "Run"
    memory_budget: Optional[int] = None
    workers: int = 0


def Run_factory(memory_budget: Optional[int] = None, workers: int = 0) -> Run:
    return Run(memory_budget=memory_budget, workers=int(workers))


# --- load settings ----------------------------------------------------------

@spec
@dataclasses.dataclass(frozen=True, eq=False, repr=False)
class LoadSettings(Spec):
    """Ingest settings (``ps.LoadSettings(crs="ED50 / UTM 31N", aliases={...})``).
    ``crs`` is a provenance label; ``aliases`` canonicalise log mnemonics at load."""

    _tag = "LoadSettings"
    crs: Optional[str] = None
    aliases: Tuple[Tuple[str, str], ...] = ()

    @classmethod
    def make(cls, crs: Optional[str] = None,
             aliases: Optional[Dict[str, str]] = None) -> "LoadSettings":
        pairs = tuple((str(k), str(v)) for k, v in (aliases or {}).items())
        return cls(crs=crs, aliases=pairs)

    def alias_dict(self) -> Dict[str, str]:
        return {k: v for k, v in self.aliases}


def LoadSettings_factory(crs: Optional[str] = None,
                         aliases: Optional[Dict[str, str]] = None) -> LoadSettings:
    return LoadSettings.make(crs=crs, aliases=aliases)


# --- view settings ----------------------------------------------------------

@spec
@dataclasses.dataclass(frozen=True, eq=False, repr=False)
class ViewSettings(Spec):
    """Render settings (``ps.ViewSettings(property="PORO", open_browser=False)``).
    The render/serve HOW — passed through to the petektools viewer unit."""

    _tag = "ViewSettings"
    property: Optional[str] = None
    open_browser: bool = True
    port: int = 0
    block: bool = False


def ViewSettings_factory(property: Optional[str] = None, open_browser: bool = True,
                         port: int = 0, block: bool = False) -> ViewSettings:
    return ViewSettings(property=property, open_browser=open_browser,
                        port=int(port), block=block)
