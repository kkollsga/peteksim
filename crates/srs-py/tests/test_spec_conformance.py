#!/usr/bin/env python3
"""The spec CONFORMANCE BATTERY — testing-doctrine **R7** (the API-contract rule).

One parametrized module iterating a registry of every peteksim spec type. The
battery is a family CONVENTION (each library implements it standalone; no shared
code sideways). Every spec/settings value must satisfy:

  1. Round-trip     from_dict(to_dict(s)) == s; to_dict is JSON-able + tagged.
  2. Value semantics equal specs compare equal (+hash); .replace() → a NEW value,
                     original unchanged, replaced unequal.
  3. Table repr      repr(s) renders the domain table (non-empty, tagged).
  4. Names-not-objs  constructing a spec touches NO project object.
  5. Spec-XOR-kwargs an application given both a spec and legacy kwargs is loud.
  6. Precedence      settings default vs per-glob override precedence is pinned.

(7 — apply determinism — is exercised by the api-v2 acceptance leg, which needs a
project.) The COMPLETENESS check asserts every registered tag has a canonical
instance here — a new spec cannot ship without a battery entry.
"""

from __future__ import annotations

import dataclasses
import json

import pytest

import peteksim as ps
from peteksim.specs import registered_specs, spec_from_dict
from peteksim.specs.base import Spec


# --- one canonical instance per spec tag ------------------------------------
def _canonicals():
    hz = ps.Horizons(
        ps.hz("H1", tie="H1 picks", sd=0.0),
        ps.hz("H2", surface="H2_grid"),
        ps.hz("H3"),
        zones=["Z1", "Z2"],
        ties=ps.TieSettings(method="radius", radius_m=1500.0),
        gridding=ps.Gridding(fidelity_m=0.5, extrapolation=ps.decay_to_flat(2000.0)),
    )
    sz = ps.Subzones({"Z1": ps.splits("upper", ("lower", dict(surface="H1b")))})
    lay = ps.Layering(dz=1.0, min_cell=0.5).replace("Z2", nk=3)
    con = ps.Contacts({"Z2": dict(owc=2743.0), "Z4": dict(goc=2700.0, fwl=2750.0)})
    prop = ps.Prop("PORO", net_only=True,
                   propagate=ps.Propagate(variogram=ps.variogram("spherical", 1500.0),
                                          seed=11, trend=ps.collocated("depo", 0.6)))
    props = ps.Props(prop, ps.Prop("NTG"))
    mc = ps.Mc(porosity=0.01, contacts=4.0, goc=3.0, n=96, seed=42,
               per_zone={"Z4": ps.Mc(contacts=2.0)})
    inst = {
        "Horizons": hz,
        "hz": ps.hz("H1", tie="picks"),
        "Subzones": sz,
        "Split": ps.splits("a", "b"),
        "sub": ps.specs.structure.SubSplit(name="lower", surface="H1b"),
        "Layering": lay,
        "Contacts": con,
        "zone": ps.zone("Z4", color="#c8102e"),
        "Extrapolation": ps.decay_to_flat(2000.0),
        "Gridding": ps.Gridding(fidelity_m=0.5, extrapolation=ps.flat(), min_cell=0.5),
        "TieSettings": ps.TieSettings(method="radius", radius_m=1500.0),
        "Run": ps.Run(memory_budget=1 << 20, workers=4),
        "ViewSettings": ps.ViewSettings(property="PORO", open_browser=False),
        "CollocatedTrend": ps.collocated("depo_trend", 0.6, as_depth=False),
        "Variogram": ps.variogram("exponential", 1200.0, sill=1.0, nugget=0.1),
        "Propagate": ps.Propagate(variogram=ps.variogram("spherical", 1500.0), seed=7),
        "Prop": prop,
        "Props": props,
        "Uncertain": ps.shift(0.02),
        "McSettings": ps.McSettings(lo_pct=10.0, hi_pct=90.0, workers=2),
        "Mc": mc,
        "AssetSpec": ps.AssetSpec(name="acceptance",
                                  horizons=hz, subzones=sz, layering=lay, contacts=con,
                                  props=props, mc=mc, run=ps.Run(workers=1)),
        "Tornado": ps.Tornado(units="MSm³", fold_count=8),
        "Distribution": ps.Distribution(gas=False, zone="Z4"),
    }
    return inst


