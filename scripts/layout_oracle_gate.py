#!/usr/bin/env python3
"""layout_oracle_gate.py — Gate A (geometry) of the dual-oracle parity gate.

Plan: docs/PARITY_FINISH_LINE_PLAN_2026-08-04.md §2. Baseline and rules:
trench/BASELINE-parity-finish-line.md.

Gate A compares RustKit's exported layout tree against Chrome's committed
DOMRects and fails any box whose geometry differs by more than 0.5px. It is the
PRIMARY grind driver of the campaign: unlike paint, box math can be bit-exact,
so a geometry delta is always a real defect and never rasterizer noise.

    layout.json          (RustKit)  <-- crates/parity-capture --dump-layout
    layout-rects.json    (Chrome)   <-- baselines/chrome-148/<scope>/<case>/

Until 2026-08-05 this file was a stub: `extract_layout_from_rustkit` returned
None with a comment saying layout dumping did not exist yet. It does now, and
since P0a-0 the export carries the join key (`selector`) that makes the
comparison possible at all.

WHAT JOINS TO WHAT
------------------
Chrome's `rect` is `getBoundingClientRect()`, which is the BORDER box in
viewport coordinates. Its RustKit counterpart is `border_box`, not
`content_rect` — content_rect is inset by padding and border and would report a
constant, bogus delta on every padded element. Captures run unscrolled, so
viewport and document coordinates coincide.

The join key is the selector string, which is unique within a case on the
Chrome side (verified across all 32 committed baselines). Boxes with no
selector — anonymous boxes and text boxes — have no originating element and are
EXCLUDED, never paired positionally. An oracle that silently pairs an anonymous
box with a real element reports geometry failures that do not exist, which is
the exact class of instrument lie this campaign was opened to end.

WHAT CHROME'S CAPTURE OMITS
---------------------------
tools/parity_oracle/capture_baseline.mjs drops two groups of elements before
writing layout-rects.json, and this gate must mirror both or it will invent
failures:

  * zero-size elements (`rect.width === 0 && rect.height === 0`)
  * the tags in CHROME_SKIPPED_TAGS below

So a RustKit box with no Chrome counterpart is only a defect when Chrome WOULD
have captured it: a non-skipped tag that RustKit gave a non-zero size. That is
reported as a `phantom_box` — RustKit laid out something Chrome collapsed.

Usage:
    python3 scripts/layout_oracle_gate.py --layout-root parity-baseline/captures
    python3 scripts/layout_oracle_gate.py --layout-root <dir> --case bg-solid
    python3 scripts/layout_oracle_gate.py --layout-root <dir> --json out.json

Exit codes:
    0 = every discovered case is geometry-green
    1 = at least one case has a failing box, or the run measured nothing
"""

import argparse
import json
import sys
from pathlib import Path
from typing import Any, Dict, Iterator, List, Optional, Sequence, Tuple

REPO_ROOT = Path(__file__).resolve().parent.parent

# Gate A bar, plan §2. One constant, one place. Do not add a second number:
# a per-case geometry tolerance is how "≤ 0.5px every box" quietly becomes
# "≤ 0.5px except where it was inconvenient".
GEOMETRY_TOLERANCE_PX = 0.5

# The axes compared, in receipt order. Chrome also emits top/right/bottom/left,
# but those are derived from x/y/width/height — comparing them too would report
# one defect as up to eight failing lines and inflate any per-box count.
AXES = ("x", "y", "width", "height")

# Mirrors the skip list in tools/parity_oracle/capture_baseline.mjs. Kept as a
# literal rather than parsed out of the .mjs on purpose: if the capture script
# drifts, this gate should keep scoring against the committed baselines it was
# built for, and the drift should surface as a diff here rather than silently
# re-interpreting 1593 committed join keys.
CHROME_SKIPPED_TAGS = frozenset(
    {"script", "style", "meta", "link", "head", "title", "html"}
)

# The holdout scope is canary-only until the 26-case gate set is green
# (plan §3.6). It is discovered and scored, but never gates.
NON_GATING_SCOPES = frozenset({"holdout"})


