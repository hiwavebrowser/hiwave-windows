#!/usr/bin/env python3
"""seat_control_paint_report.py — how much of Gate B's paint error is this seat?

THE PROBLEM THIS SOLVES
-----------------------
`scripts/seat_control_report.py` (night 44) split Gate A's GEOMETRY error into
the part that is RustKit and the part that is the seat. Gate B had no such
split, so every paint figure taken anywhere but macOS was a sum nobody could
decompose:

    Δ_reported = Δ_real + Δ_confound

For paint the terms are pixels outside the pinned tolerance:

    Δ_confound = Chrome_seat  vs Chrome_pinned    <- this script
    Δ_real     = RustKit_seat vs Chrome_seat      <- this script
    Δ_reported = RustKit_seat vs Chrome_pinned    <- what Gate B prints

The control is `baselines/seat-control/`, captured by
`tools/parity_oracle/capture_seat_control.mjs`, which already writes a
`baseline.png` per case through the same `captureBaseline` code that produced
the pinned set. Nothing read those PNGs until this script.

THE MASK, AND WHY THE THREE COUNTS DO NOT ADD UP
------------------------------------------------
Pixel counts are not additive. A pixel where Chrome_seat and Chrome_pinned
disagree may be a pixel where RustKit happens to land on the pinned value, so
`Δ_confound + Δ_real` is neither an upper nor a lower bound on `Δ_reported`.
Measured 2026-09-26 on all 26 gating cases: `Δ_real` EXCEEDS `Δ_reported` on
six of them. Subtracting one from another is therefore wrong, and this report
never does it.

What can be done instead is to mask. The pixels where the two Chromes agree
within tolerance are pixels the platform is not speaking about; inside that
mask, a RustKit disagreement is not Chrome-side platform difference. So the
report also gives:

    kept          pixels where |Chrome_seat - Chrome_pinned| <= tolerance
    masked        of those, the fraction where RustKit is outside tolerance

The mask removes the CHROME half of the confound and nothing else. RustKit's
own seat dependence — SwiftShader instead of Metal, and on the Linux trench
seat no system text backend at all — is still inside `masked`. That is why
`masked` is reported as a FLOOR and never as an estimate of the macOS number.

WHAT THIS IS FOR
----------------
One question: **is a paint delta of size X worth chasing from this seat?**
A case whose floor is larger than the macOS gap being investigated cannot be
worked here, and saying so is the useful output. Measured 2026-09-26, the four
cases then closest to the paint bar had macOS gaps of 1.06%-1.65% of pixels and
floors on the Linux seat of 1.89%-12.84%: every one of them below its own floor.

WHAT THIS IS NOT
----------------
**NEVER A RECEIPT.** No `N/26`, no conjunction, no case-green count, no
pass/fail verdict on any case. The metric is defined against the pinned macOS
set alone (trench/BASELINE-parity-finish-line.md). This script computes a
diagnostic and refuses rather than guesses when it cannot.

Usage:
    node tools/parity_oracle/capture_seat_control.mjs
    python3 scripts/seat_control_paint_report.py --capture-root <rustkit captures>

Exit codes:
    0 = a paint confound board was produced for at least one case
    1 = the control is missing, unusable, or nothing could be measured
"""

import argparse
import json
import sys
from pathlib import Path
from typing import Any, Dict, List, Optional, Tuple

sys.path.insert(0, str(Path(__file__).resolve().parent))

# Gate B owns the tolerance, the per-pixel comparison, the capture discovery and
# which cases gate. Every one of them is IMPORTED. A second copy of the
# tolerance, or a second idea of what "outside tolerance" means, is how a
# diagnostic and the gate it explains end up disagreeing about the same frame.
from paint_oracle_gate import (  # noqa: E402
    NON_GATING_SCOPES,
    REPO_ROOT,
    find_frame,
    load_aa_tolerance,
    load_case_registry,
)

# The stamp discipline is night 44's and is reused whole: a control captured
# from a fixture that has since changed reports a confound about a page that no
# longer exists, and reports it silently.
from seat_control_report import (  # noqa: E402
    PINNED_SET,
    SEAT_CONTROL_DIR,
    ControlUnusable,
    fixture_sha256,
    load_control_stamp,
)

from parity_image import Image, UnsupportedImage, read_png, read_ppm  # noqa: E402


