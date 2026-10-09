#!/usr/bin/env python3
"""Cascade matched-workload benchmark: RustKit cascade vs Chrome style on pinned bytes.

Package Z2-M1 (PLAN-z.md):
Measures RustKit cascade time against Chrome style recalculation self-time
on the EXACT SAME pinned snapshot bytes served locally on 127.0.0.1.

Rule A3 Compliance:
Preserves and publishes the legacy ratio beside the matched ratio.
  - Legacy Chrome style ms: frozen groundtruth from live pages (2026-09-26)
    cnn: 210.0 ms, github: 110.0 ms, wikipedia: 20.0 ms
  - Matched Chrome style ms: Chrome DevTools trace self-time
    (UpdateLayoutTree + RecalculateStyles) measured on the same pinned snapshot bytes.

Usage:
  python3 scripts/cascade_bench.py --capture <path/to/parity-capture> [--runs 5]
  python3 scripts/cascade_bench.py --chrome-trace cnn=trace_cnn.json --chrome-trace wikipedia=trace_wiki.json
  python3 scripts/cascade_bench.py --measure-chrome --capture <path/to/parity-capture>
"""

from __future__ import annotations

import argparse
import functools
import gzip
import http.server
import json
import os
import re
import statistics
import subprocess
import sys
import threading
from pathlib import Path
from typing import Any, Dict, List, Optional, Tuple

HERE = Path(__file__).resolve().parent
REPO_ROOT = HERE.parent
DEFAULT_SNAPSHOTS = REPO_ROOT / "trench" / "cascade" / "snapshots"

# Frozen legacy Chrome 148 style ms (UpdateLayoutTree + RecalculateStyles self time
# from live ground-truth analysis 2026-09-26).
# RULE A3: Frozen baseline — never altered.
CHROME_STYLE_MS_LEGACY: Dict[str, float] = {
    "cnn": 210.0,
    "github": 110.0,
    "wikipedia": 20.0,
}

PARSE_MS_RE = re.compile(r"parse_ms=([\d.]+)")
CASCADE_MS_RE = re.compile(r"cascade_ms=([\d.]+)")
ANSI_RE = re.compile(r"\x1b\[[0-9;]*m")

# Chrome DevTools event category map
_CAT_MAP = {
    "script": {
        "EvaluateScript", "FunctionCall", "v8.compile", "v8.compileModule", "V8.CompileCode",
        "v8.evaluateModule", "TimerFire", "EventDispatch", "RunMicrotasks", "v8.run",
        "FireAnimationFrame", "FireIdleCallback", "XHRReadyStateChange", "XHRLoad",
        "v8.callFunction", "V8.Execute", "v8.produceCache", "v8.deserializeOnBackground",
        "CompileScript", "CompileCode", "EvaluateModule",
    },
    "parse": {"ParseHTML", "ParseAuthorStyleSheet"},
    "style": {"UpdateLayoutTree", "RecalculateStyles", "ScheduleStyleRecalculation"},
    "layout": {"Layout", "UpdateLayerTree"},
    "paint": {
        "Paint", "PaintImage", "PrePaint", "Layerize", "CompositeLayers",
        "Commit", "RasterTask", "Decode Image", "ImageDecodeTask",
    },
    "gc": {
        "MinorGC", "MajorGC", "BlinkGC.AtomicPhase", "V8.GCScavenger", "V8.GCCompactor",
        "V8.GCFinalizeMC", "V8.GC_MC_BACKGROUND_MARKING", "V8.GCIncrementalMarking",
        "V8.GCFinalizeMCReduceMemory", "MinorMS", "BlinkGC.IncrementalMarkingStep",
    },
}
_EXACT_CAT = {name: cat for cat, names in _CAT_MAP.items() for name in names}


def cat_of(name: str) -> Optional[str]:
    """Map a Chrome DevTools trace event name to its execution category."""
    if name in _EXACT_CAT:
        return _EXACT_CAT[name]
    if name.startswith(("V8.GC", "BlinkGC")) or "GC_" in name:
        return "gc"
    if name.startswith(("v8.", "V8.", "LocalWindowProxy")):
        return "script"
    if name.startswith(("CSSParser", "HTMLPreloadScanner", "HTMLDocumentParser")):
        return "parse"
    if name.startswith(("Document::recalcStyle", "StyleEngine", "StyleResolver")):
        return "style"
    if name.startswith(("LocalFrameView::performLayout", "LayoutNG", "LayoutView")):
        return "layout"
    if "Paint" in name or name.startswith(("cc::", "Raster")):
        return "paint"
    return None


