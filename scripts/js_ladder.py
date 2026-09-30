#!/usr/bin/env python3
"""js-ladder rung runner: score vendored JS fixtures against pinned Chrome 148.

Rung 1 is websuite/js-ladder/01-mdn (see its manifest.json). Each fixture is
loaded the way a browser loads it, scripts included, from an in-process
http://127.0.0.1 server so both engines load the same URL:

- Chrome: the parity oracle's deterministic launch (PARITY_CHROME_PATH = the
  pinned CfT 148), networkidle (tools/parity_oracle/capture_url.mjs). The
  frame is the committed baseline under
  baselines/chrome-148/js-ladder/<rung>/<id>/baseline.png.
- RustKit: `parity-capture --url` (URL mode runs page scripts; --html-file
  mode does not, and URL mode has no file:// support).

A fixture passes when (1) its pixel diff is within the campaign cap (t15,
the parity_lib THRESHOLDS ceiling), (2) RustKit ran every script (none
skipped, unfetched or over budget), and (3) RustKit threw exactly as many
uncaught errors as Chrome (baseline page-errors.json). (2) and (3) matter
because many MDN pages only change on click: their after-load frame matches
Chrome even when no script ran. The `nojs` column is the same page with
scripts off; where it also passes, the pixel check alone says nothing about
JS. This scores the page's state after load only; the click-driven states
of the event examples need an input driver and are a later rung.

    python3 scripts/js_ladder.py                    # score rung 1
    python3 scripts/js_ladder.py --capture-chrome   # (re)capture the oracle first
    python3 scripts/js_ladder.py --chrome-only      # (re)capture the oracle, score nothing
    python3 scripts/js_ladder.py --case 33-dom-example
"""
import functools
import json
import os
import subprocess
import sys
import threading
from datetime import datetime
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
RUNG = "01-mdn"
RUNG_DIR = REPO_ROOT / "websuite" / "js-ladder" / RUNG
BASELINE_DIR = REPO_ROOT / "baselines" / "chrome-148" / "js-ladder" / RUNG
OUT_DIR = REPO_ROOT / "parity-baseline" / "captures" / "js-ladder" / RUNG
PASS_PCT = 15.0


def capture_binary() -> Path:
    target = Path(os.environ.get("CARGO_TARGET_DIR", REPO_ROOT / "target"))
    return target / "release" / "parity-capture"


def node(script: str) -> dict:
    r = subprocess.run(
        ["node", "--input-type=module", "-e", script],
        capture_output=True, text=True, cwd=REPO_ROOT, timeout=120, encoding="utf-8"
    )
    lines = [l for l in r.stdout.splitlines() if l.startswith("{")]
    if r.returncode != 0 or not lines:
        raise RuntimeError((r.stderr or r.stdout)[-400:])
    return json.loads(lines[-1])


def serve(root: Path) -> str:
    """Serve `root` on an ephemeral 127.0.0.1 port; return the base URL."""
    handler = functools.partial(QuietHandler, directory=str(root))
    httpd = ThreadingHTTPServer(("127.0.0.1", 0), handler)
    threading.Thread(target=httpd.serve_forever, daemon=True).start()
    return f"http://127.0.0.1:{httpd.server_address[1]}"


class QuietHandler(SimpleHTTPRequestHandler):
    def log_message(self, *args):
        pass


def capture_chrome(url: str, out: Path, w: int, h: int) -> dict:
    if not os.environ.get("PARITY_CHROME_PATH"):
        raise RuntimeError("PARITY_CHROME_PATH is unset; the oracle must be the pinned CfT 148")
    r = subprocess.run(
        ["node", "tools/parity_oracle/capture_url.mjs", url, str(out / "baseline.png"), str(w), str(h)],
        capture_output=True, text=True, cwd=REPO_ROOT, timeout=120, encoding="utf-8"
    )
    lines = [l for l in r.stdout.splitlines() if l.startswith("{")]
    if r.returncode != 0 or not lines:
        raise RuntimeError((r.stderr or r.stdout)[-400:])
    res = json.loads(lines[-1])
    # Some MDN pages throw on purpose (the troubleshooting lesson); RustKit
    # must throw exactly as often as Chrome did.
    (out / "page-errors.json").write_text(json.dumps(res.get("pageErrors", []), indent=2) + "\n", encoding="utf-8")
    return res


def run_capture(source: list, out: Path, frame: str, w: int, h: int, extra: list) -> dict:
    out.mkdir(parents=True, exist_ok=True)
    cmd = [
        str(capture_binary()), *source,
        "--width", str(w), "--height", str(h),
        "--dump-frame", str(out / frame),
        "--timeout-ms", "30000", *extra,
    ]
    r = subprocess.run(cmd, capture_output=True, text=True, timeout=45, cwd=REPO_ROOT, encoding="utf-8")
    lines = [l for l in r.stdout.splitlines() if l.startswith("{")]
    return json.loads(lines[-1]) if lines else {"status": "error", "error": r.stderr[-300:]}


