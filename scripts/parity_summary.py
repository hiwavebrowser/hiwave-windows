#!/usr/bin/env python3
"""
parity_summary.py - Generate parity progress trend reports

Generates a progress report across all historical runs, including:
- PARITY_PROGRESS.md - Human-readable progress report
- progress_report.json - Machine-readable trend data

Usage:
    python3 scripts/parity_summary.py [--last N] [--output-dir <dir>]
    
Examples:
    python3 scripts/parity_summary.py
    python3 scripts/parity_summary.py --last 10
"""

import json
import sys
from datetime import datetime
from pathlib import Path
from typing import Dict, Any, List, Optional


def load_all_summaries(history_dir: Path, limit: Optional[int] = None) -> List[Dict[str, Any]]:
    """Load all run summaries from history."""
    if not history_dir.exists():
        return []
    
    runs = []
    for d in sorted(history_dir.iterdir()):
        if d.is_dir() and d.name != "latest":
            summary_path = d / "summary.json"
            if summary_path.exists():
                with open(summary_path, encoding="utf-8") as f:
                    summary = json.load(f)
                    summary["run_id"] = d.name
                    runs.append(summary)
    
    # Sort by run_id (timestamp)
    runs.sort(key=lambda x: x["run_id"])
    
    if limit:
        runs = runs[-limit:]
    
    return runs


def compute_trend(values: List[float], window: int = 3) -> str:
    """Compute trend indicator from recent values."""
    if len(values) < 2:
        return "="
    
    recent = values[-window:] if len(values) >= window else values
    
    if len(recent) < 2:
        return "="
    
    delta = recent[-1] - recent[0]
    
    if delta > 1:
        return "▲"
    elif delta < -1:
        return "▼"
    else:
        return "="


def format_date(run_id: str) -> str:
    """Format run_id timestamp for display."""
    try:
        # Parse YYYYMMDD_HHMMSS
        dt = datetime.strptime(run_id, "%Y%m%d_%H%M%S")
        return dt.strftime("%b %d %H:%M")
    except ValueError:
        return run_id


def generate_sparkline(values: List[float], width: int = 10) -> str:
    """Generate a text-based sparkline."""
    if not values:
        return ""
    
    # Normalize to 0-1 range
    min_val = min(values)
    max_val = max(values)
    range_val = max_val - min_val if max_val > min_val else 1
    
    # Sparkline characters (low to high)
    chars = "▁▂▃▄▅▆▇█"
    
    # Sample values if too many
    if len(values) > width:
        step = len(values) / width
        sampled = [values[int(i * step)] for i in range(width)]
    else:
        sampled = values
    
    # Convert to sparkline
    sparkline = ""
    for v in sampled:
        normalized = (v - min_val) / range_val
        idx = int(normalized * (len(chars) - 1))
        sparkline += chars[idx]
    
    return sparkline


