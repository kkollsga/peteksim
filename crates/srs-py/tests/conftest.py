"""Pytest configuration for the peteksim test tree.

Registers the acceptance-suite markers (doctrine R6, `test_acceptance.py`) so
`pytest -m acceptance` selects the standing pre-stamp gate and the heavier
`acceptance_spill` / `acceptance_render` legs stay opt-in.
"""

from __future__ import annotations


def pytest_configure(config):
    config.addinivalue_line(
        "markers",
        "acceptance: the R6 end-to-end acceptance gate — full chain + payload "
        "invariants + planted-truth recoveries on synth asset v2 (fast, per-wave).",
    )
    config.addinivalue_line(
        "markers",
        "acceptance_spill: the out-of-core (spilled) leg of the R6 suite — opt-in "
        "(forces petekStatic's MemoryBudget below the live set).",
    )
    config.addinivalue_line(
        "markers",
        "acceptance_render: the Playwright browser-render leg of the R6 suite — "
        "opt-in; skips cleanly when node/playwright/chromium are unavailable.",
    )
    config.addinivalue_line(
        "markers",
        "scatter_perf: the scatter-conditioning dedup proof (task_suite_scatter_perf, "
        "test_scatter_dedup.py) — opt-in; drives the cap-bound scatter build on the "
        "synthetic asset and counts petekStatic's SRS_PROFILE conditioning hooks.",
    )
