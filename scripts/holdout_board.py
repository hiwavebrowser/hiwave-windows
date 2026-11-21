#!/usr/bin/env python3
"""holdout_board.py — Second Holdout Site Set Runner & Dual-Scorer (Package Z2-N1).

Guards against overfitting to the primary 20-site real-site board by evaluating
a completely unseen set of 20 consumer sites (websuite/realsite-holdout20.json)
under both scoring systems side-by-side:
  - Scorer v1: Ink fraction + word count ratio + pixel diff (realsite_board.py)
  - Scorer v2: Regional geometry + color complexity + display list ops (scorer_v2.py)

Per Rule A3, all scoring baselines and definitions remain immutable.
Holdout and Scorer v2 results publish BESIDE the existing board.

Usage:
  python3 scripts/holdout_board.py --validate
  python3 scripts/holdout_board.py --run trench/realsite/runs/<ts>
  python3 scripts/holdout_board.py --compare-top20 <dir> --compare-holdout <dir>
"""

import argparse
import json
import os
from pathlib import Path
import sys
from typing import Any, Dict, List, Optional, Tuple

REPO = Path(__file__).resolve().parent.parent
TOP20_MANIFEST = REPO / "websuite" / "realsite-top20.json"
HOLDOUT_MANIFEST = REPO / "websuite" / "realsite-holdout20.json"
TOP80_MANIFEST = REPO / "websuite" / "realsite-top80.json"

try:
    import scorer_v2
except ImportError:
    scripts_dir = str(Path(__file__).resolve().parent)
    if scripts_dir not in sys.path:
        sys.path.insert(0, scripts_dir)
    import scorer_v2


def load_manifest(path: Optional[Path] = None) -> Dict[str, Any]:
    """Load and parse a realsite manifest file."""
    p = path or HOLDOUT_MANIFEST
    if not p.exists():
        raise FileNotFoundError(f"Manifest not found: {p}")
    return json.loads(p.read_text(encoding="utf-8"))


def validate_holdout_set(
    holdout_path: Optional[Path] = None,
    top20_path: Optional[Path] = None,
    top80_path: Optional[Path] = None,
) -> Dict[str, Any]:
    """Validate holdout manifest integrity against top20 (zero overlap) and top80 (full subset)."""
    h_path = holdout_path or HOLDOUT_MANIFEST
    t20_path = top20_path or TOP20_MANIFEST
    t80_path = top80_path or TOP80_MANIFEST

    h_data = load_manifest(h_path)
    t20_data = load_manifest(t20_path)
    t80_data = load_manifest(t80_path)

    h_sites = h_data.get("sites", [])
    t20_sites = t20_data.get("sites", [])
    t80_sites = t80_data.get("sites", [])

    h_ids = [s["id"] for s in h_sites]
    t20_ids = {s["id"] for s in t20_sites}
    t80_ids = {s["id"] for s in t80_sites}

    errors = []
    if len(h_sites) != 20:
        errors.append(f"Expected 20 holdout sites, found {len(h_sites)}")

    if len(set(h_ids)) != len(h_ids):
        errors.append("Duplicate site IDs found in holdout set")

    overlap = set(h_ids).intersection(t20_ids)
    if overlap:
        errors.append(f"Holdout set has {len(overlap)} overlapping site(s) with top20: {overlap}")

    missing_from_top80 = set(h_ids) - t80_ids
    if missing_from_top80:
        errors.append(f"Holdout set has sites not in top80 corpus: {missing_from_top80}")

    vp = h_data.get("viewport", {})
    if vp.get("width") != 1280 or vp.get("height") != 800:
        errors.append(f"Invalid viewport dimensions: {vp}, expected 1280x800")

    return {
        "valid": len(errors) == 0,
        "errors": errors,
        "holdout_count": len(h_sites),
        "overlap_count": len(overlap),
        "site_ids": h_ids,
    }