def generate_progress_report(runs: List[Dict[str, Any]]) -> Dict[str, Any]:
    """Generate a comprehensive progress report."""
    if not runs:
        return {"error": "No runs found"}
    
    # Current status (latest run)
    latest = runs[-1]
    
    # Historical parity values
    parity_values = [r["estimated_parity"] for r in runs]
    tier_a_values = [r["tier_a_pass_rate"] * 100 for r in runs]
    weighted_mean_values = [r["tier_b_weighted_mean"] for r in runs]
    
    # Trend calculations
    parity_trend = compute_trend(parity_values)
    
    # Calculate deltas
    if len(runs) >= 2:
        prev = runs[-2]
        parity_delta = latest["estimated_parity"] - prev["estimated_parity"]
        tier_a_delta = (latest["tier_a_pass_rate"] - prev["tier_a_pass_rate"]) * 100
    else:
        parity_delta = 0
        tier_a_delta = 0
    
    # Best and worst runs
    best_run = max(runs, key=lambda r: r["estimated_parity"])
    worst_run = min(runs, key=lambda r: r["estimated_parity"])
    
    # Per-case trends (track which cases improved most over all runs)
    case_first_last = {}
    if len(runs) >= 2:
        first_cases = runs[0].get("case_diffs", {})
        last_cases = runs[-1].get("case_diffs", {})
        
        for case_id in set(first_cases.keys()) | set(last_cases.keys()):
            first_diff = first_cases.get(case_id, {}).get("diff_pct", 100)
            last_diff = last_cases.get(case_id, {}).get("diff_pct", 100)
            case_first_last[case_id] = {
                "first": first_diff,
                "last": last_diff,
                "delta": last_diff - first_diff,
                "improved": last_diff < first_diff,
            }
    
    # Sort cases by improvement
    most_improved = sorted(
        case_first_last.items(),
        key=lambda x: x[1]["delta"]
    )[:5]
    
    most_regressed = sorted(
        case_first_last.items(),
        key=lambda x: -x[1]["delta"]
    )[:5]
    
    return {
        "generated_at": datetime.now().isoformat(),
        "total_runs": len(runs),
        "current": {
            "run_id": latest["run_id"],
            "tag": latest.get("tag"),
            "estimated_parity": latest["estimated_parity"],
            "tier_a_pass_rate": latest["tier_a_pass_rate"],
            "tier_b_weighted_mean": latest["tier_b_weighted_mean"],
            "issue_clusters": latest.get("issue_clusters", {}),
        },
        "trends": {
            "parity_trend": parity_trend,
            "parity_delta": parity_delta,
            "tier_a_delta": tier_a_delta,
            "parity_sparkline": generate_sparkline(parity_values),
        },
        "history": [
            {
                "run_id": r["run_id"],
                "date": format_date(r["run_id"]),
                "tag": r.get("tag"),
                "estimated_parity": r["estimated_parity"],
                "tier_a_pass_rate": r["tier_a_pass_rate"],
                "tier_b_weighted_mean": r["tier_b_weighted_mean"],
                "git_commit": r.get("git_commit"),
            }
            for r in reversed(runs)  # Most recent first
        ],
        "extremes": {
            "best_run": {
                "run_id": best_run["run_id"],
                "parity": best_run["estimated_parity"],
                "tag": best_run.get("tag"),
            },
            "worst_run": {
                "run_id": worst_run["run_id"],
                "parity": worst_run["estimated_parity"],
                "tag": worst_run.get("tag"),
            },
        },
        "case_trends": {
            "most_improved": [
                {"case_id": case_id, **data}
                for case_id, data in most_improved if data["improved"]
            ],
            "most_regressed": [
                {"case_id": case_id, **data}
                for case_id, data in most_regressed if not data["improved"] and data["delta"] > 0
            ],
        },
    }