def summarise_chrome_trace(events: List[Dict[str, Any]]) -> Optional[Dict[str, Any]]:
    """Summarise Chrome trace events into self-time per phase for the renderer main thread.

    Follows the ground-truth interval stack algorithm from trench/tools/chrome_groundtruth.mjs.
    Returns:
        {
            "main_thread_busy_ms": float,
            "self_ms": {"style": float, "layout": float, "script": float, ...},
            "style_recalcs": int,
            "style_elements": int,
            "layout": int,
            "layout_dirty_objects": int,
            "minor_gc": int,
            "major_gc": int,
        }
    """
    thread_names: Dict[str, str] = {}
    for e in events:
        if e.get("ph") == "M" and e.get("name") == "thread_name":
            key = f"{e.get('pid')}:{e.get('tid')}"
            thread_names[key] = (e.get("args") or {}).get("name", "")

    busy_durations: Dict[str, float] = {}
    for e in events:
        if e.get("ph") == "X" and e.get("dur") is not None:
            key = f"{e.get('pid')}:{e.get('tid')}"
            if thread_names.get(key) == "CrRendererMain":
                busy_durations[key] = busy_durations.get(key, 0.0) + float(e["dur"])

    main_thread = None
    if busy_durations:
        main_thread = max(busy_durations.items(), key=lambda kv: kv[1])[0]
    else:
        # Fallback: thread carrying the most complete-event time overall
        all_busy: Dict[str, float] = {}
        for e in events:
            if e.get("ph") == "X" and e.get("dur") is not None:
                key = f"{e.get('pid')}:{e.get('tid')}"
                all_busy[key] = all_busy.get(key, 0.0) + float(e["dur"])
        if all_busy:
            main_thread = max(all_busy.items(), key=lambda kv: kv[1])[0]

    if not main_thread:
        return None

    xs = [
        e for e in events
        if e.get("ph") == "X" and e.get("dur") is not None and f"{e.get('pid')}:{e.get('tid')}" == main_thread
    ]
    # Sort by start timestamp ascending, tie-break by duration descending (parent first)
    xs.sort(key=lambda e: (float(e["ts"]), -float(e["dur"])))

    self_durations: Dict[str, float] = {
        "parse": 0.0, "script": 0.0, "style": 0.0, "layout": 0.0,
        "paint": 0.0, "gc": 0.0, "other": 0.0,
    }
    counts = {
        "layout": 0, "layout_dirty_objects": 0,
        "style_recalcs": 0, "style_elements": 0,
        "minor_gc": 0, "major_gc": 0,
    }

    total_dur = 0.0
    # Stack entries: [end_ts, category, child_dur, total_dur]
    stack: List[List[Any]] = []

    def close_frame(frame: List[Any]) -> None:
        exclusive = max(0.0, frame[3] - frame[2])
        category = frame[1] or "other"
        self_durations[category] = self_durations.get(category, 0.0) + exclusive

    for e in xs:
        ts = float(e["ts"])
        dur = float(e["dur"])
        end_ts = ts + dur

        while stack and stack[-1][0] <= ts:
            close_frame(stack.pop())

        parent = stack[-1] if stack else None
        c = cat_of(e.get("name", "")) or (parent[1] if parent else None)

        if parent:
            parent[2] += dur
        else:
            total_dur += dur

        stack.append([end_ts, c, 0.0, dur])

        ename = e.get("name", "")
        args = e.get("args") or {}
        if ename == "Layout":
            counts["layout"] += 1
            counts["layout_dirty_objects"] += (args.get("beginData") or {}).get("dirtyObjects", 0)
        elif ename == "UpdateLayoutTree":
            counts["style_recalcs"] += 1
            counts["style_elements"] += args.get("elementCount", 0)
        elif ename in ("MinorGC", "MinorMS"):
            counts["minor_gc"] += 1
        elif ename == "MajorGC":
            counts["major_gc"] += 1

    while stack:
        close_frame(stack.pop())

    self_ms = {k: round(v / 1000.0, 1) for k, v in self_durations.items()}
    return {
        "main_thread_busy_ms": round(total_dur / 1000.0, 1),
        "self_ms": self_ms,
        **counts,
    }


