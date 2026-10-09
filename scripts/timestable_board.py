#!/usr/bin/env python3
"""
timestable_board.py — Time-stable multi-sample real-site board runner.

Package Z2-M4 (PLAN-z / Directives Atlas #609 & #610).
Measures real-site visual parity across 4 temporal milestones (t = 1s, 3s, 5s, 10s)
in both Chrome and RustKit, supporting live and deterministic HAR record/replay.

Under Rule A3:
- Does NOT mutate the legacy 3-point scoring scale (20 sites * 3 points = 60 max).
- Publishes beside the legacy board into trench/realsite/timestable/.
"""

import argparse
import json
import os
import subprocess
import sys
import time
from pathlib import Path
from typing import Any, Dict, List, Optional, Tuple

REPO = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO))
sys.path.insert(0, str(REPO / "tools" / "parity_oracle"))

from tools.parity_oracle.har_server import start_har_server_in_thread

CATALOG_PATH = REPO / "websuite" / "realsite-top20.json"
ORACLE_SCRIPT = REPO / "tools" / "parity_oracle" / "realsite.mjs"
DEFAULT_OUT_DIR = REPO / "trench" / "realsite" / "timestable"

MILESTONES = [
    {"label": "1s", "time_ms": 1000},
    {"label": "3s", "time_ms": 3000},
    {"label": "5s", "time_ms": 5000},
    {"label": "10s", "time_ms": 10000},
]


def build_timestable_actions(prefix: str = "rustkit") -> List[Dict[str, Any]]:
    """Build the canonical actions JSON sequence for capturing at 1s, 3s, 5s, 10s."""
    actions: List[Dict[str, Any]] = []
    current_ms = 0
    step = 0
    for m in MILESTONES:
        delta = m["time_ms"] - current_ms
        if delta > 0:
            actions.append({"step": step, "type": "wait", "ms": delta})
            step += 1
        current_ms = m["time_ms"]
        actions.append(
            {
                "step": step,
                "type": "capture",
                "label": m["label"],
                "frame": f"{prefix}_{m['label']}.ppm",
            }
        )
        step += 1
    return actions


def classify_trajectory(
    diff_1s: Optional[float],
    diff_5s: Optional[float],
    diff_10s: Optional[float],
    c_motion: Optional[float],
) -> str:
    """Classify the temporal convergence trajectory."""
    if diff_1s is None or diff_10s is None:
        return "UNKNOWN"
    if c_motion is not None and c_motion > 5.0:
        return "DYNAMIC"
    diff_delta = diff_10s - diff_1s
    if abs(diff_delta) <= 1.0:
        return "STABLE"
    elif diff_delta < -1.0:
        return "CONVERGING"
    else:
        return "DIVERGING"


def compare_frames(frame_a: Path, frame_b: Path, diff_out: Optional[Path] = None) -> Optional[float]:
    """Run compare_pixels.mjs / realsite.mjs diff between two frames."""
    if not frame_a.exists() or not frame_b.exists():
        return None
    cmd = ["node", str(ORACLE_SCRIPT), "diff", str(frame_a), str(frame_b)]
    if diff_out:
        cmd.append(str(diff_out))
    try:
        proc = subprocess.run(cmd, capture_output=True, text=True, check=True, timeout=30, encoding="utf-8")
        out = json.loads(proc.stdout.strip())
        return float(out.get("diffPercent", 0.0))
    except Exception as e:
        sys.stderr.write(f"Diff error ({frame_a} vs {frame_b}): {e}\n")
        return None


