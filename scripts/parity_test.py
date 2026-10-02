#!/usr/bin/env python3
"""
parity_test.py - Triple-verified parity testing against Chrome baselines

This script:
1. Captures RustKit rendering for each test case
2. Compares against Chrome baselines (pixel diff)
3. Compares computed styles
4. Compares layout rects
5. Generates comprehensive report

Usage:
    python3 scripts/parity_test.py [--scope <scope>] [--case <name>] [--threshold <pct>]
    
Examples:
    python3 scripts/parity_test.py                    # All cases
    python3 scripts/parity_test.py --scope builtins   # Built-ins only
    python3 scripts/parity_test.py --case new_tab     # Single case
    python3 scripts/parity_test.py --threshold 10     # Strict threshold
"""

import json
import os
import subprocess
import sys
import statistics
from datetime import datetime
from pathlib import Path
from typing import Optional, List, Dict, Any

REPO_ROOT = Path(__file__).parent.parent
# Campaign pin (trench/BASELINE-macos.md): Chrome for Testing 148.0.7778.216.
# Override with PARITY_BASELINE_SET only for deliberate cross-version experiments.
BASELINES_DIR = REPO_ROOT / "baselines" / os.environ.get("PARITY_BASELINE_SET", "chrome-148")
OUTPUT_DIR = REPO_ROOT / "parity-baseline"

# Case definitions — SINGLE SOURCE OF TRUTH: cases/registry.json, loaded via
# parity_lib. This file used to carry its own copy of the table; by the time
# the registry was cut the two copies had diverged by two cases
# (gradient-no-radius / gradient-radius-only existed only here). Edit the
# registry, not a table.
sys.path.insert(0, str(Path(__file__).parent))
from parity_lib import BUILTINS, WEBSUITE, MICRO_TESTS  # noqa: E402

# Thresholds live in parity_lib (single source, next to the registry) —
# this file carried a divergence-prone copy until the T6 collapse.
from parity_lib import THRESHOLDS, get_threshold  # noqa: E402, F401

# Finish-line condition 3's iteration count. Cited, not copied — see the
# constant's note in parity_lib.
from parity_lib import STABILITY_MIN_RUNS  # noqa: E402



def _fmt(diff_pct) -> str:
    """None means the instrument refused to measure, not a 100% difference."""
    return "NOT-MEASURED" if diff_pct is None else f"{diff_pct:.2f}%"


def worst_first(results: List[Dict[str, Any]]) -> List[Dict[str, Any]]:
    """Order results the way the 'Worst N' banner claims to: attention-first.

    Unmeasured cases lead — the instrument refusing to measure outranks any diff it did produce
    (#65). Measured cases follow by DESCENDING diff.

    This sorted ASCENDING until 2026-07-29, so the banner printed the three *best* cases under the
    heading "Worst 3 Cases" — on tonight's board, `bg-pure 0.00% / gradients 1.06% / bg-solid 1.61%`
    while the real worst were `gradient-backgrounds 14.44 / gradient-no-radius 13.96 / about 13.14`.
    Harmless on a green board; on a red one it aimed the night's dig at the healthiest pages.
    """
    return sorted(
        results,
        key=lambda r: (r.get("diff_pct") is not None, -(r.get("diff_pct") or 0.0)),
    )