def load_chrome_trace(path: str | Path) -> Optional[Dict[str, Any]]:
    """Load trace JSON or gzipped trace and compute summary."""
    p = Path(path)
    if not p.is_file():
        return None
    try:
        if p.name.endswith(".gz"):
            with gzip.open(p, "rt", encoding="utf-8", errors="replace") as f:
                data = json.load(f)
        else:
            with open(p, "r", encoding="utf-8", errors="replace") as f:
                data = json.load(f)
    except Exception as exc:
        print(f"Warning: failed to parse trace {path}: {exc}", file=sys.stderr)
        return None

    events = data.get("traceEvents") if isinstance(data, dict) else data
    if not isinstance(events, list):
        return None
    return summarise_chrome_trace(events)


class _QuietHTTPHandler(http.server.SimpleHTTPRequestHandler):
    def log_message(self, *args: Any) -> None:
        pass


def serve_snapshots(directory: Path | str) -> http.server.ThreadingHTTPServer:
    """Serve snapshot directory on 127.0.0.1 on an ephemeral port."""
    handler = functools.partial(_QuietHTTPHandler, directory=str(directory))
    srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    return srv


def extract_timings(stderr: str) -> List[Tuple[float, float]]:
    """Parse (parse_ms, cascade_ms) per 'Cascade timing' line in stderr."""
    out: List[Tuple[float, float]] = []
    clean = ANSI_RE.sub("", stderr)
    for line in clean.splitlines():
        if "Cascade timing" in line:
            p = PARSE_MS_RE.search(line)
            c = CASCADE_MS_RE.search(line)
            if p and c:
                out.append((float(p.group(1)), float(c.group(1))))
    return out


def run_rustkit_once(
    capture_bin: str,
    url: str,
    timeout_ms: int,
    log_path: Optional[str | Path] = None,
    extra_env: Optional[Dict[str, str]] = None,
) -> Dict[str, Any]:
    """Execute a single RustKit parity-capture run and extract cascade timing."""
    env = dict(
        os.environ,
        RUSTKIT_CASCADE_TIMING="1",
        RUST_LOG="warn,rustkit_engine=info",
        NO_COLOR="1",
    )
    if extra_env:
        env.update(extra_env)

    proc = subprocess.run(
        [
            capture_bin,
            "--url", url,
            "--width", "1280",
            "--height", "800",
            "--timeout-ms", str(timeout_ms),
        ],
        env=env,
        capture_output=True,
        text=True,
        errors="replace", encoding="utf-8"
    )

    if log_path:
        with open(log_path, "w", encoding="utf-8") as f:
            f.write(proc.stderr)

    builds = extract_timings(proc.stderr)
    return {
        "exit": proc.returncode,
        "builds": len(builds),
        "parse_ms": round(sum(b[0] for b in builds), 1),
        "cascade_ms": round(sum(b[1] for b in builds), 1),
        "per_build_ms": [round(b[1], 1) for b in builds],
    }