# ---------------------------------------------------------------------------
# Failure records
# ---------------------------------------------------------------------------


class Failure:
    """One failing box, in the receipt format fixed by plan §2.

    `case_id · box path · axis · expected · actual · Δ`

    Join failures (missing/ambiguous/phantom) have no axis and no delta; they
    reuse the same six columns with the reason in the axis slot so that a
    geometry receipt has exactly one format and PR prose cannot invent another
    mid-campaign.
    """

    GEOMETRY_KINDS = frozenset({"delta"})

    def __init__(
        self,
        case_id: str,
        path: str,
        selector: Optional[str],
        kind: str,
        axis: Optional[str] = None,
        expected: Optional[float] = None,
        actual: Optional[float] = None,
    ) -> None:
        self.case_id = case_id
        self.path = path
        self.selector = selector
        self.kind = kind
        self.axis = axis
        self.expected = expected
        self.actual = actual

    @property
    def delta(self) -> Optional[float]:
        if self.expected is None or self.actual is None:
            return None
        return self.actual - self.expected

    def box_path(self) -> str:
        """Root-relative child indices, plus the selector when known."""
        if self.selector:
            return f"{self.path} {self.selector}"
        return self.path

    def receipt(self) -> str:
        delta = self.delta
        return " · ".join(
            [
                self.case_id,
                self.box_path(),
                self.axis or self.kind,
                fmt_px(self.expected),
                fmt_px(self.actual),
                fmt_delta(delta),
            ]
        )

    def to_json(self) -> Dict[str, Any]:
        return {
            "case_id": self.case_id,
            "path": self.path,
            "selector": self.selector,
            "kind": self.kind,
            "axis": self.axis,
            "expected": self.expected,
            "actual": self.actual,
            "delta": self.delta,
        }


def fmt_px(value: Optional[float]) -> str:
    if value is None:
        return "—"
    text = f"{value:.4f}".rstrip("0").rstrip(".")
    return text if text not in ("", "-0") else "0"


def fmt_delta(value: Optional[float]) -> str:
    if value is None:
        return "—"
    return ("+" if value >= 0 else "") + fmt_px(value)


# ---------------------------------------------------------------------------
# Loading
# ---------------------------------------------------------------------------


def load_json(path: Path) -> Optional[Dict[str, Any]]:
    if not path.exists():
        return None
    with open(path, encoding="utf-8") as handle:
        return json.load(handle)


def load_case_registry() -> Dict[str, Dict[str, Any]]:
    with open(REPO_ROOT / "cases" / "registry.json", encoding="utf-8") as handle:
        return json.load(handle)["cases"]


def baselines_dir() -> Path:
    import os

    return REPO_ROOT / "baselines" / os.environ.get("PARITY_BASELINE_SET", "chrome-148")


def chrome_rects_path(case_id: str, scope: str) -> Path:
    return baselines_dir() / scope / case_id / "layout-rects.json"


def walk_rustkit(
    node: Dict[str, Any], path: Tuple[int, ...] = ()
) -> Iterator[Tuple[Tuple[int, ...], Dict[str, Any]]]:
    """Depth-first walk yielding (root-relative child-index path, box)."""
    yield path, node
    for index, child in enumerate(node.get("children") or []):
        yield from walk_rustkit(child, path + (index,))


def fmt_path(path: Tuple[int, ...]) -> str:
    return ".".join(str(i) for i in path) if path else "root"


def index_rustkit(root: Dict[str, Any]) -> Tuple[
    Dict[str, List[Tuple[str, Dict[str, Any]]]], int, int
]:
    """Index RustKit boxes by selector.

    Returns (index, identified_count, total_count). The index maps a selector
    to every box claiming it — a list, not a single box, because a duplicate
    selector is an instrument ambiguity that must be REPORTED rather than
    resolved by taking the first match. Silently first-matching would score one
    of two boxes and call the case green.
    """
    index: Dict[str, List[Tuple[str, Dict[str, Any]]]] = {}
    identified = 0
    total = 0
    for path, box in walk_rustkit(root):
        total += 1
        selector = box.get("selector")
        if not selector:
            continue
        identified += 1
        index.setdefault(selector, []).append((fmt_path(path), box))
    return index, identified, total


