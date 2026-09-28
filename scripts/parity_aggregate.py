#!/usr/bin/env python3
"""
parity_aggregate.py - Aggregate parity results and attribution data

This script:
1. Ingests swarm_report.json or individual attribution.json files
2. Produces global aggregate_report.json with:
   - Top selectors by diff contribution (global fix scoreboard)
   - Top taxonomy buckets
   - Per-case summaries with artifact links
   - Projected gain from fixing top N contributors
3. Compares two reports for regression detection

Usage:
    # Aggregate from swarm run
    python3 scripts/parity_aggregate.py --run-id <id>

    # Aggregate from multiple shards
    python3 scripts/parity_aggregate.py --runs <id1>,<id2>,<id3>

    # Compare for regressions
    python3 scripts/parity_aggregate.py --compare --baseline <old> --current <new>

    # Aggregate raw attribution files
    python3 scripts/parity_aggregate.py --attribution-dir <path>
"""

import argparse
import json
import sys
from collections import defaultdict
from dataclasses import dataclass, asdict, field
from datetime import datetime
from pathlib import Path
from typing import Dict, List, Any, Optional, Tuple

REPO_ROOT = Path(__file__).parent.parent
DEFAULT_RESULTS_ROOT = REPO_ROOT / "parity-results"


# ============================================================================
# Data structures
# ============================================================================

@dataclass
class ContributorStats:
    """Aggregated stats for a single selector across all cases."""
    selector: str
    tag: Optional[str] = None
    total_diff_pixels: int = 0
    total_contribution_pct: float = 0.0
    case_count: int = 0
    cases: List[str] = field(default_factory=list)
    likely_cause: Optional[str] = None
    avg_corner_ratio: float = 0.0


@dataclass
class TaxonomyStats:
    """Aggregated stats for a taxonomy bucket."""
    bucket: str
    total_contribution_pct: float = 0.0
    total_diff_pixels: int = 0
    case_count: int = 0
    top_selectors: List[str] = field(default_factory=list)


@dataclass
class CaseSummary:
    """Summary for a single case."""
    case_id: str
    viewport: str
    # None = the instrument refused to measure this cell. Distinct from 100.0,
    # which means it measured a total mismatch.
    diff_pct: Optional[float]
    passed: bool
    stable: bool
    threshold: float
    # ATTEMPTED iterations.
    pixel_runs: int = 1
    # MEASURED iterations — the ones that produced a diff. Distinct from
    # pixel_runs on purpose: three attempts of which two errored is one
    # measurement, and only measurements can support a stability verdict.
    # None means the producer did not say, which parity_gate treats as no
    # evidence rather than as enough.
    measured_runs: Optional[int] = None
    overlay_path: Optional[str] = None
    attribution_path: Optional[str] = None
    top_contributors: List[Dict] = field(default_factory=list)
    taxonomy: Dict[str, float] = field(default_factory=dict)
    error: Optional[str] = None


def _fmt_pct(value) -> str:
    """None means nothing was measured — never print it as a number."""
    return "NOT-MEASURED" if value is None else f"{value:.2f}%"


def _mean_or_none(values):
    """Average, or None when there is nothing to average.

    Returning 0.0 for an empty set claims perfect parity from zero evidence.
    """
    return (sum(values) / len(values)) if values else None


def _measured_runs(r: Dict) -> Optional[int]:
    """How many iterations of this row produced a MEASUREMENT.

    The swarm publishes `iteration_diffs` (one entry per iteration that
    actually scored); parity_test.py publishes `measured_runs` directly, and
    older rows put the per-run diff list in `pixel_runs`. `iterations` is the
    ATTEMPT count and is deliberately not consulted here — it is what let a
    row with two errored captures claim three runs' worth of stability
    evidence. None when nothing in the row says.
    """
    v = r.get("measured_runs")
    if isinstance(v, int) and not isinstance(v, bool):
        return v
    for key in ("iteration_diffs", "pixel_runs"):
        v = r.get(key)
        if isinstance(v, list):
            return len(v)
    return None


