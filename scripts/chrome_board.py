#!/usr/bin/env python3
"""
chrome_board.py — the chrome board (CH3.A1): HiWave's own UI pages in RustKit
vs the oracle Chromium, per page x state x size.

    python3 scripts/chrome_board.py --parity-capture target/release/parity-capture

Builds nothing. For every row of tools/parity_oracle/chrome_fixtures/board.json:

  1. composes the page once per page: the IPC stub before the page's own
     script, the page's fixture after it (realistic state through the host
     entry points), any host injection (report.html gets Chart.js inline, as
     the app gives it);
  2. renders it in Chromium (tools/parity_oracle/chrome_board.mjs) and in
     RustKit (parity-capture --run-inline-scripts --actions) with ONE actions
     file for the state;
  3. diffs pixels (compare_baseline.mjs: diff %, per-element attribution)
     and geometry (layout_oracle_gate.compare_case: Chrome rects vs RustKit
     boxes on the join selector, 0.5 px);
  4. writes <out>/BOARD.md and <out>/board.json: one row per page x state x
     size with pixel diff %, geometry diff count and the top 3 differing
     elements by selector, the worst rows first, and the CSS census red
     list of each page (the census's red properties that the page declares).

The gate (PLAN-z CH0): every row <= 0.5 % pixel diff. A row whose capture
failed is listed as such, never scored. A case whose fixture is missing is a
refusal before anything runs (scripts/tests/test_chrome_board.py).
Captures (frames, rects, layouts) stay in --work (default target/chrome_board,
gitignored); only the board files are for committing.
"""
import argparse
import json
import os
import re
import subprocess
import sys
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Callable, Dict, List, Optional

REPO_ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO_ROOT / "scripts"))

DEFAULT_BOARD = REPO_ROOT / "tools" / "parity_oracle" / "chrome_fixtures" / "board.json"
DEFAULT_OUT = REPO_ROOT / "trench" / "chrome"
DEFAULT_WORK = REPO_ROOT / "target" / "chrome_board"
ORACLE_DIR = REPO_ROOT / "tools" / "parity_oracle"
CHROMIUM_ARM = ORACLE_DIR / "chrome_board.mjs"
CENSUS_DIR = REPO_ROOT / "docs" / "census"
PIXEL_GATE_PCT = 0.5
TOP_N = 3


def _node_env() -> Dict[str, str]:
    """The environment for node: the Homebrew prefix is added only when it
    exists and is not already on PATH (launchd lanes start without it)."""
    env = dict(os.environ)
    path = env.get("PATH", "")
    for prefix in ("/opt/homebrew/bin", "/usr/local/bin"):
        if os.path.isdir(prefix) and prefix not in path.split(os.pathsep):
            path = f"{prefix}{os.pathsep}{path}" if path else prefix
    env["PATH"] = path
    return env


NODE_ENV = _node_env()


# The oracle context freezes transitions, animations and the caret for every
# campaign capture (baselines/common/parity-freeze.js). The board's states are
# transitions (sidebar collapse, focus mode, the find modal), so the same
# freeze goes INTO the composed page, where both engines see it: the board
# scores end states, not the moment a 0.2 s fade happened to be sampled.
FREEZE_STYLE = (
    '<style data-chrome-board="freeze">\n'
    '  *, *::before, *::after {\n'
    '    transition: none !important;\n'
    '    animation: none !important;\n'
    '    animation-play-state: paused !important;\n'
    '    caret-color: transparent !important;\n'
    '    scroll-behavior: auto !important;\n'
    '  }\n'
    '</style>'
)


class BoardError(Exception):
    """A board that cannot be run honestly: refused before any capture."""


@dataclass
class Row:
    page: str
    state: str
    width: int
    height: int
    html_path: Path
    actions_path: Path
    out_dir: Path
    actions: List[Dict[str, Any]] = field(default_factory=list)

    @property
    def size(self) -> str:
        return f"{self.width}x{self.height}"

    @property
    def case_id(self) -> str:
        return f"{self.page}/{self.state}/{self.size}"