def run_rustkit_capture(case_id: str, html_path: str, width: int, height: int) -> dict:
    """Capture RustKit rendering for a case."""
    output_dir = OUTPUT_DIR / "captures" / case_id
    output_dir.mkdir(parents=True, exist_ok=True)
    
    frame_path = output_dir / "frame.ppm"
    layout_path = output_dir / "layout.json"
    
    # Run capture
    capture_cmd = [
        str(REPO_ROOT / "target" / "release" / ("parity-capture.exe" if os.name == "nt" else "parity-capture")),
        "--html-file", str(REPO_ROOT / html_path),
        "--width", str(width),
        "--height", str(height),
        "--dump-frame", str(frame_path),
        "--dump-layout", str(layout_path),
    ]
    
    try:
        result = subprocess.run(
            capture_cmd,
            capture_output=True,
            text=True,
            timeout=30,
            cwd=REPO_ROOT, encoding="utf-8"
        )
        
        if result.returncode == 0:
            # Check if files were created
            if frame_path.exists():
                return {"success": True, "output_dir": str(output_dir)}
            else:
                return {"success": False, "error": "No frame output"}
        else:
            return {"success": False, "error": result.stderr[:200]}
    except subprocess.TimeoutExpired:
        return {"success": False, "error": "Timeout"}
    except Exception as e:
        return {"success": False, "error": str(e)}


def compare_pixels(
    chrome_png: Path,
    rustkit_ppm: Path,
    output_dir: Path,
    chrome_rects: Optional[Path] = None,
    chrome_styles: Optional[Path] = None,
) -> dict:
    """Compare pixel data using Node.js tool."""
    output_dir.mkdir(parents=True, exist_ok=True)

    chrome_rects_arg = str(chrome_rects) if chrome_rects and chrome_rects.exists() else ""
    chrome_styles_arg = str(chrome_styles) if chrome_styles and chrome_styles.exists() else ""

    cmd = [
        "node", "-e", f"""
import {{ comparePixels }} from './tools/parity_oracle/compare_baseline.mjs';
const result = await comparePixels(
    {json.dumps(str(chrome_png))},
    {json.dumps(str(rustkit_ppm))},
    {json.dumps(str(output_dir))},
    {{
      chromeRectsPath: {json.dumps(chrome_rects_arg)},
      chromeStylesPath: {json.dumps(chrome_styles_arg)},
      attributionTopN: 10,
    }}
);
console.log(JSON.stringify(result));
"""
    ]
    
    try:
        result = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            timeout=30,
            cwd=REPO_ROOT,
            env={**os.environ, "PATH": "/opt/homebrew/bin" + os.pathsep + os.environ.get('PATH', '')}, encoding="utf-8"
        )
        
        if result.returncode == 0:
            for line in result.stdout.strip().split('\n'):
                if line.startswith('{'):
                    return json.loads(line)
        return {"error": result.stderr[:200]}
    except Exception as e:
        return {"error": str(e)}


def compare_styles(chrome_styles: Path, rustkit_styles: Path) -> dict:
    """Compare computed styles."""
    if not chrome_styles.exists():
        return {"error": "Chrome styles not found"}
    if not rustkit_styles.exists():
        return {"error": "RustKit styles not found", "matched": 0, "mismatched": 0}
    
    try:
        chrome = json.loads(chrome_styles.read_text(encoding="utf-8"))
        rustkit = json.loads(rustkit_styles.read_text(encoding="utf-8"))
        
        chrome_map = {e["selector"]: e for e in chrome.get("elements", [])}
        rustkit_map = {e.get("selector", ""): e for e in rustkit.get("elements", [])}
        
        matched = 0
        mismatched = 0
        differences = []
        
        key_props = ["display", "width", "height", "margin-top", "padding-top", "position"]
        
        for selector, chrome_el in chrome_map.items():
            rustkit_el = rustkit_map.get(selector)
            if not rustkit_el:
                mismatched += 1
                continue
            
            diffs = []
            for prop in key_props:
                cv = chrome_el.get("styles", {}).get(prop)
                rv = rustkit_el.get("styles", {}).get(prop)
                if cv != rv:
                    diffs.append({"prop": prop, "chrome": cv, "rustkit": rv})
            
            if diffs:
                mismatched += 1
                differences.append({"selector": selector, "diffs": diffs})
            else:
                matched += 1
        
        return {
            "matched": matched,
            "mismatched": mismatched,
            "differences": differences[:10],  # Top 10
        }
    except Exception as e:
        return {"error": str(e)}