def three_way_counts(
    pinned: Image, control: Image, rustkit: Image, tolerance: int
) -> Dict[str, int]:
    """One pass, four counts.

    `count_outside_tolerance` in Gate B answers this for one pair; calling it
    three times reads every frame three times and still would not give the
    masked count, which needs two comparisons at the same pixel.

    The per-channel rule is Gate B's and is restated nowhere: a pixel is outside
    tolerance when its WORST channel is, so a blue-for-red swap cannot average
    itself green.
    """
    a, b, r = pinned.rgb, control.rgb, rustkit.rgb
    confound = reported = real = kept = masked_bad = 0
    for i in range(0, len(a), 3):
        a0, a1, a2 = a[i], a[i + 1], a[i + 2]
        b0, b1, b2 = b[i], b[i + 1], b[i + 2]
        r0, r1, r2 = r[i], r[i + 1], r[i + 2]

        chrome_differs = (
            abs(a0 - b0) > tolerance
            or abs(a1 - b1) > tolerance
            or abs(a2 - b2) > tolerance
        )
        rustkit_differs = (
            abs(a0 - r0) > tolerance
            or abs(a1 - r1) > tolerance
            or abs(a2 - r2) > tolerance
        )
        if chrome_differs:
            confound += 1
        if rustkit_differs:
            reported += 1
        if (
            abs(b0 - r0) > tolerance
            or abs(b1 - r1) > tolerance
            or abs(b2 - r2) > tolerance
        ):
            real += 1
        if not chrome_differs:
            kept += 1
            if rustkit_differs:
                masked_bad += 1
    return {
        "confound_px": confound,
        "reported_px": reported,
        "real_px": real,
        "kept_px": kept,
        "masked_bad_px": masked_bad,
    }


def unmeasured(case_id: str, scope: str, reason: str) -> Dict[str, Any]:
    """A case the report could not score. NOT a zero confound.

    Same rule as both gates: a control that did not run and a control that
    found nothing must not print the same.
    """
    return {"case_id": case_id, "scope": scope, "status": "UNMEASURED", "reason": reason}


def read_frame(path: Path) -> Optional[Image]:
    try:
        if path.suffix.lower() == ".png":
            return read_png(path)
        return read_ppm(path)
    except (UnsupportedImage, OSError, ValueError):
        return None


def score_case(
    case_id: str,
    case: Dict[str, Any],
    capture_root: Path,
    control_dir: Path,
    stamp: Dict[str, Any],
    tolerance: int,
) -> Dict[str, Any]:
    scope = case["scope"]

    recorded = (stamp.get("fixtures") or {}).get(case_id)
    current = fixture_sha256(case["html"])
    if recorded is None:
        return unmeasured(case_id, scope, "seat control does not cover this case — recapture")
    if current is None or recorded != current:
        return unmeasured(
            case_id, scope, f"fixture changed since the control was captured ({case['html']}) — recapture"
        )

    pinned_path = REPO_ROOT / "baselines" / PINNED_SET / scope / case_id / "baseline.png"
    control_path = control_dir / scope / case_id / "baseline.png"
    if not pinned_path.exists():
        return unmeasured(case_id, scope, "no pinned baseline frame")
    if not control_path.exists():
        return unmeasured(case_id, scope, "no seat control frame for this case")

    native_viewport = f"{case['width']}x{case['height']}"
    frame_path, refusal = find_frame(capture_root, case_id, native_viewport)
    if frame_path is None:
        return unmeasured(case_id, scope, refusal or "no RustKit capture")

    pinned = read_frame(pinned_path)
    control = read_frame(control_path)
    rustkit = read_frame(frame_path)
    for image, what in ((pinned, "pinned"), (control, "control"), (rustkit, "rustkit")):
        if image is None:
            return unmeasured(case_id, scope, f"could not read the {what} frame")

    # Scaling any side to fit would make every number below a comparison
    # between pictures that were never the same picture. Gate B refuses this
    # for two frames; a three-way report has three chances to get it wrong.
    if not (pinned.size == control.size == rustkit.size):
        return unmeasured(
            case_id,
            scope,
            f"frame sizes disagree: pinned {pinned.width}x{pinned.height}, "
            f"control {control.width}x{control.height}, "
            f"rustkit {rustkit.width}x{rustkit.height}",
        )

    counts = three_way_counts(pinned, control, rustkit, tolerance)
    total = pinned.width * pinned.height
    kept = counts["kept_px"]

    # A mask that kept nothing has no denominator. Printing 0.0 would read as
    # "RustKit agrees everywhere the platform does", which is the opposite of
    # what an empty mask means.
    if kept == 0:
        return unmeasured(case_id, scope, "the two Chromes disagree on every pixel — no mask")

    record = {
        "case_id": case_id,
        "scope": scope,
        "status": "MEASURED",
        "total_px": total,
        **counts,
        "confound_pct": round(counts["confound_px"] / total * 100, 4),
        "reported_pct": round(counts["reported_px"] / total * 100, 4),
        "real_pct": round(counts["real_px"] / total * 100, 4),
        "kept_pct": round(kept / total * 100, 4),
        # The denominator is the MASK, not the frame. Over the whole frame this
        # number would fall purely because the mask shrank, and a case whose
        # confound grew would look like a case that improved.
        "masked_pct": round(counts["masked_bad_px"] / kept * 100, 4),
    }
    # The smallest RustKit-vs-Chrome paint difference this seat can tell apart
    # from its own platform noise. Masking removes the Chrome half of the
    # confound, so the masked residual is the better floor — but only where it
    # is the smaller of the two, and it is reported as a floor, never as an
    # estimate of what macOS would say.
    record["floor_pct"] = min(record["confound_pct"], record["masked_pct"])
    return record