def border_box(box: Dict[str, Any]) -> Optional[Dict[str, float]]:
    """The RustKit rect that corresponds to Chrome's getBoundingClientRect.

    `getBoundingClientRect()` is POST-transform, and CSS transforms do not
    change layout — so on a transformed box the layout `border_box` and
    Chrome's rect are measuring two different things, and the difference
    between them is the renderer's own translate rather than a layout defect.
    Scored that way, sticky-scroll's `.overflow-content` (`translate(-50%,
    -50%)`) read 139.53px out of place while its layout position was correct,
    and getting the layout position RIGHT made the reported delta larger.

    The engine emits `visual_border_box` exactly where a transform is in
    effect on the box or an ancestor. Prefer it: it is the same quantity
    Chrome's baseline is. Absent, the layout rect IS the visual rect.

    An inline whose text wrapped is the SAME class of mismatch one layer
    along. Chrome's rect for such an inline is the union of its line
    fragments; RustKit has no inline fragment model, so its box is one
    fragment tall and the rest of the content hangs outside it. Scored
    against `border_box`, `article-typography`'s `pre > code` read 16.32
    against Chrome's 148.38 — a 132px geometry defect on text that is laid
    out over 152.06, i.e. in the right place. The engine emits
    `fragment_union_border_box` exactly where an inline has a text
    descendant on more than one line; prefer it over the layout rect for the
    same reason, and below `visual_border_box`, which the engine already
    computes FROM the union when both apply.

    Text and image boxes emit a flat `rect` instead of the four box-model
    rects; they have no identity so the gate never reaches them through the
    join, but the fallback keeps this function total.
    """
    rect = (
        box.get("visual_border_box")
        or box.get("fragment_union_border_box")
        or box.get("border_box")
        or box.get("rect")
    )
    if not isinstance(rect, dict):
        return None
    return rect


# ---------------------------------------------------------------------------
# Comparison
# ---------------------------------------------------------------------------


def compare_case(
    case_id: str,
    chrome: Dict[str, Any],
    rustkit: Dict[str, Any],
    tolerance: float = GEOMETRY_TOLERANCE_PX,
) -> Dict[str, Any]:
    """Score one case. Returns a per-case record with its failure list."""
    root = rustkit.get("root", rustkit)
    index, identified, total_boxes = index_rustkit(root)

    failures: List[Failure] = []
    compared = 0
    matched_selectors = set()

    for element in chrome.get("elements", []):
        selector = element.get("selector")
        expected = element.get("rect") or {}
        candidates = index.get(selector, [])

        if not candidates:
            failures.append(
                Failure(case_id, "—", selector, "missing_box")
            )
            continue

        matched_selectors.add(selector)

        if len(candidates) > 1:
            paths = ",".join(path for path, _ in candidates)
            failures.append(
                Failure(case_id, f"[{paths}]", selector, "ambiguous_selector")
            )
            continue

        path, box = candidates[0]
        actual = border_box(box)
        if actual is None:
            failures.append(Failure(case_id, path, selector, "no_border_box"))
            continue

        compared += 1
        for axis in AXES:
            want = expected.get(axis)
            got = actual.get(axis)
            if want is None or got is None:
                failures.append(
                    Failure(case_id, path, selector, "missing_axis", axis=axis)
                )
                continue
            if abs(float(got) - float(want)) > tolerance:
                failures.append(
                    Failure(
                        case_id,
                        path,
                        selector,
                        "delta",
                        axis=axis,
                        expected=float(want),
                        actual=float(got),
                    )
                )

    # RustKit boxes Chrome never saw. Only a defect where Chrome WOULD have
    # captured the element — see the module docstring.
    for selector, candidates in index.items():
        if selector in matched_selectors:
            continue
        for path, box in candidates:
            tag = (box.get("tag") or "").lower()
            if tag in CHROME_SKIPPED_TAGS:
                continue
            rect = border_box(box) or {}
            width = float(rect.get("width") or 0.0)
            height = float(rect.get("height") or 0.0)
            if width == 0.0 and height == 0.0:
                continue
            failures.append(Failure(case_id, path, selector, "phantom_box"))

    geometry_failures = [f for f in failures if f.kind in Failure.GEOMETRY_KINDS]
    join_failures = [f for f in failures if f.kind not in Failure.GEOMETRY_KINDS]

    return {
        "case_id": case_id,
        "measured": True,
        "green": not failures,
        "chrome_boxes": len(chrome.get("elements", [])),
        "rustkit_boxes": total_boxes,
        "rustkit_identified": identified,
        "compared": compared,
        "geometry_failures": len(geometry_failures),
        "join_failures": len(join_failures),
        "failures": [f.to_json() for f in failures],
        "receipts": [f.receipt() for f in failures],
    }


