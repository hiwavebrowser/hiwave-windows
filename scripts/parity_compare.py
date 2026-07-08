#!/usr/bin/env python3
"""
parity_compare.py - Compare two parity baseline runs

Compares two runs and produces a delta report showing improvements
and regressions.

Usage:
    python3 scripts/parity_compare.py [run1] [run2]
    python3 scripts/parity_compare.py                  # Compare latest vs previous
    python3 scripts/parity_compare.py latest 20260107_024500
    
Examples:
    python3 scripts/parity_compare.py
    python3 scripts/parity_compare.py 20260107_024500 20260107_031200
"""

import json
import sys
from pathlib import Path
from typing import Dict, Any, List, Tuple, Optional


def load_summary(history_dir: Path, run_id: str) -> Optional[Dict[str, Any]]:
    """Load summary.json for a run."""
    # Handle 'latest' symlink
    if run_id == "latest":
        latest_link = history_dir / "latest"
        if latest_link.is_symlink():
            run_id = latest_link.resolve().name
        elif not latest_link.exists():
            return None
    
    summary_path = history_dir / run_id / "summary.json"
    if not summary_path.exists():
        return None
    
    with open(summary_path, encoding="utf-8") as f:
        return json.load(f)


def get_all_runs(history_dir: Path) -> List[str]:
    """Get all run IDs sorted by timestamp."""
    if not history_dir.exists():
        return []
    
    runs = []
    for d in history_dir.iterdir():
        if d.is_dir() and d.name != "latest":
            runs.append(d.name)
    
    return sorted(runs)


def get_latest_two_runs(history_dir: Path) -> Tuple[Optional[str], Optional[str]]:
    """Get the two most recent run IDs."""
    runs = get_all_runs(history_dir)
    
    if len(runs) == 0:
        return None, None
    elif len(runs) == 1:
        return runs[0], None
    else:
        return runs[-1], runs[-2]


def format_delta(old: float, new: float, invert: bool = False) -> str:
    """Format a delta value with indicator."""
    delta = new - old
    
    # For diff percentages, lower is better (invert=True)
    # For parity percentage, higher is better (invert=False)
    if invert:
        improved = delta < 0
    else:
        improved = delta > 0
    
    if abs(delta) < 0.1:
        return "="
    
    sign = "+" if delta > 0 else ""
    indicator = "✓" if improved else "✗"
    
    return f"({sign}{delta:.1f}%) {indicator}"


def compare_runs(
    old_summary: Dict[str, Any],
    new_summary: Dict[str, Any],
    significance_threshold: float = 5.0,
) -> Dict[str, Any]:
    """Compare two run summaries and produce a delta report."""
    
    # Overall parity change
    old_parity = old_summary["estimated_parity"]
    new_parity = new_summary["estimated_parity"]
    parity_delta = new_parity - old_parity
    
    # Tier metrics
    old_tier_a = old_summary["tier_a_pass_rate"]
    new_tier_a = new_summary["tier_a_pass_rate"]
    
    old_weighted = old_summary["tier_b_weighted_mean"]
    new_weighted = new_summary["tier_b_weighted_mean"]
    
    # Per-case changes
    old_cases = old_summary.get("case_diffs", {})
    new_cases = new_summary.get("case_diffs", {})
    
    all_case_ids = set(old_cases.keys()) | set(new_cases.keys())
    
    case_changes = []
    for case_id in sorted(all_case_ids):
        old_diff = old_cases.get(case_id, {}).get("diff_pct", 100)
        new_diff = new_cases.get(case_id, {}).get("diff_pct", 100)
        case_type = new_cases.get(case_id, old_cases.get(case_id, {})).get("type", "unknown")
        
        delta = new_diff - old_diff
        
        case_changes.append({
            "case_id": case_id,
            "type": case_type,
            "old_diff": old_diff,
            "new_diff": new_diff,
            "delta": delta,
            "improved": delta < 0,
            "significant": abs(delta) >= significance_threshold,
        })
    
    # Sort by delta (biggest improvements first)
    case_changes.sort(key=lambda x: x["delta"])
    
    # Issue cluster changes
    old_clusters = old_summary.get("issue_clusters", {})
    new_clusters = new_summary.get("issue_clusters", {})
    
    all_clusters = set(old_clusters.keys()) | set(new_clusters.keys())
    
    cluster_changes = {}
    for cluster in all_clusters:
        old_count = old_clusters.get(cluster, 0)
        new_count = new_clusters.get(cluster, 0)
        cluster_changes[cluster] = {
            "old": old_count,
            "new": new_count,
            "delta": new_count - old_count,
        }
    
    # Count improvements and regressions
    improvements = [c for c in case_changes if c["improved"] and c["significant"]]
    regressions = [c for c in case_changes if not c["improved"] and c["significant"]]
    
    return {
        "old_run": old_summary.get("run_id", "unknown"),
        "new_run": new_summary.get("run_id", "unknown"),
        "old_tag": old_summary.get("tag"),
        "new_tag": new_summary.get("tag"),
        "overall": {
            "old_parity": old_parity,
            "new_parity": new_parity,
            "parity_delta": parity_delta,
            "improved": parity_delta > 0,
            "old_tier_a": old_tier_a,
            "new_tier_a": new_tier_a,
            "tier_a_delta": new_tier_a - old_tier_a,
            "old_weighted_mean": old_weighted,
            "new_weighted_mean": new_weighted,
            "weighted_mean_delta": new_weighted - old_weighted,
        },
        "case_changes": case_changes,
        "cluster_changes": cluster_changes,
        "summary": {
            "total_cases": len(case_changes),
            "improvements": len(improvements),
            "regressions": len(regressions),
            "unchanged": len(case_changes) - len(improvements) - len(regressions),
        },
    }