def run_timestable_site(
    site_id: str,
    url: str,
    out_dir: Path,
    har_path: Optional[Path] = None,
    parity_capture_bin: Optional[Path] = None,
    width: int = 1280,
    height: int = 800,
) -> Dict[str, Any]:
    """Execute time-stable capture for a single site in both Chrome and RustKit."""
    site_out = out_dir / site_id
    site_out.mkdir(parents=True, exist_ok=True)

    result: Dict[str, Any] = {
        "site_id": site_id,
        "url": url,
        "status": "ok",
        "replay": bool(har_path),
        "milestones": {},
        "chrome_motion_5_10": None,
        "rustkit_motion_5_10": None,
        "trajectory": "UNKNOWN",
        "error": None,
    }

    # 1. Run Chrome timestable
    chrome_cmd = [
        "node",
        str(ORACLE_SCRIPT),
        "timestable",
        url,
        str(site_out),
        "chrome",
        str(har_path) if har_path else "",
        str(width),
        str(height),
    ]
    try:
        proc_c = subprocess.run(chrome_cmd, capture_output=True, text=True, check=True, timeout=60, encoding="utf-8")
        c_res = json.loads(proc_c.stdout.strip())
        if c_res.get("status") != "ok":
            result["status"] = "chrome_error"
            result["error"] = c_res.get("error")
            return result
    except Exception as e:
        result["status"] = "chrome_error"
        result["error"] = str(e)
        return result

    # 2. Run RustKit timestable via parity-capture --actions
    if parity_capture_bin and parity_capture_bin.exists():
        actions = build_timestable_actions(prefix="rustkit")
        actions_json = json.dumps(actions)

        # If HAR replay, start local HAR server
        har_server = None
        har_thread = None
        replay_proxy_url = None
        if har_path and har_path.exists():
            har_server, har_thread, replay_proxy_url = start_har_server_in_thread(har_path)

        try:
            rk_cmd = [
                str(parity_capture_bin),
                "--url",
                url,
                "--actions",
                actions_json,
                "--actions-out-dir",
                str(site_out),
                "--width",
                str(width),
                "--height",
                str(height),
                "--timer-horizon-ms",
                "10000",
                "--timeout-ms",
                "45000",
            ]
            if replay_proxy_url:
                rk_cmd.extend(["--replay-proxy", replay_proxy_url])

            proc_rk = subprocess.run(rk_cmd, capture_output=True, text=True, timeout=60, encoding="utf-8")
            rk_res = json.loads(proc_rk.stdout.strip())
            if rk_res.get("status") != "ok":
                result["status"] = "rustkit_error"
                result["error"] = rk_res.get("error")
        except Exception as e:
            result["status"] = "rustkit_error"
            result["error"] = str(e)
        finally:
            if har_server:
                har_server.shutdown()
                har_server.server_close()
                if har_thread:
                    har_thread.join(timeout=2)

    # 3. Compute Diffs and Internal Motion
    for m in MILESTONES:
        lbl = m["label"]
        c_frame = site_out / f"chrome_{lbl}.png"
        r_frame = site_out / f"rustkit_{lbl}.ppm"
        diff_png = site_out / f"diff_{lbl}.png"

        diff_pct = compare_frames(c_frame, r_frame, diff_png)
        result["milestones"][lbl] = {
            "chrome_frame": str(c_frame) if c_frame.exists() else None,
            "rustkit_frame": str(r_frame) if r_frame.exists() else None,
            "diff_percent": diff_pct,
        }

    # Internal motion 1s vs 10s and 5s vs 10s
    c_1s = site_out / "chrome_1s.png"
    c_5s = site_out / "chrome_5s.png"
    c_10s = site_out / "chrome_10s.png"
    r_1s = site_out / "rustkit_1s.ppm"
    r_5s = site_out / "rustkit_5s.ppm"
    r_10s = site_out / "rustkit_10s.ppm"

    result["chrome_motion_1_10"] = compare_frames(c_1s, c_10s)
    result["rustkit_motion_1_10"] = compare_frames(r_1s, r_10s)
    result["chrome_motion_5_10"] = compare_frames(c_5s, c_10s)
    result["rustkit_motion_5_10"] = compare_frames(r_5s, r_10s)

    c_m1_10 = result["chrome_motion_1_10"] or 0.0
    r_m1_10 = result["rustkit_motion_1_10"] or 0.0
    result["late_content"] = bool(c_m1_10 >= 1.0 and r_m1_10 < 0.2)

    d_1s = result["milestones"].get("1s", {}).get("diff_percent")
    d_5s = result["milestones"].get("5s", {}).get("diff_percent")
    d_10s = result["milestones"].get("10s", {}).get("diff_percent")

    result["trajectory"] = classify_trajectory(d_1s, d_5s, d_10s, result["chrome_motion_5_10"])
    return result


