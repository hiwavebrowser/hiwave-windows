#!/usr/bin/env python3
"""
parity_baseline.py - Capture baseline parity metrics and cluster failures

This script:
1. Runs RustKit capture for all built-ins + websuite cases
2. Computes per-case pixel diff % (simulated if no Chromium baseline available)
3. Computes weighted tiered metrics (built-ins 60%, websuite 40%)
4. Clusters failures into: sizing/layout, paint, text, images
5. Saves a reproducible baseline report
6. Auto-archives to parity-history/ for tracking progress

Usage:
    python3 scripts/parity_baseline.py [--tag <name>] [--output-dir <dir>]
    
Examples:
    python3 scripts/parity_baseline.py
    python3 scripts/parity_baseline.py --tag "after-flex-fix"
    python3 scripts/parity_baseline.py --no-archive
    python3 scripts/parity_baseline.py --gpu  # Requires display/GPU
    python3 scripts/parity_baseline.py --case css-selectors  # Single case
    
Options:
    --gpu           Use GPU-based capture (requires display). 
                    NOTE: GPU capture requires running outside of sandboxed environments.
                    If running from Cursor/IDE, ensure the terminal has full disk/GPU access.
    --case <name>   Run only a single case (for fast iteration).
    --tag <name>    Tag this run with a name for easier identification.
    --output-dir    Output directory for captures and reports.
    --no-archive    Skip auto-archiving to parity-history/.
    --release       Use release build (default: release).
    --debug         Use debug build instead of release.
"""

import json
import os
import shutil
import subprocess
import sys
from datetime import datetime
from pathlib import Path
from typing import Dict, List, Any, Tuple, Optional

# Import archive functions
from parity_archive import archive_run, get_previous_run, extract_summary

# Configuration
BUILTINS_WEIGHT = 0.60
WEBSUITE_WEIGHT = 0.40
TIER_A_THRESHOLD = 25  # Start with 25% diff threshold

# Built-in pages (60% weight)
BUILTINS = [
    ("new_tab", "crates/hiwave-app/src/ui/new_tab.html", 1280, 800),
    ("about", "crates/hiwave-app/src/ui/about.html", 800, 600),
    ("settings", "crates/hiwave-app/src/ui/settings.html", 1024, 768),
    ("chrome_rustkit", "crates/hiwave-app/src/ui/chrome_rustkit.html", 1280, 100),
    ("shelf", "crates/hiwave-app/src/ui/shelf.html", 1280, 120),
]

# Websuite cases (40% weight)
WEBSUITE = [
    ("article-typography", "websuite/cases/article-typography/index.html", 1280, 800),
    ("card-grid", "websuite/cases/card-grid/index.html", 1280, 800),
    ("css-selectors", "websuite/cases/css-selectors/index.html", 800, 1200),
    ("flex-positioning", "websuite/cases/flex-positioning/index.html", 800, 1000),
    ("form-elements", "websuite/cases/form-elements/index.html", 800, 600),
    ("gradient-backgrounds", "websuite/cases/gradient-backgrounds/index.html", 800, 600),
    ("image-gallery", "websuite/cases/image-gallery/index.html", 1280, 800),
    ("sticky-scroll", "websuite/cases/sticky-scroll/index.html", 1280, 800),
]


