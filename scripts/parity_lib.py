#!/usr/bin/env python3
"""
parity_lib.py - Core parity testing library for parallel/swarm execution

This module provides:
- Run-scoped artifact paths (safe for parallel execution)
- Single-case execution logic
- Result aggregation helpers

All outputs are written to: parity-results/<run_id>/<case_id>/<viewport>/<iteration>/
"""

import json
import os
import subprocess
import statistics
import uuid
from dataclasses import dataclass, field, asdict
from datetime import datetime
from pathlib import Path
from typing import Optional, List, Dict, Any, Tuple

# ============================================================================
# Configuration
# ============================================================================

REPO_ROOT = Path(__file__).parent.parent
# Campaign pin (trench/BASELINE-macos.md): Chrome for Testing 148.0.7778.216.
# Override with PARITY_BASELINE_SET only for deliberate cross-version experiments.
BASELINES_DIR = REPO_ROOT / "baselines" / os.environ.get("PARITY_BASELINE_SET", "chrome-148")
DEFAULT_RESULTS_ROOT = REPO_ROOT / "parity-results"

# Case definitions — SINGLE SOURCE OF TRUTH: cases/registry.json.
# (R0, VIEWPORT_RESOLUTION_PLAN P0.2: parity_lib and parity_test carried
# separately-maintained copies of this table; they had already diverged by
# two cases when the registry was cut. Do not add cases here — edit the
# registry.)
CASE_REGISTRY_PATH = REPO_ROOT / "cases" / "registry.json"
with open(CASE_REGISTRY_PATH, encoding="utf-8") as _f:
    CASE_REGISTRY = json.load(_f)


def _cases_for_scope(scope: str) -> List[Tuple[str, str, int, int]]:
    return [
        (cid, c["html"], c["width"], c["height"])
        for cid, c in CASE_REGISTRY["cases"].items()
        if c["scope"] == scope
    ]


BUILTINS = _cases_for_scope("builtins")
WEBSUITE = _cases_for_scope("websuite")
MICRO_TESTS = _cases_for_scope("micro")

# Iterations required before a stability verdict means anything — finish-line
# condition 3, "stable across 3 iterations" (plan §2, §3.3).
#
# SINGLE SOURCE OF TRUTH, and pinned for the same reason `aa_tolerance` is:
# the number lived as a bare literal `3` in three places (this file's
# aggregate_iterations, parity_test.py's verdict, parity_test.py's printout).
# Three copies of a bar is three chances for the producer to call a row stable
# on evidence the gate would reject, or the reverse. Cite it, do not copy it.
STABILITY_MIN_RUNS = 3

# Standard viewports for multi-viewport testing
VIEWPORTS = [
    (800, 600, "800x600"),
    (1280, 800, "1280x800"),
    (1920, 1080, "1920x1080"),
]

# Thresholds by component type.
#
# T6 THRESHOLD COLLAPSE (Pete-locked, 2026-07-11, per the test-fidelity
# hardening plan): the sticky (25) and text (20) specials are GONE — both
# were free-pass zones that absorbed real wrongness ("a page can be
# visibly wrong and still pass"). Everything now caps at t15; categories
# already tighter than 15 stay tight. Cases this flips to failing carry
# known_fail in cases/registry.json (gate ceiling only) until actually
# fixed. Per the hardening banlist: no threshold moves without Pete.
THRESHOLDS = {
    "layout_structure": 5,
    "solid_backgrounds": 8,
    "images_replaced": 10,
    "gradients_effects": 15,
    "form_controls": 12,
    "text_rendering": 15,  # was 20
    "sticky_scroll": 15,  # was 25
    "default": 15,
}

# CI PR-gate scope caps (test-fidelity A5 tier table): builtins and micro
# gate at t8 in CI — product chrome and micro contracts are held tighter
# than full websuite pages. Reporting (pass@t15 scoreboard) is unchanged;
# this only tightens what a PR may regress.
GATE_SCOPE_CAPS = {
    "builtins": 8.0,
    "micro": 8.0,
    "websuite": 15.0,
    "holdout": 15.0,
}

# Blank frame detection threshold (>99.9% background = blank)
BLANK_FRAME_THRESHOLD = 0.999


# ============================================================================
# Data classes
# ============================================================================