def generate_markdown(report: Dict[str, Any]) -> str:
    """Generate PARITY_PROGRESS.md content."""
    lines = []
    
    lines.append("# Parity Progress Report")
    lines.append("")
    lines.append(f"Generated: {report['generated_at']}")
    lines.append("")
    
    # Current Status
    current = report["current"]
    trends = report["trends"]
    
    lines.append("## Current Status")
    lines.append("")
    lines.append(f"- **Estimated Parity**: {current['estimated_parity']:.1f}%")
    lines.append(f"- **Tier A Pass Rate**: {current['tier_a_pass_rate']*100:.1f}%")
    lines.append(f"- **Tier B Mean Diff**: {current['tier_b_weighted_mean']:.1f}%")
    
    trend_indicator = trends["parity_trend"]
    delta = trends["parity_delta"]
    if delta != 0:
        lines.append(f"- **Trend**: {trend_indicator} {delta:+.1f}% (vs previous)")
    
    lines.append(f"- **Sparkline**: {trends['parity_sparkline']}")
    lines.append("")
    
    # Issue Clusters
    clusters = current.get("issue_clusters", {})
    if clusters:
        lines.append("### Issue Clusters")
        lines.append("")
        for cluster, count in sorted(clusters.items(), key=lambda x: -x[1]):
            lines.append(f"- {cluster}: {count}")
        lines.append("")
    
    # Historical Trend
    lines.append("## Historical Trend")
    lines.append("")
    lines.append("| Date | Parity | Tier A | Tag | Commit |")
    lines.append("|------|--------|--------|-----|--------|")
    
    history = report["history"]
    prev_parity = None
    for h in history:
        parity = h["estimated_parity"]
        tier_a = h["tier_a_pass_rate"] * 100
        tag = h.get("tag") or ""
        commit = h.get("git_commit", "")[:8] if h.get("git_commit") else ""
        
        # Calculate change from previous
        if prev_parity is not None:
            delta = parity - prev_parity
            delta_str = f" ({delta:+.1f}%)" if abs(delta) >= 0.1 else ""
        else:
            delta_str = ""
        
        lines.append(f"| {h['date']} | {parity:.1f}%{delta_str} | {tier_a:.1f}% | {tag} | {commit} |")
        prev_parity = parity
    
    lines.append("")
    
    # Extremes
    extremes = report["extremes"]
    lines.append("## Best / Worst Runs")
    lines.append("")
    best = extremes["best_run"]
    worst = extremes["worst_run"]
    lines.append(f"- **Best**: {best['run_id']} at {best['parity']:.1f}%" + (f" ({best['tag']})" if best.get('tag') else ""))
    lines.append(f"- **Worst**: {worst['run_id']} at {worst['parity']:.1f}%" + (f" ({worst['tag']})" if worst.get('tag') else ""))
    lines.append("")
    
    # Case Trends
    case_trends = report.get("case_trends", {})
    
    improved = case_trends.get("most_improved", [])
    if improved:
        lines.append("## Most Improved Cases (Overall)")
        lines.append("")
        for c in improved[:5]:
            lines.append(f"- {c['case_id']}: {c['first']:.1f}% -> {c['last']:.1f}% ({c['delta']:+.1f}%)")
        lines.append("")
    
    regressed = case_trends.get("most_regressed", [])
    if regressed:
        lines.append("## Regressions to Watch")
        lines.append("")
        for c in regressed[:5]:
            lines.append(f"- {c['case_id']}: {c['first']:.1f}% -> {c['last']:.1f}% ({c['delta']:+.1f}%)")
        lines.append("")
    
    # Usage
    lines.append("---")
    lines.append("")
    lines.append("## How to Update")
    lines.append("")
    lines.append("```bash")
    lines.append("# Run a new baseline capture")
    lines.append('python3 scripts/parity_baseline.py --tag "description"')
    lines.append("")
    lines.append("# Compare to previous run")
    lines.append("python3 scripts/parity_compare.py")
    lines.append("")
    lines.append("# Regenerate this report")
    lines.append("python3 scripts/parity_summary.py")
    lines.append("```")
    lines.append("")
    
    return "\n".join(lines)


def main():
    history_dir = Path("parity-history")
    output_dir = Path(".")
    limit = None
    
    # Parse arguments
    args = sys.argv[1:]
    i = 0
    while i < len(args):
        if args[i] == "--last" and i + 1 < len(args):
            limit = int(args[i + 1])
            i += 2
        elif args[i] == "--output-dir" and i + 1 < len(args):
            output_dir = Path(args[i + 1])
            i += 2
        elif args[i] == "--history" and i + 1 < len(args):
            history_dir = Path(args[i + 1])
            i += 2
        else:
            i += 1
    
    print(f"Loading runs from: {history_dir}")
    if limit:
        print(f"Limiting to last {limit} runs")
    
    # Load all summaries
    runs = load_all_summaries(history_dir, limit)
    
    if not runs:
        print("No runs found in history.")
        print("Run parity_archive.py first to create history.")
        sys.exit(1)
    
    print(f"Found {len(runs)} runs")
    
    # Generate report
    report = generate_progress_report(runs)
    
    # Save JSON report
    json_path = output_dir / "progress_report.json"
    with open(json_path, "w", encoding="utf-8") as f:
        json.dump(report, f, indent=2)
    print(f"JSON report saved to: {json_path}")
    
    # Generate and save markdown
    markdown = generate_markdown(report)
    md_path = output_dir / "PARITY_PROGRESS.md"
    with open(md_path, "w", encoding="utf-8") as f:
        f.write(markdown)
    print(f"Markdown report saved to: {md_path}")
    
    # Print summary
    current = report["current"]
    trends = report["trends"]
    
    print("\n" + "=" * 60)
    print("PARITY PROGRESS SUMMARY")
    print("=" * 60)
    print(f"\nCurrent Parity: {current['estimated_parity']:.1f}%")
    print(f"Trend: {trends['parity_trend']} {trends['parity_delta']:+.1f}%")
    print(f"Sparkline: {trends['parity_sparkline']}")
    print(f"\nTotal runs tracked: {report['total_runs']}")
    
    extremes = report["extremes"]
    print(f"Best run: {extremes['best_run']['parity']:.1f}%")
    print(f"Worst run: {extremes['worst_run']['parity']:.1f}%")


if __name__ == "__main__":
    main()

