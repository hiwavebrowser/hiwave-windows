#!/usr/bin/env python3
"""
parity_gate.py - CI gate for parity threshold enforcement

This script checks that the parity baseline meets minimum requirements.
Exit codes:
  0 = Pass (parity meets threshold)
  1 = Fail (parity below threshold or errors)

Usage:
    python3 scripts/parity_gate.py [--minimum <pct>] [--report <path>]
    python3 scripts/parity_gate.py --mode test_results [--level <commit|pr_merge|nightly|release>] [--max-diff <pct>]
    
Examples:
    python3 scripts/parity_gate.py --minimum 80
    python3 scripts/parity_gate.py --report parity-baseline/baseline_report.json
    python3 scripts/parity_gate.py --minimum 80 --fail-on-regression 2
    python3 scripts/parity_gate.py --mode test_results --level commit
"""

import json
import sys
from pathlib import Path
from typing import Dict, Any, Optional


def load_report(report_path: Path) -> Optional[Dict[str, Any]]:
    """Load a baseline report JSON."""
    if not report_path.exists():
        print(f"Error: Report not found at {report_path}")
        return None
    
    try:
        with open(report_path, encoding="utf-8") as f:
            return json.load(f)
    except json.JSONDecodeError as e:
        print(f"Error: Invalid JSON in {report_path}: {e}")
        return None


def compute_parity(report: Dict[str, Any]) -> float:
    """Compute overall parity percentage from a report."""
    metrics = report.get("metrics", {})
    weighted_mean_diff = metrics.get("tier_b_weighted_mean", 100)
    return 100 - weighted_mean_diff


def check_regressions(current_report: Dict, previous_report: Dict, threshold: float) -> list:
    """Check for case regressions exceeding threshold."""
    regressions = []
    
    current_results = {}
    for r in current_report.get("builtin_results", []) + current_report.get("websuite_results", []):
        current_results[r["case_id"]] = r.get("estimated_diff_pct", 100)
    
    previous_results = {}
    for r in previous_report.get("builtin_results", []) + previous_report.get("websuite_results", []):
        previous_results[r["case_id"]] = r.get("estimated_diff_pct", 100)
    
    for case_id, current_diff in current_results.items():
        previous_diff = previous_results.get(case_id, 100)
        delta = current_diff - previous_diff
        
        if delta > threshold:
            regressions.append({
                "case_id": case_id,
                "previous_diff": previous_diff,
                "current_diff": current_diff,
                "delta": delta,
            })
    
    return regressions


def detect_mode(report: Dict[str, Any]) -> str:
    # Legacy baseline report
    if "metrics" in report and ("builtin_results" in report or "websuite_results" in report):
        return "baseline_report"
    # parity_test.py output
    if "results" in report and isinstance(report.get("results"), list):
        return "test_results"
    return "unknown"


def level_defaults(level: str) -> Dict[str, Any]:
    # Percent units (diff percent).
    if level == "commit":
        return {"max_diff": 1.0, "require_stable": False, "max_variance": 0.10, "regression_budget": 0.0}
    if level == "pr_merge":
        return {"max_diff": 0.5, "require_stable": True, "max_variance": 0.10, "regression_budget": 0.1}
    if level == "nightly":
        return {"max_diff": 0.25, "require_stable": True, "max_variance": 0.10, "regression_budget": 0.0}
    if level == "release":
        return {"max_diff": 0.0, "require_stable": True, "max_variance": 0.10, "regression_budget": 0.0}
    return {"max_diff": 1.0, "require_stable": False, "max_variance": 0.10, "regression_budget": 0.0}