# ---------------------------------------------------------------------------
# Plan
# ---------------------------------------------------------------------------

def load_board(path: Path) -> Dict[str, Any]:
    with open(path, encoding="utf-8") as handle:
        board = json.load(handle)
    board["_dir"] = str(Path(path).resolve().parent)
    return board


def compose_page(board: Dict[str, Any], page: str, repo_root: Path, work: Path) -> Path:
    """The page with its stub before and its fixture after its own script."""
    spec = board["pages"][page]
    fixtures_dir = Path(board["_dir"])
    html = (repo_root / spec["html"]).read_text(encoding="utf-8")
    prelude = (fixtures_dir / board["prelude"]).read_text(encoding="utf-8")
    fixture = (fixtures_dir / spec["fixture"]).read_text(encoding="utf-8")
    for marker, source in (spec.get("inject") or {}).items():
        if marker not in html:
            raise BoardError(f"{page}: injection marker not in page: {marker!r}")
        html = html.replace(marker, "<script>" + (repo_root / source).read_text(encoding="utf-8") + "</script>")
    head = re.search(r"<head[^>]*>", html, re.IGNORECASE)
    if not head:
        raise BoardError(f"{page}: no <head> to put the IPC stub in")
    stub_tag = f"<script data-chrome-board=\"stub\">\n{prelude}\n</script>"
    html = html[: head.end()] + "\n" + stub_tag + "\n" + FREEZE_STYLE + html[head.end():]
    fixture_tag = f"<script data-chrome-board=\"fixture\">\n{fixture}\n</script>\n"
    body_end = html.lower().rfind("</body>")
    if body_end < 0:
        raise BoardError(f"{page}: no </body> to put the fixture before")
    html = html[:body_end] + fixture_tag + html[body_end:]
    out = work / "pages" / f"{page}.html"
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(html, encoding="utf-8")
    return out


def plan(board: Dict[str, Any], repo_root: Path, work: Path, only: Optional[str] = None) -> List[Row]:
    """Every row of the board, refused whole if any case cannot run."""
    fixtures_dir = Path(board["_dir"])
    problems = []
    if not (fixtures_dir / board["prelude"]).exists():
        problems.append(f"prelude {board['prelude']} missing")
    for page, spec in board["pages"].items():
        if not (repo_root / spec["html"]).exists():
            problems.append(f"{page}: html {spec['html']} missing")
        if not (fixtures_dir / spec["fixture"]).exists():
            problems.append(f"{page}: fixture {spec['fixture']} missing")
        for source in (spec.get("inject") or {}).values():
            if not (repo_root / source).exists():
                problems.append(f"{page}: injection {source} missing")
        for state in spec["states"]:
            if state not in board["states"]:
                problems.append(f"{page}: state {state} not defined")
    if problems:
        raise BoardError("board refused: " + "; ".join(problems))

    rows: List[Row] = []
    for page, spec in board["pages"].items():
        if only and page != only:
            continue
        html_path = compose_page(board, page, repo_root, work)
        for state in spec["states"]:
            actions = board["states"][state]
            actions_path = work / "actions" / f"{state}.json"
            actions_path.parent.mkdir(parents=True, exist_ok=True)
            actions_path.write_text(json.dumps(actions, indent=2) + "\n", encoding="utf-8")
            for width, height in board["sizes"]:
                rows.append(Row(
                    page=page, state=state, width=int(width), height=int(height),
                    html_path=html_path, actions_path=actions_path,
                    out_dir=work / page / state / f"{width}x{height}", actions=actions,
                ))
    return rows


# ---------------------------------------------------------------------------
# The two arms: one actions file, two engines
# ---------------------------------------------------------------------------

def chromium_command(row: Row) -> List[str]:
    return [
        "node", str(CHROMIUM_ARM), str(row.html_path), str(row.out_dir),
        str(row.width), str(row.height), str(row.actions_path),
    ]