def main():
    parser = argparse.ArgumentParser(description="Time-stable multi-sample real-site board runner")
    parser.add_argument("--catalog", default=str(CATALOG_PATH), help="Path to catalog JSON")
    parser.add_argument("--out-dir", default=str(DEFAULT_OUT_DIR), help="Output directory")
    parser.add_argument("--site", help="Run a single site ID from catalog")
    parser.add_argument("--har-dir", help="Directory of pinned HAR archives for deterministic replay")
    parser.add_argument("--record-har", action="store_true", help="Record HAR files for target sites")
    parser.add_argument(
        "--bin",
        default=str(REPO / "target" / "release" / "parity-capture.exe"),
        help="Path to parity-capture binary",
    )
    args = parser.parse_args()

    out_dir = Path(args.out_dir)
    out_dir.mkdir(parents=True, exist_ok=True)

    catalog_path = Path(args.catalog)
    if not catalog_path.exists():
        sys.stderr.write(f"Error: catalog not found: {catalog_path}\n")
        sys.exit(1)

    catalog = json.loads(catalog_path.read_text(encoding="utf-8"))
    raw_sites = catalog.get("sites", {})
    if isinstance(raw_sites, list):
        sites = {s["id"]: s for s in raw_sites}
    else:
        sites = raw_sites

    if args.site:
        if args.site not in sites:
            sys.stderr.write(f"Error: site {args.site} not found in catalog\n")
            sys.exit(1)
        sites = {args.site: sites[args.site]}

    har_dir = Path(args.har_dir) if args.har_dir else None

    # Handle --record-har
    if args.record_har:
        if not har_dir:
            sys.stderr.write("Error: --har-dir is required when recording HAR archives\n")
            sys.exit(1)
        har_dir.mkdir(parents=True, exist_ok=True)
        for s_id, s_data in sites.items():
            u = s_data["url"]
            dest = har_dir / f"{s_id}.har"
            print(f"Recording HAR for {s_id} ({u}) -> {dest}...", flush=True)
            cmd = ["node", str(ORACLE_SCRIPT), "record-har", u, str(dest)]
            subprocess.run(cmd, check=True)
        print("HAR recording complete.")
        return

    parity_bin = Path(args.bin)
    results = []

    print(f"Starting Time-Stable Board across {len(sites)} sites...")
    for s_id, s_data in sites.items():
        u = s_data["url"]
        har_file = (har_dir / f"{s_id}.har") if har_dir else None
        print(f"\n--- Site: {s_id} ({u}) [replay={bool(har_file)}] ---", flush=True)
        site_res = run_timestable_site(
            site_id=s_id,
            url=u,
            out_dir=out_dir,
            har_path=har_file if (har_file and har_file.exists()) else None,
            parity_capture_bin=parity_bin,
        )
        results.append(site_res)

        ms = site_res["milestones"]
        d1 = ms.get("1s", {}).get("diff_percent")
        d3 = ms.get("3s", {}).get("diff_percent")
        d5 = ms.get("5s", {}).get("diff_percent")
        d10 = ms.get("10s", {}).get("diff_percent")
        c_m1_10 = site_res.get("chrome_motion_1_10")
        r_m1_10 = site_res.get("rustkit_motion_1_10")
        late = site_res.get("late_content")
        c_m_str = f"{c_m1_10:.2f}%" if c_m1_10 is not None else "N/A"
        r_m_str = f"{r_m1_10:.2f}%" if r_m1_10 is not None else "N/A"
        print(
            f"  1s: {d1}% | 3s: {d3}% | 5s: {d5}% | 10s: {d10}% | Trajectory: {site_res['trajectory']} | C-Mot(1-10): {c_m_str} | R-Mot(1-10): {r_m_str} | LateContent: {late}",
            flush=True,
        )

    summary_file = out_dir / "timestable_summary.json"
    summary_file.write_text(json.dumps({"results": results}, indent=2), encoding="utf-8")
    print(f"\nSummary report saved to {summary_file}")


if __name__ == "__main__":
    main()