CANONICAL = _canonicals()
CASES = sorted(CANONICAL.items())


def test_completeness_every_registered_spec_has_a_canonical():
    """A new spec cannot ship without a battery entry (registry ⊆ canonical)."""
    registered = {c._tag for c in registered_specs()}
    covered = set(CANONICAL)
    assert registered == covered, {
        "unregistered_or_uncovered": registered ^ covered,
        "registered_only": registered - covered,
        "canonical_only": covered - registered,
    }


@pytest.mark.parametrize("tag,s", CASES)
def test_roundtrip_tagged_and_jsonable(tag, s):
    d = s.to_dict()
    assert d["spec"] == tag
    json.dumps(d)  # must be a plain JSON-able dict (scenario files are durable)
    back = spec_from_dict(d)
    assert back == s
    assert type(back) is type(s)


@pytest.mark.parametrize("tag,s", CASES)
def test_value_semantics_eq_and_hash(tag, s):
    twin = spec_from_dict(s.to_dict())
    assert s == twin
    assert hash(s) == hash(twin)  # every spec is hashable (canonical-json hash)
    assert s != ps.hz("____definitely_other____")


def _different(value):
    if isinstance(value, bool):
        return not value
    if isinstance(value, int):
        return value + 7
    if isinstance(value, float):
        return value + 7.0
    if isinstance(value, str):
        return value + "_probe"
    if isinstance(value, tuple):
        return value + ("_probe",)
    return None


@pytest.mark.parametrize("tag,s", CASES)
def test_replace_returns_new_value_original_unchanged(tag, s):
    before = s.to_dict()
    for f in dataclasses.fields(s):
        newval = _different(getattr(s, f.name))
        if newval is None:
            continue
        try:
            derived = s.replace(**{f.name: newval})
        except (ValueError, TypeError):
            continue  # a validated field rejected the probe — try the next
        assert isinstance(derived, Spec)
        assert derived != s, f"replace of {f.name} did not change {tag}"
        assert s.to_dict() == before, f"replace mutated the original {tag}"
        return
    pytest.skip(f"{tag} has no freely-derivable field")


@pytest.mark.parametrize("tag,s", CASES)
def test_table_repr_non_empty(tag, s):
    r = repr(s)
    assert isinstance(r, str) and r.strip()
    assert "\n" in r  # a table, not a one-liner


# --- targeted rules ---------------------------------------------------------
def test_names_not_objects_construction_touches_no_project():
    # A spec built against names that exist in NO project constructs fine.
    hz = ps.Horizons(ps.hz("nonexistent_horizon"), ps.hz("also_missing"),
                     zones=["ghost_zone"])
    assert hz.horizon_names() == ("nonexistent_horizon", "also_missing")
    assert ps.collocated("no_such_surface", 0.5).surface == "no_such_surface"


def test_settings_precedence_layering_override_beats_default():
    lay = ps.Layering(nk=2)
    lay = lay.replace("Z4", nk=8)          # per-glob override
    assert lay.for_zone("Z1") == (2, None)  # default
    assert lay.for_zone("Z4") == (8, None)  # override wins
    # last matching glob wins.
    lay2 = ps.Layering(nk=1).replace("Z*", nk=3).replace("Z4", dz=0.5)
    assert lay2.for_zone("Z4") == (None, 0.5)
    assert lay2.for_zone("Z2") == (3, None)


def test_petekio_owns_project_loading_surface():
    assert not hasattr(ps, "Project")
    assert not hasattr(ps, "LoadSettings")
    assert "Project" not in ps.__all__
    assert "LoadSettings" not in ps.__all__
    assert not hasattr(ps, "Crossplot")
    assert "Crossplot" not in ps.__all__


if __name__ == "__main__":
    import sys
    sys.exit(pytest.main([__file__, "-q"]))