def _worst_first(c: "CaseSummary"):
    """Sort key: unmeasured cells first, then worst measured diff.

    Unmeasured leads because a cell nobody measured needs attention before any
    number does, and because -None is a TypeError.
    """
    return (c.diff_pct is not None, -(c.diff_pct or 0.0))


# ============================================================================
# Aggregation logic
# ============================================================================

def load_swarm_report(run_id: str, results_root: Path = DEFAULT_RESULTS_ROOT) -> Optional[Dict]:
    """Load swarm_report.json for a run."""
    report_path = results_root / run_id / "swarm_report.json"
    if not report_path.exists():
        print(f"Warning: No swarm report at {report_path}")
        return None
    
    with open(report_path, encoding="utf-8") as f:
        return json.load(f)


def load_attribution(path: Path) -> Optional[Dict]:
    """Load a single attribution.json file."""
    if not path.exists():
        return None
    try:
        with open(path, encoding="utf-8") as f:
            return json.load(f)
    except Exception as e:
        print(f"Warning: Failed to load {path}: {e}")
        return None


def find_attribution_files(run_dir: Path) -> List[Path]:
    """Find all attribution.json files under a run directory."""
    return list(run_dir.rglob("attribution.json"))


def aggregate_from_swarm_reports(reports: List[Dict]) -> Dict[str, Any]:
    """
    Aggregate multiple swarm reports into a single global report.
    
    Used for merging shard outputs.
    """
    all_results: List[Dict] = []
    all_raw_scout: List[Dict] = []
    all_raw_exploit: List[Dict] = []
    
    for report in reports:
        all_results.extend(report.get("results", []))
        raw = report.get("raw_results", {})
        all_raw_scout.extend(raw.get("scout", []))
        all_raw_exploit.extend(raw.get("exploit", []))
    
    # Deduplicate and merge by (case_id, viewport)
    merged: Dict[Tuple[str, str], Dict] = {}
    for r in all_results:
        key = (r["case_id"], r["viewport"])
        if key not in merged:
            merged[key] = r
        else:
            # Keep the one with more iterations or better stats
            existing = merged[key]
            if r.get("iterations", 0) > existing.get("iterations", 0):
                merged[key] = r
    
    return aggregate_from_results(list(merged.values()))


