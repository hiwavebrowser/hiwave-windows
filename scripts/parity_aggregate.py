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
    python scripts/parity_aggregate.py --run-id <id>

    # Aggregate from multiple shards
    python scripts/parity_aggregate.py --runs <id1>,<id2>,<id3>

    # Compare for regressions
    python scripts/parity_aggregate.py --compare --baseline <old> --current <new>

    # Aggregate raw attribution files
    python scripts/parity_aggregate.py --attribution-dir <path>

Platform: Windows (ported from macOS)
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
    diff_pct: float
    passed: bool
    stable: bool
    threshold: float
    overlay_path: Optional[str] = None
    attribution_path: Optional[str] = None
    top_contributors: List[Dict] = field(default_factory=list)
    taxonomy: Dict[str, float] = field(default_factory=dict)


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
        diff_pct = r.get("diff_pct_median", r.get("diff_pct", 100))

        summary = CaseSummary(
            case_id=case_id,
            viewport=viewport,
            diff_pct=diff_pct,
            passed=r.get("passed", False),
            stable=r.get("stable", False),
            threshold=r.get("threshold", 15),
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
            "avg_diff_pct": sum(c.diff_pct for c in case_summaries) / max(1, len(case_summaries)),
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
                "overlay_path": c.overlay_path,
                "attribution_path": c.attribution_path,
                "top_contributors": c.top_contributors[:3],
                "taxonomy": c.taxonomy,
            }
            for c in sorted(case_summaries, key=lambda x: -x.diff_pct)
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

    return {
        "timestamp": datetime.now().isoformat(),
        "regression_budget": regression_budget,
        "summary": {
            "regressions": len(regressions),
            "improvements": len(improvements),
            "new_failures": len(new_failures),
            "total_regression_delta": total_regression,
            "total_improvement_delta": total_improvement,
            "net_delta": total_regression - total_improvement,
            "pass": len(regressions) == 0 and len(new_failures) == 0,
        },
        "regressions": sorted(regressions, key=lambda x: -x["delta"]),
        "improvements": sorted(improvements, key=lambda x: x["delta"]),
        "new_failures": new_failures,
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
                        help="Max allowed regression per case (default: 0.1%%)")

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
    print(f"Average diff: {s['avg_diff_pct']:.2f}%")

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