def compare_rects(chrome_rects: Path, rustkit_rects: Path, tolerance: float = 5.0) -> dict:
    """Compare layout rects."""
    if not chrome_rects.exists():
        return {"error": "Chrome rects not found"}
    if not rustkit_rects.exists():
        return {"error": "RustKit rects not found", "matched": 0, "mismatched": 0}
    
    try:
        chrome = json.loads(chrome_rects.read_text(encoding="utf-8"))
        rustkit = json.loads(rustkit_rects.read_text(encoding="utf-8"))
        
        chrome_map = {e["selector"]: e for e in chrome.get("elements", [])}
        rustkit_map = {e.get("selector", ""): e for e in rustkit.get("elements", [])}
        
        matched = 0
        mismatched = 0
        differences = []
        
        for selector, chrome_el in chrome_map.items():
            rustkit_el = rustkit_map.get(selector)
            if not rustkit_el:
                mismatched += 1
                continue
            
            cr = chrome_el.get("rect", {})
            rr = rustkit_el.get("rect", rustkit_el.get("content_rect", {}))
            
            diffs = []
            for prop in ["width", "height", "x", "y"]:
                cv = cr.get(prop, 0)
                rv = rr.get(prop, 0)
                if abs(cv - rv) > tolerance:
                    diffs.append({"prop": prop, "chrome": cv, "rustkit": rv})
            
            if diffs:
                mismatched += 1
                differences.append({"selector": selector, "diffs": diffs})
            else:
                matched += 1
        
        return {
            "matched": matched,
            "mismatched": mismatched,
            "differences": differences[:10],  # Top 10
        }
    except Exception as e:
        return {"error": str(e)}


