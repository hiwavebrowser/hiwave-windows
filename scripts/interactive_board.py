#!/usr/bin/env python3
"""
interactive_board.py — Interactive Actions Runner & Per-Step Frame Diff (Package Z2-I1).

Executes scripted interaction sequences (wait/click/key/resize/capture) across
both RustKit (parity-capture --actions) and pinned Chrome (Playwright),
evaluating per-step visual frame differences and DOM/visual responsiveness.

Publishes diagnostics beside the legacy board under Rule A3.
"""

import argparse
import json
import os
import shutil
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Callable, Dict, List, Optional, Tuple

REPO = Path(__file__).resolve().parent.parent
CATALOG_PATH = REPO / "websuite" / "interactions-top20.json"
ORACLE_SCRIPT = REPO / "tools" / "parity_oracle" / "realsite.mjs"
DEFAULT_SITES_FILE = REPO / "websuite" / "realsite-top20.json"
DEFAULT_CAPTURE_BIN = (
    REPO / "target" / "release" / ("parity-capture.exe" if sys.platform == "win32" else "parity-capture")
)
DEFAULT_CHROME = Path(os.environ.get("PARITY_CHROME_PATH", "")) if os.environ.get("PARITY_CHROME_PATH") else None
if not DEFAULT_CHROME or not DEFAULT_CHROME.exists():
    DEFAULT_CHROME = Path.home() / "chrome-for-testing" / "chrome" / "win64-148.0.7778.216" / "chrome-win64" / "chrome.exe"
if not DEFAULT_CHROME.exists():
    DEFAULT_CHROME = Path.home() / "Repos" / "hiwave" / "hiwave-macos" / ".browsers" / "chrome" / "mac_arm-148.0.7778.216"


def validate_action_sequence(actions: List[Dict[str, Any]]) -> List[str]:
    """Validate action schema for supported primitives: wait, click, key, resize, capture."""
    errors = []
    if not isinstance(actions, list):
        return ["actions must be a list of action objects"]

    for i, a in enumerate(actions):
        if not isinstance(a, dict):
            errors.append(f"action[{i}]: must be an object")
            continue
        atype = a.get("type")
        if atype not in ("wait", "click", "key", "resize", "capture"):
            errors.append(f"action[{i}]: unsupported action type '{atype}'")
            continue

        if atype == "wait":
            ms = a.get("ms")
            selector = a.get("selector")
            if ms is None and selector is None:
                errors.append(f"action[{i}]: wait requires 'ms' or 'selector'")
            if ms is not None and (not isinstance(ms, (int, float)) or ms < 0):
                errors.append(f"action[{i}]: wait 'ms' must be non-negative number")

        elif atype == "click":
            selector = a.get("selector")
            x, y = a.get("x"), a.get("y")
            if selector is None and (x is None or y is None):
                errors.append(f"action[{i}]: click requires 'selector' or both 'x' and 'y'")

        elif atype == "key":
            text = a.get("text")
            key = a.get("key")
            if text is None and key is None:
                errors.append(f"action[{i}]: key requires 'text' or 'key'")

        elif atype == "resize":
            w = a.get("width")
            h = a.get("height")
            if not isinstance(w, int) or w <= 0 or not isinstance(h, int) or h <= 0:
                errors.append(f"action[{i}]: resize requires positive integer 'width' and 'height'")

        elif atype == "capture":
            frame = a.get("frame")
            if not frame:
                errors.append(f"action[{i}]: capture requires 'frame' filename")

    return errors


def catalog_to_actions(site_id: str, catalog_entry: Dict[str, Any]) -> List[Dict[str, Any]]:
    """Convert an interactions-top20 catalog entry into an action sequence."""
    target_selector = catalog_entry.get("target_selector", "")
    action = catalog_entry.get("action", "click")
    timeout_ms = catalog_entry.get("timeout_ms", 1000)

    seq = [
        {"type": "capture", "label": "before", "frame": f"{site_id}_before.png", "step": 0},
    ]

    if action == "click":
        seq.append({"type": "click", "selector": target_selector, "step": 1})
    elif action == "key":
        seq.append({"type": "key", "selector": target_selector, "text": "test", "step": 1})
    else:
        seq.append({"type": "click", "selector": target_selector, "step": 1})

    seq.append({"type": "wait", "ms": timeout_ms, "step": 2})
    seq.append({"type": "capture", "label": "after", "frame": f"{site_id}_after.png", "step": 3})

    return seq


