"""Monte-Carlo specs — one ``Mc`` value unifying the ``uncertainty`` /
``zoned_uncertainty`` kwarg pair (auto-routed on ``model.is_zoned()``), with
per-zone overrides and an ``McSettings`` (lo/hi percentiles + workers).

Distribution inputs are serializable ``Uncertain`` descriptors (a scalar is
sugar: a property scalar → a level shift sd; a contact scalar → a pick-spread
sd_m). The STRUCTURAL leg (contact draws) stays here; the horizon-depth/isochore
structural draws migrate to ``Horizons`` sd/vgm (task_petekstatic_structural_
uncertainty).
"""

from __future__ import annotations

import dataclasses
from typing import Any, Dict, Optional, Tuple

from .base import ApplyError, Spec, render_table, spec

_DIST_ARITY = {
    "level_shift": ("sd",),
    "normal": ("mean", "sd"),
    "lognormal": ("mean", "sd"),
    "uniform": ("lo", "hi"),
    "triangular": ("lo", "mode", "hi"),
    "truncated_normal": ("mean", "sd", "lo", "hi"),
}


@spec
@dataclasses.dataclass(frozen=True, eq=False, repr=False)
class Uncertain(Spec):
    """A serializable property distribution (``kind`` + ordered ``params``), built
    into a ``_core`` Dist at apply. ``ps.shift(sd)`` / ``ps.dist(kind, ...)``."""

    _tag = "Uncertain"
    kind: str = "level_shift"
    params: Tuple[float, ...] = ()

    def __post_init__(self) -> None:
        arity = _DIST_ARITY.get(self.kind)
        if arity is None:
            raise ValueError(f"unknown distribution kind {self.kind!r}")
        if len(self.params) != len(arity):
            raise ValueError(
                f"{self.kind} needs {len(arity)} params {arity}, got {self.params}")

    def resolve(self, core):
        builders = {
            "level_shift": core.level_shift,
            "normal": core.normal,
            "lognormal": core.lognormal,
            "uniform": core.uniform,
            "triangular": core.triangular,
            "truncated_normal": core.truncated_normal,
        }
        return builders[self.kind](*self.params)


def shift(sd: float) -> Uncertain:
    """A zero-mean level shift (``ps.shift(0.01)``) for a modelled cube."""
    return Uncertain(kind="level_shift", params=(float(sd),))


def dist(kind: str, *params: float) -> Uncertain:
    """A named distribution descriptor (``ps.dist("normal", 0.25, 0.02)``)."""
    return Uncertain(kind=kind, params=tuple(float(p) for p in params))


def _as_uncertain(v: Any, name: str) -> Optional[Uncertain]:
    if v is None:
        return None
    if isinstance(v, Uncertain):
        return v
    if isinstance(v, (int, float)):
        return shift(float(v))  # a bare property scalar = a level shift sd
    raise ApplyError(
        f"Mc field {name!r} must be a scalar sd, ps.shift(...)/ps.dist(...), or None "
        f"(an opaque _core Dist cannot be serialized — use the value builders)")


def _as_sd(v: Any, name: str) -> Optional[float]:
    if v is None:
        return None
    if isinstance(v, (int, float)):
        return float(v)
    sd = getattr(v, "sd_m", None)  # a _core.PickSpread exposes sd_m
    if sd is not None:
        return float(sd)
    raise ApplyError(f"Mc contact {name!r} must be a float sd_m, ps.pick_spread(...), or None")


@spec
@dataclasses.dataclass(frozen=True, eq=False, repr=False)
class McSettings(Spec):
    """MC run settings (``ps.McSettings(lo_pct=10, hi_pct=90, workers=4)``) — the
    percentile band + realize-loop sharding. Mirrors petekStatic's McSettings."""

    _tag = "McSettings"
    lo_pct: float = 10.0
    hi_pct: float = 90.0
    workers: int = 0


@spec
@dataclasses.dataclass(frozen=True, eq=False, repr=False)
class Mc(Spec):
    """The unified uncertainty spec. Property fields take a scalar sd / ``ps.shift``
    / ``ps.dist``; contact fields (``contacts`` = lower FWL/OWC, ``goc``) take a
    scalar sd_m / ``ps.pick_spread``. ``per_zone`` overrides by zone. Auto-routes
    to ``zoned_uncertainty`` on a zoned model, else ``uncertainty``."""

    _tag = "Mc"
    porosity: Optional[Uncertain] = None
    net_to_gross: Optional[Uncertain] = None
    water_saturation: Optional[Uncertain] = None
    fvf: Optional[Uncertain] = None
    gas_fvf: Optional[Uncertain] = None
    contacts_sd_m: Optional[float] = None
    goc_sd_m: Optional[float] = None
    per_zone: Tuple[Tuple[str, "Mc"], ...] = ()
    settings: McSettings = dataclasses.field(default_factory=McSettings)
    n: int = 10_000
    seed: int = 42

    def __repr__(self) -> str:
        rows = [
            ["porosity", self.porosity],
            ["net_to_gross", self.net_to_gross],
            ["water_saturation", self.water_saturation],
            ["contacts_sd_m", self.contacts_sd_m],
            ["goc_sd_m", self.goc_sd_m],
            ["n / seed", f"{self.n} / {self.seed}"],
            ["lo/hi/workers",
             f"{self.settings.lo_pct}/{self.settings.hi_pct}/{self.settings.workers}"],
        ]
        rows += [[f"per_zone[{z}]", "override"] for z, _ in self.per_zone]
        return render_table("Mc", ["input", "uncertainty"], rows)


def Mc_factory(porosity: Any = None, net_to_gross: Any = None, water_saturation: Any = None,
               fvf: Any = None, gas_fvf: Any = None, contacts: Any = None, goc: Any = None,
               per_zone: Optional[Dict[str, "Mc"]] = None,
               settings: Optional[McSettings] = None,
               n: int = 10_000, seed: int = 42) -> Mc:
    """Build an ``Mc`` — normalizing scalars/``_core`` spreads into serializable
    descriptors. ``contacts`` is the lower (FWL/OWC) spread; ``goc`` its own."""
    pz = tuple((z, m) for z, m in (per_zone or {}).items())
    return Mc(
        porosity=_as_uncertain(porosity, "porosity"),
        net_to_gross=_as_uncertain(net_to_gross, "net_to_gross"),
        water_saturation=_as_uncertain(water_saturation, "water_saturation"),
        fvf=_as_uncertain(fvf, "fvf"),
        gas_fvf=_as_uncertain(gas_fvf, "gas_fvf"),
        contacts_sd_m=_as_sd(contacts, "contacts"),
        goc_sd_m=_as_sd(goc, "goc"),
        per_zone=pz,
        settings=settings or McSettings(),
        n=int(n), seed=int(seed),
    )