def run_rustkit_capture(
    case_id: str, 
    html_path: str, 
    width: int, 
    height: int, 
    output_dir: Path,
    use_gpu: bool = False,
    use_release: bool = True,
) -> Dict[str, Any]:
    """Run parity-capture to capture a frame and layout tree.
    
    Args:
        case_id: Identifier for this test case
        html_path: Path to HTML file to render
        width: Viewport width
        height: Viewport height
        output_dir: Directory to save outputs
        use_gpu: If True, use GPU-based capture (requires display). 
                 If False, use headless capture (may fail without GPU adapter).
        use_release: If True, use release build. If False, use debug.
    """
    frame_path = output_dir / f"{case_id}.ppm"
    layout_path = output_dir / f"{case_id}.layout.json"
    
    # Build command based on capture mode
    build_mode = "--release" if use_release else ""
    
    if use_gpu:
        # GPU mode: Use parity-capture but it should find GPU when display is available
        # The compositor now automatically falls back to software if hardware unavailable
        cmd = [
            "cargo", "run", "-p", "parity-capture",
        ]
        if use_release:
            cmd.insert(2, "--release")
        cmd.extend([
            "--",
            "--html-file", html_path,
            "--width", str(width),
            "--height", str(height),
            "--dump-frame", str(frame_path),
            "--dump-layout", str(layout_path),
            "-v",  # Verbose mode to see GPU adapter info
        ])
    else:
        # Headless mode: Use parity-capture (may fail without GPU adapter)
        cmd = [
            "cargo", "run", "-p", "parity-capture",
        ]
        if use_release:
            cmd.insert(2, "--release")
        cmd.extend([
            "--",
            "--html-file", html_path,
            "--width", str(width),
            "--height", str(height),
            "--dump-frame", str(frame_path),
            "--dump-layout", str(layout_path),
        ])
    
    try:
        result = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            timeout=120,
            cwd=Path(__file__).parent.parent,
        )
        
        # Parse JSON result from stdout (last line)
        capture_result = {}
        for line in result.stdout.strip().split('\n'):
            if line.startswith('{') and '"status"' in line:
                try:
                    capture_result = json.loads(line)
                except json.JSONDecodeError:
                    pass
        
        success = capture_result.get("status") == "ok" and frame_path.exists()
        
        # Determine error message
        error_msg = None
        if not success:
            error_msg = capture_result.get("error")
            if not error_msg:
                # No JSON error, check stderr or exit code
                if result.returncode != 0:
                    # Get last few lines of stderr
                    stderr_lines = result.stderr.strip().split('\n')[-3:]
                    error_msg = '; '.join(line.strip() for line in stderr_lines if line.strip())
                if not error_msg:
                    error_msg = f"Process exited with code {result.returncode}"
            
            # Suggest --gpu flag for GPU errors
            if error_msg and "GPU" in error_msg:
                error_msg += " (Try running with --gpu flag if you have a display)"
        
        return {
            "case_id": case_id,
            "html_path": html_path,
            "width": width,
            "height": height,
            "success": success,
            "frame_path": str(frame_path) if success else None,
            "layout_path": str(layout_path) if layout_path.exists() else None,
            "layout_stats": capture_result.get("layout_stats"),
            "perf": {},
            "error": error_msg,
            "capture_mode": "gpu" if use_gpu else "headless",
        }
    except subprocess.TimeoutExpired:
        return {
            "case_id": case_id,
            "success": False,
            "error": "Timeout after 120s",
        }
    except Exception as e:
        return {
            "case_id": case_id,
            "success": False,
            "error": str(e),
        }