def score_dual_run(run_dir: Path) -> Dict[str, Any]:
    """Score a run directory using both Scorer v1 and Scorer v2 side-by-side.

    Rule A3 invariant: V1 baseline summary and per-site JSON files are preserved
    unmutated. V2 classifications and diagnostics are emitted beside them into
    summary_v2.json and <site>.v2.json.
    """
    if not run_dir.exists() or not run_dir.is_dir():
        raise FileNotFoundError(f"Run directory not found: {run_dir}")

    # Load existing V1 summary
    v1_summary_path = run_dir / "summary.json"
    if v1_summary_path.exists():
        v1_summary = json.loads(v1_summary_path.read_text(encoding="utf-8"))
    else:
        v1_summary = {}

    # Run Scorer v2 diagnostic classification
    v2_summary = scorer_v2.score_run_v2(run_dir)

    return {
        "run_dir": str(run_dir),
        "v1_summary": v1_summary,
        "v2_summary": v2_summary,
    }


def compare_boards(
    top20_summary: Dict[str, Any],
    holdout_summary: Dict[str, Any],
) -> Dict[str, Any]:
    """Compare performance across primary top20 board and holdout20 to assess overfitting.

    Computes:
      - Point pass rates and generalization gap
      - V1 LOADS vs V2 adjusted LOADS gaps
      - Category distribution shifts
    """
    t20_v1 = top20_summary.get("v1", {}) or top20_summary.get("v1_summary", {})
    t20_v2 = top20_summary.get("v2", {}) or top20_summary.get("v2_summary", {})

    ho_v1 = holdout_summary.get("v1", {}) or holdout_summary.get("v1_summary", {})
    ho_v2 = holdout_summary.get("v2", {}) or holdout_summary.get("v2_summary", {})

    t20_sites = t20_v1.get("sites") or t20_v2.get("total_sites") or 20
    ho_sites = ho_v1.get("sites") or ho_v2.get("total_sites") or 20

    t20_max_pts = t20_v1.get("max_points") or (3 * t20_sites)
    ho_max_pts = ho_v1.get("max_points") or (3 * ho_sites)

    t20_pts = t20_v1.get("points", 0)
    ho_pts = ho_v1.get("points", 0)

    t20_pts_rate = (t20_pts / max(t20_max_pts, 1)) * 100.0
    ho_pts_rate = (ho_pts / max(ho_max_pts, 1)) * 100.0

    t20_v1_loads = t20_v1.get("loads") or t20_v2.get("v1_loads_count", 0)
    ho_v1_loads = ho_v1.get("loads") or ho_v2.get("v1_loads_count", 0)

    t20_v1_loads_rate = (t20_v1_loads / max(t20_sites, 1)) * 100.0
    ho_v1_loads_rate = (ho_v1_loads / max(ho_sites, 1)) * 100.0

    t20_v2_loads = t20_v2.get("v2_adjusted_loads_count", t20_v1_loads)
    ho_v2_loads = ho_v2.get("v2_adjusted_loads_count", ho_v1_loads)

    t20_v2_loads_rate = (t20_v2_loads / max(t20_sites, 1)) * 100.0
    ho_v2_loads_rate = (ho_v2_loads / max(ho_sites, 1)) * 100.0

    # Category deltas
    t20_cats = t20_v2.get("categories", {})
    ho_cats = ho_v2.get("categories", {})
    all_cats = sorted(set(t20_cats.keys()).union(set(ho_cats.keys())))
    category_deltas = {}
    for c in all_cats:
        category_deltas[c] = {
            "top20": t20_cats.get(c, 0),
            "holdout": ho_cats.get(c, 0),
            "delta": ho_cats.get(c, 0) - t20_cats.get(c, 0),
        }

    return {
        "top20_sites": t20_sites,
        "holdout_sites": ho_sites,
        "top20_points_rate": round(t20_pts_rate, 2),
        "holdout_points_rate": round(ho_pts_rate, 2),
        "points_rate_gap": round(t20_pts_rate - ho_pts_rate, 2),
        "top20_v1_loads_rate": round(t20_v1_loads_rate, 2),
        "holdout_v1_loads_rate": round(ho_v1_loads_rate, 2),
        "v1_loads_rate_gap": round(t20_v1_loads_rate - ho_v1_loads_rate, 2),
        "top20_v2_adjusted_loads_rate": round(t20_v2_loads_rate, 2),
        "holdout_v2_adjusted_loads_rate": round(ho_v2_loads_rate, 2),
        "v2_adjusted_loads_rate_gap": round(t20_v2_loads_rate - ho_v2_loads_rate, 2),
        "category_deltas": category_deltas,
    }