def compute_step_diffs(
    chrome_captures: List[Dict[str, Any]],
    rustkit_captures: List[Dict[str, Any]],
    diff_fn: Optional[Callable[[Path, Path], Dict[str, Any]]] = None,
) -> List[Dict[str, Any]]:
    """Compute per-step frame differences between Chrome and RustKit captures."""
    report = []
    # Index captures by step/label
    chrome_by_label = {c.get("label", str(c.get("step"))): c for c in chrome_captures}
    rustkit_by_label = {r.get("label", str(r.get("step"))): r for r in rustkit_captures}

    common_labels = [l for l in chrome_by_label if l in rustkit_by_label]

    for label in common_labels:
        c_item = chrome_by_label[label]
        r_item = rustkit_by_label[label]
        c_frame = Path(c_item["frame"])
        r_frame = Path(r_item["frame"])

        diff_res = {}
        if diff_fn:
            diff_res = diff_fn(c_frame, r_frame)

        diff_pct = diff_res.get("diffPercent", 0.0)
        report.append({
            "step": c_item.get("step", r_item.get("step")),
            "label": label,
            "chrome_frame": str(c_frame),
            "rustkit_frame": str(r_frame),
            "diff_percent": diff_pct,
            "raw": diff_res,
        })

    return report


def run_chrome_actions(
    url: str,
    actions: List[Dict[str, Any]],
    out_dir: Path,
    chrome_bin: Optional[Path] = None,
    width: int = 1280,
    height: int = 800,
    settle_ms: int = 3000,
) -> Dict[str, Any]:
    """Execute action sequence in Chrome via Playwright realsite.mjs mirror."""
    out_dir.mkdir(parents=True, exist_ok=True)
    actions_json = json.dumps(actions)
    env = os.environ.copy()
    if chrome_bin and Path(chrome_bin).exists():
        env["CHROME_BIN"] = str(chrome_bin)
        env["PARITY_CHROME_PATH"] = str(chrome_bin)

    cmd = [
        "node",
        str(ORACLE_SCRIPT),
        "actions",
        url,
        actions_json,
        str(out_dir),
        str(width),
        str(height),
        str(settle_ms),
    ]
    try:
        proc = subprocess.run(cmd, capture_output=True, text=True, timeout=60, env=env, encoding="utf-8")
        stdout = proc.stdout.strip()
        lines = stdout.splitlines()
        for line in reversed(lines):
            line = line.strip()
            if line.startswith("{") and line.endswith("}"):
                return json.loads(line)
        return {"status": "error", "error": proc.stderr or stdout}
    except Exception as e:
        return {"status": "error", "error": str(e)}


def run_rustkit_actions(
    url: str,
    actions: List[Dict[str, Any]],
    out_dir: Path,
    capture_bin: Path,
    width: int = 1280,
    height: int = 800,
    timeout_ms: int = 35000,
) -> Dict[str, Any]:
    """Execute action sequence in RustKit via parity-capture --actions."""
    out_dir.mkdir(parents=True, exist_ok=True)
    actions_json = json.dumps(actions)
    dump_scripts_path = out_dir / "rustkit-scripts.json"
    if dump_scripts_path.exists():
        try:
            dump_scripts_path.unlink()
        except OSError:
            pass
    cmd = [
        str(capture_bin),
        "--url",
        url,
        "--actions",
        actions_json,
        "--actions-out-dir",
        str(out_dir),
        "--dump-scripts",
        str(dump_scripts_path),
        "--width",
        str(width),
        "--height",
        str(height),
        "--timeout-ms",
        str(timeout_ms),
    ]
    res = {}
    try:
        proc = subprocess.run(cmd, capture_output=True, text=True, timeout=60, encoding="utf-8")
        stdout = proc.stdout.strip()
        lines = stdout.splitlines()
        for line in reversed(lines):
            line = line.strip()
            if line.startswith("{") and line.endswith("}"):
                res = json.loads(line)
                break
        if not res:
            res = {"status": "error", "error": proc.stderr or stdout}
    except Exception as e:
        res = {"status": "error", "error": str(e)}

    first_error = None
    if dump_scripts_path.exists():
        try:
            s_data = json.loads(dump_scripts_path.read_text(encoding="utf-8", errors="replace"))
            scripts_list = s_data.get("scripts", []) if isinstance(s_data, dict) else s_data
            for rec in scripts_list:
                if rec.get("outcome") == "threw":
                    first_error = rec.get("detail") or rec.get("outcome")
                    break
        except Exception:
            pass
    res["first_script_error"] = first_error
    return res


