#!/usr/bin/env python3
"""The parity-capture suites must actually EXECUTE in CI.

WHAT THIS GUARDS
----------------
Until 2026-10-27 CI only `cargo build`ed parity-capture. The #796 LiveWake
pin (`an_inserted_script_runs_when_its_fetch_arrives_not_at_the_end_of_the_window`)
and the diagnose report ranking units lived on the crate but never ran on a
PR — a regression would silently sleep through `--live-ms` again.

This guard holds the `capture-suites` lane (F2c) to:

1. It exists and invokes `cargo test -p parity-capture` (binary crate, no `--lib`).
2. It does not pass `--no-run`.
3. It is BLOCKING: no `continue-on-error`.
4. It runs on macos-14 (same CoreText / headless surface as F2/F2b).
5. It publishes receipts to `$GITHUB_STEP_SUMMARY`.
6. It is not conditioned away on pull requests.

Run: python3 scripts/tests/test_capture_suites_actually_run.py
"""
import sys
from pathlib import Path

import yaml

REPO_ROOT = Path(__file__).resolve().parents[2]
WORKFLOW = REPO_ROOT / ".github" / "workflows" / "parity.yml"

REQUIRED_SUITES = ("parity-capture",)


def _workflow():
    return yaml.safe_load(WORKFLOW.read_text(encoding="utf-8"))


def _lane():
    jobs = _workflow()["jobs"]
    assert "capture-suites" in jobs, (
        "no `capture-suites` job in parity.yml — parity-capture suites must run in CI"
    )
    return jobs["capture-suites"]


def _lane_step():
    steps = [s for s in _lane()["steps"] if "run" in s]
    assert len(steps) == 1, f"expected one run step in capture-suites, got {len(steps)}"
    return steps[0]


def test_the_lane_executes_rather_than_compiling():
    run = _lane_step()["run"]
    assert "cargo test" in run, "the capture-suites lane does not invoke cargo test"
    assert "--no-run" not in run, (
        "the capture-suites lane passes --no-run: tests would never execute"
    )
    for pkg in REQUIRED_SUITES:
        assert pkg in run, f"{pkg} is not in the capture-suites lane"


def test_the_lane_runs_the_binary_crate_not_lib_only():
    """parity-capture is a binary crate; its tests live in main.rs / report.rs."""
    run = _lane_step()["run"]
    assert "--lib" not in run, (
        "the capture-suites lane must NOT pass --lib; parity-capture has no lib target"
    )


def test_the_lane_is_blocking():
    step = _lane_step()
    lane = _lane()
    assert not step.get("continue-on-error"), (
        "capture-suites step has continue-on-error: true; it must be blocking"
    )
    assert not lane.get("continue-on-error"), (
        "capture-suites job has continue-on-error: true; it must be blocking"
    )


def test_the_lane_runs_on_macos14():
    assert _lane()["runs-on"] == "macos-14", "capture-suites must run on macos-14"


def test_the_receipt_reaches_the_job_summary():
    assert "$GITHUB_STEP_SUMMARY" in _lane_step()["run"], (
        "capture-suites does not write receipts to $GITHUB_STEP_SUMMARY"
    )


def test_the_lane_is_not_skipped_on_pull_requests():
    cond = str(_lane().get("if", "")).strip()
    assert "pull_request" not in cond, (
        f"capture-suites is conditioned away from PR lane (if: {cond})"
    )


if __name__ == "__main__":
    assert WORKFLOW.exists(), f"no workflow at {WORKFLOW}"
    failed = 0
    for name, fn in sorted(globals().items()):
        if name.startswith("test_") and callable(fn):
            try:
                fn()
                print(f"ok  {name}")
            except AssertionError as exc:
                failed = 1
                print(f"FAIL {name}: {exc}")
    if failed:
        sys.exit(1)
    print("PASS: the parity-capture suites execute and block in CI")
