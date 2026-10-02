#!/usr/bin/env python3
"""forensic_board.py — Gate C (forensic) of the dual-oracle parity gate.

Plan: docs/PARITY_FINISH_LINE_PLAN_2026-08-04.md §2. Rules:
trench/BASELINE-parity-finish-line.md.

Gates A and B answer "is this case acceptable". Gate C answers a question they
structurally cannot: **where is the difference, and what shape is it.** Plan §2
gives it one job — full raw pixel heatmap plus worst-N, published on every PR,
never gating — because A and B each look through a keyhole:

    Gate A  reads four numbers per element box (x, y, width, height). A shadow
            painted on the wrong side, a z-order inversion between two boxes
            that both have correct geometry, an outline drawn 1px out — none of
            these move a rect, so none of them exist to Gate A.
    Gate B  grades a percentage inside a tolerance and runs two discrete
            detectors inside element interiors. Its percentage half is exactly
            the number §1 of the plan records being gamed: master's collapsed
            shelf scored 3.71% and passed. A defect small enough always passes.

So Gate C keeps the number Gate B refuses to grade on — the RAW per-pixel
difference at zero tolerance — and never lets it decide anything. That is not a
contradiction, it is the point. The mean-diff figure is dangerous as a gate and
genuinely useful as a map, and the only safe way to hold both is to make it
structurally incapable of passing or failing a PR.

NON-GATING IS NOT THE SAME AS ALWAYS-GREEN
------------------------------------------
This is the one thing in this file that is easy to get wrong, and getting it
wrong reproduces the campaign's founding failure in a new place. "Non-gating"
means the NUMBERS never fail a PR. It does not mean the process always exits 0:

    ran, published a board, numbers are terrible   -> exit 0   (non-gating)
    ran, published a board, numbers are perfect    -> exit 0   (non-gating)
    could not run / measured zero cases            -> exit 1   (NOT a pass)

An instrument that could not run reports the same silent "nothing wrong" as one
that ran clean. That is the fleet rule in trench/BASELINE-parity-finish-line.md
— a blank row is not a pass — applied to the board's own exit status. A Gate C
that swallowed its own breakage would be a forensic board that quietly stopped
being published, which nobody notices, because the way you notice is by reading
the board.

THE TOLERANCE SWEEP
-------------------
Every case gets pixel counts at 0, at the pinned aa_tolerance, and at 2x and 4x
that. The shape of the collapse is the forensic signal:

    diff that halves at each step        antialiasing, resample kernels, dither
    diff that barely moves               structure — wrong colour, wrong place

This is why the sweep exists rather than a single raw number. A case at 9% raw
that drops to 0.2% by 4x tolerance and a case at 9% raw that is still 8.7% at
4x are entirely different problems, and the mean cannot tell you which you have.
The multipliers are derived from the ONE pinned constant, never typed as new
numbers: plan §2 permits exactly one tolerance to exist, and a sweep that
introduced 5/10/20 as literals would be three more of them.

WHAT IS COMPARED TO WHAT
------------------------
    Chrome   baselines/chrome-148/<scope>/<case>/baseline.png
    RustKit  <root>/<case>/<viewport>/iter-<n>/capture/frame.ppm

Same discovery discipline as Gates A and B, including the same refusal: only
the case's registry viewport is read, because Chrome's frame exists at that
viewport and nowhere else. A case captured only off-viewport is reported
UNMEASURED rather than measured wrongly.

USAGE
    python3 scripts/forensic_board.py --capture-root parity-results/<run>
    python3 scripts/forensic_board.py --capture-root <run> --out forensic/

Outputs, under --out:
    board.json          full machine-readable board
    board.md            the published summary (PR comment / job summary)
    <case>/heatmap.png  per-pixel difference map
"""

import argparse
import json
import os
import sys
from pathlib import Path
from typing import Any, Dict, List, Optional, Sequence, Tuple

sys.path.insert(0, str(Path(__file__).resolve().parent))

from parity_image import Image, UnsupportedImage, read_png, read_ppm, write_png  # noqa: E402
from paint_oracle_gate import (  # noqa: E402
    NON_GATING_SCOPES,
    PolicyError,
    baselines_dir,
    find_frame,
    load_aa_tolerance,
    load_case_registry,
    px_box,
)