def rustkit_command(row: Row, binary: Path) -> List[str]:
    return [
        str(binary),
        "--html-file", str(row.html_path),
        "--width", str(row.width),
        "--height", str(row.height),
        "--run-inline-scripts",
        "--actions", str(row.actions_path),
        "--actions-out-dir", str(row.out_dir),
        "--dump-frame", str(row.out_dir / "rustkit.ppm"),
        "--dump-layout", str(row.out_dir / "rustkit-layout.json"),
    ]


def _run(cmd: List[str], timeout: int, cwd: Path = REPO_ROOT) -> Dict[str, Any]:
    try:
        proc = subprocess.run(cmd, capture_output=True, text=True, timeout=timeout, cwd=cwd, env=NODE_ENV, encoding="utf-8")
    except subprocess.TimeoutExpired:
        return {"status": "timeout", "error": f"timeout ({timeout}s)"}
    except OSError as exc:
        return {"status": "error", "error": str(exc)}
    record: Dict[str, Any] = {}
    for line in reversed(proc.stdout.strip().splitlines()):
        if line.startswith("{"):
            try:
                record = json.loads(line)
                break
            except json.JSONDecodeError:
                continue
    if proc.returncode != 0 and record.get("status", "ok") == "ok":
        record["status"] = "error"
        record["error"] = (proc.stderr or proc.stdout)[-400:].strip()
    record.setdefault("status", "ok")
    return record


def capture(row: Row, binary: Path) -> Dict[str, Any]:
    """Both arms for one row, then the diffs. Pure function of the row."""
    row.out_dir.mkdir(parents=True, exist_ok=True)
    chromium = _run(chromium_command(row), timeout=120)
    rustkit = _run(rustkit_command(row, binary), timeout=180)
    result: Dict[str, Any] = {"chromium": chromium, "rustkit": rustkit}
    chrome_png = row.out_dir / "chrome.png"
    rects_json = row.out_dir / "layout-rects.json"
    rustkit_ppm = row.out_dir / "rustkit.ppm"
    rustkit_layout = row.out_dir / "rustkit-layout.json"
    if chromium.get("status") != "ok" or not chrome_png.exists():
        result["status"] = "chromium_failed"
        return result
    if rustkit.get("status") != "ok" or not rustkit_ppm.exists():
        result["status"] = "rustkit_failed"
        return result
    result["pixels"] = compare_pixels(chrome_png, rustkit_ppm, row.out_dir / "diff", rects_json)
    result["geometry"] = compare_geometry(row.case_id, rects_json, rustkit_layout)
    result["status"] = "ok" if "error" not in result["pixels"] else "diff_failed"
    return result


def compare_pixels(chrome_png: Path, rustkit_ppm: Path, out_dir: Path, rects_json: Path) -> Dict[str, Any]:
    out_dir.mkdir(parents=True, exist_ok=True)
    script = f"""
import {{ comparePixels }} from './tools/parity_oracle/compare_baseline.mjs';
const r = await comparePixels({json.dumps(str(chrome_png))}, {json.dumps(str(rustkit_ppm))}, {json.dumps(str(out_dir))},
  {{ chromeRectsPath: {json.dumps(str(rects_json) if rects_json.exists() else "")}, attributionTopN: 10 }});
console.log(JSON.stringify({{ diffPercent: r.diffPercent, diffPixels: r.diffPixels, totalPixels: r.totalPixels,
  taxonomy: r.taxonomy, top: (r.attribution && r.attribution.top_contributors) || [], error: r.error || null }}));
"""
    record = _run(["node", "--input-type=module", "-e", script], timeout=120)
    if record.get("status") != "ok":
        return {"error": record.get("error", "pixel diff failed")}
    if record.get("error"):
        return {"error": record["error"]}
    record.pop("error", None)
    return record


