"""A report that doesn't name its engine can't warn you it's the wrong one.

E0a guard. The nightly regression compare ran cross-engine for a week
(Aug-3 fossil baseline vs post-#110 master) and nothing in either JSON could
say so. These tests pin the two halves of the fix: aggregation stamps
provenance when asked, and compare_reports surfaces both sides and flags an
engine mismatch — without changing pass/fail (advisory, per the E0 posture
that the ratchet is the blocking layer).
"""

import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from parity_aggregate import compare_reports, stamp_provenance  # noqa: E402


def _report(cases=(), provenance=None):
    r = {"cases": list(cases), "taxonomy": {"buckets": []}}
    if provenance:
        r["provenance"] = provenance
    return r


def _case(case_id="bg-pure", diff=1.0, passed=True):
    return {"case_id": case_id, "viewport": "1280x800", "diff_pct": diff,
            "passed": passed}


class StampTests(unittest.TestCase):
    def test_stamp_writes_both_fields(self):
        r = stamp_provenance({}, "abc123", "31624231006")
        self.assertEqual(r["provenance"],
                         {"engine_sha": "abc123", "receipt_run": "31624231006"})

    def test_no_args_means_no_provenance_key(self):
        # Pre-E0a callers keep producing byte-compatible reports.
        self.assertNotIn("provenance", stamp_provenance({}, None, None))


class CompareCarryTests(unittest.TestCase):
    def test_both_sides_carried_and_mismatch_flagged(self):
        cmp = compare_reports(
            _report([_case()], {"engine_sha": "old", "receipt_run": "1"}),
            _report([_case()], {"engine_sha": "new", "receipt_run": "2"}),
        )
        self.assertEqual(cmp["baseline_provenance"]["engine_sha"], "old")
        self.assertEqual(cmp["current_provenance"]["engine_sha"], "new")
        self.assertTrue(cmp["cross_engine"])

    def test_same_engine_not_flagged(self):
        cmp = compare_reports(
            _report([_case()], {"engine_sha": "same", "receipt_run": "1"}),
            _report([_case()], {"engine_sha": "same", "receipt_run": "2"}),
        )
        self.assertFalse(cmp["cross_engine"])

    def test_unstamped_baseline_is_not_cross_engine(self):
        # A pre-E0a artifact has no provenance; absence of evidence must not
        # read as a mismatch.
        cmp = compare_reports(
            _report([_case()]),
            _report([_case()], {"engine_sha": "new", "receipt_run": "2"}),
        )
        self.assertIsNone(cmp["baseline_provenance"])
        self.assertFalse(cmp["cross_engine"])

    def test_cross_engine_does_not_change_pass(self):
        # Advisory: the flag must not flip summary.pass on its own.
        cmp = compare_reports(
            _report([_case(diff=1.0)], {"engine_sha": "a", "receipt_run": "1"}),
            _report([_case(diff=1.0)], {"engine_sha": "b", "receipt_run": "2"}),
        )
        self.assertTrue(cmp["summary"]["pass"])


if __name__ == "__main__":
    # CI's guard loop invokes `python3 <file>`; without this the file is a
    # silent no-op that reads as PASS (the #144 livesuite failure class).
    raise SystemExit(unittest.main(verbosity=2))