def aggregate_from_results(results: List[Dict]) -> Dict[str, Any]:
    """
    Aggregate from a list of per-case result dicts.
    
    Produces:
    - Global top selectors (fix scoreboard)
    - Global taxonomy
    - Projected gains
    - Case summaries
    """
    # Global selector stats
    selector_stats: Dict[str, ContributorStats] = {}
    
    # Global taxonomy
    taxonomy_totals: Dict[str, TaxonomyStats] = {}
    
    # Case summaries
    case_summaries: List[CaseSummary] = []
    
    # Track total diff pixels across all cases for normalization
    total_global_diff_pixels = 0
    
    for r in results:
        case_id = r.get("case_id", "")
        viewport = r.get("viewport", "")
        # Preserve None. Defaulting a refusal to 100 here is how an
        # instrument failure became a render score in the first place.
        diff_pct = r.get("diff_pct_median")
        if diff_pct is None:
            diff_pct = r.get("diff_pct")
        
        summary = CaseSummary(
            case_id=case_id,
            viewport=viewport,
            diff_pct=diff_pct,
            passed=r.get("passed", False),
            stable=r.get("stable", False),
            threshold=r.get("threshold", 15),
            error=r.get("error"),
            pixel_runs=int(r.get("iterations") or r.get("pixel_runs") or 1),
            measured_runs=_measured_runs(r),
            overlay_path=r.get("best_overlay_path"),
            attribution_path=r.get("best_attribution_path"),
        )
        
        # Process top contributors
        contributors = r.get("best_top_contributors") or r.get("top_contributors") or []
        summary.top_contributors = contributors[:5]
        
        for c in contributors:
            selector = c.get("selector", "")
            if not selector:
                continue
            
            diff_pixels = c.get("diff_pixels", 0)
            contrib_pct = c.get("contribution_percent", 0)
            likely_cause = c.get("likely_cause")
            corner_ratio = c.get("corner_ratio", 0)
            
            total_global_diff_pixels += diff_pixels
            
            if selector not in selector_stats:
                selector_stats[selector] = ContributorStats(
                    selector=selector,
                    tag=c.get("tag"),
                    likely_cause=likely_cause,
                )
            
            stats = selector_stats[selector]
            stats.total_diff_pixels += diff_pixels
            stats.total_contribution_pct += contrib_pct
            stats.case_count += 1
            if case_id not in stats.cases:
                stats.cases.append(case_id)
            if likely_cause and not stats.likely_cause:
                stats.likely_cause = likely_cause
            # Running average of corner ratio
            prev_total = stats.avg_corner_ratio * (stats.case_count - 1)
            stats.avg_corner_ratio = (prev_total + corner_ratio) / stats.case_count
        
        # Process taxonomy
        taxonomy = r.get("best_taxonomy") or r.get("taxonomy") or {}
        summary.taxonomy = taxonomy
        
        for bucket, pct in taxonomy.items():
            if bucket not in taxonomy_totals:
                taxonomy_totals[bucket] = TaxonomyStats(bucket=bucket)
            
            tax = taxonomy_totals[bucket]
            tax.total_contribution_pct += pct
            tax.case_count += 1
        
        case_summaries.append(summary)
    
    # Sort selectors by total diff pixels
    sorted_selectors = sorted(
        selector_stats.values(),
        key=lambda s: -s.total_diff_pixels
    )
    
    # Compute projected gains
    cumulative_gain = 0.0
    projected_gains: Dict[str, float] = {}
    for i, s in enumerate(sorted_selectors[:20]):
        if total_global_diff_pixels > 0:
            pct = (s.total_diff_pixels / total_global_diff_pixels) * 100
            cumulative_gain += pct
        projected_gains[f"top_{i+1}"] = cumulative_gain
    
    # Link top selectors to taxonomy buckets
    for bucket_name, tax in taxonomy_totals.items():
        tax.top_selectors = [
            s.selector for s in sorted_selectors[:50]
            if s.likely_cause == bucket_name
        ][:5]
    
    # Sort taxonomy by contribution
    sorted_taxonomy = sorted(
        taxonomy_totals.values(),
        key=lambda t: -t.total_contribution_pct
    )
    
    # Build final report
    return {
        "timestamp": datetime.now().isoformat(),
        "summary": {
            "total_cases": len(case_summaries),
            "passed": sum(1 for c in case_summaries if c.passed),
            "failed": sum(1 for c in case_summaries if not c.passed),
            "stable": sum(1 for c in case_summaries if c.stable),
            # Measured cells only — see CaseSummary.diff_pct.
            # 65-B (Prometheus): max(1, 0) turned "nothing was measured" into
            # 0.0 — which reads as PERFECT PARITY and is a worse lie than the
            # 100.0 this whole change set exists to remove. It also disagreed
            # with extract_parity_metrics, which correctly returns None. No
            # measurements means no average.
            "avg_diff_pct": _mean_or_none([
                c.diff_pct for c in case_summaries if c.diff_pct is not None
            ]),
            "measured_cases": sum(1 for c in case_summaries if c.diff_pct is not None),
            "not_measured_cases": sum(1 for c in case_summaries if c.diff_pct is None),
            "total_global_diff_pixels": total_global_diff_pixels,
        },
        "fix_scoreboard": {
            "description": "Top selectors by diff pixel contribution. Fixing these has highest impact.",
            "top_contributors": [
                {
                    "rank": i + 1,
                    "selector": s.selector,
                    "tag": s.tag,
                    "total_diff_pixels": s.total_diff_pixels,
                    "contribution_pct": (s.total_diff_pixels / max(1, total_global_diff_pixels)) * 100,
                    "case_count": s.case_count,
                    "cases": s.cases[:5],
                    "likely_cause": s.likely_cause,
                    "corner_ratio": s.avg_corner_ratio,
                }
                for i, s in enumerate(sorted_selectors[:20])
            ],
            "projected_gains": projected_gains,
        },
        "taxonomy": {
            "description": "Diff contribution by root cause category.",
            "buckets": [
                {
                    "bucket": t.bucket,
                    "total_contribution_pct": t.total_contribution_pct,
                    "case_count": t.case_count,
                    "top_selectors": t.top_selectors,
                }
                for t in sorted_taxonomy
            ],
        },
        "cases": [
            {
                "case_id": c.case_id,
                "viewport": c.viewport,
                "diff_pct": c.diff_pct,
                "passed": c.passed,
                "stable": c.stable,
                "threshold": c.threshold,
                "measured_runs": c.measured_runs,
                "overlay_path": c.overlay_path,
                "attribution_path": c.attribution_path,
                "top_contributors": c.top_contributors[:3],
                "taxonomy": c.taxonomy,
            }
            for c in sorted(case_summaries, key=_worst_first)
        ],
        # CI-1 schema alias (2026-07-11): parity_gate reads `results[]` with
        # `diff_pct_median`. Without this alias, a re-homed aggregate passed
        # the gate on "All 0 case(s)" — decorative red would have become
        # decorative GREEN. `cases[]` above stays for humans/scoreboards.
        "results": [
            {
                "case_id": c.case_id,
                "viewport": c.viewport,
                "diff_pct_median": c.diff_pct,
                "diff_pct": c.diff_pct,
                "passed": c.passed,
                "stable": c.stable,
                "threshold": c.threshold,
                "pixel_runs": c.pixel_runs,
                # Carried so parity_gate can hold a row to the stability bar
                # on the evidence that exists, not on the attempt count.
                "measured_runs": c.measured_runs,
                # Was hardcoded None. The aggregate was ERASING shard errors
                # before parity_gate could see them, so a gate that correctly
                # fails on `error` never got one to fail on.
                "error": c.error,
            }
            for c in sorted(case_summaries, key=_worst_first)
        ],
    }


