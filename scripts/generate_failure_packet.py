#!/usr/bin/env python3
"""
generate_failure_packet.py - Generate structured failure packets for parity debugging

A failure packet contains everything needed to diagnose a pixel parity failure:
- RustKit frame (PPM)
- Chromium baseline (PNG)
- Diff image (PNG)
- Oracle data (computed styles + DOMRects)
- Comparison report (JSON)
- DisplayList dump (if available)
- Minimal reproduction HTML (if generatable)

Usage:
    python3 scripts/generate_failure_packet.py <case_id> <run_dir>
"""

import json
import os
import shutil
import sys
from datetime import datetime
from pathlib import Path
from typing import Dict, Any, Optional

def load_oracle(oracle_path: Path) -> Optional[Dict[str, Any]]:
    """Load oracle JSON if available."""
    if oracle_path.exists():
        with open(oracle_path, encoding="utf-8") as f:
            return json.load(f)
    return None

def load_comparison(comparison_path: Path) -> Optional[Dict[str, Any]]:
    """Load comparison report if available."""
    if comparison_path.exists():
        with open(comparison_path, encoding="utf-8") as f:
            return json.load(f)
    return None

def load_perf(perf_path: Path) -> Optional[Dict[str, Any]]:
    """Load perf JSON if available."""
    if perf_path.exists():
        with open(perf_path, encoding="utf-8") as f:
            return json.load(f)
    return None

def classify_failure(comparison: Dict[str, Any], oracle: Optional[Dict[str, Any]]) -> str:
    """
    Classify the failure into a category for routing.
    
    Categories:
    - text: Text rendering differences (fonts, metrics, AA)
    - layout: Box geometry differences
    - paint: Color, border, shadow differences
    - image: Image rendering differences
    - compositing: Layer/z-order issues
    - unknown: Cannot determine
    """
    if not comparison:
        return "unknown"
    
    comp = comparison.get("comparison", {})
    
    if comp.get("size_mismatch"):
        return "layout"  # Size mismatch indicates layout issue
    
    # Use the pre-classified category if available
    category = comparison.get("failure_category", "unknown")
    
    if category == "text_aa":
        return "text"
    elif category == "paint":
        return "paint"
    elif category == "layout":
        return "layout"
    
    return "unknown"

def generate_minimal_repro(
    case_id: str,
    oracle: Optional[Dict[str, Any]],
    comparison: Optional[Dict[str, Any]]
) -> Optional[str]:
    """
    Generate a minimal HTML reproduction for the failure.
    
    This is a simplified version - a full implementation would
    analyze the diff regions and extract only the relevant elements.
    """
    if not oracle:
        return None
    
    # For now, just note that minimal repro generation is pending
    return f"""<!-- Minimal reproduction for {case_id} -->
<!-- TODO: Auto-generate based on diff regions and oracle data -->
<!DOCTYPE html>
<html>
<head>
    <meta charset="utf-8">
    <title>Minimal Repro: {case_id}</title>
    <style>
        /* Extracted styles would go here */
    </style>
</head>
<body>
    <!-- Extracted elements would go here -->
    <p>Minimal reproduction pending implementation.</p>
</body>
</html>
"""