def format_comparison_table(comp: Dict[str, Any]) -> str:
    """Format an overfitting diagnostic comparison table."""
    lines = []
    lines.append("=" * 86)
    lines.append("OVERFITTING GENERALIZATION REPORT: TOP-20 PRIMARY vs HOLDOUT-20 (Package Z2-N1)")
    lines.append("=" * 86)
    lines.append(f"{'Metric':36} {'Top-20':15} {'Holdout-20':15} {'Generalization Gap':20}")
    lines.append("-" * 86)
    lines.append(
        f"{'V1 Points Rate':36} {comp['top20_points_rate']:>6.2f}%        {comp['holdout_points_rate']:>6.2f}%        {comp['points_rate_gap']:>+6.2f}%"
    )
    lines.append(
        f"{'V1 LOADS Pass Rate':36} {comp['top20_v1_loads_rate']:>6.2f}%        {comp['holdout_v1_loads_rate']:>6.2f}%        {comp['v1_loads_rate_gap']:>+6.2f}%"
    )
    lines.append(
        f"{'V2 Adjusted LOADS Rate':36} {comp['top20_v2_adjusted_loads_rate']:>6.2f}%        {comp['holdout_v2_adjusted_loads_rate']:>6.2f}%        {comp['v2_adjusted_loads_rate_gap']:>+6.2f}%"
    )
    lines.append("-" * 86)
    lines.append("V2 Category Distribution:")
    for cat, d in comp.get("category_deltas", {}).items():
        lines.append(f"  {cat:20} Top-20: {d['top20']:2d}   Holdout: {d['holdout']:2d}   (delta: {d['delta']:+2d})")
    lines.append("=" * 86)
    return "\n".join(lines)


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--validate", action="store_true", help="validate holdout manifest against top20 and top80")
    ap.add_argument("--run", help="score a specific run directory with both V1 and V2 scorers")
    ap.add_argument("--compare-top20", help="top20 run directory for comparison")
    ap.add_argument("--compare-holdout", help="holdout run directory for comparison")
    ap.add_argument("--json", action="store_true", help="output JSON instead of table")
    args = ap.parse_args()

    if args.validate:
        val = validate_holdout_set()
        if args.json:
            print(json.dumps(val, indent=2))
        else:
            if val["valid"]:
                print(f"VALID: {val['holdout_count']} holdout sites verified. 0 overlap with top20, 100% in top80.")
            else:
                print(f"INVALID holdout set: {val['errors']}")
        sys.exit(0 if val["valid"] else 1)

    if args.compare_top20 and args.compare_holdout:
        top_res = score_dual_run(Path(args.compare_top20))
        ho_res = score_dual_run(Path(args.compare_holdout))
        comp = compare_boards(top_res, ho_res)
        if args.json:
            print(json.dumps(comp, indent=2))
        else:
            print(format_comparison_table(comp))
        sys.exit(0)

    if args.run:
        dual_res = score_dual_run(Path(args.run))
        if args.json:
            print(json.dumps(dual_res, indent=2))
        else:
            print(scorer_v2.format_table(dual_res["v2_summary"]))
        sys.exit(0)

    # Default action: run validation
    val = validate_holdout_set()
    if val["valid"]:
        print(f"VALID: {val['holdout_count']} holdout sites verified (websuite/realsite-holdout20.json).")
    else:
        print(f"INVALID: {val['errors']}")


if __name__ == "__main__":
    main()