def aggregate_from_attribution_files(files: List[Path]) -> Dict[str, Any]:
    """
    Aggregate directly from attribution.json files.
    
    Used when swarm_report.json is not available.
    """
    results = []
    
    for f in files:
        attr = load_attribution(f)
        if not attr:
            continue
        
        # Extract case info from path: .../case_id/viewport/iter-N/diff/attribution.json
        parts = f.parts
        try:
            diff_idx = parts.index("diff")
            iter_part = parts[diff_idx - 1]  # iter-N
            viewport = parts[diff_idx - 2]
            case_id = parts[diff_idx - 3]
            
            results.append({
                "case_id": case_id,
                "viewport": viewport,
                "diff_pct": attr.get("diffPercent", 100),
                "passed": attr.get("diffPercent", 100) < 15,
                "stable": False,
                "threshold": 15,
                "top_contributors": attr.get("topContributors", []),
                "taxonomy": attr.get("taxonomy", {}),
            })
        except (ValueError, IndexError):
            print(f"Warning: Could not parse path structure for {f}")
            continue
    
    return aggregate_from_results(results)


# ============================================================================
# Provenance (E0a)
# ============================================================================

def stamp_provenance(report: Dict[str, Any], engine_sha: Optional[str],
                     receipt_run: Optional[str]) -> Dict[str, Any]:
    """Record which engine produced a report.

    E0a: nightly regression comparisons ran cross-engine for a week without
    anyone being able to tell from the JSON (the Aug-3 fossil vs post-#110
    master). A report that names its engine makes that class of comparison
    visible instead of silent.
    """
    if engine_sha or receipt_run:
        report["provenance"] = {
            "engine_sha": engine_sha,
            "receipt_run": receipt_run,
        }
    return report


