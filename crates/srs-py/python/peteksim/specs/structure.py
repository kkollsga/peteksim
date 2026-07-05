"""Structure specs — the stratigraphic framework as declarative values.

``Horizons`` (the ordered column + per-row ties/uncertainty), ``Subzones`` (per-
zone sub-splits + conformity), ``Layering`` (dz/nk allocation with glob
overrides), ``Contacts`` (per-zone fluid contacts by glob), and ``zone(name,
color=)`` (a zone's display colour). All hold NAMES, resolved at apply.
"""

from __future__ import annotations

import dataclasses
from typing import Any, Dict, Optional, Tuple

from .base import ApplyError, Spec, match_glob, render_table, spec


# --- horizons ---------------------------------------------------------------

@spec
@dataclasses.dataclass(frozen=True, eq=False, repr=False)
class HorizonRow(Spec):
    """One horizon in the column (``ps.hz``). ``surface`` defaults to ``name`` at
    apply (resolved: a loaded point-set → Scatter, a loaded grid → Mapped).
    ``tie`` names the pick set the horizon ties to (defaults to ``name``). ``sd``
    (m) + ``vgm`` = (model, range) declare the structural-uncertainty field —
    serialized now; the ENGINE capability lands with task_petekstatic_structural_
    uncertainty (top row = a depth perturbation; deeper rows = isochore)."""

    _tag = "hz"
    name: str
    surface: Optional[str] = None
    tie: Optional[str] = None
    sd: float = 0.0
    vgm: Optional[Tuple[str, float]] = None

    def resolved_surface(self) -> str:
        return self.surface if self.surface is not None else self.name

    def resolved_tie(self) -> str:
        return self.tie if self.tie is not None else self.name

    def has_uncertainty(self) -> bool:
        return bool(self.sd) or self.vgm is not None


def hz(name: str, surface: Optional[str] = None, tie: Optional[str] = None,
       sd: float = 0.0, vgm: Optional[Tuple[str, float]] = None) -> HorizonRow:
    """A horizon row for ``ps.Horizons`` — ``ps.hz("H1", tie="H1 picks",
    sd=3.0, vgm=("spherical", 2500))``."""
    return HorizonRow(name=name, surface=surface, tie=tie, sd=float(sd),
                      vgm=tuple(vgm) if vgm is not None else None)


@spec
@dataclasses.dataclass(frozen=True, eq=False, repr=False)
class Horizons(Spec):
    """The ordered horizon column (top→down) + the zones between them.

    ``zones`` names the zones top→down (one per gap); zone *i* sits between
    ``rows[i]`` and ``rows[i+1]`` (its base = ``rows[i+1]``). ``ties``/``gridding``
    are the default settings objects (per-row/per-zone exceptions allowed)."""

    _tag = "Horizons"
    rows: Tuple[HorizonRow, ...] = ()
    zones: Tuple[str, ...] = ()
    ties: Optional[Spec] = None
    gridding: Optional[Spec] = None

    def horizon_names(self) -> Tuple[str, ...]:
        return tuple(r.name for r in self.rows)

    def row(self, name: str) -> HorizonRow:
        for r in self.rows:
            if r.name == name:
                return r
        raise KeyError(name)

    def below_horizon(self, zone: str) -> str:
        """The base horizon of ``zone`` (the next row after the zone's index)."""
        try:
            i = self.zones.index(zone)
        except ValueError:
            raise ApplyError(f"zone {zone!r} is not in Horizons.zones {list(self.zones)}")
        if i + 1 >= len(self.rows):
            raise ApplyError(
                f"zone {zone!r} (index {i}) has no base horizon — Horizons needs "
                f"at least {i + 2} rows, has {len(self.rows)}")
        return self.rows[i + 1].name

    def replace(self, *args: Any, **changes: Any) -> "Horizons":
        """``hz.replace("H1", surface=...)`` derives a new column with the named/
        globbed row(s) changed; keyword-only replaces the column's own fields."""
        if not args:
            return dataclasses.replace(self, **changes)
        target = args[0]
        new_rows = tuple(
            dataclasses.replace(r, **changes) if match_glob(target, r.name) else r
            for r in self.rows)
        if new_rows == self.rows:
            raise ApplyError(f"Horizons.replace: no row matches {target!r}")
        return dataclasses.replace(self, rows=new_rows)

    def __repr__(self) -> str:
        rows = []
        for i, r in enumerate(self.rows):
            zone = self.zones[i] if i < len(self.zones) else ""
            rows.append([i, r.name, r.resolved_surface(), r.resolved_tie(),
                         f"{r.sd:g}", r.vgm or "", zone])
        return render_table(
            "Horizons (stratigraphic column, top→down)",
            ["#", "horizon", "surface", "tie", "sd_m", "vgm", "zone_below"], rows)


