#!/usr/bin/env python3
"""
Extract parity metrics from test results for tracking over time.

Usage:
    python3 extract_parity_metrics.py --input parity_test_results.json --output metrics.json
    python3 extract_parity_metrics.py --input parity_test_results.json --format markdown
"""

import argparse
import json
import sys
from datetime import datetime
from pathlib import Path


def extract_metrics(results: dict, commit: str = "", branch: str = "") -> dict:
    """Extract key metrics from parity test results."""

    tests = []
    total_diff = 0.0
    measured_count = 0

    for result in results.get("results", []):
        case_id = result.get("case_id", "unknown")
        threshold = result.get("threshold", 15)

        # Three states, not two. A case is MEASURED (a real diff), FAILED
        # (measured as a total mismatch), or NOT MEASURED (the instrument
        # refused — dimension mismatch, blank frame, missing baseline).
        #
        # The third used to be folded into the second: an instrument failure
        # was recorded as diff 100.0 and then averaged into the campaign
        # number as though it were a render result. That is how the nightly
        # gate reported "avg diff 73.36%" for a tree measuring 6.75 on the
        # same cases — the average was mostly instrument, not renderer.
        pixel_data = result.get("pixel") or {}
        instrument_failure = (
            result.get("instrument_failure")
            or pixel_data.get("instrumentFailure")
        )

        raw_diff = pixel_data.get("diffPercent", result.get("diff_pct"))

        # An error IS a refusal, whatever number came with it. parity_test.py
        # historically stamped diff_pct=100.0 alongside "Capture failed" — a
        # timeout is not a 100% render diff, it is an absence of measurement,
        # and treating it as a score is the same lie in a different costume.
        not_measured = (
            instrument_failure is not None
            or raw_diff is None
            or result.get("error") is not None
        )
        diff_percent = None if not_measured else float(raw_diff)

        # Not-measured never passes, and never contributes to the average.
        passed = (
            not not_measured
            and diff_percent <= threshold
            and not result.get("error")
        )

        tests.append({
            "name": case_id,
            "diff": diff_percent,
            "threshold": threshold,
            "passed": passed,
            "type": result.get("type", "unknown"),
            "error": result.get("error"),
            "not_measured": not_measured,
            "instrument_failure": instrument_failure,
        })

        if not not_measured:
            total_diff += diff_percent
            measured_count += 1

    # Calculate summary metrics
    total = len(tests)
    passed_count = sum(1 for t in tests if t["passed"])
    failed_count = total - passed_count
    not_measured_count = sum(1 for t in tests if t["not_measured"])

    # Average over MEASURED cases only. Averaging instrument failures in as
    # 100.0 was what turned a 6.75 tree into a reported 73.36.
    avg_diff = total_diff / measured_count if measured_count > 0 else None

    # Worst case among measured cases; None sorts nowhere.
    measured = [t for t in tests if not t["not_measured"]]
    worst = max(measured, key=lambda t: t["diff"]) if measured else {"name": "none", "diff": None}

    # Sort: unmeasured first (they need attention most), then worst diff.
    tests.sort(key=lambda t: (not t["not_measured"], -(t["diff"] or 0.0)))

    return {
        "timestamp": results.get("timestamp", datetime.utcnow().isoformat()),
        "commit": commit,
        "branch": branch,
        "total": total,
        "passed": passed_count,
        "failed": failed_count,
        # Surfaced so a reader can tell "the renderer is bad" from "the
        # instrument did not run". A gate that hides this distinction is
        # decorative.
        "not_measured": not_measured_count,
        "measured": measured_count,
        "average_diff": avg_diff,
        "worst_case": {
            "name": worst["name"],
            "diff": worst["diff"]
        },
        "tests": tests
    }


def format_markdown(metrics: dict) -> str:
    """Format metrics as GitHub-flavored markdown."""

    lines = [
        "# 📊 Parity Test Metrics",
        "",
        f"**Timestamp:** {metrics['timestamp']}",
        f"**Commit:** `{metrics['commit'][:8] if metrics['commit'] else 'N/A'}`",
        f"**Branch:** `{metrics['branch'] or 'N/A'}`",
        "",
        "## Summary",
        "",
        "| Metric | Value |",
        "|--------|-------|",
        f"| **Average Diff** | {metrics['average_diff']:.2f}% |",
        f"| **Passed** | {metrics['passed']}/{metrics['total']} |",
        f"| **Failed** | {metrics['failed']}/{metrics['total']} |",
        f"| **Pass Rate** | {100 * metrics['passed'] / metrics['total']:.1f}% |" if metrics['total'] > 0 else "| **Pass Rate** | N/A |",
        f"| **Worst Case** | {metrics['worst_case']['name']} ({metrics['worst_case']['diff']:.2f}%) |",
        "",
        "## All Tests",
        "",
        "| Status | Test | Diff % | Threshold |",
        "|--------|------|--------|-----------|",
    ]

    for test in metrics["tests"]:
        status = "✅" if test["passed"] else "❌"
        lines.append(f"| {status} | {test['name']} | {test['diff']:.2f}% | {test['threshold']}% |")

    # Add a trend indicator section if we have historical context
    lines.extend([
        "",
        "---",
        "",
        "### Metrics by Category",
        "",
    ])

    # Group by type
    by_type = {}
    for test in metrics["tests"]:
        t = test.get("type", "unknown")
        if t not in by_type:
            by_type[t] = {"tests": [], "total_diff": 0}
        by_type[t]["tests"].append(test)
        by_type[t]["total_diff"] += test["diff"]

    lines.append("| Category | Count | Avg Diff | Passed |")
    lines.append("|----------|-------|----------|--------|")

    for category, data in sorted(by_type.items()):
        count = len(data["tests"])
        avg = data["total_diff"] / count if count > 0 else 0
        passed = sum(1 for t in data["tests"] if t["passed"])
        lines.append(f"| {category} | {count} | {avg:.2f}% | {passed}/{count} |")

    return "\n".join(lines)


def main():
    parser = argparse.ArgumentParser(description="Extract parity metrics from test results")
    parser.add_argument("--input", "-i", required=True, help="Path to parity_test_results.json")
    parser.add_argument("--output", "-o", help="Path to output metrics JSON file")
    parser.add_argument("--format", "-f", choices=["json", "markdown"], default="json",
                        help="Output format (default: json)")
    parser.add_argument("--commit", default="", help="Git commit SHA")
    parser.add_argument("--branch", default="", help="Git branch name")

    args = parser.parse_args()

    # Read input
    input_path = Path(args.input)
    if not input_path.exists():
        print(f"Error: Input file not found: {args.input}", file=sys.stderr)
        sys.exit(1)

    with open(input_path, encoding="utf-8") as f:
        results = json.load(f)

    # Extract metrics
    metrics = extract_metrics(results, args.commit, args.branch)

    # Output
    if args.format == "markdown":
        output = format_markdown(metrics)
        if args.output:
            with open(args.output, "w", encoding="utf-8") as f:
                f.write(output)
        else:
            print(output)
    else:
        output = json.dumps(metrics, indent=2)
        if args.output:
            with open(args.output, "w", encoding="utf-8") as f:
                f.write(output)
        else:
            print(output)

    # Return exit code based on pass rate
    # This can be used for CI gating
    if metrics["failed"] > 0:
        sys.exit(0)  # Don't fail the build, just report

    sys.exit(0)


if __name__ == "__main__":
    main()