def load_case_gates() -> Dict[str, Dict[str, Any]]:
    """Per-case gate config from cases/registry.json + campaign thresholds.

    Test-fidelity hardening T0 (2026-07-11): a flat --max-diff 25 meant the
    PR gate almost never blocked — a case could be visibly wrong and land.
    Each case now gates at its OWN campaign threshold; cases the campaign
    has not fixed yet carry "known_fail": true in the registry and only the
    hard ceiling applies to them (they may not get WORSE than the ceiling).
    Fixing a case and clearing its known_fail flag ratchets it permanently.
    """
    import sys as _sys

    _sys.path.insert(0, str(Path(__file__).parent))
    from parity_lib import CASE_REGISTRY, GATE_SCOPE_CAPS, get_threshold  # noqa: E402

    gates: Dict[str, Dict[str, Any]] = {}
    for case_id, case in CASE_REGISTRY["cases"].items():
        scope_cap = GATE_SCOPE_CAPS.get(case.get("scope", ""), 15.0)
        gates[case_id] = {
            # T6 tier table: a case gates at the TIGHTER of its category
            # threshold and its scope cap (builtins/micro: t8 in CI).
            "threshold": min(get_threshold(case_id), scope_cap),
            "known_fail": bool(case.get("known_fail", False)),
            # CI-2: a known_fail's ceiling is FROZEN at its value when the
            # flag was set (+1 margin), not a flat band — image-gallery may
            # not drift 16 -> 24 under a 25% blanket, and a flat 15 would
            # red-lock cases currently above it. Fix -> clear flag ->
            # permanent ratchet at the campaign threshold.
            "kf_ceiling": float(case.get("kf_ceiling", 25.0)),
        }
    return gates


def stability_min_runs() -> int:
    """Finish-line condition 3's iteration count, cited from parity_lib.

    Read, not copied — same rule as gate B's `aa_tolerance`. One bar, one
    place, so the producer and the gate cannot drift into disagreeing about
    what "stable across 3 iterations" means.
    """
    import sys as _sys

    _sys.path.insert(0, str(Path(__file__).parent))
    from parity_lib import STABILITY_MIN_RUNS  # noqa: E402

    return int(STABILITY_MIN_RUNS)


def measured_runs(r: Dict[str, Any]) -> int:
    """Iterations of this row that produced a MEASUREMENT. 0 when unknown.

    Three schemas reach this gate and they disagree about the field:
    parity_aggregate emits `measured_runs` (int); the swarm emits
    `iteration_diffs` (list of the diffs that scored); parity_test.py emits
    `pixel_runs` as a LIST of per-run diffs, where the aggregate emits the
    same key as an INT attempt count. The old
    `int(r.get("pixel_runs") or r.get("iterations") or 1)` raised TypeError on
    the list form, so pointing the gate straight at a parity_test report
    crashed instead of grading it.

    `iterations` is the ATTEMPT count and is never consulted: three attempts
    of which two errored is one measurement, and a stability verdict drawn
    from one measurement is exactly the blank row this campaign refuses to
    read as green. Unknown returns 0 — absence of evidence is not evidence of
    stability.
    """
    v = r.get("measured_runs")
    if isinstance(v, int) and not isinstance(v, bool):
        return v
    for key in ("iteration_diffs", "pixel_runs"):
        v = r.get(key)
        if isinstance(v, list):
            return len(v)
    return 0


def primary_viewport_filter(results: list) -> list:
    """Keep only each case's REGISTRY viewport row (CI-1 §C2, 2026-07-11).

    The PR swarm's exploit phase re-runs worst cases at fixed viewports
    with no baselines — those rows read 100% (instrument/no-baseline, not
    paint) and must inform digs, never block merges. A case not in the
    registry is kept as-is (fail-visible, not fail-silent).
    """
    import sys as _sys

    _sys.path.insert(0, str(Path(__file__).parent))
    from parity_lib import CASE_REGISTRY  # noqa: E402

    native = {
        cid: f"{c['width']}x{c['height']}"
        for cid, c in CASE_REGISTRY["cases"].items()
    }
    kept = []
    for r in results:
        cid = r.get("case_id")
        vp = str(r.get("viewport", ""))
        if cid in native and vp and vp != native[cid]:
            continue
        kept.append(r)
    return kept


