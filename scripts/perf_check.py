#!/usr/bin/env python3
"""
perf_check.py - Performance budget validation and regression detection

Checks captured perf data against budgets and detects regressions.

Usage:
    python3 scripts/perf_check.py [perf_dir] [--baseline baseline.json] [--update-baseline]
"""

import json
import os
import sys
from datetime import datetime
from pathlib import Path
from typing import Dict, List, Optional, Tuple

PROJECT_DIR = Path(__file__).parent.parent
BUDGETS_FILE = PROJECT_DIR / "config" / "perf_budgets.json"
DEFAULT_PERF_DIR = PROJECT_DIR / "builtins-captures"
DEFAULT_BASELINE_FILE = PROJECT_DIR / "perf_baseline.json"


def load_budgets() -> dict:
    """Load performance budget configuration."""
    with open(BUDGETS_FILE, encoding="utf-8") as f:
        return json.load(f)


def load_perf_files(perf_dir: Path) -> List[Tuple[str, dict]]:
    """Load all .perf.json files from a directory."""
    results = []
    for f in perf_dir.glob("*.perf.json"):
        with open(f, encoding="utf-8") as pf:
            data = json.load(pf)
            page_id = f.stem.replace(".perf", "")
            # Handle both "perf" and "timings" keys for compatibility
            perf = data.get("perf") or data.get("timings", {})
            results.append((page_id, perf))
    return results


def load_baseline(baseline_file: Path) -> Optional[dict]:
    """Load baseline performance data."""
    if baseline_file.exists():
        with open(baseline_file, encoding="utf-8") as f:
            return json.load(f)
    return None


def save_baseline(baseline_file: Path, data: dict):
    """Save baseline performance data."""
    with open(baseline_file, "w", encoding="utf-8") as f:
        json.dump(data, f, indent=2)


def check_budget(metric: str, value: float, budget: dict) -> Tuple[str, str]:
    """Check a metric against its budget.
    
    Returns: (status, message)
    status: 'pass', 'warning', 'fail'
    """
    budget_ms = budget.get("budget_ms", float("inf"))
    warn_threshold = budget.get("warning_threshold", 0.8)
    fail_threshold = budget.get("fail_threshold", 1.5)
    
    ratio = value / budget_ms if budget_ms > 0 else 0
    
    if ratio > fail_threshold:
        return "fail", f"{value:.2f}ms > {budget_ms * fail_threshold:.2f}ms ({ratio:.1f}x budget)"
    elif ratio > warn_threshold:
        return "warning", f"{value:.2f}ms approaching budget ({ratio:.1f}x)"
    else:
        return "pass", f"{value:.2f}ms within budget ({ratio:.1f}x)"


def check_regression(
    metric: str, 
    value: float, 
    baseline_value: Optional[float],
    config: dict
) -> Tuple[str, str]:
    """Check for regression vs baseline.
    
    Returns: (status, message)
    status: 'regression', 'improvement', 'stable', 'new'
    """
    if baseline_value is None:
        return "new", "No baseline"
    
    regression_threshold = config.get("regression_threshold", 1.2)
    improvement_threshold = config.get("improvement_threshold", 0.8)
    
    ratio = value / baseline_value if baseline_value > 0 else 0
    
    if ratio > regression_threshold:
        return "regression", f"Regressed {((ratio - 1) * 100):.1f}% vs baseline"
    elif ratio < improvement_threshold:
        return "improvement", f"Improved {((1 - ratio) * 100):.1f}% vs baseline"
    else:
        return "stable", f"Stable ({ratio:.2f}x baseline)"