def Horizons_factory(*rows: HorizonRow, zones: Optional[Any] = None,
                     ties: Optional[Spec] = None,
                     gridding: Optional[Spec] = None) -> Horizons:
    return Horizons(rows=tuple(rows), zones=tuple(zones or ()),
                    ties=ties, gridding=gridding)


# --- subzones / splits ------------------------------------------------------

@spec
@dataclasses.dataclass(frozen=True, eq=False, repr=False)
class SubSplit(Spec):
    """One internal split of a zone (``name`` [+ optional ``surface``/``tie``])."""

    _tag = "sub"
    name: str
    surface: Optional[str] = None
    tie: Optional[str] = None


@spec
@dataclasses.dataclass(frozen=True, eq=False, repr=False)
class Split(Spec):
    """A zone's split recipe (``ps.splits``): ordered internal ``subs`` +
    ``conformity`` (proportional | follow_top | follow_base)."""

    _tag = "Split"
    subs: Tuple[SubSplit, ...] = ()
    conformity: str = "proportional"


def splits(*entries: Any, conformity: str = "proportional") -> Split:
    """A zone split recipe — each entry a name or ``(name, dict(surface=,
    tie=))``: ``ps.splits("upper", ("lower", dict(surface="H3b")))``."""
    subs = []
    for e in entries:
        if isinstance(e, SubSplit):
            subs.append(e)
        elif isinstance(e, str):
            subs.append(SubSplit(name=e))
        elif isinstance(e, (tuple, list)) and len(e) == 2 and isinstance(e[1], dict):
            subs.append(SubSplit(name=e[0], surface=e[1].get("surface"), tie=e[1].get("tie")))
        else:
            raise ValueError(f"split entry must be a name or (name, dict), got {e!r}")
    return Split(subs=tuple(subs), conformity=conformity)


@spec
@dataclasses.dataclass(frozen=True, eq=False, repr=False)
class Subzones(Spec):
    """Per-zone split recipes (``ps.Subzones({zone: ps.splits(...)})``). Absent =
    a single interval per zone (proportional)."""

    _tag = "Subzones"
    entries: Tuple[Tuple[str, Split], ...] = ()

    @classmethod
    def _from_mapping(cls, mapping: Dict[str, Split]) -> "Subzones":
        return cls(entries=tuple((k, v) for k, v in mapping.items()))

    def get(self, zone: str) -> Optional[Split]:
        for k, v in self.entries:
            if k == zone:
                return v
        return None

    def replace(self, *args: Any, **changes: Any) -> "Subzones":
        if not args:
            return dataclasses.replace(self, **changes)
        target = args[0]
        new = tuple((k, dataclasses.replace(v, **changes) if match_glob(target, k) else v)
                    for k, v in self.entries)
        return dataclasses.replace(self, entries=new)

    def __repr__(self) -> str:
        rows = [[k, len(v.subs) or 1, v.conformity,
                 ", ".join(s.name for s in v.subs)] for k, v in self.entries]
        return render_table("Subzones", ["zone", "n_split", "conformity", "splits"], rows)


def Subzones_factory(mapping: Optional[Dict[str, Split]] = None) -> Subzones:
    return Subzones._from_mapping(mapping or {})


# --- layering ---------------------------------------------------------------