@dataclass
class WorkUnit:
    """A single unit of work: one case, one viewport, one iteration."""
    case_id: str
    html_path: str
    width: int
    height: int
    case_type: str  # builtins, websuite, micro
    viewport_name: str
    iteration: int
    
    def key(self) -> str:
        return f"{self.case_id}:{self.viewport_name}:iter{self.iteration}"


@dataclass
class CaseResult:
    """Result of running a single work unit."""
    case_id: str
    case_type: str
    viewport: str
    iteration: int
    width: int
    height: int
    
    # Paths to artifacts
    capture_dir: str = ""
    diff_dir: str = ""
    
    # Results
    # None means NOT MEASURED (instrument failure). 100.0 means measured
    # as a total mismatch. Collapsing the two is the bug this file had.
    #
    # The DEFAULT is None: a result that never reached a comparison has not
    # measured anything, and defaulting to 100.0 meant every early return had
    # to remember to override it. Three of them did not.
    diff_pct: Optional[float] = None
    instrument_failure: Optional[str] = None
    diff_pixels: int = 0
    total_pixels: int = 0
    threshold: float = 15.0
    passed: bool = False
    error: Optional[str] = None
    
    # Blank frame detection (critical gate)
    is_blank_frame: bool = False
    blank_frame_ratio: float = 0.0
    unique_colors: int = 0
    
    # Attribution
    attribution_path: Optional[str] = None
    overlay_path: Optional[str] = None
    taxonomy: Optional[Dict[str, float]] = None
    top_contributors: Optional[List[Dict]] = None
    
    # Timing
    capture_ms: int = 0
    compare_ms: int = 0


@dataclass
class AggregatedResult:
    """Aggregated result for a case across iterations."""
    case_id: str
    case_type: str
    viewport: str
    width: int
    height: int
    threshold: float
    
    # Stats across iterations
    # 65-D: these defaulted to 100.0, so a case whose every iteration errored
    # kept publishing a scored total-mismatch for something nobody measured.
    # None means NOT MEASURED; 100.0 has to be earned by an actual comparison.
    diff_pct_median: Optional[float] = None
    diff_pct_min: Optional[float] = None
    diff_pct_max: Optional[float] = None
    diff_pct_variance: float = 0.0
    iterations: int = 0
    stable: bool = False
    passed: bool = False
    
    # Best iteration's artifacts
    best_diff_dir: str = ""
    best_attribution_path: Optional[str] = None
    best_overlay_path: Optional[str] = None
    best_taxonomy: Optional[Dict[str, float]] = None
    best_top_contributors: Optional[List[Dict]] = None
    
    # All iteration results
    iteration_diffs: List[float] = field(default_factory=list)
    errors: List[str] = field(default_factory=list)


# ============================================================================
# Helpers
# ============================================================================

def get_threshold(case_id: str) -> float:
    """Get appropriate threshold for a case."""
    if "form" in case_id:
        return THRESHOLDS["form_controls"]
    if "image" in case_id or "gallery" in case_id:
        return THRESHOLDS["images_replaced"]
    if "gradient" in case_id:
        return THRESHOLDS["gradients_effects"]
    if "sticky" in case_id or "scroll" in case_id:
        return THRESHOLDS["sticky_scroll"]
    if "typography" in case_id or "text" in case_id:
        return THRESHOLDS["text_rendering"]
    return THRESHOLDS["default"]


def get_case_type(case_id: str) -> str:
    """Determine case type from case_id."""
    if any(c[0] == case_id for c in BUILTINS):
        return "builtins"
    if any(c[0] == case_id for c in MICRO_TESTS):
        return "micro"
    return "websuite"


def get_all_cases() -> Dict[str, Tuple[str, str, int, int, str]]:
    """Get all cases as dict: case_id -> (case_id, html_path, width, height, type)."""
    result = {}
    for c in BUILTINS:
        result[c[0]] = (c[0], c[1], c[2], c[3], "builtins")
    for c in WEBSUITE:
        result[c[0]] = (c[0], c[1], c[2], c[3], "websuite")
    for c in MICRO_TESTS:
        result[c[0]] = (c[0], c[1], c[2], c[3], "micro")
    return result