def format_table(results: Dict[str, Any], runs: int) -> str:
    """Format benchmark results into Markdown table showing both legacy and matched ratios."""
    lines = [
        f"| site | Chrome legacy ms | Chrome matched ms | RustKit cascade ms (median of {runs}) | builds | parse ms | Legacy ratio | Matched ratio | raw cascade ms |",
        "|---|---|---|---|---|---|---|---|---|",
    ]
    for site, s in results.items():
        leg_ms = s.get("chrome_legacy_style_ms")
        leg_str = f"{leg_ms:.0f}" if leg_ms is not None else "-"
        mat_ms = s.get("chrome_matched_style_ms")
        mat_str = f"{mat_ms:.1f}" if mat_ms is not None else "n/a"

        rk_med = s.get("median_cascade_ms")
        rk_str = f"{rk_med:.0f}" if rk_med is not None else "FAIL"

        builds_set = sorted({r["builds"] for r in s.get("runs", []) if "builds" in r})
        builds_str = "/".join(map(str, builds_set)) if builds_set else "-"

        parse_med = s.get("median_parse_ms")
        parse_str = f"{parse_med:.0f}" if parse_med is not None else "-"

        leg_ratio = s.get("legacy_ratio")
        leg_ratio_str = f"{leg_ratio:.1f}×" if leg_ratio is not None else "-"

        mat_ratio = s.get("matched_ratio")
        mat_ratio_str = f"{mat_ratio:.1f}×" if mat_ratio is not None else "n/a"

        raw_cascades = [f"{r['cascade_ms']:.0f}" for r in s.get("runs", []) if "cascade_ms" in r]
        raw_str = ", ".join(raw_cascades) if raw_cascades else "-"

        lines.append(
            f"| {site} | {leg_str} | {mat_str} | {rk_str} | {builds_str} | {parse_str} | {leg_ratio_str} | {mat_ratio_str} | {raw_str} |"
        )
    return "\n".join(lines)


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Cascade matched-workload benchmark: RustKit cascade vs Chrome style."
    )
    parser.add_argument("--capture", help="path to release parity-capture binary")
    parser.add_argument("--runs", type=int, default=5, help="runs per site (default: 5)")
    parser.add_argument(
        "--sites",
        default=",".join(CHROME_STYLE_MS_LEGACY.keys()),
        help=f"comma-separated sites (default: {','.join(CHROME_STYLE_MS_LEGACY.keys())})",
    )
    parser.add_argument(
        "--snapshots-dir",
        default=str(DEFAULT_SNAPSHOTS),
        help=f"directory of pinned snapshots (default: {DEFAULT_SNAPSHOTS})",
    )
    parser.add_argument("--timeout-ms", type=int, default=120000, help="timeout per capture in ms")
    parser.add_argument("--json", help="path to write JSON output")
    parser.add_argument("--logs", help="directory to store raw stderr logs")
    parser.add_argument(
        "--env",
        action="append",
        default=[],
        metavar="KEY=VAL",
        help="extra env var for capture; repeatable",
    )
    parser.add_argument(
        "--chrome-trace",
        action="append",
        default=[],
        metavar="SITE=PATH",
        help="path to pre-captured Chrome trace for a site; repeatable",
    )
    parser.add_argument(
        "--chrome-traces-dir",
        help="directory containing pre-captured <site>.json(.gz) Chrome traces",
    )
    parser.add_argument(
        "--chrome-style-ms",
        action="append",
        default=[],
        metavar="SITE=MS",
        help="override matched Chrome style ms for a site; repeatable",
    )
    parser.add_argument(
        "--measure-chrome",
        action="store_true",
        help="execute Playwright Chromium trace pass on the snapshot URLs",
    )

    args = parser.parse_args()
    extra_env = dict(kv.split("=", 1) for kv in args.env)
    sites = [s.strip() for s in args.sites.split(",") if s.strip()]

    # Parse trace arguments
    trace_files: Dict[str, Path] = {}
    for entry in args.chrome_trace:
        if "=" in entry:
            site, path = entry.split("=", 1)
            trace_files[site.strip()] = Path(path.strip())

    if args.chrome_traces_dir:
        tdir = Path(args.chrome_traces_dir)
        for s in sites:
            if s not in trace_files:
                for candidate in (tdir / f"{s}.json", tdir / f"{s}.json.gz"):
                    if candidate.is_file():
                        trace_files[s] = candidate
                        break

    # Parse style overrides
    chrome_matched_overrides: Dict[str, float] = {}
    for entry in args.chrome_style_ms:
        if "=" in entry:
            site, ms = entry.split("=", 1)
            chrome_matched_overrides[site.strip()] = float(ms.strip())

    if args.logs:
        os.makedirs(args.logs, exist_ok=True)

    snapshots_dir = Path(args.snapshots_dir)
    srv: Optional[http.server.ThreadingHTTPServer] = None
    port = 0
    if snapshots_dir.is_dir():
        srv = serve_snapshots(snapshots_dir)
        port = srv.server_address[1]

    results: Dict[str, Any] = {}

    try:
        for site in sites:
            matched_style_ms: Optional[float] = None
            trace_summary: Optional[Dict[str, Any]] = None

            # 1. Resolve matched Chrome style time
            if site in chrome_matched_overrides:
                matched_style_ms = chrome_matched_overrides[site]
            elif site in trace_files:
                trace_summary = load_chrome_trace(trace_files[site])
                if trace_summary and "self_ms" in trace_summary:
                    matched_style_ms = trace_summary["self_ms"].get("style")
            elif args.measure_chrome and port:
                # Live trace pass using helper script if available
                helper_script = HERE / "chrome_trace_cascade.mjs"
                trace_out = Path(args.logs or ".") / f"trace_{site}.json"
                url = f"http://127.0.0.1:{port}/{site}/index.html"
                if helper_script.is_file():
                    proc = subprocess.run(
                        ["node", str(helper_script), "--url", url, "--out", str(trace_out)],
                        capture_output=True, text=True, errors="replace", encoding="utf-8"
                    )
                    if proc.returncode == 0 and trace_out.is_file():
                        trace_summary = load_chrome_trace(trace_out)
                        if trace_summary and "self_ms" in trace_summary:
                            matched_style_ms = trace_summary["self_ms"].get("style")

            # 2. Run RustKit capture if binary is specified
            runs_data: List[Dict[str, Any]] = []
            if args.capture:
                if not port:
                    print(f"Error: snapshot directory not found at {snapshots_dir}", file=sys.stderr)
                    return 2
                url = f"http://127.0.0.1:{port}/{site}/index.html"
                for i in range(args.runs):
                    log_file = (
                        os.path.join(args.logs, f"{site}-{i + 1}.log") if args.logs else None
                    )
                    r = run_rustkit_once(args.capture, url, args.timeout_ms, log_file, extra_env)
                    runs_data.append(r)
                    print(
                        f"  {site:<10} run {i + 1}: exit {r['exit']} builds {r['builds']} "
                        f"cascade {r['cascade_ms']:8.1f} ms parse {r['parse_ms']:7.1f} ms",
                        file=sys.stderr,
                        flush=True,
                    )

            good_runs = [r for r in runs_data if r.get("exit") == 0 and r.get("builds", 0) > 0]
            med_cascade = (
                statistics.median(r["cascade_ms"] for r in good_runs) if good_runs else None
            )
            med_parse = (
                statistics.median(r["parse_ms"] for r in good_runs) if good_runs else None
            )

            legacy_chrome_ms = CHROME_STYLE_MS_LEGACY.get(site)
            legacy_ratio = (
                round(med_cascade / legacy_chrome_ms, 1)
                if med_cascade is not None and legacy_chrome_ms
                else None
            )
            matched_ratio = (
                round(med_cascade / matched_style_ms, 1)
                if med_cascade is not None and matched_style_ms
                else None
            )

            results[site] = {
                "runs": runs_data,
                "median_cascade_ms": med_cascade,
                "median_parse_ms": med_parse,
                "chrome_legacy_style_ms": legacy_chrome_ms,
                "chrome_matched_style_ms": matched_style_ms,
                "legacy_ratio": legacy_ratio,
                "matched_ratio": matched_ratio,
                "trace_summary": trace_summary,
            }
    finally:
        if srv:
            srv.shutdown()

    # Print markdown table
    print("\n" + format_table(results, args.runs))

    legacy_ratios = [s["legacy_ratio"] for s in results.values() if s.get("legacy_ratio")]
    worst_legacy = max(legacy_ratios) if legacy_ratios else None
    matched_ratios = [s["matched_ratio"] for s in results.values() if s.get("matched_ratio")]
    worst_matched = max(matched_ratios) if matched_ratios else None

    print(
        f"\nworst legacy ratio:  {f'{worst_legacy:.1f}×' if worst_legacy else 'n/a'}"
    )
    print(
        f"worst matched ratio: {f'{worst_matched:.1f}×' if worst_matched else 'n/a'}"
    )

    if args.json:
        payload = {
            "sites": results,
            "worst_legacy_ratio": worst_legacy,
            "worst_matched_ratio": worst_matched,
            "rule_a3_note": (
                "Both legacy live groundtruth and matched pinned trace ratios published beside each other."
            ),
        }
        with open(args.json, "w", encoding="utf-8") as f:
            json.dump(payload, f, indent=2)

    return 0


if __name__ == "__main__":
    sys.exit(main())
