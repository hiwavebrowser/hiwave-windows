#!/usr/bin/env python3
"""
parity_rerun.py - Fast subset rerun for parity iteration

This script:
1. Reads the baseline report to find the worst N cases
2. Re-runs only those cases
3. Generates failure packets for debugging
4. Compares against the baseline to show improvement

Usage:
    python3 scripts/parity_rerun.py [--top N] [--case <case_id>]
"""

import json
import os
import subprocess
import sys
from datetime import datetime
from pathlib import Path
from typing import Dict, List, Any

def load_baseline(baseline_dir: Path = Path("parity-baseline")) -> Dict[str, Any]:
    """Load the most recent baseline report."""
    report_path = baseline_dir / "baseline_report.json"
    if not report_path.exists():
        print(f"No baseline found at {report_path}")
        print("Run `python3 scripts/parity_baseline.py` first")
        sys.exit(1)
    
    with open(report_path, encoding="utf-8") as f:
        return json.load(f)


def get_worst_cases(report: Dict, n: int = 3) -> List[Dict]:
    """Get the N worst cases from the baseline report."""
    all_results = report.get("builtin_results", []) + report.get("websuite_results", [])
    all_results.sort(key=lambda x: x.get("estimated_diff_pct", 100), reverse=True)
    return all_results[:n]


def run_single_case(case: Dict, output_dir: Path) -> Dict[str, Any]:
    """Run a single case and capture results."""
    case_id = case["case_id"]
    html_path = case.get("html_path", "")
    width = case.get("width", 1280)
    height = case.get("height", 800)
    
    frame_path = output_dir / f"{case_id}.ppm"
    layout_path = output_dir / f"{case_id}.layout.json"
    
    cmd = [
        "cargo", "run", "-p", "hiwave-smoke", "--",
        "--html-file", html_path,
        "--width", str(width),
        "--height", str(height),
        "--dump-frame", str(frame_path),
        "--layout-json", str(layout_path),
    ]
    
    try:
        result = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            timeout=60,
            cwd=Path(__file__).parent.parent, encoding="utf-8"
        )
        
        success = result.returncode == 0 and frame_path.exists()
        
        return {
            "case_id": case_id,
            "success": success,
            "frame_path": str(frame_path) if success else None,
            "layout_path": str(layout_path) if layout_path.exists() else None,
            "stdout": result.stdout,
            "stderr": result.stderr,
        }
    except subprocess.TimeoutExpired:
        return {
            "case_id": case_id,
            "success": False,
            "error": "Timeout",
        }


def generate_failure_packet(case: Dict, rerun_result: Dict, output_dir: Path) -> Path:
    """Generate a failure packet for debugging."""
    case_id = case["case_id"]
    packet_dir = output_dir / f"failure-packet-{case_id}"
    packet_dir.mkdir(exist_ok=True)
    
    # Copy frame if available
    if rerun_result.get("frame_path") and Path(rerun_result["frame_path"]).exists():
        import shutil
        shutil.copy(rerun_result["frame_path"], packet_dir / "frame.ppm")
    
    # Copy layout if available
    if rerun_result.get("layout_path") and Path(rerun_result["layout_path"]).exists():
        import shutil
        shutil.copy(rerun_result["layout_path"], packet_dir / "layout.json")
    
    # Save case info
    packet_info = {
        "case_id": case_id,
        "html_path": case.get("html_path"),
        "width": case.get("width"),
        "height": case.get("height"),
        "baseline_diff_pct": case.get("estimated_diff_pct", 100),
        "baseline_issues": case.get("issue_clusters", {}),
        "baseline_layout_stats": case.get("layout_stats", {}),
        "rerun_success": rerun_result.get("success", False),
        "timestamp": datetime.now().isoformat(),
    }
    
    with open(packet_dir / "info.json", "w", encoding="utf-8") as f:
        json.dump(packet_info, f, indent=2)
    
    # Save logs
    if rerun_result.get("stdout"):
        with open(packet_dir / "stdout.log", "w", encoding="utf-8") as f:
            f.write(rerun_result["stdout"])
    
    if rerun_result.get("stderr"):
        with open(packet_dir / "stderr.log", "w", encoding="utf-8") as f:
            f.write(rerun_result["stderr"])
    
    return packet_dir


def main():
    # Parse arguments
    top_n = 3
    specific_case = None
    
    if "--top" in sys.argv:
        idx = sys.argv.index("--top")
        if idx + 1 < len(sys.argv):
            top_n = int(sys.argv[idx + 1])
    
    if "--case" in sys.argv:
        idx = sys.argv.index("--case")
        if idx + 1 < len(sys.argv):
            specific_case = sys.argv[idx + 1]
    
    # Load baseline
    report = load_baseline()
    baseline_metrics = report.get("metrics", {})
    
    print("=" * 60)
    print("Parity Subset Rerun")
    print(f"Baseline parity: {100 - baseline_metrics.get('tier_b_weighted_mean', 100):.1f}%")
    print("=" * 60)
    
    # Determine which cases to run
    if specific_case:
        all_results = report.get("builtin_results", []) + report.get("websuite_results", [])
        cases = [r for r in all_results if r["case_id"] == specific_case]
        if not cases:
            print(f"Case '{specific_case}' not found in baseline")
            sys.exit(1)
    else:
        cases = get_worst_cases(report, top_n)
    
    print(f"\nRunning {len(cases)} case(s):")
    for c in cases:
        print(f"  - {c['case_id']}: baseline diff {c.get('estimated_diff_pct', 100):.1f}%")
    
    # Create output directory
    output_dir = Path("parity-rerun")
    output_dir.mkdir(exist_ok=True)
    captures_dir = output_dir / "captures"
    captures_dir.mkdir(exist_ok=True)
    
    # Run cases
    print("\n--- Running Cases ---")
    results = []
    for case in cases:
        print(f"  {case['case_id']}...", end=" ", flush=True)
        rerun_result = run_single_case(case, captures_dir)
        
        if rerun_result["success"]:
            print("OK")
            # Generate failure packet for analysis
            packet_path = generate_failure_packet(case, rerun_result, output_dir)
            rerun_result["packet_path"] = str(packet_path)
        else:
            print(f"FAIL: {rerun_result.get('error', 'Unknown')[:50]}")
        
        results.append({
            "case": case,
            "rerun": rerun_result,
        })
    
    # Summary
    print("\n" + "=" * 60)
    print("RERUN SUMMARY")
    print("=" * 60)
    
    for r in results:
        case = r["case"]
        rerun = r["rerun"]
        status = "✓" if rerun["success"] else "✗"
        print(f"\n{status} {case['case_id']}")
        print(f"  Baseline diff: {case.get('estimated_diff_pct', 100):.1f}%")
        print(f"  Baseline issues: {case.get('issue_clusters', {})}")
        if rerun.get("packet_path"):
            print(f"  Packet: {rerun['packet_path']}")
    
    # Save rerun report
    rerun_report = {
        "timestamp": datetime.now().isoformat(),
        "baseline_parity": 100 - baseline_metrics.get("tier_b_weighted_mean", 100),
        "cases_run": len(cases),
        "results": results,
    }
    
    with open(output_dir / "rerun_report.json", "w", encoding="utf-8") as f:
        json.dump(rerun_report, f, indent=2, default=str)
    
    print(f"\nRerun report saved to: {output_dir / 'rerun_report.json'}")
    print(f"Failure packets saved to: {output_dir}/failure-packet-*")


if __name__ == "__main__":
    main()