def analyze_layout(layout_path: str) -> Dict[str, Any]:
    """Analyze a layout tree JSON for issues.
    
    Uses AREA-ONLY zero detection: only counts boxes with zero area (w*h==0)
    as problematic, and excludes known-benign node types like text runs and
    inline elements with height but no width.
    """
    if not layout_path or not Path(layout_path).exists():
        return {"error": "No layout file"}
    
    with open(layout_path, encoding="utf-8") as f:
        data = json.load(f)
    
    stats = {
        "total_boxes": 0,
        "positioned": 0,
        "sized": 0,
        "zero_area": 0,           # Boxes with w*h == 0 (true zero area)
        "zero_width_only": 0,     # Boxes with w==0 but h>0 (often benign)
        "zero_height_only": 0,    # Boxes with h==0 but w>0 (often benign)
        "zero_size": 0,           # Legacy: any w==0 or h==0 (for comparison)
        "at_origin": 0,
        "form_controls": 0,
        "text_boxes": 0,
        "block_boxes": 0,
        "inline_boxes": 0,
        "image_boxes": 0,
        "issues": [],
    }
    
    # Node types that are benign when zero-width (e.g., text runs, inline wrappers)
    BENIGN_ZERO_WIDTH_TYPES = {"text", "text_run", "inline", "anonymous_inline", "anonymous_block"}
    # Node types that indicate real layout problems when zero-area
    PROBLEMATIC_TYPES = {"block", "form_control", "image", "replaced", "flex_item", "grid_item"}
    
    def walk(node, depth=0):
        rect = node.get("content_rect") or node.get("rect", {})
        x, y = rect.get("x", 0), rect.get("y", 0)
        w, h = rect.get("width", 0), rect.get("height", 0)
        node_type = node.get("type", "unknown")
        box_type = node.get("box_type", node_type)  # Some layouts use box_type
        effective_type = box_type if box_type != "unknown" else node_type
        
        stats["total_boxes"] += 1
        
        # Positioning check
        if x != 0 or y != 0:
            stats["positioned"] += 1
        else:
            stats["at_origin"] += 1
        
        # Area-based sizing check (Phase A fix)
        area = w * h
        has_width = w > 0
        has_height = h > 0
        
        if has_width and has_height:
            stats["sized"] += 1
        else:
            # Legacy counter for comparison
            stats["zero_size"] += 1
            
            # More nuanced zero detection
            if area == 0:
                if not has_width and not has_height:
                    stats["zero_area"] += 1
                elif not has_width:
                    stats["zero_width_only"] += 1
                else:
                    stats["zero_height_only"] += 1
            
            # Only flag as issue if it's a problematic type with zero area
            # and not a known-benign node
            is_benign = effective_type.lower() in BENIGN_ZERO_WIDTH_TYPES
            is_problematic = effective_type.lower() in PROBLEMATIC_TYPES
            
            if depth > 1 and not is_benign and (is_problematic or area == 0):
                stats["issues"].append({
                    "type": "zero_area" if area == 0 else "zero_dimension",
                    "node_type": effective_type,
                    "depth": depth,
                    "width": w,
                    "height": h,
                })
        
        # Type counting
        type_lower = effective_type.lower()
        if type_lower in ["form_control", "input", "button", "select", "textarea"]:
            stats["form_controls"] += 1
        elif type_lower in ["text", "text_run"]:
            stats["text_boxes"] += 1
        elif type_lower in ["block", "div"]:
            stats["block_boxes"] += 1
        elif type_lower in ["inline", "span"]:
            stats["inline_boxes"] += 1
        elif type_lower in ["image", "img", "replaced"]:
            stats["image_boxes"] += 1
        
        for child in node.get("children", []):
            walk(child, depth + 1)
    
    if "root" in data:
        walk(data["root"])
    
    # Compute sizing rate using area-based metric
    stats["sizing_rate"] = stats["sized"] / max(1, stats["total_boxes"])
    stats["positioning_rate"] = stats["positioned"] / max(1, stats["total_boxes"])
    
    # Also compute an "area sizing rate" that's more lenient
    # (counts zero-width-only as sized if type is benign)
    benign_zeros = stats["zero_width_only"]  # These are often text runs with height
    stats["area_sizing_rate"] = (stats["sized"] + benign_zeros) / max(1, stats["total_boxes"])
    
    return stats


def classify_issues(layout_stats: Dict[str, Any]) -> Dict[str, int]:
    """Classify issues into buckets based on node types and issue patterns."""
    clusters = {
        "sizing_layout": 0,
        "paint": 0,
        "text": 0,
        "images": 0,
    }
    
    for issue in layout_stats.get("issues", []):
        issue_type = issue.get("type", "")
        node_type = issue.get("node_type", "").lower()
        
        # Only count actual zero-area issues as problems
        if issue_type in ["zero_area", "zero_dimension"]:
            if node_type in ["form_control", "input", "button", "select", "textarea", 
                            "block", "div", "flex_item", "grid_item"]:
                clusters["sizing_layout"] += 1
            elif node_type in ["text", "text_run"]:
                clusters["text"] += 1
            elif node_type in ["image", "img", "replaced"]:
                clusters["images"] += 1
            else:
                # Unknown types default to sizing_layout
                clusters["sizing_layout"] += 1
    
    # Additional heuristics based on aggregate stats
    zero_area = layout_stats.get("zero_area", 0)
    if zero_area > 10:
        # Many true zero-area boxes suggest layout engine issues
        clusters["sizing_layout"] += max(0, zero_area - 10)
    
    return clusters


def estimate_diff_percent(layout_stats: Dict[str, Any]) -> float:
    """
    Estimate pixel diff % based on layout analysis.
    
    This is a heuristic until we have actual Chromium baselines.
    Uses AREA-BASED sizing rate for more accurate estimation.
    
    The formula weights:
    - 50% area_sizing_rate (lenient, counts benign zero-width as sized)
    - 30% strict sizing_rate
    - 20% positioning_rate
    
    Penalties are applied only for TRUE zero-area boxes, not benign zeros.
    """
    if "error" in layout_stats:
        return 100.0
    
    # Use area-based sizing rate if available, fall back to strict
    area_sizing_rate = layout_stats.get("area_sizing_rate", layout_stats.get("sizing_rate", 0))
    sizing_rate = layout_stats.get("sizing_rate", 0)
    positioning_rate = layout_stats.get("positioning_rate", 0)
    
    # Base diff estimate using blended rates
    base_diff = 100 * (1 - (
        area_sizing_rate * 0.50 +  # Most lenient
        sizing_rate * 0.30 +       # Strict
        positioning_rate * 0.20    # Position matters too
    ))
    
    # Penalty only for TRUE zero-area boxes (not benign zero-width)
    zero_area = layout_stats.get("zero_area", 0)
    zero_penalty = min(20, zero_area * 1.5)  # Reduced from 2x multiplier
    
    # Small penalty for problematic issues (true layout failures)
    issues = layout_stats.get("issues", [])
    issue_penalty = min(10, len(issues) * 0.5)
    
    # Clamp to 0-100
    return min(100, max(0, base_diff + zero_penalty + issue_penalty))


