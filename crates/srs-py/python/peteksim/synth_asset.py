"""Compatibility shim for the suite synthetic asset.

The complete synthetic Petrel-export composer now lives in the horizontal
``petektools.synth_asset`` unit. petekSim keeps this module so existing imports
(``peteksim.synth_asset`` and ``from peteksim.synth_asset import synth_asset``)
continue to work, and keeps the petekSim-owned spill-forcing recipe because it
depends on the petekStatic live-set estimate surfaced through ``peteksim._core``.
"""

from __future__ import annotations

from petektools import synth_asset as _synth_asset

__all__ = ["synth_asset", "spill_recipe"]

NZONE = 6


def spill_recipe(*, ncol: int = 61, n_cubes: int = 3, nk_per_zone: int = 14) -> dict:
    """Return the documented SPILL-FORCING recipe for the suite asset.

    The estimate is petekStatic's own budget formula, read across the seam via
    ``peteksim._core.live_set_bytes``. Only the asset size and layering live here;
    the build applies the returned ``force_budget_bytes``.
    """
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
        "force_budget_bytes": int(est * 0.5),
        "recipe": (
            f"synth_asset(root, ncol={ncol}) built with ~1 m proportional layering "
            f"(~{nk_per_zone} layers/zone, {nk_total} total) + {n_cubes} property "
            f"cubes => live set ~ {est / (1 << 20):.1f} MiB; force "
            f"MemoryBudget::bytes({int(est * 0.5)}) to spill."
        ),
    }


def synth_asset(*args, **kwargs) -> dict:
    """Delegate to ``petektools.synth_asset`` and preserve petekSim manifest keys."""
    manifest = _synth_asset(*args, **kwargs)
    manifest.setdefault("spill_recipe", spill_recipe())
    return manifest