def compare_geometry(case_id: str, rects_json: Path, rustkit_layout: Path) -> Dict[str, Any]:
    if not rects_json.exists() or not rustkit_layout.exists():
        return {"error": "rects or layout missing", "geometry_failures": None}
    from layout_oracle_gate import compare_case  # scripts/, on sys.path above

    with open(rects_json, encoding="utf-8") as handle:
        chrome = json.load(handle)
    with open(rustkit_layout, encoding="utf-8") as handle:
        rustkit = json.load(handle)
    record = compare_case(case_id, chrome, rustkit)
    deltas = [f for f in record["failures"] if f.get("kind") == "delta" and f.get("delta") is not None]
    deltas.sort(key=lambda f: -abs(f["delta"]))
    worst: Dict[str, float] = {}
    for f in deltas:
        sel = f.get("selector") or f.get("path")
        worst[sel] = max(worst.get(sel, 0.0), abs(f["delta"]))
    return {
        "geometry_failures": record["geometry_failures"],
        "join_failures": record["join_failures"],
        "compared": record["compared"],
        "chrome_boxes": record["chrome_boxes"],
        "worst_selectors": [{"selector": s, "max_delta_px": round(d, 2)} for s, d in
                            sorted(worst.items(), key=lambda kv: -kv[1])[:10]],
    }


# ---------------------------------------------------------------------------
# Census: the red properties each page declares
# ---------------------------------------------------------------------------

def latest_census(census_dir: Path = CENSUS_DIR) -> Optional[Path]:
    files = sorted(census_dir.glob("CSS_PROPERTY_CENSUS_*.json"))
    return files[-1] if files else None


def page_properties(html: str) -> List[str]:
    """CSS property names the page declares, in <style> blocks and style attributes."""
    names = set()
    for block in re.findall(r"<style[^>]*>(.*?)</style>", html, re.IGNORECASE | re.DOTALL):
        block = re.sub(r"/\*.*?\*/", "", block, flags=re.DOTALL)
        for body in re.findall(r"\{([^{}]*)\}", block):
            for decl in body.split(";"):
                if ":" in decl:
                    name = decl.split(":", 1)[0].strip().lower()
                    if re.fullmatch(r"-?[a-z][a-z0-9-]*", name) and not name.startswith("--"):
                        names.add(name)
    for attr in re.findall(r"\sstyle=\"([^\"]*)\"", html, re.IGNORECASE):
        for decl in attr.split(";"):
            if ":" in decl:
                name = decl.split(":", 1)[0].strip().lower()
                if re.fullmatch(r"-?[a-z][a-z0-9-]*", name):
                    names.add(name)
    return sorted(names)


def census_red_lists(board: Dict[str, Any], repo_root: Path, census_path: Optional[Path]) -> Dict[str, Any]:
    if not census_path or not census_path.exists():
        return {"source": None, "pages": {}}
    with open(census_path, encoding="utf-8") as handle:
        census = json.load(handle)
    red = set(census.get("red", []))
    by_name = {p["name"]: p for p in census.get("properties", [])}
    pages = {}
    for page, spec in board["pages"].items():
        declared = page_properties((repo_root / spec["html"]).read_text(encoding="utf-8"))
        hits = []
        for name in declared:
            if name in red:
                row = by_name.get(name, {})
                hits.append({"property": name, "class": row.get("class"), "sites": row.get("sites")})
        pages[page] = {"declared": len(declared), "red": hits}
    # The json may live in a build dir; name it, and point at the committed
    # report of the same run when trench/chrome/census/ holds it.
    report = Path("trench/chrome/census") / census_path.with_suffix(".md").name
    return {
        "source": census_path.name,
        "report": str(report) if (repo_root / report).exists() else None,
        "date": census.get("meta", {}).get("date"),
        "oracle": census.get("meta", {}).get("oracle"),
        "red_total": len(red),
        "pages": pages,
    }


# ---------------------------------------------------------------------------
# Report
# ---------------------------------------------------------------------------

