"""An instrument failure must never be published as a render score.

This is the defect that made the nightly Parity Gate decorative: the pixel
oracle already refuses to score a dimension mismatch and reports
`instrumentFailure`, but until 2026-07-29 no consumer read that field. A
capture the instrument declined to measure was recorded as a 100.0 render
diff with error=null and averaged into the campaign number — which is how a
tree measuring 6.75 was reported at 73.36 and the gate went permanently red.

Run: python3 scripts/tests/test_instrument_not_a_score.py
"""
import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))
from extract_parity_metrics import extract_metrics  # noqa: E402


def test_instrument_failure_is_not_averaged_as_a_diff():
    """The sharp case: old code runs to completion and reports a LIE.

    Two cases, one healthy at 6.75 and one the instrument refused. The honest
    average is 6.75 over one measured case. Scoring the refusal as 100.0
    yields 53.375 — a number describing the harness, not the renderer.
    """
    results = {"results": [
        {"case_id": "healthy", "type": "builtin", "threshold": 15,
         "pixel": {"diffPercent": 6.75}},
        {"case_id": "wrong_viewport", "type": "builtin", "threshold": 15,
         "pixel": {"diffPercent": 100,
                   "instrumentFailure": "dimension_mismatch: Chrome 800x600 vs RustKit 1920x1080"}},
    ]}
    m = extract_metrics(results, "abc123", "master")

    assert m["average_diff"] == 6.75, (
        f"average must cover MEASURED cases only; got {m['average_diff']} "
        "(53.375 means the instrument failure was averaged in as a render diff)"
    )
    assert m["measured"] == 1 and m["not_measured"] == 1
    by = {t["name"]: t for t in m["tests"]}
    assert by["wrong_viewport"]["diff"] is None, "a refused capture must carry no score"
    assert by["wrong_viewport"]["not_measured"] is True
    assert by["wrong_viewport"]["passed"] is False, "not-measured must never pass"
    assert m["worst_case"]["name"] == "healthy", "worst case must come from measured cases"


def test_not_measured_sorts_first():
    """Unmeasured cases need attention most, so they lead the report."""
    results = {"results": [
        {"case_id": "ok", "type": "builtin", "threshold": 15, "pixel": {"diffPercent": 1.0}},
        {"case_id": "refused", "type": "builtin", "threshold": 15,
         "pixel": {"diffPercent": 100, "instrumentFailure": "dimension_mismatch: x"}},
    ]}
    m = extract_metrics(results, "abc", "master")
    assert m["tests"][0]["name"] == "refused"


def test_blank_frame_carries_no_score():
    """A blank frame is also a refusal, and arrives with diff_pct=None."""
    results = {"results": [
        {"case_id": "blank", "type": "builtin", "threshold": 15,
         "pixel": None, "diff_pct": None, "error": "BLANK_FRAME: 100.0% background"},
    ]}
    m = extract_metrics(results, "abc", "master")
    assert m["tests"][0]["not_measured"] is True
    assert m["tests"][0]["diff"] is None
    assert m["average_diff"] is None, "no measured cases means no average, not 0.0"


if __name__ == "__main__":
    for name, fn in sorted(globals().items()):
        if name.startswith("test_") and callable(fn):
            fn()
            print(f"ok  {name}")
    print("PASS: instrument failures are refusals, not scores")