def build_report(
    capture_root: Path,
    control_dir: Path = SEAT_CONTROL_DIR,
    case_ids: Optional[List[str]] = None,
    tolerance: Optional[int] = None,
) -> Dict[str, Any]:
    stamp = load_control_stamp(control_dir)
    if tolerance is None:
        tolerance = load_aa_tolerance()

    registry = load_case_registry()
    cases = {
        cid: case
        for cid, case in registry.items()
        if case.get("scope") not in NON_GATING_SCOPES
        and (case_ids is None or cid in case_ids)
    }

    records = [
        score_case(cid, cases[cid], capture_root, control_dir, stamp, tolerance)
        for cid in sorted(cases)
    ]
    measured = [r for r in records if r["status"] == "MEASURED"]
    return {
        # Named so no reader and no downstream script can mistake this document
        # for a gate receipt. `finish_line_receipt.py` consumes gate-a/gate-b
        # by shape; this shape is deliberately not theirs.
        "kind": "seat-control-paint-confound",
        "receipt": False,
        "aa_tolerance": tolerance,
        "pinned_set": PINNED_SET,
        "control": {
            "captured_at": stamp.get("captured_at"),
            "platform": stamp.get("platform"),
        },
        "cases_discovered": len(cases),
        "cases_measured": len(measured),
        "cases": records,
    }


def print_report(report: Dict[str, Any]) -> None:
    print("seat-control PAINT confound report — NOT A RECEIPT, NOT AN N/26.")
    print("The metric is defined against the pinned macOS set alone.\n")
    control = report["control"]
    print(f"control captured {control.get('captured_at')} on {control.get('platform')}")
    print(f"pinned set {report['pinned_set']}, aa_tolerance {report['aa_tolerance']}\n")
    header = (
        f"{'case':26s} {'reported':>10s} {'confound':>10s} {'real':>10s} "
        f"{'kept':>8s} {'masked':>9s} {'floor':>9s}"
    )
    print(header)
    for record in report["cases"]:
        if record["status"] != "MEASURED":
            print(f"{record['case_id']:26s}  UNMEASURED — {record['reason']}")
            continue
        print(
            f"{record['case_id']:26s} {record['reported_pct']:9.4f}% "
            f"{record['confound_pct']:9.4f}% {record['real_pct']:9.4f}% "
            f"{record['kept_pct']:7.2f}% {record['masked_pct']:8.4f}% "
            f"{record['floor_pct']:8.4f}%"
        )
    print(
        f"\n{report['cases_measured']}/{report['cases_discovered']} cases measured. "
        "`floor` is the smallest RustKit-vs-Chrome paint difference this seat can\n"
        "distinguish from its own platform noise. A macOS gap below a case's floor\n"
        "cannot be classified here — that is a statement about this seat, not the engine."
    )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--capture-root", required=True, type=Path)
    parser.add_argument("--control-dir", type=Path, default=SEAT_CONTROL_DIR)
    parser.add_argument("--case", action="append", dest="cases")
    parser.add_argument("--json", type=Path)
    args = parser.parse_args()

    try:
        report = build_report(args.capture_root, args.control_dir, args.cases)
    except ControlUnusable as err:
        print(f"REFUSED: {err}")
        return 1

    print_report(report)
    if args.json:
        args.json.parent.mkdir(parents=True, exist_ok=True)
        args.json.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")

    # A board that measured nothing is not a clean board. Same rule as Gate C.
    return 0 if report["cases_measured"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