def summarize(row: Row, result: Dict[str, Any]) -> Dict[str, Any]:
    entry: Dict[str, Any] = {
        "page": row.page, "state": row.state, "size": row.size,
        "width": row.width, "height": row.height, "status": result["status"],
        "pixel_diff_pct": None, "geometry_diffs": None, "top": [], "notes": [],
    }
    chromium = result.get("chromium", {})
    rustkit = result.get("rustkit", {})
    # Shapes only (the board is committed): arm, step, status, the error's
    # class and length — never its text, which can quote a value.
    for arm, rec in (("chromium", chromium), ("rustkit", rustkit)):
        if rec.get("status") != "ok":
            entry["notes"].append(f"{arm}: {rec.get('status')} {error_shape(rec.get('error'))}".strip())
        for a in rec.get("action_results") or []:
            if a.get("status") != "ok":
                entry["notes"].append(f"{arm} action {a.get('step')} {a.get('type')}: {a.get('status')}")
    for err in chromium.get("page_errors") or []:
        entry["notes"].append(f"chromium pageerror: {error_shape(err)}")
    for err in chromium.get("fixture_errors") or []:
        cmd = str(err).split(":", 1)[0].strip()
        entry["notes"].append(f"fixture: {cmd} handler threw {error_shape(str(err).split(':', 1)[1] if ':' in str(err) else '')}")
    if result["status"] != "ok":
        if "pixels" in result and "error" in result["pixels"]:
            entry["notes"].append("diff: " + error_shape(result["pixels"]["error"]))
        return entry
    pixels = result["pixels"]
    entry["pixel_diff_pct"] = round(float(pixels["diffPercent"]), 3)
    entry["taxonomy"] = pixels.get("taxonomy")
    geometry = result.get("geometry", {})
    entry["geometry_diffs"] = geometry.get("geometry_failures")
    entry["join_failures"] = geometry.get("join_failures")
    top = [{"selector": c["selector"], "pct": round(c.get("contribution_percent", 0.0), 1)}
           for c in pixels.get("top", [])[:TOP_N]]
    if len(top) < TOP_N:
        seen = {t["selector"] for t in top}
        for g in geometry.get("worst_selectors", []):
            if g["selector"] not in seen:
                top.append({"selector": g["selector"], "delta_px": g["max_delta_px"]})
                seen.add(g["selector"])
            if len(top) >= TOP_N:
                break
    entry["top"] = top
    entry["worst_geometry"] = geometry.get("worst_selectors", [])[:TOP_N]
    return entry


def error_shape(text: Optional[str]) -> str:
    """An error's class and length, not its text: `TypeError (71 chars)`."""
    if not text:
        return ""
    text = str(text).strip()
    head = re.match(r"\s*([A-Z][A-Za-z]*(?:Error|Exception))\b", text)
    kind = head.group(1) if head else "error"
    return f"{kind} ({len(text)} chars)"


def _fmt_top(top: List[Dict[str, Any]]) -> str:
    parts = []
    for t in top:
        if "pct" in t:
            parts.append(f"`{t['selector']}` {t['pct']}%")
        else:
            parts.append(f"`{t['selector']}` Δ{t['delta_px']}px")
    return "; ".join(parts) or "—"


def _fmt_pct(v: Optional[float]) -> str:
    return "—" if v is None else f"{v:.3f}"


def sort_key(entry: Dict[str, Any]):
    pct = entry["pixel_diff_pct"]
    return (0 if entry["status"] != "ok" else 1, -(pct if pct is not None else 0.0))