def run_checks(
    perf_dir: Path,
    baseline_file: Path,
    update_baseline: bool = False
) -> dict:
    """Run all performance checks.
    
    Returns summary dict.
    """
    budgets = load_budgets()
    baseline = load_baseline(baseline_file)
    perf_data = load_perf_files(perf_dir)
    
    results = {
        "timestamp": datetime.now().isoformat(),
        "perf_dir": str(perf_dir),
        "pages": [],
        "summary": {
            "total_checks": 0,
            "passed": 0,
            "warnings": 0,
            "failures": 0,
            "regressions": 0,
            "improvements": 0
        }
    }
    
    new_baseline = {}
    
    for page_id, perf in perf_data:
        page_result = {
            "page_id": page_id,
            "metrics": []
        }
        
        new_baseline[page_id] = {}
        
        for metric, data in perf.items():
            if isinstance(data, dict) and "avg_ms" in data:
                value = data["avg_ms"]
            else:
                continue
            
            new_baseline[page_id][metric] = value
            
            budget = budgets.get("budgets", {}).get(metric, {})
            budget_status, budget_msg = check_budget(metric, value, budget)
            
            baseline_value = None
            if baseline and page_id in baseline:
                baseline_value = baseline[page_id].get(metric)
            
            regression_status, regression_msg = check_regression(
                metric, value, baseline_value,
                budgets.get("regression_detection", {})
            )
            
            metric_result = {
                "metric": metric,
                "value_ms": round(value, 2),
                "budget_status": budget_status,
                "budget_message": budget_msg,
                "regression_status": regression_status,
                "regression_message": regression_msg
            }
            
            page_result["metrics"].append(metric_result)
            results["summary"]["total_checks"] += 1
            
            if budget_status == "pass":
                results["summary"]["passed"] += 1
            elif budget_status == "warning":
                results["summary"]["warnings"] += 1
            else:
                results["summary"]["failures"] += 1
            
            if regression_status == "regression":
                results["summary"]["regressions"] += 1
            elif regression_status == "improvement":
                results["summary"]["improvements"] += 1
        
        results["pages"].append(page_result)
    
    if update_baseline:
        save_baseline(baseline_file, new_baseline)
        results["baseline_updated"] = True
    
    return results


def print_results(results: dict):
    """Print formatted results to stdout."""
    print("Performance Budget Check")
    print("========================")
    print()
    
    summary = results["summary"]
    print(f"Total checks: {summary['total_checks']}")
    print(f"  Passed:      {summary['passed']}")
    print(f"  Warnings:    {summary['warnings']}")
    print(f"  Failures:    {summary['failures']}")
    print(f"  Regressions: {summary['regressions']}")
    print(f"  Improvements:{summary['improvements']}")
    print()
    
    for page in results["pages"]:
        print(f"[{page['page_id']}]")
        for metric in page["metrics"]:
            status_icon = {
                "pass": "✓",
                "warning": "⚠",
                "fail": "✗"
            }.get(metric["budget_status"], "?")
            
            regression_icon = {
                "regression": "↓",
                "improvement": "↑",
                "stable": "→",
                "new": "•"
            }.get(metric["regression_status"], "?")
            
            print(f"  {status_icon} {metric['metric']}: {metric['value_ms']:.2f}ms")
            print(f"    Budget: {metric['budget_message']}")
            print(f"    {regression_icon} {metric['regression_message']}")
        print()
    
    if results.get("baseline_updated"):
        print("Baseline updated.")
    
    # Return exit code
    if summary["failures"] > 0 or summary["regressions"] > 0:
        return 1
    return 0


def main():
    import argparse
    
    parser = argparse.ArgumentParser(description="Performance budget validation")
    parser.add_argument("perf_dir", nargs="?", default=str(DEFAULT_PERF_DIR),
                        help="Directory containing .perf.json files")
    parser.add_argument("--baseline", default=str(DEFAULT_BASELINE_FILE),
                        help="Baseline file for regression detection")
    parser.add_argument("--update-baseline", action="store_true",
                        help="Update baseline with current values")
    parser.add_argument("--json", action="store_true",
                        help="Output JSON instead of formatted text")
    
    args = parser.parse_args()
    
    results = run_checks(
        Path(args.perf_dir),
        Path(args.baseline),
        args.update_baseline
    )
    
    if args.json:
        print(json.dumps(results, indent=2))
        return 0 if results["summary"]["failures"] == 0 else 1
    else:
        return print_results(results)


if __name__ == "__main__":
    sys.exit(main())

