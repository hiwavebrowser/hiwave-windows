"""A registry case's `capture_args` must reach the parity-capture command line.

The Inspector (`fixtures/inspector/inspector_page.html`) is a built-in page
whose content its own script builds: fixture captures run no script unless
the case asks for `--run-inline-scripts`. If the argument were dropped on the
way, RustKit would be scored on an empty page against a Chromium baseline of
a full one, and the diff would blame the engine for the harness.

The case is not registered until its pinned Chrome baseline exists, so this
guard registers it in memory.
"""

import subprocess
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import parity_lib  # noqa: E402
import parity_test  # noqa: E402

INSPECTOR_HTML = "fixtures/inspector/inspector_page.html"
INSPECTOR_CASE = {
    "html": INSPECTOR_HTML,
    "width": 1280,
    "height": 800,
    "scope": "builtins",
    "capture_args": ["--run-inline-scripts"],
}
REAL_CASES = dict(parity_lib.CASE_REGISTRY["cases"])
parity_lib.CASE_REGISTRY["cases"]["inspector_rustkit"] = INSPECTOR_CASE


def _captured_cmd(run):
    """Run `run()` with subprocess.run stubbed; return the command it ran."""
    seen = []

    def fake_run(cmd, **_kw):
        seen.append(cmd)
        return subprocess.CompletedProcess(cmd, 1, "", "stubbed")

    real = subprocess.run
    subprocess.run = fake_run
    try:
        run()
    finally:
        subprocess.run = real
    assert len(seen) == 1, seen
    return seen[0]


def test_the_inspector_case_asks_for_its_script():
    assert parity_lib.capture_args_for(INSPECTOR_HTML) == ["--run-inline-scripts"]


def test_other_cases_ask_for_nothing():
    for case_id, case in REAL_CASES.items():
        assert parity_lib.capture_args_for(case["html"]) == [], case_id
    assert parity_lib.capture_args_for("not/a/case.html") == []


def test_the_swarm_capture_passes_them():
    tmp = Path(tempfile.mkdtemp())
    cmd = _captured_cmd(
        lambda: parity_lib.run_rustkit_capture(INSPECTOR_HTML, 1280, 800, tmp / "f.ppm", tmp / "l.json")
    )
    assert cmd[-1] == "--run-inline-scripts", cmd


def test_parity_test_passes_them():
    parity_test.OUTPUT_DIR = Path(tempfile.mkdtemp())
    cmd = _captured_cmd(lambda: parity_test.run_rustkit_capture("inspector_rustkit", INSPECTOR_HTML, 1280, 800))
    assert cmd[-1] == "--run-inline-scripts", cmd


if __name__ == "__main__":
    # CI's guard loop runs `python3 <file>`: run every test_* function and
    # fail loudly (see test_worst_first_is_worst.py).
    failed = 0
    for _name, _fn in sorted(globals().items()):
        if _name.startswith("test_") and callable(_fn):
            try:
                _fn()
                print(f"ok {_name}")
            except AssertionError:
                import traceback
                traceback.print_exc()
                print(f"FAIL {_name}")
                failed = 1
    raise SystemExit(failed)
