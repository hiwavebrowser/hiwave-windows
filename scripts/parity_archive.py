#!/usr/bin/env python3
"""
parity_archive.py - Archive parity baseline runs with timestamps

Archives each baseline run to a timestamped directory for historical tracking.
Creates a summary.json with key metrics for fast comparison.

Usage:
    python3 scripts/parity_archive.py [--tag <name>] [--source <dir>]
    
Examples:
    python3 scripts/parity_archive.py
    python3 scripts/parity_archive.py --tag "after-flex-fix"
    python3 scripts/parity_archive.py --source parity-baseline
"""

import json
import os
import shutil
import subprocess
import sys
from datetime import datetime
from pathlib import Path
from typing import Dict, Any, Optional


def get_git_commit() -> str:
    """Get current git commit hash."""
    try:
        result = subprocess.run(
            ["git", "rev-parse", "HEAD"],
            capture_output=True,
            text=True,
            timeout=5,
        )
        if result.returncode == 0:
            return result.stdout.strip()[:12]
    except Exception:
        pass
    return "unknown"


def get_git_branch() -> str:
    """Get current git branch name."""
    try:
        result = subprocess.run(
            ["git", "rev-parse", "--abbrev-ref", "HEAD"],
            capture_output=True,
            text=True,
            timeout=5,
        )
        if result.returncode == 0:
            return result.stdout.strip()
    except Exception:
        pass
    return "unknown"


def extract_summary(report: Dict[str, Any]) -> Dict[str, Any]:
    """Extract key metrics from a full baseline report."""
    metrics = report.get("metrics", {})
    
    # Extract per-case diff percentages
    case_diffs = {}
    for result in report.get("builtin_results", []):
        case_diffs[result["case_id"]] = {
            "type": "builtin",
            "diff_pct": result.get("estimated_diff_pct", 100),
        }
    for result in report.get("websuite_results", []):
        case_diffs[result["case_id"]] = {
            "type": "websuite",
            "diff_pct": result.get("estimated_diff_pct", 100),
        }
    
    return {
        "timestamp": report.get("timestamp"),
        "estimated_parity": 100 - metrics.get("tier_b_weighted_mean", 100),
        "tier_a_pass_rate": metrics.get("tier_a_pass_rate", 0),
        "tier_a_builtin_pass": metrics.get("tier_a_builtin_pass", 0),
        "tier_a_websuite_pass": metrics.get("tier_a_websuite_pass", 0),
        "tier_b_median_diff": metrics.get("tier_b_median_diff", 100),
        "tier_b_weighted_mean": metrics.get("tier_b_weighted_mean", 100),
        "builtin_mean_diff": metrics.get("builtin_mean_diff", 100),
        "websuite_mean_diff": metrics.get("websuite_mean_diff", 100),
        "issue_clusters": report.get("issue_clusters", {}),
        "worst_3_cases": metrics.get("worst_3_cases", []),
        "case_diffs": case_diffs,
    }


def archive_run(
    source_dir: Path,
    history_dir: Path,
    tag: Optional[str] = None,
) -> Optional[Path]:
    """
    Archive a baseline run to the history directory.
    
    Returns the path to the archived run directory, or None if failed.
    """
    # Load the baseline report
    report_path = source_dir / "baseline_report.json"
    if not report_path.exists():
        print(f"Error: No baseline report found at {report_path}")
        return None
    
    with open(report_path, encoding="utf-8") as f:
        report = json.load(f)
    
    # Create timestamp-based directory name
    timestamp = datetime.now().strftime("%Y%m%d_%H%M%S")
    run_dir = history_dir / timestamp
    
    # Ensure history directory exists
    history_dir.mkdir(parents=True, exist_ok=True)
    
    # Create run directory
    run_dir.mkdir(exist_ok=True)
    
    # Copy full report
    shutil.copy(report_path, run_dir / "report.json")
    
    # Create summary
    summary = extract_summary(report)
    summary["run_id"] = timestamp
    summary["tag"] = tag
    summary["git_commit"] = get_git_commit()
    summary["git_branch"] = get_git_branch()
    
    with open(run_dir / "summary.json", "w", encoding="utf-8") as f:
        json.dump(summary, f, indent=2)
    
    # Save git commit
    with open(run_dir / "git_commit.txt", "w", encoding="utf-8") as f:
        f.write(f"commit: {summary['git_commit']}\n")
        f.write(f"branch: {summary['git_branch']}\n")
        if tag:
            f.write(f"tag: {tag}\n")
    
    # Copy captures directory if it exists
    captures_src = source_dir / "captures"
    if captures_src.exists():
        captures_dst = run_dir / "captures"
        if captures_dst.exists():
            shutil.rmtree(captures_dst)
        shutil.copytree(captures_src, captures_dst)
    
    # Update 'latest' pointer. Symlinks need elevation on Windows
    # (WinError 1314), so fall back to a plain latest.txt pointer file.
    latest_link = history_dir / "latest"
    if latest_link.is_symlink():
        latest_link.unlink()
    elif latest_link.exists():
        latest_link.unlink()
    try:
        latest_link.symlink_to(timestamp)
    except OSError:
        (history_dir / "latest.txt").write_text(timestamp, encoding="utf-8")
    
    return run_dir