def compute_weighted_metrics(
    builtin_results: List[Dict],
    websuite_results: List[Dict],
) -> Dict[str, Any]:
    """Compute weighted tiered metrics."""
    
    def get_diffs(results):
        return [r.get("estimated_diff_pct", 100) for r in results]
    
    builtin_diffs = get_diffs(builtin_results)
    websuite_diffs = get_diffs(websuite_results)
    
    # Tier A: Pass rate under threshold
    builtin_pass = sum(1 for d in builtin_diffs if d <= TIER_A_THRESHOLD) / max(1, len(builtin_diffs))
    websuite_pass = sum(1 for d in websuite_diffs if d <= TIER_A_THRESHOLD) / max(1, len(websuite_diffs))
    
    weighted_pass_rate = builtin_pass * BUILTINS_WEIGHT + websuite_pass * WEBSUITE_WEIGHT
    
    # Tier B: Median diff
    all_diffs = builtin_diffs + websuite_diffs
    all_diffs.sort()
    median_diff = all_diffs[len(all_diffs) // 2] if all_diffs else 100
    
    # Weighted median (approximate)
    weighted_median = (
        (sum(builtin_diffs) / max(1, len(builtin_diffs))) * BUILTINS_WEIGHT +
        (sum(websuite_diffs) / max(1, len(websuite_diffs))) * WEBSUITE_WEIGHT
    )
    
    # Top 3 worst cases
    all_results = [(r, "builtin") for r in builtin_results] + [(r, "websuite") for r in websuite_results]
    all_results.sort(key=lambda x: x[0].get("estimated_diff_pct", 100), reverse=True)
    worst_3 = [
        {"case_id": r["case_id"], "type": t, "diff_pct": r.get("estimated_diff_pct", 100)}
        for r, t in all_results[:3]
    ]
    
    return {
        "tier_a_threshold": TIER_A_THRESHOLD,
        "tier_a_pass_rate": weighted_pass_rate,
        "tier_a_builtin_pass": builtin_pass,
        "tier_a_websuite_pass": websuite_pass,
        "tier_b_median_diff": median_diff,
        "tier_b_weighted_mean": weighted_median,
        "worst_3_cases": worst_3,
        "builtin_mean_diff": sum(builtin_diffs) / max(1, len(builtin_diffs)),
        "websuite_mean_diff": sum(websuite_diffs) / max(1, len(websuite_diffs)),
    }


def get_all_cases() -> Dict[str, Tuple[str, str, int, int, str]]:
    """Return all cases as a dict keyed by case_id."""
    cases = {}
    for case_id, html_path, width, height in BUILTINS:
        cases[case_id] = (case_id, html_path, width, height, "builtin")
    for case_id, html_path, width, height in WEBSUITE:
        cases[case_id] = (case_id, html_path, width, height, "websuite")
    return cases


def run_oracle(cases: List[str], output_dir: Path, scope: str = "top") -> Optional[Dict]:
    """Run the Chromium oracle to capture baselines and compare pixels.
    
    Returns oracle results dict or None if oracle not available.
    """
    oracle_script = Path(__file__).parent.parent / "tools" / "parity_oracle" / "run_oracle.mjs"
    
    if not oracle_script.exists():
        print("  Oracle not available (run: cd tools/parity_oracle && npm install)")
        return None
    
    # Check if npm dependencies are installed
    node_modules = oracle_script.parent / "node_modules"
    if not node_modules.exists():
        print("  Oracle dependencies not installed (run: cd tools/parity_oracle && npm install)")
        return None
    
    try:
        # Run oracle full pipeline
        cmd = [
            "node", str(oracle_script), "full",
            "--scope", scope,
            "--output", str(output_dir),
        ]
        
        result = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            timeout=300,  # 5 min timeout for full oracle
            cwd=Path(__file__).parent.parent,
        )
        
        if result.returncode != 0:
            print(f"  Oracle failed: {result.stderr[:100]}")
            return None
        
        # Load oracle results
        oracle_results_path = output_dir / "oracle_results.json"
        if oracle_results_path.exists():
            with open(oracle_results_path, encoding="utf-8") as f:
                return json.load(f)
        
        return None
    except subprocess.TimeoutExpired:
        print("  Oracle timed out after 5 minutes")
        return None
    except Exception as e:
        print(f"  Oracle error: {e}")
        return None