def gate_test_results(
    report: Dict[str, Any],
    max_diff: float,
    require_stable: bool,
    max_variance: float,
    per_case_thresholds: bool = False,
    primary_viewport_only: bool = False,
) -> Dict[str, Any]:
    failures = []
    results = report.get("results", [])
    # B2 fallback: an aggregate that only carries `cases[]` still gates.
    if not results and report.get("cases"):
        results = [
            {
                "case_id": c.get("case_id"),
                "viewport": c.get("viewport"),
                "diff_pct_median": c.get("diff_pct"),
                "diff_pct": c.get("diff_pct"),
                "passed": c.get("passed"),
                "stable": c.get("stable"),
                # Carried through, or this fallback would report every row as
                # stability_unmeasured no matter how many iterations ran.
                "measured_runs": c.get("measured_runs"),
                "iteration_diffs": c.get("iteration_diffs"),
                "error": None,
            }
            for c in report["cases"]
        ]
    # B3 tripwire: an empty report is a schema/path bug, NEVER a pass.
    # ("PASS: All 0 case(s)" is how a broken pipeline turns green.)
    if not results:
        return {
            "failures": [{"case_id": "<pipeline>", "reason": "empty_report"}],
            "total": 0,
        }
    if primary_viewport_only:
        results = primary_viewport_filter(results)

    case_gates = load_case_gates() if per_case_thresholds else {}

    for r in results:
        case_id = r.get("case_id", "<unknown>")
        err = r.get("error")
        if err:
            failures.append({"case_id": case_id, "reason": "error", "detail": err})
            continue

        diff = r.get("diff_pct_median", r.get("diff_pct", 100.0))
        variance = r.get("diff_pct_variance", None)
        stable = r.get("stable", None)

        if diff is None:
            failures.append({"case_id": case_id, "reason": "missing_diff"})
            continue

        # Per-case gate: campaign threshold for fixed cases, ceiling-only
        # for registry-declared known_fail cases.
        if per_case_thresholds and case_id in case_gates:
            g = case_gates[case_id]
            limit = (
                min(g["kf_ceiling"], max_diff)
                if g["known_fail"]
                else min(g["threshold"], max_diff)
            )
            if float(diff) > limit:
                failures.append({
                    "case_id": case_id,
                    "reason": "diff",
                    "diff": float(diff),
                    "max_diff": limit,
                    "known_fail": g["known_fail"],
                })
                continue
        elif float(diff) > max_diff:
            failures.append({"case_id": case_id, "reason": "diff", "diff": float(diff), "max_diff": max_diff})
            continue

        if require_stable:
            # P0a (2026-08-07): stability had never gated anything at
            # pr_merge. The old rule held only rows with >= 2 runs to the bar
            # and waived the rest — and the PR/nightly scout phase runs each
            # native case ONCE, so after --primary-viewport-only every
            # surviving row was a single-run row and every one was waived.
            # The waiver was written to avoid red-locking PRs, and it worked:
            # it made the check unreachable instead.
            #
            # The bar is now evidence, not exemption. A row that cannot show
            # STABILITY_MIN_RUNS measurements FAILS as `stability_unmeasured`
            # — a distinct reason from `unstable`, because "we looked three
            # times and it moved" and "we looked once" are different facts and
            # a gate that reports them identically teaches nobody anything.
            # This is the baseline file's rule for blank instrument rows
            # applied to the gate's own output: a row with no instrument
            # reads NOT MEASURED, never green.
            #
            # The producers were changed in the same commit to supply the
            # evidence (`--iterations 3` on the PR and nightly scout phases in
            # .github/workflows/parity.yml). Tightening the gate without that
            # would red-lock every PR, which is the trap the original waiver
            # was dodging; the fix is to run the iterations, not to stop
            # asking for them. `commit` level does not require stability and
            # its lane still runs once.
            min_runs = stability_min_runs()
            runs = measured_runs(r)
            if runs < min_runs:
                failures.append({
                    "case_id": case_id,
                    "reason": "stability_unmeasured",
                    "measured_runs": runs,
                    "required_runs": min_runs,
                })
                continue
            if stable is not True:
                failures.append({"case_id": case_id, "reason": "unstable", "variance": variance, "max_variance": max_variance})
                continue
            if variance is not None and float(variance) > max_variance:
                failures.append({"case_id": case_id, "reason": "variance", "variance": float(variance), "max_variance": max_variance})

    return {"failures": failures, "total": len(results)}


