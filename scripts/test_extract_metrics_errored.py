"""Regression check: errored parity cases never pass, and never carry a score.

Guards the CI/local discrepancy fixed in trench night-2: parity_test.py
scored errored captures as 100 while extract_parity_metrics.py crashed on
pixel=None (or would have defaulted a missing diffPercent to 0.0 — a pass).

CONTRACT CHANGE 2026-07-29: the guard is unchanged — an errored case must
never pass and must never read as 0.0 — but the representation is now
three-state. An errored case reports diff=None and not_measured=True instead
of diff=100.0. A capture timeout is an ABSENCE of measurement, not a 100%
render diff, and averaging it in as one is what made the nightly gate report
73.36 for a tree measuring 6.75. The `== 100.0` assertions below were the old
mechanism of this guard, not its intent; they are replaced by assertions on
the intent itself.
Run: python3 scripts/test_extract_metrics_errored.py
"""
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from extract_parity_metrics import extract_metrics

# An errored case exactly as run_test() emits it (pixel=None, error set)
errored = {
    "case_id": "settings",
    "type": "builtins",
    "threshold": 15,
    "pixel": None,
    "styles": None,
    "rects": None,
    "passed": False,
    "error": "Capture failed: Timeout",
    "diff_pct": 100.0,
}
# A legacy errored case from before this fix (no diff_pct at all)
legacy_errored = dict(errored, case_id="legacy", diff_pct=None)
del legacy_errored["diff_pct"]
# A healthy passing case
healthy = {
    "case_id": "about",
    "type": "builtins",
    "threshold": 15,
    "pixel": {"diffPercent": 3.2},
    "passed": True,
}

m = extract_metrics({"results": [errored, legacy_errored, healthy]})
by_name = {t["name"]: t for t in m["tests"]}

# The intent: an errored case NEVER passes and NEVER reads as a low diff.
assert by_name["settings"]["passed"] is False
assert by_name["settings"]["diff"] != 0.0, "an errored case must never read as a pass"
assert by_name["settings"]["not_measured"] is True
assert by_name["settings"]["diff"] is None, by_name["settings"]
assert by_name["settings"]["error"] == "Capture failed: Timeout"

# Same for the legacy shape that carried no diff_pct at all.
assert by_name["legacy"]["passed"] is False
assert by_name["legacy"]["not_measured"] is True
assert by_name["legacy"]["diff"] is None, by_name["legacy"]

# A healthy case is untouched by any of this.
assert by_name["about"]["diff"] == 3.2
assert by_name["about"]["passed"] is True
assert by_name["about"]["not_measured"] is False

assert m["passed"] == 1 and m["failed"] == 2
assert m["measured"] == 1 and m["not_measured"] == 2
# The average and worst case describe the RENDERER, so they see only the
# one case that was actually measured.
assert m["average_diff"] == 3.2, m["average_diff"]
assert m["worst_case"]["diff"] == 3.2

print("OK: errored cases are refusals (diff=None, never pass); healthy case unaffected")
print(json.dumps(m["worst_case"]))