REPO_ROOT = Path(__file__).resolve().parent.parent

# Heatmap tile edge, in pixels. 32 is small enough to localise a defect to one
# element on a 1280x800 page and large enough that a tile's count means
# something — a 4px tile is one glyph edge and ranks noise.
TILE_PX = 32

# How many worst tiles to report per case, and how many cases to rank.
DEFAULT_WORST_TILES = 8
DEFAULT_WORST_CASES = 10

# Tolerance sweep multipliers. These multiply the ONE pinned constant from
# docs/VISUAL_DIFF_POLICY.md; they are not tolerances themselves and no
# absolute channel value may be written here. See THE TOLERANCE SWEEP above.
SWEEP_MULTIPLIERS = (0, 1, 2, 4)


def delta_map(chrome: Image, rustkit: Image) -> bytearray:
    """Per-pixel worst-channel absolute difference, one byte per pixel.

    Worst channel, not mean and not sum: a blue-for-red swap has a zero delta
    on green and a mean that understates it by a third. Gate B makes the same
    choice for the same reason; the board must not be gentler than the gate it
    exists to explain.
    """
    a = chrome.rgb
    b = rustkit.rgb
    out = bytearray(chrome.width * chrome.height)
    for i in range(len(out)):
        j = i * 3
        d = abs(a[j] - b[j])
        d1 = abs(a[j + 1] - b[j + 1])
        if d1 > d:
            d = d1
        d2 = abs(a[j + 2] - b[j + 2])
        if d2 > d:
            d = d2
        out[i] = d
    return out


def delta_histogram(deltas: bytearray) -> List[int]:
    """256-bucket histogram. Every threshold count is a suffix sum of this."""
    hist = [0] * 256
    for d in deltas:
        hist[d] += 1
    return hist


def count_above(hist: Sequence[int], threshold: int) -> int:
    """Pixels with delta STRICTLY greater than `threshold`.

    Strictly greater, matching Gate B's `> tolerance`. An off-by-one here would
    make the board and the gate disagree about the same frame, and the board
    would lose the argument for no reason.
    """
    if threshold >= 255:
        return 0
    return sum(hist[threshold + 1 :])