def main():
    output_dir = Path("parity-baseline")
    history_dir = Path("parity-history")
    tag = None
    auto_archive = True
    use_gpu = False
    use_release = True
    single_case = None  # --case flag for single case iteration
    use_oracle = False  # --oracle flag for Chromium comparison
    oracle_scope = "top"  # --oracle-scope: top, builtins, websuite, all
    
    # Parse arguments
    args = sys.argv[1:]
    i = 0
    while i < len(args):
        if args[i] == "--output-dir" and i + 1 < len(args):
            output_dir = Path(args[i + 1])
            i += 2
        elif args[i] == "--tag" and i + 1 < len(args):
            tag = args[i + 1]
            i += 2
        elif args[i] == "--history" and i + 1 < len(args):
            history_dir = Path(args[i + 1])
            i += 2
        elif args[i] == "--case" and i + 1 < len(args):
            single_case = args[i + 1]
            i += 2
        elif args[i] == "--oracle":
            use_oracle = True
            # Check if next arg is the oracle type (chromium)
            if i + 1 < len(args) and not args[i + 1].startswith("--"):
                oracle_type = args[i + 1]  # Currently only "chromium" supported
                i += 2
            else:
                i += 1
        elif args[i] == "--oracle-scope" and i + 1 < len(args):
            oracle_scope = args[i + 1]
            i += 2
        elif args[i] == "--no-archive":
            auto_archive = False
            i += 1
        elif args[i] == "--gpu":
            use_gpu = True
            i += 1
        elif args[i] == "--release":
            use_release = True
            i += 1
        elif args[i] == "--debug":
            use_release = False
            i += 1
        elif args[i] in ["-h", "--help"]:
            print(__doc__)
            sys.exit(0)
        else:
            i += 1
    
    # Validate single_case if provided
    all_cases = get_all_cases()
    if single_case:
        if single_case not in all_cases:
            print(f"Error: Unknown case '{single_case}'")
            print(f"Available cases: {', '.join(sorted(all_cases.keys()))}")
            sys.exit(1)
        auto_archive = False  # Don't archive single-case runs
    
    output_dir.mkdir(parents=True, exist_ok=True)
    captures_dir = output_dir / "captures"
    captures_dir.mkdir(exist_ok=True)
    
    print("=" * 60)
    print("Parity Baseline Capture")
    print(f"Output: {output_dir}")
    if tag:
        print(f"Tag: {tag}")
    if single_case:
        print(f"Single Case Mode: {single_case}")
    print(f"Capture Mode: {'GPU (requires display)' if use_gpu else 'Headless'}")
    print(f"Build: {'Release' if use_release else 'Debug'}")
    if use_oracle:
        print(f"Oracle: Chromium (scope: {oracle_scope})")
    print(f"Timestamp: {datetime.now().isoformat()}")
    print("=" * 60)
    
    def capture_case(case_id: str, html_path: str, width: int, height: int) -> Dict[str, Any]:
        """Capture a single case and analyze it."""
        print(f"  Capturing {case_id}...", end=" ", flush=True)
        result = run_rustkit_capture(case_id, html_path, width, height, captures_dir, use_gpu, use_release)
        
        if result["success"]:
            # ALWAYS analyze the layout JSON for accurate clustering
            # (capture result's layout_stats may lack per-node detail)
            layout_path = result.get("layout_path")
            layout_stats = analyze_layout(layout_path) if layout_path else result.get("layout_stats", {})
            
            # Merge with any stats from capture (e.g., perf data)
            if result.get("layout_stats"):
                for key in ["total_boxes", "sized", "zero_size"]:
                    if key not in layout_stats and key in result["layout_stats"]:
                        layout_stats[key] = result["layout_stats"][key]
            
            result["layout_stats"] = layout_stats
            result["estimated_diff_pct"] = estimate_diff_percent(layout_stats)
            result["issue_clusters"] = classify_issues(layout_stats)
            
            # Print detailed stats
            area_rate = layout_stats.get('area_sizing_rate', layout_stats.get('sizing_rate', 0))
            zero_area = layout_stats.get('zero_area', 0)
            print(f"OK (area_sizing: {area_rate*100:.1f}%, zero_area: {zero_area}, est. diff: {result['estimated_diff_pct']:.1f}%)")
        else:
            result["estimated_diff_pct"] = 100
            result["issue_clusters"] = {"sizing_layout": 1}
            error_msg = result.get('error') or 'Unknown error'
            print(f"FAIL: {error_msg[:50]}")
        
        return result
    
    # Filter cases if single_case mode
    builtins_to_run = BUILTINS
    websuite_to_run = WEBSUITE
    
    if single_case:
        case_info = all_cases[single_case]
        case_type = case_info[4]  # "builtin" or "websuite"
        if case_type == "builtin":
            builtins_to_run = [(case_info[0], case_info[1], case_info[2], case_info[3])]
            websuite_to_run = []
        else:
            builtins_to_run = []
            websuite_to_run = [(case_info[0], case_info[1], case_info[2], case_info[3])]
    
    # Capture built-ins
    builtin_results = []
    if builtins_to_run:
        print("\n--- Built-in Pages (60% weight) ---")
        for case_id, html_path, width, height in builtins_to_run:
            result = capture_case(case_id, html_path, width, height)
            builtin_results.append(result)
    
    # Capture websuite
    websuite_results = []
    if websuite_to_run:
        print("\n--- Websuite Cases (40% weight) ---")
        for case_id, html_path, width, height in websuite_to_run:
            result = capture_case(case_id, html_path, width, height)
            websuite_results.append(result)
    
    # Run Chromium oracle if requested
    oracle_results = None
    if use_oracle:
        print("\n--- Running Chromium Oracle ---")
        oracle_results = run_oracle(
            [r["case_id"] for r in builtin_results + websuite_results if r.get("success")],
            output_dir,
            oracle_scope,
        )
        
        if oracle_results:
            oracle_cases = oracle_results.get("cases", {})
            # Update results with oracle pixel diff (ground truth)
            for result in builtin_results + websuite_results:
                case_id = result["case_id"]
                if case_id in oracle_cases and oracle_cases[case_id].get("success"):
                    oracle_diff = oracle_cases[case_id].get("diff_pct", 100)
                    heuristic_diff = result.get("estimated_diff_pct", 100)
                    
                    # Store both for comparison
                    result["oracle_diff_pct"] = oracle_diff
                    result["heuristic_diff_pct"] = heuristic_diff
                    result["diff_source"] = "oracle"
                    
                    # Use oracle as primary diff (ground truth)
                    result["estimated_diff_pct"] = oracle_diff
                    
                    print(f"  {case_id}: oracle={oracle_diff:.1f}% (heuristic={heuristic_diff:.1f}%)")
    
    # Compute metrics
    print("\n--- Computing Weighted Tiered Metrics ---")
    metrics = compute_weighted_metrics(builtin_results, websuite_results)
    
    # Aggregate issue clusters
    total_clusters = {"sizing_layout": 0, "paint": 0, "text": 0, "images": 0}
    for r in builtin_results + websuite_results:
        for k, v in r.get("issue_clusters", {}).items():
            total_clusters[k] += v
    
    # Build report
    report = {
        "timestamp": datetime.now().isoformat(),
        "config": {
            "builtins_weight": BUILTINS_WEIGHT,
            "websuite_weight": WEBSUITE_WEIGHT,
            "tier_a_threshold": TIER_A_THRESHOLD,
            "oracle_enabled": use_oracle,
            "oracle_scope": oracle_scope if use_oracle else None,
        },
        "metrics": metrics,
        "issue_clusters": total_clusters,
        "builtin_results": builtin_results,
        "websuite_results": websuite_results,
        "oracle_results": oracle_results,
    }
    
    # Save report
    report_path = output_dir / "baseline_report.json"
    with open(report_path, "w", encoding="utf-8") as f:
        json.dump(report, f, indent=2, default=str)
    
    # Print summary
    print("\n" + "=" * 60)
    print("BASELINE SUMMARY")
    print("=" * 60)
    print(f"\nTier A (Pass Rate @ {TIER_A_THRESHOLD}% threshold):")
    print(f"  Weighted: {metrics['tier_a_pass_rate']*100:.1f}%")
    print(f"  Built-ins: {metrics['tier_a_builtin_pass']*100:.1f}%")
    print(f"  Websuite: {metrics['tier_a_websuite_pass']*100:.1f}%")
    
    print(f"\nTier B (Diff %):")
    print(f"  Median: {metrics['tier_b_median_diff']:.1f}%")
    print(f"  Weighted Mean: {metrics['tier_b_weighted_mean']:.1f}%")
    print(f"  Built-in Mean: {metrics['builtin_mean_diff']:.1f}%")
    print(f"  Websuite Mean: {metrics['websuite_mean_diff']:.1f}%")
    
    print(f"\nWorst 3 Cases:")
    for w in metrics["worst_3_cases"]:
        print(f"  {w['case_id']} ({w['type']}): {w['diff_pct']:.1f}%")
    
    print(f"\nIssue Clusters:")
    for k, v in sorted(total_clusters.items(), key=lambda x: -x[1]):
        print(f"  {k}: {v}")
    
    print(f"\nReport saved to: {report_path}")
    
    # Generate WorkOrders for dominant clusters
    workorders_dir = output_dir / "workorders"
    workorders_dir.mkdir(exist_ok=True)
    
    print("\n--- Auto-Generated WorkOrders ---")
    workorders_created = generate_workorders(total_clusters, metrics["worst_3_cases"], workorders_dir)
    for wo in workorders_created:
        print(f"  Created: {wo}")
    
    # Generate failure packets for top 3 worst cases
    packets_dir = output_dir / "failure_packets"
    packets_dir.mkdir(exist_ok=True)
    
    print("\n--- Generating Failure Packets for Top 3 Cases ---")
    all_results = {r["case_id"]: r for r in builtin_results + websuite_results}
    for worst in metrics["worst_3_cases"]:
        case_id = worst["case_id"]
        result = all_results.get(case_id)
        if result and result.get("success"):
            packet_path = generate_failure_packet(
                case_id,
                result,
                packets_dir,
            )
            if packet_path:
                print(f"  Generated: {packet_path}")
    
    # Determine overall parity estimate
    parity_estimate = 100 - metrics["tier_b_weighted_mean"]
    print(f"\n>>> ESTIMATED PARITY: {parity_estimate:.1f}% <<<")
    
    # Auto-archive the run
    if auto_archive:
        print("\n--- Auto-Archiving Run ---")
        run_dir = archive_run(output_dir, history_dir, tag)
        if run_dir:
            print(f"  Archived to: {run_dir}")
            
            # Compare to previous run
            prev_run_id = get_previous_run(history_dir, run_dir.name)
            if prev_run_id:
                print("\n--- Comparison to Previous Run ---")
                prev_summary_path = history_dir / prev_run_id / "summary.json"
                curr_summary_path = run_dir / "summary.json"
                
                if prev_summary_path.exists() and curr_summary_path.exists():
                    with open(prev_summary_path, encoding="utf-8") as f:
                        prev_summary = json.load(f)
                    with open(curr_summary_path, encoding="utf-8") as f:
                        curr_summary = json.load(f)
                    
                    prev_parity = prev_summary["estimated_parity"]
                    curr_parity = curr_summary["estimated_parity"]
                    delta = curr_parity - prev_parity
                    
                    indicator = "▲" if delta > 0 else "▼" if delta < 0 else "="
                    status = "IMPROVED" if delta > 0 else "REGRESSED" if delta < 0 else "UNCHANGED"
                    
                    print(f"  Previous: {prev_parity:.1f}%")
                    print(f"  Current:  {curr_parity:.1f}%")
                    print(f"  Change:   {indicator} {delta:+.1f}% {status}")
                    
                    # Show significant case changes
                    prev_cases = prev_summary.get("case_diffs", {})
                    curr_cases = curr_summary.get("case_diffs", {})
                    
                    significant_changes = []
                    for case_id in set(prev_cases.keys()) | set(curr_cases.keys()):
                        prev_diff = prev_cases.get(case_id, {}).get("diff_pct", 100)
                        curr_diff = curr_cases.get(case_id, {}).get("diff_pct", 100)
                        case_delta = curr_diff - prev_diff
                        if abs(case_delta) >= 5:
                            significant_changes.append((case_id, prev_diff, curr_diff, case_delta))
                    
                    if significant_changes:
                        print("\n  Significant Case Changes (>5%):")
                        for case_id, prev_diff, curr_diff, case_delta in sorted(significant_changes, key=lambda x: x[3]):
                            ind = "✓" if case_delta < 0 else "✗"
                            print(f"    {case_id}: {prev_diff:.1f}% -> {curr_diff:.1f}% ({case_delta:+.1f}%) {ind}")
        else:
            print("  Failed to archive run")
    
    print("\n" + "=" * 60)
    print("Run complete!")
    if auto_archive:
        print("Use 'python3 scripts/parity_compare.py' to see full comparison")
        print("Use 'python3 scripts/parity_summary.py' to see trend report")
    print("=" * 60)