def unmeasured_case(case_id: str, reason: str) -> Dict[str, Any]:
    """A case the gate could not score.

    NOT a pass. A capture that never ran and a capture that is perfect look
    identical to a gate that skips missing files, and the first one is how a
    broken pipeline turns green.
    """
    return {
        "case_id": case_id,
        "measured": False,
        "green": False,
        "reason": reason,
        "chrome_boxes": 0,
        "rustkit_boxes": 0,
        "rustkit_identified": 0,
        "compared": 0,
        "geometry_failures": 0,
        "join_failures": 0,
        "failures": [],
        "receipts": [],
    }


def find_layout_json(
    layout_root: Path, case_id: str, native_viewport: str
) -> Tuple[Optional[Path], Optional[str]]:
    """Locate a case's RustKit layout dump. Returns (path, refusal_reason).

    Two capture layouts exist in the tree. parity_test.py writes a flat
    `<root>/<case_id>/layout.json`; parity_lib.py — the one the PR and nightly
    swarms use — writes
    `<root>/<run_id>/<case_id>/<viewport>/iter-<n>/layout.json`, so one case
    yields several dumps at several viewports.

    Only the case's REGISTRY viewport is accepted. Chrome's rects were captured
    at that viewport and nowhere else, so scoring an 1920x1080 dump against
    800x600 baselines would report a page-wide geometry catastrophe that is
    purely an instrument mismatch — the same trap parity_gate.py's
    primary_viewport_filter exists to close on the pixel side. A case captured
    only off-viewport is UNMEASURED, which fails, rather than measured wrongly.

    Iterations are interchangeable here: geometry is deterministic, and gate A
    scores one dump. Stability across iterations is enforced separately.
    """
    direct = layout_root / case_id / "layout.json"
    if direct.exists():
        return direct, None

    matches = sorted(layout_root.glob(f"**/{case_id}/**/layout.json"))
    if not matches:
        return None, "no_rustkit_capture"

    native = [p for p in matches if native_viewport in p.parts]
    if not native:
        return None, "no_native_viewport_capture"
    return min(native, key=lambda p: (len(p.parts), str(p))), None


def run_gate(
    layout_root: Path,
    case_ids: Optional[Sequence[str]] = None,
    include_non_gating: bool = False,
    tolerance: float = GEOMETRY_TOLERANCE_PX,
) -> Dict[str, Any]:
    registry = load_case_registry()

    selected = []
    for case_id, case in sorted(registry.items()):
        if case_ids is not None and case_id not in case_ids:
            continue
        if case["scope"] in NON_GATING_SCOPES and not include_non_gating:
            continue
        selected.append((case_id, case))

    cases = []
    for case_id, case in selected:
        chrome = load_json(chrome_rects_path(case_id, case["scope"]))
        if chrome is None:
            cases.append(unmeasured_case(case_id, "no_chrome_baseline"))
            continue
        layout_path, refusal = find_layout_json(
            layout_root, case_id, f"{case['width']}x{case['height']}"
        )
        if layout_path is None:
            cases.append(unmeasured_case(case_id, refusal or "no_rustkit_capture"))
            continue
        rustkit = load_json(layout_path)
        if rustkit is None:
            cases.append(unmeasured_case(case_id, "unreadable_rustkit_capture"))
            continue
        record = compare_case(case_id, chrome, rustkit, tolerance=tolerance)
        record["scope"] = case["scope"]
        cases.append(record)

    measured = [c for c in cases if c["measured"]]
    green = [c for c in cases if c["green"]]

    return {
        "gate": "A-geometry",
        "tolerance_px": tolerance,
        "layout_root": str(layout_root),
        "cases": cases,
        "summary": {
            "total_cases": len(cases),
            "measured": len(measured),
            "unmeasured": len(cases) - len(measured),
            "green": len(green),
            "red": len(cases) - len(green),
            "geometry_failures": sum(c["geometry_failures"] for c in cases),
            "join_failures": sum(c["join_failures"] for c in cases),
        },
    }