def print_comparison(comparison: Dict[str, Any]):
    """Print a formatted comparison report."""
    old_run = comparison["old_run"]
    new_run = comparison["new_run"]
    old_tag = comparison.get("old_tag")
    new_tag = comparison.get("new_tag")
    
    old_label = f"{old_run}" + (f" ({old_tag})" if old_tag else "")
    new_label = f"{new_run}" + (f" ({new_tag})" if new_tag else "")
    
    print("\n" + "=" * 70)
    print(f"PARITY COMPARISON")
    print("=" * 70)
    print(f"\nComparing: {old_label}")
    print(f"       to: {new_label}")
    
    overall = comparison["overall"]
    old_parity = overall["old_parity"]
    new_parity = overall["new_parity"]
    delta = overall["parity_delta"]
    
    indicator = "▲" if delta > 0 else "▼" if delta < 0 else "="
    status = "IMPROVED" if delta > 0 else "REGRESSED" if delta < 0 else "UNCHANGED"
    
    print(f"\n{'─' * 70}")
    print(f"OVERALL PARITY: {old_parity:.1f}% -> {new_parity:.1f}% ({indicator} {delta:+.1f}%) {status}")
    print(f"{'─' * 70}")
    
    # Tier metrics
    print(f"\nTier A Pass Rate: {overall['old_tier_a']*100:.1f}% -> {overall['new_tier_a']*100:.1f}% {format_delta(overall['old_tier_a']*100, overall['new_tier_a']*100)}")
    print(f"Tier B Mean Diff: {overall['old_weighted_mean']:.1f}% -> {overall['new_weighted_mean']:.1f}% {format_delta(overall['old_weighted_mean'], overall['new_weighted_mean'], invert=True)}")
    
    # Summary counts
    summary = comparison["summary"]
    print(f"\nCase Summary: {summary['improvements']} improved, {summary['regressions']} regressed, {summary['unchanged']} unchanged")
    
    # Per-case changes (significant only)
    case_changes = comparison["case_changes"]
    significant = [c for c in case_changes if c["significant"]]
    
    if significant:
        print(f"\n{'─' * 70}")
        print("SIGNIFICANT CHANGES (>5% delta)")
        print(f"{'─' * 70}")
        
        # Improvements first
        improvements = [c for c in significant if c["improved"]]
        if improvements:
            print("\n✓ Improvements:")
            for c in improvements:
                print(f"  {c['case_id']:25} {c['old_diff']:5.1f}% -> {c['new_diff']:5.1f}% ({c['delta']:+.1f}%)")
        
        # Then regressions
        regressions = [c for c in significant if not c["improved"]]
        if regressions:
            print("\n✗ Regressions:")
            for c in regressions:
                print(f"  {c['case_id']:25} {c['old_diff']:5.1f}% -> {c['new_diff']:5.1f}% ({c['delta']:+.1f}%)")
    
    # All case changes
    print(f"\n{'─' * 70}")
    print("ALL CASE CHANGES")
    print(f"{'─' * 70}")
    
    for c in case_changes:
        indicator = "✓" if c["improved"] else "✗" if c["delta"] > 0 else "="
        print(f"  {c['case_id']:25} {c['old_diff']:5.1f}% -> {c['new_diff']:5.1f}% ({c['delta']:+5.1f}%) {indicator}")
    
    # Issue clusters
    cluster_changes = comparison["cluster_changes"]
    print(f"\n{'─' * 70}")
    print("ISSUE CLUSTERS")
    print(f"{'─' * 70}")
    
    for cluster, data in sorted(cluster_changes.items(), key=lambda x: -abs(x[1]["delta"])):
        delta = data["delta"]
        indicator = "✓" if delta < 0 else "✗" if delta > 0 else "="
        print(f"  {cluster:20} {data['old']:5} -> {data['new']:5} ({delta:+5}) {indicator}")
    
    print()


def main():
    history_dir = Path("parity-history")
    
    if not history_dir.exists():
        print(f"Error: History directory not found: {history_dir}")
        print("Run parity_archive.py first to create history.")
        sys.exit(1)
    
    # Parse arguments
    args = [a for a in sys.argv[1:] if not a.startswith("-")]
    
    if len(args) == 0:
        # Compare latest vs previous
        latest, previous = get_latest_two_runs(history_dir)
        if not latest:
            print("Error: No runs found in history")
            sys.exit(1)
        if not previous:
            print(f"Only one run found: {latest}")
            print("Need at least two runs to compare.")
            sys.exit(1)
        run1, run2 = previous, latest
    elif len(args) == 1:
        # Compare given run vs latest
        run1 = args[0]
        latest, _ = get_latest_two_runs(history_dir)
        if not latest:
            print("Error: No runs found in history")
            sys.exit(1)
        run2 = latest
    else:
        run1, run2 = args[0], args[1]
    
    # Load summaries
    old_summary = load_summary(history_dir, run1)
    new_summary = load_summary(history_dir, run2)
    
    if not old_summary:
        print(f"Error: Could not load run: {run1}")
        sys.exit(1)
    if not new_summary:
        print(f"Error: Could not load run: {run2}")
        sys.exit(1)
    
    # Compare and print
    comparison = compare_runs(old_summary, new_summary)
    print_comparison(comparison)
    
    # Save comparison to file
    output_path = history_dir / f"comparison_{run1}_to_{run2}.json"
    with open(output_path, "w", encoding="utf-8") as f:
        json.dump(comparison, f, indent=2)
    print(f"Comparison saved to: {output_path}")


if __name__ == "__main__":
    main()