def run_test(
    case_id: str,
    html_path: str,
    width: int,
    height: int,
    case_type: str,
    iterations: int = 1,
    max_variance: float = 0.10,
) -> dict:
    """Run full triple-verification test for a case."""
    baseline_dir = BASELINES_DIR / case_type / case_id
    capture_dir = OUTPUT_DIR / "captures" / case_id
    diff_dir = OUTPUT_DIR / "diffs" / case_id
    
    result = {
        "case_id": case_id,
        "type": case_type,
        "threshold": get_threshold(case_id),
        "pixel": None,
        "styles": None,
        "rects": None,
        "passed": False,
    }
    
    # Check baseline exists
    chrome_png = baseline_dir / "baseline.png"
    if not chrome_png.exists():
        result["error"] = "No Chrome baseline"
        result["diff_pct"] = None  # refusal, not a measured 100% diff
        return result

    chrome_rects = baseline_dir / "layout-rects.json"
    chrome_styles = baseline_dir / "computed-styles.json"

    run_diffs: List[float] = []
    last_pixel_result: Optional[Dict[str, Any]] = None

    for run_idx in range(iterations):
        # Capture RustKit
        capture_result = run_rustkit_capture(case_id, html_path, width, height)
        if not capture_result.get("success"):
            result["error"] = f"Capture failed: {capture_result.get('error', 'Unknown')}"
            result["diff_pct"] = None  # refusal, not a measured 100% diff
            return result

        # Find RustKit output
        rustkit_ppm = capture_dir / "frame.ppm"
        if not rustkit_ppm.exists():
            result["error"] = "No RustKit capture output"
            result["diff_pct"] = None  # refusal, not a measured 100% diff
            return result

        # 1. Pixel comparison (per-run output)
        run_diff_dir = diff_dir / f"run-{run_idx+1}"
        pixel_result = compare_pixels(chrome_png, rustkit_ppm, run_diff_dir, chrome_rects, chrome_styles)
        last_pixel_result = pixel_result

        # Instrument failure != measurement. See parity_lib.py for the full
        # note; the short version is that the oracle refuses to score a
        # dimension mismatch and every consumer used to ignore the refusal.
        if pixel_result.get("instrumentFailure"):
            result["error"] = f"INSTRUMENT: {pixel_result['instrumentFailure']}"
            result["instrument_failure"] = pixel_result["instrumentFailure"]
            result["diff_pct"] = None
            return result

        if pixel_result.get("error"):
            result["error"] = f"Pixel compare error: {pixel_result.get('error')}"
            result["diff_pct"] = None  # refusal, not a measured 100% diff
            return result

        run_diffs.append(float(pixel_result.get("diffPercent", 100.0)))

    result["pixel_runs"] = run_diffs
    if run_diffs:
        result["diff_pct_median"] = float(statistics.median(run_diffs))
        result["diff_pct_min"] = float(min(run_diffs))
        result["diff_pct_max"] = float(max(run_diffs))
        result["diff_pct_variance"] = float(max(run_diffs) - min(run_diffs))
        # `len(run_diffs)`, not `iterations`: three attempts of which two
        # errored is ONE measurement, and a stability verdict drawn from one
        # measurement is the blank row this campaign refuses to read as green.
        result["measured_runs"] = len(run_diffs)
        result["stable"] = (len(run_diffs) >= STABILITY_MIN_RUNS) and (
            result["diff_pct_variance"] <= max_variance
        )

    # Attach last-run artifacts (diff/heatmap/overlay/attribution) for inspection
    result["pixel"] = last_pixel_result

    # 2. Style comparison
    chrome_styles = baseline_dir / "computed-styles.json"
    rustkit_styles = capture_dir / "computed-styles.json"
    result["styles"] = compare_styles(chrome_styles, rustkit_styles)
    
    # 3. Rect comparison
    chrome_rects = baseline_dir / "layout-rects.json"
    rustkit_rects = capture_dir / "layout.json"
    result["rects"] = compare_rects(chrome_rects, rustkit_rects)
    
    # Determine pass/fail
    diff_pct = result.get("diff_pct_median", last_pixel_result.get("diffPercent", 100) if last_pixel_result else 100)
    result["diff_pct"] = diff_pct
    result["passed"] = diff_pct is not None and diff_pct <= result["threshold"]
    
    return result