def generate_failure_packet(case_id: str, result: Dict[str, Any], output_dir: Path) -> Optional[str]:
    """Generate a failure packet for a specific case."""
    packet_dir = output_dir / case_id
    packet_dir.mkdir(exist_ok=True)
    
    packet = {
        "case_id": case_id,
        "generated_at": datetime.now().isoformat(),
        "estimated_diff_pct": result.get("estimated_diff_pct", 100),
        "html_path": result.get("html_path"),
        "dimensions": {
            "width": result.get("width"),
            "height": result.get("height"),
        },
    }
    
    # Copy frame if available
    frame_path = result.get("frame_path")
    if frame_path and Path(frame_path).exists():
        dest_frame = packet_dir / "rustkit_frame.ppm"
        import shutil
        shutil.copy(frame_path, dest_frame)
        packet["rustkit_frame"] = str(dest_frame)
    
    # Include layout stats
    layout_stats = result.get("layout_stats", {})
    if layout_stats:
        packet["layout_stats"] = layout_stats
    
    # Include issue clusters
    issue_clusters = result.get("issue_clusters", {})
    if issue_clusters:
        packet["issue_clusters"] = issue_clusters
    
    # Identify dominant issue
    if issue_clusters:
        dominant = max(issue_clusters.items(), key=lambda x: x[1])
        packet["dominant_issue"] = dominant[0]
        packet["dominant_count"] = dominant[1]
    
    # Include perf data if available
    perf = result.get("perf", {})
    if perf:
        packet["perf"] = perf
    
    # Save packet manifest
    manifest_path = packet_dir / "manifest.json"
    with open(manifest_path, "w", encoding="utf-8") as f:
        json.dump(packet, f, indent=2)
    
    return str(packet_dir)