def diff_frames(frame_a: Path, frame_b: Path, diff_path: Optional[Path] = None) -> Dict[str, Any]:
    """Compare two frames using compare_pixels.mjs."""
    cmd = ["node", str(ORACLE_SCRIPT), "diff", str(frame_a), str(frame_b)]
    if diff_path:
        cmd.append(str(diff_path))
    try:
        proc = subprocess.run(cmd, capture_output=True, text=True, timeout=30, encoding="utf-8")
        stdout = proc.stdout.strip()
        lines = stdout.splitlines()
        for line in reversed(lines):
            line = line.strip()
            if line.startswith("{") and line.endswith("}"):
                return json.loads(line)
        return {"diffPercent": 100.0, "error": proc.stderr or stdout}
    except Exception as e:
        return {"diffPercent": 100.0, "error": str(e)}


def classify_interactive_outcome(s_data: Dict[str, Any]) -> str:
    """Classify interactive outcome according to REAL_SITE_INTERACTIVE_DESIGN_2026-10-03.md: PASS, fail, unstable."""
    chrome_status = s_data.get("chrome_status")
    rk_status = s_data.get("rustkit_status")

    if chrome_status != "ok":
        return "unstable"

    chrome_actions = s_data.get("chrome_action_results", [])
    chrome_click = next((a for a in chrome_actions if a.get("type") in ("click", "key")), None)
    if not chrome_click or chrome_click.get("status") != "ok":
        return "unstable"

    if rk_status != "ok":
        return "fail"

    rk_actions = s_data.get("rustkit_action_results", [])
    rk_click = next((a for a in rk_actions if a.get("type") in ("click", "key")), None)
    if not rk_click or rk_click.get("status") != "ok":
        return "fail"

    r_delta = s_data.get("rustkit_action_delta")
    c_delta = s_data.get("chrome_action_delta")

    # If Chrome showed negligible visual movement (< 0.1%), the action was visually inert in the oracle
    if c_delta is not None and c_delta < 0.1:
        if r_delta is not None and r_delta < 0.1:
            return "unstable"

    # Measurable visual delta on RustKit (>= 0.5%) confirms responsiveness
    if r_delta is not None and r_delta >= 0.5:
        return "PASS"

    return "fail"


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--site", action="append", help="site id to test from catalog")
    ap.add_argument("--actions-file", help="custom actions JSON file to execute")
    ap.add_argument("--catalog", default=str(CATALOG_PATH), help="interactions catalog JSON")
    ap.add_argument("--sites-file", default=str(DEFAULT_SITES_FILE), help="realsite top-20 JSON")
    ap.add_argument("--capture-bin", default=str(DEFAULT_CAPTURE_BIN), help="parity-capture binary path")
    ap.add_argument("--chrome-bin", default=str(DEFAULT_CHROME), help="chrome binary path")
    ap.add_argument("--out-dir", help="output directory for captures and diffs")
    ap.add_argument("--json", action="store_true", help="output summary JSON on stdout")
    args = ap.parse_args()

    cat_path = Path(args.catalog)
    if not cat_path.exists():
        sys.exit(f"interactions catalog missing: {cat_path}")

    sites_path = Path(args.sites_file)
    url_map = {}
    if sites_path.exists():
        sites_data = json.loads(sites_path.read_text(encoding="utf-8"))
        for s in sites_data.get("sites", []):
            url_map[s["id"]] = s["url"]

    catalog = json.loads(cat_path.read_text(encoding="utf-8")).get("interactions", {})
    target_sites = args.site if args.site else list(catalog.keys())

    run_timestamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    run_dir = Path(args.out_dir) if args.out_dir else REPO / "trench" / "realsite" / "interactive_runs" / run_timestamp
    run_dir.mkdir(parents=True, exist_ok=True)

    summary = {
        "timestamp": run_timestamp,
        "capture_bin": str(args.capture_bin),
        "target_count": len(target_sites),
        "sites": {},
    }

    for site_id in target_sites:
        cat_entry = catalog.get(site_id, {})
        url = cat_entry.get("url") or url_map.get(site_id, f"https://www.{site_id}.com/")
        actions = catalog_to_actions(site_id, cat_entry)

        site_dir = run_dir / site_id
        chrome_dir = site_dir / "chrome"
        rk_dir = site_dir / "rustkit"
        diff_dir = site_dir / "diffs"
        diff_dir.mkdir(parents=True, exist_ok=True)

        chrome_res = run_chrome_actions(url, actions, chrome_dir, chrome_bin=Path(args.chrome_bin))
        rk_res = run_rustkit_actions(url, actions, rk_dir, capture_bin=Path(args.capture_bin))

        c_captures = chrome_res.get("captures", [])
        r_captures = rk_res.get("captures", [])

        def _step_diff(a_p: Path, b_p: Path) -> Dict[str, Any]:
            diff_png = diff_dir / f"diff_{a_p.stem}_vs_{b_p.stem}.png"
            return diff_frames(a_p, b_p, diff_png)

        step_diffs = compute_step_diffs(c_captures, r_captures, diff_fn=_step_diff)

        # Compute internal responsiveness deltas (before vs after frame diff)
        c_before = next((c["frame"] for c in c_captures if c.get("label") == "before"), None)
        c_after = next((c["frame"] for c in c_captures if c.get("label") == "after"), None)
        r_before = next((r["frame"] for r in r_captures if r.get("label") == "before"), None)
        r_after = next((r["frame"] for r in r_captures if r.get("label") == "after"), None)

        c_delta = diff_frames(Path(c_before), Path(c_after)).get("diffPercent", 0.0) if c_before and c_after else None
        r_delta = diff_frames(Path(r_before), Path(r_after)).get("diffPercent", 0.0) if r_before and r_after else None

        site_summary = {
            "url": url,
            "chrome_status": chrome_res.get("status"),
            "rustkit_status": rk_res.get("status"),
            "chrome_error": chrome_res.get("error"),
            "rustkit_error": rk_res.get("error"),
            "first_script_error": rk_res.get("first_script_error"),
            "chrome_action_delta": c_delta,
            "rustkit_action_delta": r_delta,
            "step_diffs": step_diffs,
            "chrome_action_results": chrome_res.get("action_results", []),
            "rustkit_action_results": rk_res.get("action_results", []),
        }
        site_summary["outcome"] = classify_interactive_outcome(site_summary)
        summary["sites"][site_id] = site_summary

    summary_file = run_dir / "interactive_summary.json"
    summary_file.write_text(json.dumps(summary, indent=2), encoding="utf-8")

    if args.json:
        print(json.dumps(summary, indent=2))
    else:
        print(f"\nInteractive Board Run Complete. Saved to {run_dir}")
        print(f"{'Site':<14} {'Chrome':<7} {'RustKit':<8} {'Outcome':<8} {'C-Delta':<9} {'R-Delta':<9} {'BeforeDiff':<11} {'AfterDiff':<11} {'First Script Error'}")
        print("-" * 120)
        for s_id, s_data in summary["sites"].items():
            b_diff = next((d["diff_percent"] for d in s_data["step_diffs"] if d.get("label") == "before"), None)
            a_diff = next((d["diff_percent"] for d in s_data["step_diffs"] if d.get("label") == "after"), None)
            c_d = f"{s_data['chrome_action_delta']:.2f}%" if s_data['chrome_action_delta'] is not None else "N/A"
            r_d = f"{s_data['rustkit_action_delta']:.2f}%" if s_data['rustkit_action_delta'] is not None else "N/A"
            b_d = f"{b_diff:.2f}%" if b_diff is not None else "N/A"
            a_d = f"{a_diff:.2f}%" if a_diff is not None else "N/A"
            outcome = s_data.get("outcome", "unknown")
            err = s_data.get("first_script_error") or s_data.get("rustkit_error") or "none"
            err_str = str(err).replace("\n", " ")
            if len(err_str) > 40:
                err_str = err_str[:37] + "..."
            print(f"{s_id:<14} {s_data['chrome_status']:<7} {s_data['rustkit_status']:<8} {outcome:<8} {c_d:<9} {r_d:<9} {b_d:<11} {a_d:<11} {err_str}")


if __name__ == "__main__":
    main()