def generate_run_id() -> str:
    """Generate a unique run ID."""
    ts = datetime.now().strftime("%Y%m%d-%H%M%S")
    short_uuid = uuid.uuid4().hex[:8]
    return f"{ts}-{short_uuid}"


# ============================================================================
# Artifact path management (run-scoped)
# ============================================================================

def get_artifact_paths(
    run_id: str,
    case_id: str,
    viewport: str,
    iteration: int,
    results_root: Path = DEFAULT_RESULTS_ROOT,
) -> Dict[str, Path]:
    """
    Get all artifact paths for a work unit.
    
    Layout:
      parity-results/<run_id>/<case_id>/<viewport>/iter-<N>/
        ├── capture/
        │   ├── frame.ppm
        │   └── layout.json
        └── diff/
            ├── diff.png
            ├── heatmap.png
            ├── overlay.png
            └── attribution.json
    """
    base = results_root / run_id / case_id / viewport / f"iter-{iteration}"
    capture_dir = base / "capture"
    diff_dir = base / "diff"
    
    return {
        "base": base,
        "capture_dir": capture_dir,
        "diff_dir": diff_dir,
        "frame_ppm": capture_dir / "frame.ppm",
        "layout_json": capture_dir / "layout.json",
        "diff_png": diff_dir / "diff.png",
        "heatmap_png": diff_dir / "heatmap.png",
        "overlay_png": diff_dir / "overlay.png",
        "attribution_json": diff_dir / "attribution.json",
    }


def get_baseline_paths(case_id: str, case_type: str) -> Dict[str, Path]:
    """Get Chrome baseline paths for a case."""
    base = BASELINES_DIR / case_type / case_id
    return {
        "base": base,
        "baseline_png": base / "baseline.png",
        "layout_rects": base / "layout-rects.json",
        "computed_styles": base / "computed-styles.json",
    }


# ============================================================================
# Blank frame detection (critical gate)
# ============================================================================

def analyze_frame_blankness(
    ppm_path: Path,
    background_color: Tuple[int, int, int] = (255, 255, 255)
) -> Dict[str, Any]:
    """
    Analyze a PPM frame to detect if it's effectively blank (uniform color).
    
    This is a CRITICAL GATE to prevent "blank white screen = high parity" lies.
    A blank frame should ALWAYS fail, regardless of layout health metrics.
    
    Returns:
        - is_blank: True if frame is >99.9% background color
        - background_ratio: percentage of pixels matching background
        - unique_colors: number of distinct colors in the frame
        - total_pixels: total pixel count
    """
    if not ppm_path or not ppm_path.exists():
        return {"error": "No frame file", "is_blank": True, "background_ratio": 1.0, "unique_colors": 0}
    
    try:
        with open(ppm_path, 'rb') as f:
            # Parse PPM header
            header = f.readline().decode('ascii').strip()
            if header not in ('P6', 'P3'):
                return {"error": f"Unknown PPM format: {header}", "is_blank": True, "background_ratio": 1.0, "unique_colors": 0}
            
            # Skip comments
            line = f.readline()
            while line.startswith(b'#'):
                line = f.readline()
            
            # Read dimensions
            dims = line.decode('ascii').strip().split()
            width, height = int(dims[0]), int(dims[1])
            
            # Read max value
            max_val = int(f.readline().decode('ascii').strip())
            
            # Read pixel data
            if header == 'P6':
                # Binary PPM
                pixel_data = f.read()
            else:
                # ASCII PPM (P3)
                pixel_data = bytes([int(x) for x in f.read().decode('ascii').split()])
        
        total_pixels = width * height
        if total_pixels == 0:
            return {"error": "Empty frame (0x0)", "is_blank": True, "background_ratio": 1.0, "unique_colors": 0}
        
        # Count colors and background matches
        bg_r, bg_g, bg_b = background_color
        bg_count = 0
        color_counts: Dict[Tuple[int, int, int], int] = {}
        
        for i in range(0, min(len(pixel_data), total_pixels * 3), 3):
            if i + 2 >= len(pixel_data):
                break
            r, g, b = pixel_data[i], pixel_data[i+1], pixel_data[i+2]
            
            color = (r, g, b)
            color_counts[color] = color_counts.get(color, 0) + 1
            
            # Check if it matches background (with tolerance for compression)
            if abs(r - bg_r) <= 2 and abs(g - bg_g) <= 2 and abs(b - bg_b) <= 2:
                bg_count += 1
        
        actual_pixels = len(pixel_data) // 3
        background_ratio = bg_count / max(1, actual_pixels)
        unique_colors = len(color_counts)
        
        # Frame is "blank" if >99.9% matches background
        is_blank = background_ratio >= BLANK_FRAME_THRESHOLD
        
        # Also flag as blank if dominated by a single color (even if not white)
        if unique_colors < 10 and unique_colors > 0 and not is_blank:
            dominant_color = max(color_counts.items(), key=lambda x: x[1])
            dominant_ratio = dominant_color[1] / max(1, actual_pixels)
            if dominant_ratio >= BLANK_FRAME_THRESHOLD:
                is_blank = True
        
        return {
            "is_blank": is_blank,
            "background_ratio": background_ratio,
            "unique_colors": unique_colors,
            "total_pixels": actual_pixels,
            "width": width,
            "height": height,
        }
    except Exception as e:
        return {"error": str(e), "is_blank": True, "background_ratio": 1.0, "unique_colors": 0}