# ============================================================================
# Regression detection
# ============================================================================

def compare_reports(
    baseline: Dict[str, Any],
    current: Dict[str, Any],
    regression_budget: float = 0.1,
) -> Dict[str, Any]:
    """
    Compare two aggregate reports and detect regressions.
    
    Returns:
    - Per-case regressions (diff increased beyond budget)
    - Taxonomy shifts
    - New failures
    """
    baseline_cases = {(c["case_id"], c["viewport"]): c for c in baseline.get("cases", [])}
    current_cases = {(c["case_id"], c["viewport"]): c for c in current.get("cases", [])}
    
    regressions = []
    improvements = []
    new_failures = []
    not_measured = []
    
    for key, cur in current_cases.items():
        base = baseline_cases.get(key)
        
        if not base:
            # New case
            if not cur["passed"]:
                new_failures.append({
                    "case_id": cur["case_id"],
                    "viewport": cur["viewport"],
                    "diff_pct": cur["diff_pct"],
                    "type": "new_failure",
                })
            continue
        
        # 65-A (Prometheus): CaseSummary.diff_pct is Optional since the
        # three-state change, so either side of this subtraction can be None.
        # No delta exists between a measurement and a non-measurement — and
        # inventing one manufactures a regression when a capture fails, then
        # an improvement when it recovers. Report it as unmeasured instead.
        if cur["diff_pct"] is None or base["diff_pct"] is None:
            not_measured.append({
                "case_id": cur["case_id"],
                "viewport": cur["viewport"],
                "baseline_diff": base["diff_pct"],
                "current_diff": cur["diff_pct"],
                "type": "not_measured",
            })
            continue

        delta = cur["diff_pct"] - base["diff_pct"]
        
        if delta > regression_budget:
            regressions.append({
                "case_id": cur["case_id"],
                "viewport": cur["viewport"],
                "baseline_diff": base["diff_pct"],
                "current_diff": cur["diff_pct"],
                "delta": delta,
                "type": "regression",
            })
        elif delta < -regression_budget:
            improvements.append({
                "case_id": cur["case_id"],
                "viewport": cur["viewport"],
                "baseline_diff": base["diff_pct"],
                "current_diff": cur["diff_pct"],
                "delta": delta,
                "type": "improvement",
            })
    
    # Taxonomy shifts
    baseline_tax = {t["bucket"]: t["total_contribution_pct"] for t in baseline.get("taxonomy", {}).get("buckets", [])}
    current_tax = {t["bucket"]: t["total_contribution_pct"] for t in current.get("taxonomy", {}).get("buckets", [])}
    
    taxonomy_shifts = []
    for bucket in set(baseline_tax.keys()) | set(current_tax.keys()):
        base_pct = baseline_tax.get(bucket, 0)
        cur_pct = current_tax.get(bucket, 0)
        delta = cur_pct - base_pct
        if abs(delta) > 5:  # Significant shift
            taxonomy_shifts.append({
                "bucket": bucket,
                "baseline_pct": base_pct,
                "current_pct": cur_pct,
                "delta": delta,
            })
    
    # Summary
    total_regression = sum(r["delta"] for r in regressions)
    total_improvement = sum(abs(i["delta"]) for i in improvements)
    
    # E0a: carry both sides' provenance (absent on pre-E0a reports) and flag
    # engine mismatch. Advisory only — the compare still runs, but a
    # cross-engine delta can no longer masquerade as a same-engine one.
    base_prov = baseline.get("provenance")
    cur_prov = current.get("provenance")
    cross_engine = bool(
        base_prov and cur_prov
        and base_prov.get("engine_sha") and cur_prov.get("engine_sha")
        and base_prov["engine_sha"] != cur_prov["engine_sha"]
    )

    return {
        "timestamp": datetime.now().isoformat(),
        "regression_budget": regression_budget,
        "baseline_provenance": base_prov,
        "current_provenance": cur_prov,
        "cross_engine": cross_engine,
        "summary": {
            "regressions": len(regressions),
            "improvements": len(improvements),
            "new_failures": len(new_failures),
            "not_measured": len(not_measured),
            "total_regression_delta": total_regression,
            "total_improvement_delta": total_improvement,
            "net_delta": total_regression - total_improvement,
            "pass": len(regressions) == 0 and len(new_failures) == 0,
        },
        "regressions": sorted(regressions, key=lambda x: -x["delta"]),
        "improvements": sorted(improvements, key=lambda x: x["delta"]),
        "new_failures": new_failures,
        "not_measured": not_measured,
        "taxonomy_shifts": sorted(taxonomy_shifts, key=lambda x: -abs(x["delta"])),
    }


