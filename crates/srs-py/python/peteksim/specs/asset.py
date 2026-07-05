"""Asset + chart specs.

``AssetSpec`` composes a whole modelling scenario (load + structure + props + mc)
into ONE durable value — a scenario is a savable file. ``ChartSpec`` (Crossplot /
Tornado / Distribution) names WHAT data a chart shows (peteksim composes it);
petekTools renders it.
"""

from __future__ import annotations

import dataclasses
from typing import Any, Optional, Tuple

from .base import Spec, render_table, spec
from .mc import Mc
from .props import Props
from .settings import Gridding, LoadSettings, Run, TieSettings, ViewSettings
from .structure import Contacts, Horizons, Layering, Subzones


@spec
@dataclasses.dataclass(frozen=True, eq=False, repr=False)
class AssetSpec(Spec):
    """A whole modelling scenario as one value — the durable, re-applicable
    descriptor (``asset.to_dict()`` is a savable scenario file). Every field is a
    spec, so the round-trip is total."""

    _tag = "AssetSpec"
    name: str = ""
    load: Optional[LoadSettings] = None
    horizons: Optional[Horizons] = None
    subzones: Optional[Subzones] = None
    layering: Optional[Layering] = None
    contacts: Optional[Contacts] = None
    ties: Optional[TieSettings] = None
    gridding: Optional[Gridding] = None
    props: Optional[Props] = None
    mc: Optional[Mc] = None
    run: Optional[Run] = None
    view: Optional[ViewSettings] = None

    def __repr__(self) -> str:
        rows = [[f.name, "set" if getattr(self, f.name) not in (None, "") else "-"]
                for f in dataclasses.fields(self)]
        return render_table(f"AssetSpec({self.name})", ["part", "state"], rows)


# --- chart specs ------------------------------------------------------------

@spec
@dataclasses.dataclass(frozen=True, eq=False, repr=False)
class Crossplot(Spec):
    """A scatter chart (``ps.Crossplot(x="PHIE", y="PERM", y_log=True)``) — applied
    on the project (``proj.crossplot_bundle``)."""

    _tag = "Crossplot"
    x: str = ""
    y: str = ""
    wells: Tuple[str, ...] = ()
    color_by: str = "well"
    x_log: bool = False
    y_log: bool = False
    regression: bool = False

    def bundle(self, project_core):
        return project_core.crossplot_bundle(
            self.x, self.y, list(self.wells) or None, self.color_by,
            self.x_log, self.y_log, self.regression)


@spec
@dataclasses.dataclass(frozen=True, eq=False, repr=False)
class Tornado(Spec):
    """A tornado chart (``ps.Tornado(units="MSm³")``) — applied on an ``uncertainty``
    result (``mc.tornado_bundle``)."""

    _tag = "Tornado"
    base: Optional[float] = None
    units: str = "MSm³"
    fold_count: int = 8

    def bundle(self, unc):
        return unc.tornado_bundle(self.base, self.units, self.fold_count)


@spec
@dataclasses.dataclass(frozen=True, eq=False, repr=False)
class Distribution(Spec):
    """A distribution chart (``ps.Distribution(zone="Z4")``) — applied on an
    uncertainty result (``mc.distribution_bundle``)."""

    _tag = "Distribution"
    gas: bool = False
    zone: Optional[str] = None
    name: Optional[str] = None

    def bundle(self, unc):
        # ZonedUncertainty.distribution_bundle(gas, zone, name); Uncertainty has no zone.
        if self.zone is not None:
            return unc.distribution_bundle(self.gas, self.zone, self.name)
        try:
            return unc.distribution_bundle(self.gas, self.name)
        except TypeError:
            return unc.distribution_bundle(self.gas, None, self.name)


def crossplot(x: str, y: str, wells: Optional[Any] = None, color_by: str = "well",
              x_log: bool = False, y_log: bool = False, regression: bool = False) -> Crossplot:
    return Crossplot(x=x, y=y, wells=tuple(wells or ()), color_by=color_by,
                     x_log=x_log, y_log=y_log, regression=regression)


def tornado(base: Optional[float] = None, units: str = "MSm³", fold_count: int = 8) -> Tornado:
    return Tornado(base=base, units=units, fold_count=fold_count)


def distribution(gas: bool = False, zone: Optional[str] = None,
                 name: Optional[str] = None) -> Distribution:
    return Distribution(gas=gas, zone=zone, name=name)