# ============================================================================
# RustKit capture
# ============================================================================

def run_rustkit_capture(
    html_path: str,
    width: int,
    height: int,
    frame_output: Path,
    layout_output: Path,
) -> Dict[str, Any]:
    """
    Capture RustKit rendering to specific output paths.
    
    Returns: {"success": bool, "error": str|None, "elapsed_ms": int}
    """
    import time
    
    frame_output.parent.mkdir(parents=True, exist_ok=True)
    layout_output.parent.mkdir(parents=True, exist_ok=True)
    
    capture_cmd = [
        str(REPO_ROOT / "target" / "release" / ("parity-capture.exe" if os.name == "nt" else "parity-capture")),
        "--html-file", str(REPO_ROOT / html_path),
        "--width", str(width),
        "--height", str(height),
        "--dump-frame", str(frame_output),
        "--dump-layout", str(layout_output),
    ]
    
    start = time.time()
    try:
        result = subprocess.run(
            capture_cmd,
            capture_output=True,
            text=True,
            timeout=60,
            cwd=REPO_ROOT, encoding="utf-8"
        )
        elapsed_ms = int((time.time() - start) * 1000)
        
        if result.returncode == 0 and frame_output.exists():
            return {"success": True, "elapsed_ms": elapsed_ms}
        else:
            err = result.stderr[:300] if result.stderr else "No frame output"
            return {"success": False, "error": err, "elapsed_ms": elapsed_ms}
    except subprocess.TimeoutExpired:
        return {"success": False, "error": "Timeout (60s)", "elapsed_ms": 60000}
    except Exception as e:
        return {"success": False, "error": str(e), "elapsed_ms": 0}


# ============================================================================
# Pixel comparison
# ============================================================================