def regressions_test_results(current: Dict[str, Any], previous: Dict[str, Any], budget: float) -> list:
    # A case the instrument refused to measure has diff None. Defaulting it to
    # 100.0 here would manufacture a regression when a run merely failed to
    # capture, and — worse — manufacture an IMPROVEMENT on the next run when
    # the instrument recovered. Comparing a measurement against a
    # non-measurement is meaningless in both directions, so unmeasured cases
    # are skipped rather than defaulted.
    def _diff(r):
        return r.get("diff_pct_median", r.get("diff_pct"))

    current_map = {r.get("case_id"): _diff(r) for r in current.get("results", [])}
    prev_map = {r.get("case_id"): _diff(r) for r in previous.get("results", [])}
    regressions = []
    for case_id, cur in current_map.items():
        if case_id is None or cur is None:
            continue
        prev = prev_map.get(case_id, None)
        if prev is None:
            continue
        delta = float(cur) - float(prev)
        if delta > budget:
            regressions.append({"case_id": case_id, "previous_diff": float(prev), "current_diff": float(cur), "delta": delta})
    return regressions


def main():
    # Default values
    minimum_parity = 80.0
    report_path = Path("parity-baseline/baseline_report.json")
    previous_path = None
    regression_threshold = None
    mode = None
    level = None
    max_diff = None
    per_case_thresholds = False
    primary_viewport_only = False
    require_stable = None
    max_variance = None
    verbose = False
    
    # Parse arguments
    args = sys.argv[1:]
    i = 0
    while i < len(args):
        if args[i] == "--minimum" and i + 1 < len(args):
            minimum_parity = float(args[i + 1])
            i += 2
        elif args[i] == "--report" and i + 1 < len(args):
            report_path = Path(args[i + 1])
            i += 2
        elif args[i] == "--mode" and i + 1 < len(args):
            mode = args[i + 1]
            i += 2
        elif args[i] == "--level" and i + 1 < len(args):
            level = args[i + 1]
            i += 2
        elif args[i] == "--max-diff" and i + 1 < len(args):
            max_diff = float(args[i + 1])
            i += 2
        elif args[i] == "--per-case-thresholds":
            per_case_thresholds = True
            i += 1
        elif args[i] == "--primary-viewport-only":
            primary_viewport_only = True
            i += 1
        elif args[i] == "--require-stable":
            require_stable = True
            i += 1
        elif args[i] == "--max-variance" and i + 1 < len(args):
            max_variance = float(args[i + 1])
            i += 2
        elif args[i] == "--previous" and i + 1 < len(args):
            previous_path = Path(args[i + 1])
            i += 2
        elif args[i] == "--fail-on-regression" and i + 1 < len(args):
            regression_threshold = float(args[i + 1])
            i += 2
        elif args[i] in ["-v", "--verbose"]:
            verbose = True
            i += 1
        elif args[i] in ["-h", "--help"]:
            print(__doc__)
            sys.exit(0)
        else:
            i += 1
    
    print("=" * 60)
    print("Parity Gate Check")
    print("=" * 60)
    print(f"\nReport: {report_path}")
    
    # Load current report
    report = load_report(report_path)
    if not report:
        sys.exit(1)

    detected = detect_mode(report)
    if mode is None:
        mode = detected

    if mode == "test_results":
        defaults = level_defaults(level or "commit")
        if max_diff is None:
            max_diff = defaults["max_diff"]
        if require_stable is None:
            require_stable = defaults["require_stable"]
        if max_variance is None:
            max_variance = defaults["max_variance"]
        if regression_threshold is None and previous_path:
            regression_threshold = defaults["regression_budget"]

        print(f"Mode: test_results")
        if level:
            print(f"Level: {level}")
        if per_case_thresholds:
            print(f"Per-case thresholds: ON (registry known_fail cases gate at ceiling {max_diff}%)")
        print(f"Max diff: {max_diff}%")
        print(f"Require stable: {bool(require_stable)}")
        if require_stable:
            print(f"Max variance: {max_variance}%")
        if regression_threshold is not None and previous_path:
            print(f"Regression budget: {regression_threshold}%")

        gate = gate_test_results(
            report,
            max_diff=max_diff,
            require_stable=bool(require_stable),
            max_variance=max_variance,
            per_case_thresholds=per_case_thresholds,
            primary_viewport_only=primary_viewport_only,
        )
        failures = gate["failures"]

        regressions = []
        if previous_path and regression_threshold is not None:
            prev = load_report(previous_path)
            if prev:
                regressions = regressions_test_results(report, prev, budget=float(regression_threshold))

        if failures:
            print(f"\n✗ FAIL: {len(failures)}/{gate['total']} case(s) violated the gate:")
            for f in failures[:25]:
                print(f"  - {f['case_id']}: {f['reason']}" + (f" ({f})" if verbose else ""))
        else:
            print(f"\n✓ PASS: All {gate['total']} case(s) within max diff {max_diff}%")

        if regressions:
            print(f"\n✗ FAIL: {len(regressions)} regression(s) exceeded budget {regression_threshold}%:")
            for r in sorted(regressions, key=lambda x: -x["delta"])[:25]:
                print(f"  - {r['case_id']}: {r['previous_diff']:.2f}% -> {r['current_diff']:.2f}% (+{r['delta']:.2f}%)")
        elif previous_path and regression_threshold is not None:
            print(f"\n✓ PASS: No regressions exceeding budget {regression_threshold}%")

        print("\n" + "=" * 60)
        if not failures and not regressions:
            print("GATE: PASSED")
            sys.exit(0)
        print("GATE: FAILED")
        sys.exit(1)
    
    # Compute parity
    parity = compute_parity(report)
    metrics = report.get("metrics", {})
    
    print(f"\n--- Results ---")
    print(f"Current Parity: {parity:.1f}%")
    print(f"Tier A Pass Rate: {metrics.get('tier_a_pass_rate', 0) * 100:.1f}%")
    print(f"Weighted Mean Diff: {metrics.get('tier_b_weighted_mean', 100):.1f}%")
    
    # Check minimum parity
    parity_passed = parity >= minimum_parity
    
    if parity_passed:
        print(f"\n✓ PASS: Parity {parity:.1f}% >= {minimum_parity}% minimum")
    else:
        print(f"\n✗ FAIL: Parity {parity:.1f}% < {minimum_parity}% minimum")
    
    # Check regressions if previous report provided
    regressions = []
    if regression_threshold and previous_path:
        previous_report = load_report(previous_path)
        if previous_report:
            regressions = check_regressions(report, previous_report, regression_threshold)
            
            if regressions:
                print(f"\n✗ FAIL: {len(regressions)} case(s) regressed by >{regression_threshold}%:")
                for r in sorted(regressions, key=lambda x: -x["delta"]):
                    print(f"  - {r['case_id']}: {r['previous_diff']:.1f}% -> {r['current_diff']:.1f}% (+{r['delta']:.1f}%)")
            else:
                print(f"\n✓ PASS: No regressions exceeding {regression_threshold}%")
    
    # Verbose output
    if verbose:
        print("\n--- Per-Case Results ---")
        all_results = report.get("builtin_results", []) + report.get("websuite_results", [])
        for r in sorted(all_results, key=lambda x: x.get("estimated_diff_pct", 100), reverse=True):
            case_id = r["case_id"]
            diff = r.get("estimated_diff_pct", 100)
            status = "✓" if diff <= 25 else "✗"
            source = r.get("diff_source", "heuristic")
            print(f"  {status} {case_id}: {diff:.1f}% ({source})")
    
    # Final verdict
    print("\n" + "=" * 60)
    
    if parity_passed and not regressions:
        print("GATE: PASSED")
        sys.exit(0)
    else:
        print("GATE: FAILED")
        sys.exit(1)


if __name__ == "__main__":
    main()