def capture_rustkit(url: str, out: Path, w: int, h: int) -> dict:
    return run_capture(["--url", url], out, "frame.ppm", w, h,
                       ["--dump-scripts", str(out / "scripts.json")])


def capture_nojs(page: Path, out: Path, w: int, h: int) -> dict:
    """Control: the same page with scripts off (--html-file mode). A fixture
    whose no-JS frame also matches Chrome doesn't test JS at load."""
    return run_capture(["--html-file", str(page)], out, "nojs.ppm", w, h, [])


def script_verdict(stats: dict, chrome_errors: list) -> str:
    """'' when RustKit ran every script and threw exactly as often as Chrome."""
    if not stats:
        return "no script log"
    problems = [f"{k}={stats[k]}" for k in ("skipped", "fetch_failed", "over_budget") if stats.get(k)]
    if stats.get("threw", 0) != len(chrome_errors):
        problems.append(f"threw={stats.get('threw', 0)} vs chrome {len(chrome_errors)}")
    return ", ".join(problems)


def compare(chrome_png: Path, frame: Path, out: Path) -> dict:
    return node(
        "import { comparePixels } from './tools/parity_oracle/compare_baseline.mjs';\n"
        f"const r = await comparePixels({json.dumps(str(chrome_png))}, {json.dumps(str(frame))}, "
        f"{json.dumps(str(out))});\n"
        "console.log(JSON.stringify({diff_pct: r.diffPercent, instrument: r.instrumentFailure || null}));"
    )


def fmt_pct(pct) -> str:
    """A diff percentage for display; None (not measured) prints as `--`."""
    return "  --  " if pct is None else f"{pct:6.2f}"


def main() -> int:
    args = sys.argv[1:]
    manifest = json.loads((RUNG_DIR / "manifest.json").read_text(encoding="utf-8"))
    w, h = manifest["viewport"]
    only = args[args.index("--case") + 1] if "--case" in args else None
    chrome_only = "--chrome-only" in args
    recapture = "--capture-chrome" in args or chrome_only

    origin = serve(RUNG_DIR)
    rows = []
    for fx in manifest["fixtures"]:
        fid = fx["id"]
        if only and fid != only:
            continue
        url = f"{origin}/{fid}/index.html"
        base = BASELINE_DIR / fid
        out = OUT_DIR / fid
        row = {"id": fid, "passed": False, "diff_pct": None}
        try:
            if recapture or not (base / "baseline.png").exists():
                row["chrome_errors"] = capture_chrome(url, base, w, h).get("pageErrors")
            if chrome_only:
                print(f"chrome  {fid}  errors={row.get('chrome_errors')}")
                continue
            chrome_errors = json.loads((base / "page-errors.json").read_text(encoding="utf-8"))
            rk = capture_rustkit(url, out, w, h)
            row["capture"] = rk.get("status")
            row["scripts"] = rk.get("script_stats")
            if rk.get("status") != "ok":
                row["error"] = rk.get("error")
            else:
                cmp = compare(base / "baseline.png", out / "frame.ppm", out)
                row["diff_pct"] = cmp["diff_pct"]
                row["instrument"] = cmp["instrument"]
                row["script_problems"] = script_verdict(row["scripts"], chrome_errors)
                row["passed"] = (cmp["instrument"] is None and cmp["diff_pct"] is not None
                                 and cmp["diff_pct"] <= PASS_PCT
                                 and not row["script_problems"])
            nojs = capture_nojs(RUNG_DIR / fid / "index.html", out, w, h)
            if nojs.get("status") == "ok":
                row["nojs_diff_pct"] = compare(base / "baseline.png", out / "nojs.ppm", out / "nojs")["diff_pct"]
        except Exception as e:
            row["error"] = str(e)[-300:]
        rows.append(row)
        d = fmt_pct(row["diff_pct"])
        nj = fmt_pct(row.get("nojs_diff_pct"))
        mark = "PASS" if row["passed"] else "fail"
        extra = row.get("error") or row.get("script_problems") or ""
        print(f"{mark}  {d}%  nojs {nj}%  {fid:40s} {extra}"[:200])

    if chrome_only:
        return 0
    passed = sum(r["passed"] for r in rows)
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    summary = {
        "rung": manifest["rung"],
        "timestamp": datetime.now().isoformat(timespec="seconds"),
        "head": subprocess.run(["git", "rev-parse", "--short", "HEAD"], capture_output=True,
                               text=True, cwd=REPO_ROOT, encoding="utf-8").stdout.strip(),
        "pass_pct": PASS_PCT,
        "passed": passed,
        "total": len(rows),
        "fixtures": rows,
    }
    (OUT_DIR / "summary.json").write_text(json.dumps(summary, indent=2) + "\n", encoding="utf-8")
    print(f"\njs-ladder: rung {manifest['rung']}, {passed}/{len(rows)} fixtures passing (<= {PASS_PCT}% vs Chrome 148)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