def get_previous_run(history_dir: Path, current_run: str) -> Optional[str]:
    """Get the run ID of the previous run."""
    runs = sorted([
        d.name for d in history_dir.iterdir()
        if d.is_dir() and d.name != "latest" and d.name != current_run
    ])
    
    if not runs:
        return None
    
    # Find the run before current
    try:
        idx = runs.index(current_run)
        if idx > 0:
            return runs[idx - 1]
    except ValueError:
        pass
    
    # If current not in list, return the most recent
    return runs[-1] if runs else None


def print_summary(summary: Dict[str, Any], previous: Optional[Dict[str, Any]] = None):
    """Print a summary of the archived run."""
    print("\n" + "=" * 60)
    print("ARCHIVED RUN SUMMARY")
    print("=" * 60)
    
    print(f"\nRun ID: {summary['run_id']}")
    if summary.get('tag'):
        print(f"Tag: {summary['tag']}")
    print(f"Git: {summary['git_branch']} @ {summary['git_commit']}")
    
    parity = summary['estimated_parity']
    print(f"\nEstimated Parity: {parity:.1f}%")
    
    if previous:
        prev_parity = previous['estimated_parity']
        delta = parity - prev_parity
        indicator = "▲" if delta > 0 else "▼" if delta < 0 else "="
        status = "IMPROVED" if delta > 0 else "REGRESSED" if delta < 0 else "UNCHANGED"
        print(f"  vs Previous: {prev_parity:.1f}% -> {parity:.1f}% ({indicator} {delta:+.1f}%) {status}")
    
    print(f"\nTier A Pass Rate: {summary['tier_a_pass_rate']*100:.1f}%")
    print(f"Tier B Weighted Mean: {summary['tier_b_weighted_mean']:.1f}%")
    
    print(f"\nIssue Clusters:")
    for cluster, count in sorted(summary['issue_clusters'].items(), key=lambda x: -x[1]):
        prev_count = previous['issue_clusters'].get(cluster, 0) if previous else 0
        delta_str = ""
        if previous:
            delta = count - prev_count
            if delta != 0:
                delta_str = f" ({delta:+d})"
        print(f"  {cluster}: {count}{delta_str}")


def main():
    source_dir = Path("parity-baseline")
    history_dir = Path("parity-history")
    tag = None
    
    # Parse arguments
    args = sys.argv[1:]
    i = 0
    while i < len(args):
        if args[i] == "--tag" and i + 1 < len(args):
            tag = args[i + 1]
            i += 2
        elif args[i] == "--source" and i + 1 < len(args):
            source_dir = Path(args[i + 1])
            i += 2
        elif args[i] == "--history" and i + 1 < len(args):
            history_dir = Path(args[i + 1])
            i += 2
        else:
            i += 1
    
    print(f"Archiving baseline from: {source_dir}")
    print(f"History directory: {history_dir}")
    if tag:
        print(f"Tag: {tag}")
    
    # Archive the run
    run_dir = archive_run(source_dir, history_dir, tag)
    
    if run_dir is None:
        print("Failed to archive run")
        sys.exit(1)
    
    print(f"\nArchived to: {run_dir}")
    
    # Load summary and previous for comparison
    with open(run_dir / "summary.json", encoding="utf-8") as f:
        summary = json.load(f)
    
    previous = None
    prev_run_id = get_previous_run(history_dir, summary['run_id'])
    if prev_run_id:
        prev_summary_path = history_dir / prev_run_id / "summary.json"
        if prev_summary_path.exists():
            with open(prev_summary_path, encoding="utf-8") as f:
                previous = json.load(f)
    
    print_summary(summary, previous)
    
    print(f"\n>>> Run archived successfully <<<")


if __name__ == "__main__":
    main()