def generate_workorders(clusters: Dict[str, int], worst_cases: List[Dict], output_dir: Path) -> List[str]:
    """Generate WorkOrders based on failure clusters."""
    created = []
    
    # Find the dominant cluster
    sorted_clusters = sorted(clusters.items(), key=lambda x: -x[1])
    
    for cluster_name, count in sorted_clusters:
        if count == 0:
            continue
        
        # Create WorkOrder for this cluster
        workorder = {
            "id": f"parity-{cluster_name}-{datetime.now().strftime('%Y%m%d')}",
            "title": f"Fix {cluster_name.replace('_', ' ').title()} Issues",
            "description": f"Address {count} {cluster_name} issues identified in parity baseline.",
            "priority": "high" if count > 10 else "medium",
            "cluster": cluster_name,
            "issue_count": count,
            "affected_cases": [c["case_id"] for c in worst_cases if cluster_name in str(c)],
            "acceptance_criteria": [
                f"Reduce {cluster_name} issue count by at least 50%",
                "No regression in other clusters",
                "Tier A pass rate improves",
            ],
            "created": datetime.now().isoformat(),
        }
        
        wo_path = output_dir / f"{cluster_name}.json"
        with open(wo_path, "w", encoding="utf-8") as f:
            json.dump(workorder, f, indent=2)
        
        created.append(str(wo_path))
    
    # Create a summary WorkOrder for the top 3 worst cases
    if worst_cases:
        summary_wo = {
            "id": f"parity-top-failures-{datetime.now().strftime('%Y%m%d')}",
            "title": "Fix Top 3 Worst Parity Cases",
            "description": "Focus on the three cases with highest pixel diff.",
            "priority": "critical",
            "cases": worst_cases,
            "acceptance_criteria": [
                f"Reduce diff% for {worst_cases[0]['case_id']} below 25%",
                "All three cases show measurable improvement",
            ],
            "created": datetime.now().isoformat(),
        }
        
        wo_path = output_dir / "top_failures.json"
        with open(wo_path, "w", encoding="utf-8") as f:
            json.dump(summary_wo, f, indent=2)
        
        created.append(str(wo_path))
    
    return created


if __name__ == "__main__":
    main()