def write_board(out: Path, board_json: Dict[str, Any]) -> None:
    out.mkdir(parents=True, exist_ok=True)
    (out / "board.json").write_text(json.dumps(board_json, indent=2) + "\n", encoding="utf-8")
    rows = board_json["rows"]
    scored = [r for r in rows if r["status"] == "ok"]
    green = [r for r in scored if r["pixel_diff_pct"] <= PIXEL_GATE_PCT]
    failed = [r for r in rows if r["status"] != "ok"]
    lines = [
        "# Chrome board — HiWave's own UI pages, RustKit vs Chromium (CH3.A1)",
        "",
        f"Run: {board_json['run']['at']} · head `{board_json['run']['sha']}` on develop "
        f"`{board_json['run'].get('base') or '?'}` · oracle Chromium "
        f"{board_json['run'].get('chromium') or '?'} · parity-capture `{board_json['run']['parity_capture']}`",
        "",
        f"Gate (PLAN-z CH0): every row ≤ {PIXEL_GATE_PCT}% pixel diff. "
        f"**{len(green)}/{len(rows)} rows green**, {len(scored) - len(green)} over the gate, "
        f"{len(failed)} not scored (a capture failed).",
        "",
        "Pixel diff % is pixelmatch over the whole viewport (compare_baseline.mjs). Geometry diffs is "
        "the count of (box, axis) pairs more than 0.5 px from Chrome's rect on the join selector "
        "(layout_oracle_gate.compare_case). Top 3 is the attribution of the diff pixels to elements "
        "by selector; where fewer than three elements carry pixels, the worst geometry deltas fill in.",
        "",
        "## Rows, worst first",
        "",
        "| page | state | size | pixel diff % | geometry diffs | top 3 differing elements | notes |",
        "|---|---|---|---:|---:|---|---|",
    ]
    for r in sorted(rows, key=sort_key):
        geo = "—" if r["geometry_diffs"] is None else str(r["geometry_diffs"])
        status = "" if r["status"] == "ok" else f"**{r['status']}** "
        notes = status + "; ".join(r["notes"])
        lines.append(f"| {r['page']} | {r['state']} | {r['size']} | {_fmt_pct(r['pixel_diff_pct'])} | {geo} | "
                     f"{_fmt_top(r['top'])} | {notes} |")
    lines += ["", "## Pages", "", "| page | rows | green | worst state | worst pixel diff % |", "|---|---:|---:|---|---:|"]
    pages = sorted({r["page"] for r in rows})
    for page in pages:
        mine = [r for r in rows if r["page"] == page]
        ok = [r for r in mine if r["status"] == "ok"]
        g = [r for r in ok if r["pixel_diff_pct"] <= PIXEL_GATE_PCT]
        worst = max(ok, key=lambda r: r["pixel_diff_pct"]) if ok else None
        lines.append(f"| {page} | {len(mine)} | {len(g)} | {worst['state'] + ' ' + worst['size'] if worst else '—'} | "
                     f"{_fmt_pct(worst['pixel_diff_pct'] if worst else None)} |")
    census = board_json.get("census") or {}
    lines += ["", "## CSS census red list per page", ""]
    if census.get("source"):
        lines.append(f"Census {census.get('date')} (`{census.get('report') or census['source']}`, oracle {census.get('oracle')}): "
                     f"{census.get('red_total')} red properties. Per page, the red properties the page declares "
                     "(in its `<style>` blocks and `style=` attributes).")
        lines.append("")
        lines += ["| page | declared properties | red | red properties |", "|---|---:|---:|---|"]
        for page, info in census["pages"].items():
            red = ", ".join(f"`{h['property']}`" + (f" ({h['class']})" if h.get("class") else "") for h in info["red"])
            lines.append(f"| {page} | {info['declared']} | {len(info['red'])} | {red or '—'} |")
    else:
        lines.append("No census JSON found under docs/census; pass --census.")
    lines += ["", "## States", "", "Actions run identically by parity-capture `--actions` and chrome_board.mjs:", ""]
    for name, actions in board_json["states"].items():
        lines.append(f"- `{name}`: `{json.dumps(actions)}`")
    lines.append("")
    (out / "BOARD.md").write_text("\n".join(lines), encoding="utf-8")


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

def git_sha(repo_root: Path, ref: str = "HEAD") -> str:
    try:
        return subprocess.run(["git", "rev-parse", "--short", ref], capture_output=True, text=True,
                              cwd=repo_root, timeout=10, encoding="utf-8").stdout.strip() or "?"
    except Exception:  # noqa: BLE001 — a missing git is a label, not a failure
        return "?"


