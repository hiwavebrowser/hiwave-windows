"""The "Worst N Cases" banner must actually show the worst cases.

Until 2026-07-29 it sorted ascending and printed the three *best* ones. A banner that names the
healthiest pages as the worst is the same family as an empty capture scored 100.0 (#65): the number
is real, the label is a lie, and the reader acts on the label.
"""

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from parity_test import worst_first  # noqa: E402


def _r(case_id, diff):
    return {"case_id": case_id, "diff_pct": diff}


def test_worst_case_comes_first():
    results = [_r("bg-pure", 0.0), _r("about", 13.14), _r("gradient-backgrounds", 14.44)]
    assert [r["case_id"] for r in worst_first(results)] == [
        "gradient-backgrounds",
        "about",
        "bg-pure",
    ]


def test_unmeasured_outranks_every_diff():
    """A refusal to measure needs attention before any real diff, however large."""
    results = [_r("about", 99.9), _r("settings", None), _r("bg-pure", 0.0)]
    assert worst_first(results)[0]["case_id"] == "settings"


def test_all_unmeasured_does_not_crash():
    results = [_r("a", None), _r("b", None)]
    assert len(worst_first(results)) == 2


def test_top_three_of_a_full_green_board_are_the_real_worst():
    """Regression pinned to the 2026-07-29 board that exposed the bug."""
    board = [
        _r("bg-pure", 0.00),
        _r("gradients", 1.06),
        _r("bg-solid", 1.61),
        _r("about", 13.14),
        _r("gradient-no-radius", 13.96),
        _r("gradient-backgrounds", 14.44),
    ]
    assert [r["case_id"] for r in worst_first(board)[:3]] == [
        "gradient-backgrounds",
        "gradient-no-radius",
        "about",
    ]


if __name__ == "__main__":
    # CI's guard loop runs `python3 <file>`. Without a runner this file
    # defined its tests, executed none of them, and exited 0 — a PASS line
    # over zero assertions (the #144 livesuite class). Run every module-level
    # test_* function and fail loudly.
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