def compare_pixels(
    chrome_png: Path,
    rustkit_ppm: Path,
    output_dir: Path,
    chrome_rects: Optional[Path] = None,
    chrome_styles: Optional[Path] = None,
) -> Dict[str, Any]:
    """
    Compare pixel data using Node.js tool.
    
    Returns: {
        "diffPercent": float,
        "diffPixels": int,
        "totalPixels": int,
        "diffPath": str,
        "heatmapPath": str,
        "overlayPath": str,
        "attribution": {...},
        "taxonomy": {...},
        "error": str|None
    }
    """
    import time
    
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
      attributionTopN: 15,
    }}
);
console.log(JSON.stringify(result));
"""
    ]
    
    start = time.time()
    try:
        result = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            timeout=60,
            cwd=REPO_ROOT,
            env={**os.environ, "PATH": "/opt/homebrew/bin" + os.pathsep + os.environ.get('PATH', '')}, encoding="utf-8"
        )
        elapsed_ms = int((time.time() - start) * 1000)
        
        if result.returncode == 0:
            for line in result.stdout.strip().split('\n'):
                if line.startswith('{'):
                    data = json.loads(line)
                    data["elapsed_ms"] = elapsed_ms
                    return data
        return {"error": result.stderr[:300], "elapsed_ms": elapsed_ms}
    except Exception as e:
        return {"error": str(e), "elapsed_ms": 0}


# ============================================================================
# Work unit execution
# ============================================================================

def execute_work_unit(
    work_unit: WorkUnit,
    run_id: str,
    results_root: Path = DEFAULT_RESULTS_ROOT,
) -> CaseResult:
    """
    Execute a single work unit (one case, one viewport, one iteration).
    
    This is the core function called by both sequential and parallel runners.
    All outputs go to run-scoped paths.
    
    CRITICAL: Blank frame detection is always performed first. A blank frame
    ALWAYS fails, regardless of any other metrics.
    """
    paths = get_artifact_paths(
        run_id, work_unit.case_id, work_unit.viewport_name,
        work_unit.iteration, results_root
    )
    baseline = get_baseline_paths(work_unit.case_id, work_unit.case_type)
    
    result = CaseResult(
        case_id=work_unit.case_id,
        case_type=work_unit.case_type,
        viewport=work_unit.viewport_name,
        iteration=work_unit.iteration,
        width=work_unit.width,
        height=work_unit.height,
        threshold=get_threshold(work_unit.case_id),
        capture_dir=str(paths["capture_dir"]),
        diff_dir=str(paths["diff_dir"]),
    )
    
    # Check baseline exists
    if not baseline["baseline_png"].exists():
        result.error = f"No Chrome baseline at {baseline['baseline_png']}"
        result.is_blank_frame = True  # Treat as blank for safety
        result.diff_pct = None  # 65-D: a refusal, not a measured 100% diff
        return result
    
    # 1. Capture RustKit
    capture_result = run_rustkit_capture(
        work_unit.html_path,
        work_unit.width,
        work_unit.height,
        paths["frame_ppm"],
        paths["layout_json"],
    )
    result.capture_ms = capture_result.get("elapsed_ms", 0)
    
    if not capture_result.get("success"):
        result.error = f"Capture failed: {capture_result.get('error', 'Unknown')}"
        result.is_blank_frame = True  # Treat as blank for safety
        result.diff_pct = None  # 65-D: a refusal, not a measured 100% diff
        return result
    
    # 2. CRITICAL: Check for blank frame BEFORE pixel comparison
    #    A blank frame = FAIL, regardless of any other metrics
    blank_analysis = analyze_frame_blankness(paths["frame_ppm"])
    result.is_blank_frame = blank_analysis.get("is_blank", True)
    result.blank_frame_ratio = blank_analysis.get("background_ratio", 1.0)
    result.unique_colors = blank_analysis.get("unique_colors", 0)
    
    if result.is_blank_frame:
        result.error = f"BLANK_FRAME: {result.blank_frame_ratio*100:.1f}% background, {result.unique_colors} colors"
        # 65-D (Prometheus): a blank frame is a REFUSAL, not a 100% render
        # diff. Stamping 100.0 here left any consumer that reads diff_pct
        # without also reading error seeing a fake score — the three-state
        # contract has to hold at the SOURCE, not only after extract_metrics
        # heals it.
        result.diff_pct = None
        result.passed = False
        return result
    
    # 3. Compare pixels (only for non-blank frames)
    pixel_result = compare_pixels(
        baseline["baseline_png"],
        paths["frame_ppm"],
        paths["diff_dir"],
        baseline["layout_rects"],
        baseline["computed_styles"],
    )
    result.compare_ms = pixel_result.get("elapsed_ms", 0)
    
    # An INSTRUMENT FAILURE is not a measurement. The oracle already reports
    # dimension mismatch as instrumentFailure with diffPercent=100 — but until
    # 2026-07-29 nothing downstream read that field, so a capture the
    # instrument itself refused to score was recorded as a 100.0 render diff
    # with error=null. That is what made the nightly gate decorative: 65 of 91
    # matrix cells were instrument failures wearing measurement clothes, and a
    # gate that cannot go green stops being read.
    instrument_failure = pixel_result.get("instrumentFailure")
    if instrument_failure:
        result.error = f"INSTRUMENT: {instrument_failure}"
        result.instrument_failure = str(instrument_failure)
        result.diff_pct = None  # refuse to publish a score we did not measure
        result.passed = False
        return result

    if pixel_result.get("error"):
        result.error = f"Compare failed: {pixel_result.get('error')}"
        result.diff_pct = None  # 65-D: a refusal, not a measured 100% diff
        return result
    
    # 4. Extract results
    result.diff_pct = float(pixel_result.get("diffPercent", 100.0))
    result.diff_pixels = int(pixel_result.get("diffPixels", 0))
    result.total_pixels = int(pixel_result.get("totalPixels", 0))
    result.passed = result.diff_pct is not None and result.diff_pct <= result.threshold
    
    # Attribution artifacts
    if paths["attribution_json"].exists():
        result.attribution_path = str(paths["attribution_json"])
        try:
            attr_data = json.loads(paths["attribution_json"].read_text(encoding="utf-8"))
            result.taxonomy = attr_data.get("taxonomy")
            result.top_contributors = attr_data.get("topContributors")
        except:
            pass
    
    if paths["overlay_png"].exists():
        result.overlay_path = str(paths["overlay_png"])
    
    return result


# ============================================================================
# Aggregation
# ============================================================================

def aggregate_iterations(results: List[CaseResult], max_variance: float = 0.10) -> AggregatedResult:
    """
    Aggregate multiple iteration results for the same case+viewport.
    
    Returns a single AggregatedResult with stats and the best iteration's artifacts.
    """
    if not results:
        raise ValueError("No results to aggregate")
    
    first = results[0]
    agg = AggregatedResult(
        case_id=first.case_id,
        case_type=first.case_type,
        viewport=first.viewport,
        width=first.width,
        height=first.height,
        threshold=first.threshold,
    )
    
    # Collect diffs and errors
    diffs = []
    errors = []
    best_result: Optional[CaseResult] = None
    best_diff = float('inf')
    
    for r in results:
        if r.error or r.diff_pct is None:
            # `or diff_pct is None` is belt-and-braces: every refusal path sets
            # an error today, so the second clause should be unreachable. Four
            # misses of this exact class in one change set is enough evidence
            # that "should be unreachable" is not a guarantee worth relying on.
            if r.error:
                errors.append(r.error)
        else:
            diffs.append(r.diff_pct)
            if r.diff_pct < best_diff:
                best_diff = r.diff_pct
                best_result = r
    
    agg.errors = errors
    agg.iteration_diffs = diffs
    agg.iterations = len(results)
    
    if diffs:
        agg.diff_pct_median = float(statistics.median(diffs))
        agg.diff_pct_min = float(min(diffs))
        agg.diff_pct_max = float(max(diffs))
        agg.diff_pct_variance = agg.diff_pct_max - agg.diff_pct_min
        agg.stable = len(diffs) >= STABILITY_MIN_RUNS and agg.diff_pct_variance <= max_variance
        agg.passed = agg.diff_pct_median <= agg.threshold
        
        if best_result:
            agg.best_diff_dir = best_result.diff_dir
            agg.best_attribution_path = best_result.attribution_path
            agg.best_overlay_path = best_result.overlay_path
            agg.best_taxonomy = best_result.taxonomy
            agg.best_top_contributors = best_result.top_contributors
    else:
        # Every iteration was refused. Say so explicitly rather than letting
        # the dataclass defaults publish a score nobody measured.
        agg.diff_pct_median = None
        agg.diff_pct_min = None
        agg.diff_pct_max = None
        agg.diff_pct_variance = None
        agg.stable = False
        agg.passed = False
    
    return agg


# ============================================================================
# Build helper
# ============================================================================

def ensure_parity_capture_built() -> bool:
    """Build parity-capture if needed. Returns True on success."""
    binary = REPO_ROOT / "target" / "release" / ("parity-capture.exe" if os.name == "nt" else "parity-capture")
    
    # Always rebuild to ensure latest
    build_cmd = ["cargo", "build", "--release", "-p", "parity-capture"]
    result = subprocess.run(build_cmd, capture_output=True, text=True, cwd=REPO_ROOT, encoding="utf-8")
    
    if result.returncode != 0:
        print(f"Error building parity-capture: {result.stderr[:400]}")
        return False
    
    return binary.exists()


# ============================================================================
# Serialization
# ============================================================================

def result_to_dict(result: CaseResult) -> Dict[str, Any]:
    """Convert CaseResult to serializable dict."""
    return asdict(result)


def aggregated_to_dict(agg: AggregatedResult) -> Dict[str, Any]:
    """Convert AggregatedResult to serializable dict."""
    return asdict(agg)
