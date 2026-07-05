#!/usr/bin/env python3
"""Scatter-conditioning dedup proof — ``task_suite_scatter_perf`` (Ask 1).

petekStatic's per-horizon cold bilinear conditioning solve
(``condition_scatter`` → petekTools ``grid_min_curvature``) is the dominant cost of
the canonical scatter build (~60 s / horizon on the real model). peteksim used to
re-run it on the SAME raw scatter up to THREE times per model lifecycle — once in
the stack build (``grid.model``), once in the MC template (``Model.stack_template``),
and once in the QC scratch build (``StaticGrid.scratch_grid``). This suite proves
the dedup: after adopting petekStatic's ``condition_scatter_stack`` seam, a
build + zoned-MC run conditions each scatter horizon **exactly once**.

The proof counts petekStatic's ``[SRS_PROFILE] condition_scatter horizon=…`` stderr
hooks (one line per conditioned horizon per pass) across a full chain driven in a
subprocess with ``SRS_PROFILE=1``. The synthetic asset is emitted with
``surfaces_as_points=True`` so its horizons load as scattered point-sets and travel
petekStatic's ``from_scatter_stack`` conditioning path — the canonical real-model
shape. Size-independent (the *count*, not the wall, is the invariant), so it runs at
a small lattice and stays fast.

    .venv-srs/bin/python -m pytest crates/srs-py/tests/test_scatter_dedup.py -q
"""

from __future__ import annotations

import os
import re
import subprocess
import sys
import tempfile
from pathlib import Path

import pytest

# The dedup COUNT (one pass) is size-independent. The chain runs in the default
# gate (~2 s): pinning the scatter lattice to the asset's own spacing
# (`cell_size_m=100`, a 21x21 frame) keeps the cap-bound conditioning solve cheap.
# The `scatter_perf` marker just lets you isolate this leg (`-m scatter_perf`).
NCOL = 21
N_HORIZONS = 7  # synth asset v2 mapped horizons H0..H6 (all point-sets here)

# The full build + zoned-MC chain, run under SRS_PROFILE=1 in a clean subprocess so
# petekStatic's per-horizon conditioning hooks are captured in isolation. Mirrors the
# acceptance chain (PORO net-conditioned + NTG propagated + per-zone contacts), but
# forces the SCATTER path via surfaces_as_points=True.
_DRIVER = f"""
import tempfile
import peteksim as ps

root = tempfile.mkdtemp(prefix="scatter-dedup-")
man = ps.synth_asset(root, ncol={NCOL}, surfaces_as_points=True)
proj = ps.Project.load(man["root"], crs=man["crs"], aliases=man["aliases"])

# Sanity: the horizons must have loaded as POINT-SETS (the scatter path), else the
# proof is vacuous (a Mapped stack conditions nothing).
inv = proj.inventory()
assert set(man["horizons"]) <= set(inv.points), ("horizons not scatter", inv.points)

# cell_size_m pins the derived scatter lattice to the asset's own node spacing
# ({NCOL}x{NCOL}); without it the framework derives a finer frame and the cap-bound
# solve gets needlessly expensive.
fw = proj.framework(horizons=man["horizons"], outline="ModelEdge",
                    tie_to_tops=True, min_thickness_m=0.0, cell_size_m=100.0)
fw.set_zonation([dict(z, nk=2) for z in man["zonation"]])
grid = fw.build_grid()

por = grid.property("PORO")
por.upscale(proj.wells(), net_only=True)
# A QC pass FIRST (grid.model consumes the grid) — its scratch build historically
# RE-conditioned the scatter; it must NOT now (reuses the pre-conditioned stack).
por.qc()
por.propagate(ps.gaussian(ps.spherical(range_m=1500.0), seed=11))
ntg = grid.property("NTG")
ntg.upscale(proj.wells())
ntg.propagate(ps.gaussian(ps.spherical(range_m=1500.0), seed=12))

model = grid.model(fluid="oil", fvf=1.30, gas_fvf=0.005, wells=proj.wells())
mc = model.zoned_uncertainty(contacts=ps.pick_spread(sd_m=4.0), n=4, seed=7)
assert mc.total["stoiip"]["p50"] >= 0.0
print("CHAIN_OK")
"""

_COND_RE = re.compile(r"condition_scatter horizon=")


def _run_chain_with_profile() -> tuple[int, str]:
    """Run the chain under SRS_PROFILE=1; return (conditioning-line count, stderr)."""
    env = dict(os.environ, SRS_PROFILE="1")
    proc = subprocess.run(
        [sys.executable, "-c", _DRIVER],
        capture_output=True, text=True, env=env, timeout=600,
    )
    assert "CHAIN_OK" in proc.stdout, (proc.stdout, proc.stderr[-2000:])
    count = len(_COND_RE.findall(proc.stderr))
    return count, proc.stderr


@pytest.mark.scatter_perf
def test_build_and_zoned_mc_condition_scatter_exactly_once():
    """A build + QC + zoned-MC run conditions each scatter horizon EXACTLY ONCE.

    Before the dedup this was 2–3× (build + MC template [+ QC scratch]); the count
    is the direct regression guard against a re-conditioning path creeping back."""
    count, stderr = _run_chain_with_profile()
    # The scatter path must actually have run (else the proof is vacuous).
    assert count > 0, ("no conditioning observed — scatter path not exercised", stderr[-2000:])
    # Exactly one pass over the N horizons — no build/template/QC re-conditioning.
    assert count == N_HORIZONS, (
        f"expected {N_HORIZONS} conditioning passes (one per horizon, conditioned once), "
        f"got {count} — a re-conditioning path regressed",
    )


if __name__ == "__main__":
    sys.exit(pytest.main([__file__, "-q", "-s"]))