def gate_passes(report: Dict[str, Any]) -> bool:
    """A run that measured nothing is a FAIL.

    "PASS: all 0 cases" is how a broken pipeline reports success. The same
    tripwire already guards parity_gate.py's test_results mode (B3); geometry
    gets it from the first commit rather than after the first time it lies.

    Deliberately ONE tripwire, not two. A `total_cases == 0` check reads well
    but is subsumed — zero cases means zero measured — so it mutates green and
    is decoration. `measured == 0` catches both "the filter matched nothing"
    and "26 cases, none of which the capture produced".
    """
    if report["summary"]["measured"] == 0:
        return False
    return report["summary"]["red"] == 0


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------


def print_report(report: Dict[str, Any], verbose: bool = False) -> None:
    summary = report["summary"]
    print("Gate A — geometry")
    print(f"  tolerance:  {report['tolerance_px']}px per box, per axis")
    print(f"  layouts:    {report['layout_root']}")
    print(
        f"  cases:      {summary['green']}/{summary['total_cases']} geometry-green"
        f"  ({summary['unmeasured']} unmeasured)"
    )
    print(
        f"  failures:   {summary['geometry_failures']} geometry,"
        f" {summary['join_failures']} join"
    )
    print()

    for case in report["cases"]:
        if not case["measured"]:
            print(f"  UNMEASURED {case['case_id']}: {case['reason']}")
            continue
        mark = "GREEN" if case["green"] else "RED  "
        print(
            f"  {mark} {case['case_id']}: {case['compared']}/{case['chrome_boxes']}"
            f" boxes compared, {case['geometry_failures']} geometry,"
            f" {case['join_failures']} join"
        )
        receipts = case["receipts"] if verbose else case["receipts"][:5]
        for line in receipts:
            print(f"        {line}")
        hidden = len(case["receipts"]) - len(receipts)
        if hidden > 0:
            print(f"        … {hidden} more (use --verbose)")


def main() -> int:
    parser = argparse.ArgumentParser(description="Gate A — geometry oracle")
    parser.add_argument(
        "--layout-root",
        type=Path,
        default=REPO_ROOT / "parity-baseline" / "captures",
        help="Directory holding RustKit layout.json captures",
    )
    parser.add_argument(
        "--case",
        action="append",
        dest="cases",
        help="Limit to a case id (repeatable)",
    )
    parser.add_argument(
        "--include-non-gating",
        action="store_true",
        help="Also score the holdout scope (canary-only, plan §3.6)",
    )
    parser.add_argument(
        "--tolerance",
        type=float,
        default=GEOMETRY_TOLERANCE_PX,
        help=argparse.SUPPRESS,
    )
    parser.add_argument("--json", type=Path, help="Write the full report here")
    parser.add_argument("--verbose", action="store_true")
    args = parser.parse_args()

    report = run_gate(
        args.layout_root,
        case_ids=args.cases,
        include_non_gating=args.include_non_gating,
        tolerance=args.tolerance,
    )

    print_report(report, verbose=args.verbose)

    if args.json:
        args.json.parent.mkdir(parents=True, exist_ok=True)
        with open(args.json, "w", encoding="utf-8") as handle:
            json.dump(report, handle, indent=2)
        print(f"\nReport written to {args.json}")

    passed = gate_passes(report)
    print(f"\nGate A: {'PASS' if passed else 'FAIL'}")
    return 0 if passed else 1


if __name__ == "__main__":
    sys.exit(main())
