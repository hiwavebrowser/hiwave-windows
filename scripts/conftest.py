"""Windows: run child Python processes in UTF-8 mode.

The parity scripts print non-ASCII text (em dashes, arrows). A Python child
whose stdout is a pipe writes the locale encoding, cp1252 on Windows, and the
tests decode it as UTF-8. When that decode fails, subprocess.run on Windows
returns stdout=None and the test fails on a TypeError that says nothing about
the gate. With PYTHONUTF8=1 inherited, every child writes UTF-8, as on macOS.
Windows-only file: hiwave-macos has no conftest here, and the sync never
overwrites it.
"""
import os

os.environ.setdefault("PYTHONUTF8", "1")

from pathlib import Path

import pytest

_REPO = Path(__file__).resolve().parent.parent

# Tests in synced files that inspect the macOS parity CI lane. Windows has no
# parity CI lane (see scripts/collect_metrics.py), so they skip while
# .github/workflows/parity.yml is absent here.
_MACOS_CI_LANE = {
    "test_both_aggregate_lanes_compute_the_metric",
    "test_the_receipt_runs_even_after_an_earlier_step_failed",
    "test_the_receipt_is_fed_all_three_inputs",
    "test_the_guard_suite_runs_in_ci",
    "test_the_guard_job_installs_the_yaml_parser_the_lane_guards_need",
}

# hiwave-windows origin/master predates the engine refreshes (#91-#94), so its
# lib.rs lacks functions the census walks. The working-tree half of this test
# passes; it XPASSes once develop is released to master.
_WINDOWS_MASTER_BEHIND = {"test_committed_ledger_holds_on_the_real_trees"}


def pytest_collection_modifyitems(config, items):
    no_ci_lane = not (_REPO / ".github" / "workflows" / "parity.yml").exists()
    for item in items:
        if no_ci_lane and item.name in _MACOS_CI_LANE:
            item.add_marker(pytest.mark.skip(reason="macOS parity CI lane; Windows has none"))
        if item.name in _WINDOWS_MASTER_BEHIND:
            item.add_marker(pytest.mark.xfail(
                reason="hiwave-windows origin/master predates the engine refreshes", strict=False))