def build_heatmap_lut(tolerance: int) -> List[Tuple[int, int, int]]:
    """Delta -> colour, with a deliberate visual break at the pinned tolerance.

    The break is the whole design. A continuous ramp renders AA fringing and a
    wrong-coloured button as the same warm smear, which is precisely the
    conflation Gate B's two halves exist to separate. Below the tolerance the
    map stays cold and dim; above it, colour arrives abruptly. A reader can see
    the difference between "noisy" and "broken" without reading a number.
    """
    lut: List[Tuple[int, int, int]] = []
    for d in range(256):
        if d == 0:
            lut.append((12, 12, 18))  # agreement: near-black, not pure black,
            continue                  # so an empty heatmap is distinguishable
                                      # from a failed write.
        if d <= tolerance:
            # Within the pinned tolerance: dim blue. Present, deliberately
            # unalarming — Gate B already decided this is not a defect.
            level = 40 + int(40 * d / max(1, tolerance))
            lut.append((level // 3, level // 3, level))
            continue
        # Above tolerance: ramp cyan -> yellow -> red -> white across the
        # remaining range, so severity is legible at a glance.
        span = max(1, 255 - tolerance)
        t = (d - tolerance) / span
        if t < 0.33:
            k = t / 0.33
            lut.append((0, 170 + int(85 * k), 255 - int(100 * k)))
        elif t < 0.66:
            k = (t - 0.33) / 0.33
            lut.append((int(255 * k), 255, int(155 * (1 - k))))
        else:
            k = (t - 0.66) / 0.34
            lut.append((255, 255 - int(200 * k), int(215 * k)))
    return lut


def render_heatmap(deltas: bytearray, width: int, height: int, tolerance: int) -> Image:
    lut = build_heatmap_lut(tolerance)
    rgb = bytearray(width * height * 3)
    for i, d in enumerate(deltas):
        r, g, b = lut[d]
        j = i * 3
        rgb[j] = r
        rgb[j + 1] = g
        rgb[j + 2] = b
    return Image(width, height, bytes(rgb))


def tile_stats(
    deltas: bytearray, width: int, height: int, tolerance: int
) -> List[Dict[str, Any]]:
    """Per-tile counts of ABOVE-TOLERANCE pixels, plus the worst delta seen.

    Tiles rank on above-tolerance pixels rather than raw ones on purpose. A
    tile full of text ranks top of any raw-diff board on every case, forever,
    because glyph antialiasing never agrees bit-for-bit — and a board whose
    worst-N is the same text block every night is one nobody reads twice.
    The raw total is still reported per case; it is just not what sorts tiles.
    """
    tiles: List[Dict[str, Any]] = []
    for ty in range(0, height, TILE_PX):
        for tx in range(0, width, TILE_PX):
            x1 = min(tx + TILE_PX, width)
            y1 = min(ty + TILE_PX, height)
            above = 0
            worst = 0
            raw = 0
            for y in range(ty, y1):
                row = y * width
                for x in range(tx, x1):
                    d = deltas[row + x]
                    if d:
                        raw += 1
                        if d > tolerance:
                            above += 1
                        if d > worst:
                            worst = d
            if above:
                tiles.append(
                    {
                        "x": tx,
                        "y": ty,
                        "w": x1 - tx,
                        "h": y1 - ty,
                        "above_tolerance_px": above,
                        "raw_diff_px": raw,
                        "max_delta": worst,
                    }
                )
    # Severity breaks ties before position does. Whole regions saturate — every
    # tile over a mis-positioned block has all 1024 of its pixels above
    # tolerance — and ordering those by coordinate alone returns a run of
    # adjacent tiles from one defect while a worse one further down the page
    # never makes worst-N. Observed on the first real run: image-gallery's top
    # eight tiles were all 1024px, the first at max delta 162 and the rest at
    # 14-16, listed in that order purely because of where they sat.
    tiles.sort(key=lambda t: (-t["above_tolerance_px"], -t["max_delta"], t["y"], t["x"]))
    return tiles


def attribute_tile(
    tile: Dict[str, Any], elements: Sequence[Dict[str, Any]], width: int, height: int
) -> List[str]:
    """The most specific Chrome elements overlapping a tile.

    Smallest-area first. The page is tiled by `body` and its block descendants
    — Gate B measured 0.00% of the viewport lying outside the union of Chrome's
    rects — so the largest overlapping element is always something useless like
    `html > body`. Specificity by area is what makes the attribution worth
    printing.

    This is a POINTER, not a verdict. It names what is at those coordinates in
    Chrome's layout; it does not claim that element is the cause. Gate A owns
    whether that element's geometry is wrong, and a tile can light up because
    of a neighbour that moved into it.
    """
    tx0, ty0 = tile["x"], tile["y"]
    tx1, ty1 = tx0 + tile["w"], ty0 + tile["h"]
    hits: List[Tuple[float, str]] = []
    for element in elements:
        selector = element.get("selector")
        if not selector:
            continue
        box = px_box(element.get("rect", {}), width, height)
        if box is None:
            continue
        x0, y0, x1, y1 = box
        if x0 >= tx1 or x1 <= tx0 or y0 >= ty1 or y1 <= ty0:
            continue
        hits.append((float((x1 - x0) * (y1 - y0)), selector))
    hits.sort(key=lambda h: (h[0], h[1]))
    return [selector for _, selector in hits[:2]]


def unmeasured_case(case_id: str, scope: str, reason: str) -> Dict[str, Any]:
    """A case the board could not read.

    Recorded as a first-class row, never dropped. A board that silently omits
    the cases it failed to load shrinks toward the cases that happen to work
    and reads as complete.
    """
    return {
        "case_id": case_id,
        "scope": scope,
        "measured": False,
        "reason": reason,
        "raw_diff_px": None,
        "raw_diff_pct": None,
        "sweep": None,
        "max_delta": None,
        "worst_tiles": [],
        "heatmap": None,
    }


def analyse_case(
    case_id: str,
    scope: str,
    chrome: Image,
    rustkit: Image,
    elements: Sequence[Dict[str, Any]],
    tolerance: int,
    worst_tiles: int,
) -> Tuple[Dict[str, Any], Optional[bytearray]]:
    if chrome.size != rustkit.size:
        # Same refusal as Gate B. Scaling one side to fit would make every
        # number below a comparison between two images that were never the
        # same picture, and the heatmap a picture of the resize.
        record = unmeasured_case(
            case_id,
            scope,
            f"size_mismatch: chrome {chrome.width}x{chrome.height} "
            f"vs rustkit {rustkit.width}x{rustkit.height}",
        )
        return record, None

    deltas = delta_map(chrome, rustkit)
    hist = delta_histogram(deltas)
    total = chrome.width * chrome.height

    sweep = {}
    for multiplier in SWEEP_MULTIPLIERS:
        threshold = tolerance * multiplier
        count = count_above(hist, threshold)
        sweep[str(multiplier)] = {
            "threshold": threshold,
            "px": count,
            "pct": (count / total * 100.0) if total else 0.0,
        }

    max_delta = 0
    for d in range(255, -1, -1):
        if hist[d]:
            max_delta = d
            break

    tiles = tile_stats(deltas, chrome.width, chrome.height, tolerance)
    for tile in tiles[:worst_tiles]:
        tile["elements"] = attribute_tile(tile, elements, chrome.width, chrome.height)

    raw_px = sweep["0"]["px"]
    return (
        {
            "case_id": case_id,
            "scope": scope,
            "measured": True,
            "reason": None,
            "width": chrome.width,
            "height": chrome.height,
            "total_px": total,
            "raw_diff_px": raw_px,
            "raw_diff_pct": (raw_px / total * 100.0) if total else 0.0,
            "sweep": sweep,
            "max_delta": max_delta,
            "tiles_above_tolerance": len(tiles),
            "worst_tiles": tiles[:worst_tiles],
            "heatmap": f"{case_id}/heatmap.png",
        },
        deltas,
    )


def build_board(
    capture_root: Path,
    out_dir: Optional[Path],
    case_ids: Optional[Sequence[str]] = None,
    include_non_gating: bool = False,
    tolerance: Optional[int] = None,
    worst_tiles: int = DEFAULT_WORST_TILES,
    write_heatmaps: bool = True,
) -> Dict[str, Any]:
    if tolerance is None:
        tolerance = load_aa_tolerance()
    registry = load_case_registry()

    selected = []
    for case_id, case in sorted(registry.items()):
        if case_ids is not None and case_id not in case_ids:
            continue
        if case["scope"] in NON_GATING_SCOPES and not include_non_gating:
            continue
        selected.append((case_id, case))

    cases: List[Dict[str, Any]] = []
    for case_id, case in selected:
        scope = case["scope"]
        base = baselines_dir() / scope / case_id
        baseline_png = base / "baseline.png"
        if not baseline_png.exists():
            cases.append(unmeasured_case(case_id, scope, "no_chrome_baseline"))
            continue

        frame_path, refusal = find_frame(
            capture_root, case_id, f"{case['width']}x{case['height']}"
        )
        if frame_path is None:
            cases.append(unmeasured_case(case_id, scope, refusal or "no_rustkit_capture"))
            continue

        try:
            chrome = read_png(baseline_png)
            rustkit = read_ppm(frame_path)
        except UnsupportedImage as exc:
            cases.append(unmeasured_case(case_id, scope, f"unreadable_capture: {exc}"))
            continue

        elements: List[Dict[str, Any]] = []
        rects_path = base / "layout-rects.json"
        if rects_path.exists():
            with open(rects_path, encoding="utf-8") as handle:
                elements = json.load(handle).get("elements", [])

        record, deltas = analyse_case(
            case_id, scope, chrome, rustkit, elements, tolerance, worst_tiles
        )
        if deltas is not None and write_heatmaps and out_dir is not None:
            heatmap = render_heatmap(deltas, chrome.width, chrome.height, tolerance)
            write_png(out_dir / case_id / "heatmap.png", heatmap)
        elif deltas is None or not write_heatmaps or out_dir is None:
            record["heatmap"] = None
        cases.append(record)

    measured = [c for c in cases if c["measured"]]
    ranked = sorted(measured, key=lambda c: -c["raw_diff_pct"])

    return {
        "gate": "C-forensic",
        "gating": False,
        "aa_tolerance": tolerance,
        "tile_px": TILE_PX,
        "capture_root": str(capture_root),
        "out_dir": str(out_dir) if out_dir else None,
        "cases": cases,
        "worst_cases": [c["case_id"] for c in ranked[:DEFAULT_WORST_CASES]],
        "summary": {
            "total_cases": len(cases),
            "measured": len(measured),
            "unmeasured": len(cases) - len(measured),
            "mean_raw_diff_pct": (
                sum(c["raw_diff_pct"] for c in measured) / len(measured)
                if measured
                else None
            ),
        },
    }


def board_ran(report: Dict[str, Any]) -> bool:
    """Did the board actually observe anything?

    The ONLY thing that can make Gate C exit non-zero. Not the numbers — see
    NON-GATING IS NOT THE SAME AS ALWAYS-GREEN in the module docstring. A board
    covering zero cases has published nothing, and publishing nothing must not
    read as a clean forensic run.
    """
    return report["summary"]["measured"] > 0


def sweep_shape(case: Dict[str, Any]) -> str:
    """One word for how the diff collapses as tolerance rises.

    A crude classifier, and labelled as one. It reads the ratio of
    above-4x-tolerance pixels to raw ones; it does not know why. It exists so
    the board's first column is a hypothesis rather than a number a reader has
    to re-derive from four other numbers every time.
    """
    sweep = case.get("sweep")
    if not sweep:
        return "—"
    raw = sweep["0"]["px"]
    if raw == 0:
        return "clean"
    survives = sweep["4"]["px"] / raw
    if survives < 0.05:
        return "aa-noise"
    if survives < 0.40:
        return "mixed"
    return "structural"


def render_markdown(report: Dict[str, Any]) -> str:
    summary = report["summary"]
    lines: List[str] = []
    lines.append("## Gate C — forensic board (non-gating)")
    lines.append("")
    lines.append(
        "Raw pixel difference, published for diagnosis. **These numbers cannot "
        "pass or fail this PR** — that is Gate A (geometry) and Gate B (paint). "
        "A raw diff that improves while Gate A regresses is a worse result, not "
        "a better one."
    )
    lines.append("")
    lines.append(
        f"`aa_tolerance = {report['aa_tolerance']}` (pinned in "
        f"`docs/VISUAL_DIFF_POLICY.md`) · tile {report['tile_px']}px · "
        f"{summary['measured']}/{summary['total_cases']} cases measured"
    )
    lines.append("")

    if summary["unmeasured"]:
        lines.append(f"**{summary['unmeasured']} case(s) not measured** — not a pass:")
        lines.append("")
        for case in report["cases"]:
            if not case["measured"]:
                lines.append(f"- `{case['case_id']}` — {case['reason']}")
        lines.append("")

    measured = [c for c in report["cases"] if c["measured"]]
    if measured:
        lines.append("| case | raw diff | > tol | > 2x | > 4x | max Δ | shape |")
        lines.append("|---|---:|---:|---:|---:|---:|---|")
        for case in sorted(measured, key=lambda c: -c["raw_diff_pct"]):
            sweep = case["sweep"]
            lines.append(
                f"| `{case['case_id']}` "
                f"| {case['raw_diff_pct']:.2f}% "
                f"| {sweep['1']['pct']:.2f}% "
                f"| {sweep['2']['pct']:.2f}% "
                f"| {sweep['4']['pct']:.2f}% "
                f"| {case['max_delta']} "
                f"| {sweep_shape(case)} |"
            )
        lines.append("")
        lines.append(
            "`shape` reads how fast the diff collapses as tolerance rises: "
            "`aa-noise` mostly survives nothing (antialiasing, dither, resample "
            "kernels), `structural` survives 4x the tolerance (wrong colour, "
            "wrong place, missing paint). It is a hypothesis, not a diagnosis."
        )
        lines.append("")

        lines.append("### Worst tiles")
        lines.append("")
        for case in sorted(measured, key=lambda c: -c["raw_diff_pct"])[
            :DEFAULT_WORST_CASES
        ]:
            if not case["worst_tiles"]:
                continue
            lines.append(f"**`{case['case_id']}`**")
            lines.append("")
            for tile in case["worst_tiles"]:
                where = ", ".join(f"`{s}`" for s in tile.get("elements") or []) or "—"
                lines.append(
                    f"- ({tile['x']},{tile['y']}) {tile['w']}x{tile['h']} · "
                    f"{tile['above_tolerance_px']}px above tolerance · "
                    f"max Δ {tile['max_delta']} · {where}"
                )
            lines.append("")
        lines.append(
            "Tiles rank on above-tolerance pixels, not raw ones — otherwise "
            "every case's worst tile is the same block of text, every night. "
            "Element names are the most specific Chrome boxes at those "
            "coordinates: a pointer to look there, not a claim about cause."
        )
        lines.append("")

    return "\n".join(lines)


def print_report(report: Dict[str, Any], verbose: bool = False) -> None:
    summary = report["summary"]
    print("Gate C — forensic board (NON-GATING)")
    print(f"  tolerance:  ±{report['aa_tolerance']}/255 (pinned, for the sweep only)")
    print(f"  captures:   {report['capture_root']}")
    print(
        f"  cases:      {summary['measured']}/{summary['total_cases']} measured"
        f"  ({summary['unmeasured']} unmeasured)"
    )
    mean = summary["mean_raw_diff_pct"]
    print(f"  mean raw:   {'—' if mean is None else f'{mean:.4f}%'}  (diagnostic only)")
    print()

    for case in report["cases"]:
        if not case["measured"]:
            print(f"  UNMEASURED {case['case_id']}: {case['reason']}")
            continue
        sweep = case["sweep"]
        print(
            f"  {case['case_id']}: raw {case['raw_diff_pct']:.3f}%"
            f"  >tol {sweep['1']['pct']:.3f}%"
            f"  >4x {sweep['4']['pct']:.3f}%"
            f"  maxΔ {case['max_delta']}  [{sweep_shape(case)}]"
        )
        if verbose:
            for tile in case["worst_tiles"]:
                where = ", ".join(tile.get("elements") or []) or "—"
                print(
                    f"        ({tile['x']},{tile['y']}) "
                    f"{tile['above_tolerance_px']}px  maxΔ {tile['max_delta']}  {where}"
                )


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Gate C — forensic board (non-gating, plan §2)"
    )
    parser.add_argument(
        "--capture-root",
        type=Path,
        default=REPO_ROOT / "parity-results",
        help="Directory holding RustKit frame.ppm captures",
    )
    parser.add_argument(
        "--case", action="append", dest="cases", help="Limit to a case id (repeatable)"
    )
    parser.add_argument(
        "--include-non-gating",
        action="store_true",
        help="Also chart the holdout scope (canary-only, plan §3.6)",
    )
    parser.add_argument(
        "--out",
        type=Path,
        default=REPO_ROOT / "forensic",
        help="Directory for board.json, board.md and per-case heatmaps",
    )
    parser.add_argument(
        "--no-heatmaps",
        action="store_true",
        help="Skip PNG rendering (numbers and tiles only)",
    )
    parser.add_argument(
        "--worst-tiles", type=int, default=DEFAULT_WORST_TILES, help="Tiles per case"
    )
    parser.add_argument("--verbose", action="store_true")
    args = parser.parse_args()

    try:
        report = build_board(
            args.capture_root,
            args.out,
            case_ids=args.cases,
            include_non_gating=args.include_non_gating,
            worst_tiles=args.worst_tiles,
            write_heatmaps=not args.no_heatmaps,
        )
    except PolicyError as exc:
        # The pinned tolerance could not be read. The sweep is defined in terms
        # of it, so there is no board to publish — this is a did-not-run, and
        # did-not-run is the one thing Gate C fails on.
        print(f"Gate C: DID NOT RUN — {exc}", file=sys.stderr)
        return 1

    print_report(report, verbose=args.verbose)

    args.out.mkdir(parents=True, exist_ok=True)
    with open(args.out / "board.json", "w", encoding="utf-8") as handle:
        json.dump(report, handle, indent=2)
    with open(args.out / "board.md", "w", encoding="utf-8") as handle:
        handle.write(render_markdown(report))
    print(f"\nBoard written to {args.out}/board.json and {args.out}/board.md")

    if not board_ran(report):
        print(
            "\nGate C: DID NOT RUN — 0 cases measured. Non-gating means the "
            "numbers never fail a PR; it does not mean a board that observed "
            "nothing reports clean.",
            file=sys.stderr,
        )
        return 1

    print("\nGate C: PUBLISHED (non-gating — these numbers cannot fail a PR)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
