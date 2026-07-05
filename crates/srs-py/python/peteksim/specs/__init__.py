"""The declarative spec value layer (modelling API v2). Public spec types +
factory functions + the value-semantics foundation (base). Compute stays in the
Rust ``peteksim._core`` engine; the ``peteksim.apply`` driver resolves + applies.
"""

from __future__ import annotations

from .base import (
    ApplyError,
    NotYetSupported,
    Spec,
    registered_specs,
    spec_from_dict,
)
from .structure import (
    Contacts,
    Contacts_factory,
    HorizonRow,
    Horizons,
    Horizons_factory,
    Layering,
    Layering_factory,
    Split,
    SubSplit,
    Subzones,
    Subzones_factory,
    ZoneColor,
    hz,
    splits,
    zone,
)
from .settings import (
    Extrapolation,
    Gridding,
    Gridding_factory,
    LoadSettings,
    LoadSettings_factory,
    Run,
    Run_factory,
    TieSettings,
    TieSettings_factory,
    ViewSettings,
    ViewSettings_factory,
    decay_to_flat,
    flat,
    nearest,
)
from .props import (
    CollocatedTrend,
    Prop,
    Prop_factory,
    Propagate,
    Props,
    Props_factory,
    Variogram,
    collocated_name,
    variogram,
)
from .mc import Mc, McSettings, Mc_factory, Uncertain, dist, shift
from .asset import (
    AssetSpec,
    Crossplot,
    Distribution,
    Tornado,
    crossplot,
    distribution,
    tornado,
)

__all__ = [
    "Spec", "spec_from_dict", "registered_specs", "NotYetSupported", "ApplyError",
    # structure
    "Horizons", "HorizonRow", "hz", "Subzones", "Split", "SubSplit", "splits",
    "Layering", "Contacts", "ZoneColor", "zone",
    "Horizons_factory", "Subzones_factory", "Layering_factory", "Contacts_factory",
    # settings
    "TieSettings", "Gridding", "Extrapolation", "Run", "LoadSettings", "ViewSettings",
    "decay_to_flat", "flat", "nearest",
    "TieSettings_factory", "Gridding_factory", "Run_factory", "LoadSettings_factory",
    "ViewSettings_factory",
    # props
    "Prop", "Props", "Propagate", "Variogram", "CollocatedTrend",
    "Prop_factory", "Props_factory", "variogram", "collocated_name",
    # mc
    "Mc", "McSettings", "Uncertain", "shift", "dist", "Mc_factory",
    # asset / charts
    "AssetSpec", "Crossplot", "Tornado", "Distribution",
    "crossplot", "tornado", "distribution",
]