def main():
    scope = "all"
    single_case = None
    threshold_override = None
    iterations = 1
    max_variance = 0.10
    output_path = OUTPUT_DIR / "parity_test_results.json"
    
    # Parse arguments
    args = sys.argv[1:]
    i = 0
    while i < len(args):
        if args[i] == "--scope" and i + 1 < len(args):
            scope = args[i + 1]
            i += 2
        elif args[i] == "--case" and i + 1 < len(args):
            single_case = args[i + 1]
            i += 2
        elif args[i] == "--threshold" and i + 1 < len(args):
            threshold_override = float(args[i + 1])
            i += 2
        elif args[i] == "--iterations" and i + 1 < len(args):
            iterations = int(args[i + 1])
            i += 2
        elif args[i] == "--max-variance" and i + 1 < len(args):
            max_variance = float(args[i + 1])
            i += 2
        elif args[i] == "--output" and i + 1 < len(args):
            output_path = Path(args[i + 1])
            i += 2
        elif args[i] in ["-h", "--help"]:
            print(__doc__)
            sys.exit(0)
        else:
            i += 1
    
    print("=" * 60)
    print("Triple-Verified Parity Test")
    print("=" * 60)
    print(f"Baselines: {BASELINES_DIR}")
    print(f"Scope: {scope}")
    print(f"Iterations: {iterations}")
    if iterations >= STABILITY_MIN_RUNS:
        print(f"Stability max variance: {max_variance}%")
    print(f"Timestamp: {datetime.now().isoformat()}")
    print()

    # Build parity-capture once
    build_cmd = ["cargo", "build", "--release", "-p", "parity-capture"]
    build = subprocess.run(build_cmd, capture_output=True, text=True, cwd=REPO_ROOT, encoding="utf-8")
    if build.returncode != 0:
        print("Error: failed to build parity-capture")
        print(build.stderr[:400])
        sys.exit(1)
    
    # Determine cases to run. Scope groups come from the registry; the
    # HOLDOUT scope (test-fidelity T1) is reported as its own number and is
    # NOT part of the 26-case campaign meter — "all" deliberately excludes
    # it so the campaign scoreboard stays comparable across history. Run
    # with --scope holdout (or --scope everything) to include it.
    from parity_lib import _cases_for_scope

    HOLDOUT = _cases_for_scope("holdout")
    scope_groups = [
        ("builtins", BUILTINS),
        ("websuite", WEBSUITE),
        ("micro", MICRO_TESTS),
        ("holdout", HOLDOUT),
    ]
    cases = []
    if single_case:
        for case_type, group in scope_groups:
            for c in group:
                if c[0] == single_case:
                    cases = [(c[0], c[1], c[2], c[3], case_type)]
        if not cases:
            print(f"Error: Unknown case '{single_case}'")
            sys.exit(1)
    else:
        for case_type, group in scope_groups:
            wanted = (
                scope == case_type
                or scope == "everything"
                or (scope == "all" and case_type != "holdout")
            )
            if wanted:
                cases.extend([(c[0], c[1], c[2], c[3], case_type) for c in group])
    
    # Run tests
    results = []
    passed = 0
    failed = 0
    
    for case_id, html_path, width, height, case_type in cases:
        print(f"  Testing {case_id}...", end=" ", flush=True)
        
        result = run_test(
            case_id,
            html_path,
            width,
            height,
            case_type,
            iterations=iterations,
            max_variance=max_variance,
        )
        results.append(result)
        
        if result.get("error"):
            print(f"ERROR: {result['error'][:40]}")
            failed += 1
        elif result["passed"]:
            stable = result.get("stable")
            stable_str = ""
            if iterations >= STABILITY_MIN_RUNS:
                stable_str = " stable" if stable else " UNSTABLE"
            print(f"✓ {_fmt(result.get('diff_pct'))} (threshold: {result['threshold']}%){stable_str}")
            passed += 1
        else:
            stable = result.get("stable")
            stable_str = ""
            if iterations >= STABILITY_MIN_RUNS:
                stable_str = " stable" if stable else " UNSTABLE"
            print(f"✗ {_fmt(result.get('diff_pct'))} (threshold: {result['threshold']}%){stable_str}")
            failed += 1
    
    # Save results
    OUTPUT_DIR.mkdir(parents=True, exist_ok=True)
    output_path.parent.mkdir(parents=True, exist_ok=True)
    with open(output_path, "w", encoding="utf-8") as f:
        json.dump({
            "timestamp": datetime.now().isoformat(),
            "scope": scope,
            "iterations": iterations,
            "max_variance": max_variance,
            "passed": passed,
            "failed": failed,
            "results": results,
        }, f, indent=2)
    
    # Summary
    print()
    print("=" * 60)
    print("Summary")
    print("=" * 60)
    print(f"Passed: {passed}/{len(results)}")
    print(f"Failed: {failed}/{len(results)}")
    
    if results:
        measured = [r["diff_pct"] for r in results if r.get("diff_pct") is not None]
        if measured:
            print(f"Average Diff: {sum(measured) / len(measured):.1f}% "
                  f"({len(measured)}/{len(results)} measured)")
        else:
            print(f"Average Diff: NOT-MEASURED (0/{len(results)} measured)")
    
    print(f"\nResults saved to: {output_path}")
    
    print("\nWorst 3 Cases:")
    for r in worst_first(results)[:3]:
        print(f"  {r['case_id']}: {_fmt(r.get('diff_pct'))}")
    
    sys.exit(0 if failed == 0 else 1)


if __name__ == "__main__":
    main()