def generate_failure_packet(case_id: str, run_dir: Path) -> Dict[str, Any]:
    """Generate a complete failure packet for a case."""
    
    packet = {
        "case_id": case_id,
        "generated_at": datetime.now().isoformat(),
        "run_dir": str(run_dir),
        "artifacts": {},
        "classification": {
            "category": "unknown",
            "confidence": 0.0,
            "suggested_papa": None
        },
        "summary": {}
    }
    
    # Collect artifact paths
    rustkit_ppm = run_dir / "rustkit" / f"{case_id}.ppm"
    chromium_png = run_dir / "chromium" / f"{case_id}.png"
    diff_png = run_dir / "diffs" / f"{case_id}.diff.png"
    oracle_json = run_dir / "oracle" / f"{case_id}.oracle.json"
    comparison_json = run_dir / "diffs" / f"{case_id}.comparison.json"
    perf_json = run_dir / "rustkit" / f"{case_id}.perf.json"
    
    # Record artifact availability
    packet["artifacts"] = {
        "rustkit_frame": str(rustkit_ppm) if rustkit_ppm.exists() else None,
        "chromium_frame": str(chromium_png) if chromium_png.exists() else None,
        "diff_image": str(diff_png) if diff_png.exists() else None,
        "oracle": str(oracle_json) if oracle_json.exists() else None,
        "comparison": str(comparison_json) if comparison_json.exists() else None,
        "perf": str(perf_json) if perf_json.exists() else None
    }
    
    # Load data
    oracle = load_oracle(oracle_json)
    comparison = load_comparison(comparison_json)
    perf = load_perf(perf_json)
    
    # Classify failure
    category = classify_failure(comparison, oracle)
    packet["classification"]["category"] = category
    
    # Map category to Papa phase
    papa_mapping = {
        "text": "papa2",
        "layout": "papa3",
        "paint": "papa4",
        "image": "papa5",
        "compositing": "papa4",
        "unknown": None
    }
    packet["classification"]["suggested_papa"] = papa_mapping.get(category)
    
    # Generate summary
    if comparison:
        comp = comparison.get("comparison", {})
        packet["summary"] = {
            "status": "PASS" if comp.get("passed") else "FAIL",
            "true_diff_pixels": comp.get("true_diff_pixels", 0),
            "true_diff_percent": comp.get("true_diff_percent", 0),
            "tolerated_pixels": comp.get("tolerated_diff_pixels", 0),
            "size_mismatch": comp.get("size_mismatch", False)
        }
    
    # Add oracle summary
    if oracle:
        packet["oracle_summary"] = {
            "elements": len(oracle.get("elements", [])),
            "text_runs": len(oracle.get("textRuns", [])),
            "fonts_used": oracle.get("computedFonts", [])
        }
    
    # Add perf summary
    if perf:
        timings = perf.get("perf") or perf.get("timings", {})
        packet["perf_summary"] = {
            key: data.get("avg_ms") for key, data in timings.items()
            if isinstance(data, dict) and "avg_ms" in data
        }
    
    # Generate minimal repro
    minimal_repro = generate_minimal_repro(case_id, oracle, comparison)
    if minimal_repro:
        packet["minimal_repro"] = minimal_repro
    
    return packet

def main():
    if len(sys.argv) < 3:
        print("Usage: python3 scripts/generate_failure_packet.py <case_id> <run_dir>")
        sys.exit(1)
    
    case_id = sys.argv[1]
    run_dir = Path(sys.argv[2])
    
    if not run_dir.exists():
        print(f"Error: Run directory not found: {run_dir}")
        sys.exit(1)
    
    print(f"Generating failure packet for: {case_id}")
    print(f"Run directory: {run_dir}")
    
    packet = generate_failure_packet(case_id, run_dir)
    
    # Save packet
    packets_dir = run_dir / "failure-packets"
    packets_dir.mkdir(parents=True, exist_ok=True)
    
    packet_path = packets_dir / f"{case_id}.packet.json"
    with open(packet_path, "w", encoding="utf-8") as f:
        json.dump(packet, f, indent=2)
    
    print(f"\nFailure Packet Generated: {packet_path}")
    print(f"\nSummary:")
    print(f"  Category: {packet['classification']['category']}")
    print(f"  Suggested Papa: {packet['classification']['suggested_papa']}")
    
    if packet.get("summary"):
        print(f"  Status: {packet['summary'].get('status', 'unknown')}")
        print(f"  True diff pixels: {packet['summary'].get('true_diff_pixels', 0)}")
    
    print(f"\nArtifacts:")
    for name, path in packet["artifacts"].items():
        status = "✓" if path else "✗"
        print(f"  {status} {name}")

if __name__ == "__main__":
    main()