def git_develop_base(repo_root: Path) -> str:
    """The develop commit this head is built on (merge-base with origin/develop)."""
    try:
        base = subprocess.run(["git", "merge-base", "HEAD", "origin/develop"], capture_output=True,
                              text=True, cwd=repo_root, timeout=10, encoding="utf-8").stdout.strip()
        return git_sha(repo_root, base) if base else "?"
    except Exception:  # noqa: BLE001
        return "?"


def report_only(board: Dict[str, Any], out: Path, census_path: Optional[Path]) -> int:
    """The last run's rows, re-reported with another census; captures untouched."""
    with open(out / "board.json", encoding="utf-8") as handle:
        board_json = json.load(handle)
    board_json["census"] = census_red_lists(board, REPO_ROOT, census_path)
    write_board(out, board_json)
    print(f"board: re-reported with census {board_json['census'].get('source')} → {out / 'BOARD.md'}")
    return 0


def main(argv: Optional[List[str]] = None, capture: Callable[..., Dict[str, Any]] = capture) -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--parity-capture", required=True, help="path to a built parity-capture binary")
    ap.add_argument("--board", default=str(DEFAULT_BOARD))
    ap.add_argument("--out", default=str(DEFAULT_OUT), help="where BOARD.md and board.json go")
    ap.add_argument("--work", default=str(DEFAULT_WORK), help="captures and composed pages (not for committing)")
    ap.add_argument("--only", help="one page")
    ap.add_argument("--census", help="CSS census JSON (default: newest docs/census/CSS_PROPERTY_CENSUS_*.json)")
    ap.add_argument("--report-only", action="store_true",
                    help="rewrite BOARD.md from <out>/board.json with the census named by --census; no captures")
    args = ap.parse_args(argv)

    binary = Path(args.parity_capture)
    out = Path(args.out)
    work = Path(args.work)
    try:
        board = load_board(Path(args.board))
        if args.report_only:
            return report_only(board, out, Path(args.census) if args.census else latest_census())
        rows = plan(board, REPO_ROOT, work, only=args.only)
    except (BoardError, OSError, KeyError, json.JSONDecodeError) as exc:
        print(f"refused: {exc}", file=sys.stderr)
        return 2
    if not rows:
        print("refused: no rows (bad --only?)", file=sys.stderr)
        return 2
    if not binary.exists():
        print(f"refused: parity-capture not found at {binary}", file=sys.stderr)
        return 2

    entries = []
    chromium_version = None
    for i, row in enumerate(rows, 1):
        result = capture(row, binary)
        chromium_version = chromium_version or result.get("chromium", {}).get("browser")
        entry = summarize(row, result)
        entries.append(entry)
        print(f"[{i}/{len(rows)}] {row.case_id}: {entry['status']} "
              f"pixel={_fmt_pct(entry['pixel_diff_pct'])}% geo={entry['geometry_diffs']}", flush=True)

    census_path = Path(args.census) if args.census else latest_census()
    board_json = {
        "run": {
            "at": datetime.now(timezone.utc).strftime("%Y-%m-%d %H:%M UTC"),
            "sha": git_sha(REPO_ROOT),
            "base": git_develop_base(REPO_ROOT),
            "chromium": chromium_version,
            "parity_capture": str(binary),
            "gate_pct": PIXEL_GATE_PCT,
        },
        "sizes": board["sizes"],
        "states": {s: board["states"][s] for s in sorted({r.state for r in rows})},
        "rows": entries,
        "census": census_red_lists(board, REPO_ROOT, census_path),
    }
    write_board(out, board_json)
    scored = [e for e in entries if e["status"] == "ok"]
    green = sum(1 for e in scored if e["pixel_diff_pct"] <= PIXEL_GATE_PCT)
    print(f"board: {green}/{len(entries)} rows green at {PIXEL_GATE_PCT}% → {out / 'BOARD.md'}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