@spec
@dataclasses.dataclass(frozen=True, eq=False, repr=False)
class Layering(Spec):
    """Layer allocation (``ps.Layering(dz=1.0, min_cell=0.5)``). ``dz``/``nk`` set
    the default; ``overrides`` (glob→dict) tune per-zone: ``lay.replace("Z*",
    dz=0.5)``. ``min_cell`` (m) is the sub-threshold cell-collapse floor."""

    _tag = "Layering"
    dz: Optional[float] = None
    nk: Optional[int] = None
    min_cell: Optional[float] = None
    overrides: Tuple[Tuple[str, Tuple[Tuple[str, Any], ...]], ...] = ()

    def for_zone(self, zone: str) -> Tuple[Optional[int], Optional[float]]:
        """(nk, dz) for ``zone`` — the default unless a glob override matches
        (last match wins)."""
        nk, dz = self.nk, self.dz
        for glob, kv in self.overrides:
            if match_glob(glob, zone):
                d = dict(kv)
                if "nk" in d:
                    nk, dz = int(d["nk"]), None
                if "dz" in d:
                    dz, nk = float(d["dz"]), None
        if nk is None and dz is None:
            nk = 1
        return nk, dz

    def replace(self, *args: Any, **changes: Any) -> "Layering":
        """Keyword-only changes the defaults; ``lay.replace("Z*", dz=0.5)`` adds a
        per-glob override."""
        if not args:
            return dataclasses.replace(self, **changes)
        glob = args[0]
        kv = tuple((k, v) for k, v in changes.items())
        return dataclasses.replace(self, overrides=self.overrides + ((glob, kv),))

    def __repr__(self) -> str:
        rows = [["default", self.nk, self.dz, self.min_cell]]
        for glob, kv in self.overrides:
            d = dict(kv)
            rows.append([glob, d.get("nk"), d.get("dz"), d.get("min_cell")])
        return render_table("Layering", ["scope", "nk", "dz", "min_cell"], rows)


def Layering_factory(dz: Optional[float] = None, nk: Optional[int] = None,
                     min_cell: Optional[float] = None) -> Layering:
    return Layering(dz=dz, nk=nk, min_cell=min_cell)


# --- contacts ---------------------------------------------------------------

@spec
@dataclasses.dataclass(frozen=True, eq=False, repr=False)
class Contacts(Spec):
    """Per-zone fluid contacts by glob (``ps.Contacts({"Z4": dict(goc=.., fwl=..),
    "Z2": dict(owc=..)})``). A zone with no matching entry is contactless."""

    _tag = "Contacts"
    entries: Tuple[Tuple[str, Tuple[Tuple[str, float], ...]], ...] = ()

    @classmethod
    def _from_mapping(cls, mapping: Dict[str, Dict[str, float]]) -> "Contacts":
        out = []
        for glob, d in mapping.items():
            out.append((glob, tuple((k, float(v)) for k, v in d.items())))
        return cls(entries=tuple(out))

    def for_zone(self, zone: str) -> Optional[Dict[str, float]]:
        """The merged contacts dict for ``zone`` (goc/owc/fwl), or None."""
        merged: Dict[str, float] = {}
        for glob, kv in self.entries:
            if match_glob(glob, zone):
                merged.update(dict(kv))
        return merged or None

    def replace(self, *args: Any, **changes: Any) -> "Contacts":
        if not args:
            return dataclasses.replace(self, **changes)
        target = args[0]
        kv = tuple((k, float(v)) for k, v in changes.items())
        new = []
        matched = False
        for glob, old in self.entries:
            if glob == target:
                new.append((glob, kv))
                matched = True
            else:
                new.append((glob, old))
        if not matched:
            new.append((target, kv))
        return dataclasses.replace(self, entries=tuple(new))

    def __repr__(self) -> str:
        rows = []
        for glob, kv in self.entries:
            d = dict(kv)
            rows.append([glob, d.get("goc"), d.get("owc") or d.get("fwl")])
        return render_table("Contacts", ["zone", "goc", "owc/fwl"], rows)


def Contacts_factory(mapping: Optional[Dict[str, Dict[str, float]]] = None) -> Contacts:
    return Contacts._from_mapping(mapping or {})


# --- zone colour ------------------------------------------------------------

@spec
@dataclasses.dataclass(frozen=True, eq=False, repr=False)
class ZoneColor(Spec):
    """A zone's display colour slot (``ps.zone("Z4", color="#c8102e")``). Carried
    on the StackZone (name + colour); the viewer honours it once StackZone.color
    is wired through the build seam."""

    _tag = "zone"
    name: str
    color: Optional[str] = None


def zone(name: str, color: Optional[str] = None) -> ZoneColor:
    return ZoneColor(name=name, color=color)