# ============================================================================
# Main
# ============================================================================

def main():
    parser = argparse.ArgumentParser(
        description="Aggregate parity results and detect regressions",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=__doc__,
    )
    
    # Input sources
    parser.add_argument("--run-id", type=str, default=None,
                        help="Aggregate from a single swarm run")
    parser.add_argument("--runs", type=str, default=None,
                        help="Comma-separated run IDs to merge")
    parser.add_argument("--attribution-dir", type=str, default=None,
                        help="Aggregate from raw attribution.json files in directory")
    parser.add_argument("--results-root", type=str, default=None,
                        help="Results root directory")
    
    # Comparison mode
    parser.add_argument("--compare", action="store_true",
                        help="Compare two reports for regressions")
    parser.add_argument("--baseline", type=str, default=None,
                        help="Baseline report path or run ID")
    parser.add_argument("--current", type=str, default=None,
                        help="Current report path or run ID")
    parser.add_argument("--regression-budget", type=float, default=0.1,
                        help="Max allowed regression per case (default: 0.1%)")
    
    # Provenance (E0a) — who produced this report
    parser.add_argument("--engine-sha", type=str, default=None,
                        help="Engine commit SHA this report measures (stamped "
                             "into the report as provenance)")
    parser.add_argument("--receipt-run", type=str, default=None,
                        help="CI run id that produced the captures")

    # Output
    parser.add_argument("--output", "-o", type=str, default=None,
                        help="Output path for aggregate report")
    parser.add_argument("--format", type=str, choices=["json", "summary"], default="json",
                        help="Output format")
    
    args = parser.parse_args()
    
    results_root = Path(args.results_root) if args.results_root else DEFAULT_RESULTS_ROOT
    
    if args.compare:
        # Comparison mode
        if not args.baseline or not args.current:
            parser.error("--compare requires --baseline and --current")
        
        # Load reports
        def load_report(ref: str) -> Dict:
            # Try as path first
            path = Path(ref)
            if path.exists():
                with open(path, encoding="utf-8") as f:
                    return json.load(f)
            # Try as run ID
            report_path = results_root / ref / "aggregate_report.json"
            if report_path.exists():
                with open(report_path, encoding="utf-8") as f:
                    return json.load(f)
            # Try swarm report
            swarm_path = results_root / ref / "swarm_report.json"
            if swarm_path.exists():
                with open(swarm_path, encoding="utf-8") as f:
                    return json.load(f)
            raise FileNotFoundError(f"Could not find report: {ref}")
        
        baseline = load_report(args.baseline)
        current = load_report(args.current)
        
        comparison = compare_reports(baseline, current, args.regression_budget)

        if comparison["cross_engine"]:
            print("WARNING: cross-engine comparison — baseline engine "
                  f"{comparison['baseline_provenance']['engine_sha']} != current "
                  f"{comparison['current_provenance']['engine_sha']}. "
                  "Deltas attribute environment+engine together, not the engine.")

        # Output
        output_path = args.output or "regression_report.json"
        with open(output_path, "w", encoding="utf-8") as f:
            json.dump(comparison, f, indent=2)
        
        # Print summary
        s = comparison["summary"]
        print("\n" + "=" * 60)
        print("REGRESSION COMPARISON")
        print("=" * 60)
        print(f"Regressions: {s['regressions']}")
        print(f"Improvements: {s['improvements']}")
        print(f"New failures: {s['new_failures']}")
        print(f"Net delta: {s['net_delta']:+.2f}%")
        print(f"\nResult: {'PASS' if s['pass'] else 'FAIL'}")
        
        if comparison["regressions"]:
            print("\nRegressions:")
            for r in comparison["regressions"][:10]:
                print(f"  {r['case_id']}@{r['viewport']}: {r['baseline_diff']:.2f}% -> {r['current_diff']:.2f}% (+{r['delta']:.2f}%)")
        
        print(f"\nReport saved to: {output_path}")
        
        sys.exit(0 if s["pass"] else 1)
    
    # Aggregation mode
    report: Optional[Dict] = None
    
    if args.run_id:
        # Single run
        swarm_report = load_swarm_report(args.run_id, results_root)
        if swarm_report:
            report = aggregate_from_results(swarm_report.get("results", []))
        else:
            # Try attribution files
            run_dir = results_root / args.run_id
            files = find_attribution_files(run_dir)
            if files:
                report = aggregate_from_attribution_files(files)
    
    elif args.runs:
        # Multiple runs (merge shards)
        run_ids = args.runs.split(",")
        reports = []
        for rid in run_ids:
            r = load_swarm_report(rid.strip(), results_root)
            if r:
                reports.append(r)
        
        if reports:
            report = aggregate_from_swarm_reports(reports)
    
    elif args.attribution_dir:
        # Raw attribution files
        attr_dir = Path(args.attribution_dir)
        files = find_attribution_files(attr_dir)
        if files:
            report = aggregate_from_attribution_files(files)
    
    if not report:
        print("Error: No data to aggregate")
        sys.exit(1)

    stamp_provenance(report, args.engine_sha, args.receipt_run)

    # Save output
    if args.output:
        output_path = Path(args.output)
    elif args.run_id:
        output_path = results_root / args.run_id / "aggregate_report.json"
    else:
        output_path = Path("aggregate_report.json")
    
    output_path.parent.mkdir(parents=True, exist_ok=True)
    
    with open(output_path, "w", encoding="utf-8") as f:
        json.dump(report, f, indent=2)
    
    # Print summary
    s = report["summary"]
    print("\n" + "=" * 60)
    print("AGGREGATE REPORT")
    print("=" * 60)
    print(f"Total cases: {s['total_cases']}")
    print(f"Passed: {s['passed']}/{s['total_cases']}")
    print(f"Average diff: {_fmt_pct(s['avg_diff_pct'])}")
    
    print("\nFix Scoreboard (top 5):")
    for c in report["fix_scoreboard"]["top_contributors"][:5]:
        print(f"  #{c['rank']} {c['selector']}: {c['contribution_pct']:.1f}% ({c['total_diff_pixels']} px, {c['case_count']} cases)")
        if c["likely_cause"]:
            print(f"      Likely cause: {c['likely_cause']}")
    
    gains = report["fix_scoreboard"]["projected_gains"]
    print(f"\nProjected gains:")
    print(f"  Fix top 5: -{gains.get('top_5', 0):.1f}% diff")
    print(f"  Fix top 10: -{gains.get('top_10', 0):.1f}% diff")
    
    print("\nTaxonomy:")
    for t in report["taxonomy"]["buckets"][:5]:
        print(f"  {t['bucket']}: {t['total_contribution_pct']:.1f}%")
    
    print(f"\nReport saved to: {output_path}")


if __name__ == "__main__":
    main()
