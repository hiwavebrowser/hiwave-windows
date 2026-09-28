#!/usr/bin/env python3
"""seat_control_report.py — how much of Gate A's geometry error is this seat?

THE PROBLEM THIS SOLVES
-----------------------
Gate A (`scripts/layout_oracle_gate.py`) compares RustKit's `layout.json`
against `baselines/chrome-148/`, which was captured by Chrome 148 on macOS.
When RustKit is also on macOS that difference is a defect and nothing else.
Anywhere else it is a sum of two terms the gate cannot tell apart:

    Δ_reported = Δ_real + Δ_confound

where Δ_confound is the seat itself — fontconfig substituting for `Georgia`,
a different Chromium build, a different rasterizer. `trench/digest-parity-
finish-line.md` (night 4, 2026-08-04) recorded the split as needing "a macOS
run to make, not a cleverer analysis of this one". It needs neither: it needs a
CONTROL, captured by `tools/parity_oracle/capture_seat_control.mjs` — the same
cases through the same capture code, with the seat's own browser and fonts.

    Δ_confound = Chrome_seat  − Chrome_pinned
    Δ_real     = RustKit_seat − Chrome_seat
    Δ_reported = RustKit_seat − Chrome_pinned

This script reports all three, per case, and classifies each failing axis.

WHAT THIS IS NOT
----------------
**NEVER A RECEIPT.** Nothing here is an `N/26`. The metric is defined against
the pinned macOS set alone (`trench/BASELINE-parity-finish-line.md`), and this
script computes no finish-line verdict, no conjunction, and no case-green count.
It answers exactly one question — *is a geometry delta on this seat worth
chasing?* — and refuses rather than guesses when it cannot.

Usage:
    python3 scripts/seat_control_report.py --layout-root parity-results
    python3 scripts/seat_control_report.py --layout-root <dir> --json out.json

Exit codes:
    0 = a confound board was produced for at least one case
    1 = the control is missing, unusable, or nothing could be measured
"""

import argparse
import hashlib
import json
import sys
from pathlib import Path
from typing import Any, Dict, List, Optional, Tuple

# The gate is the single source of truth for the join, the tolerance, and which
# cases are gating. Restating any of them here would let the two drift into
# disagreeing about what a failure is — the instrument-lie class this campaign
# exists to end.
sys.path.insert(0, str(Path(__file__).resolve().parent))
from layout_oracle_gate import (  # noqa: E402
    GEOMETRY_TOLERANCE_PX,
    NON_GATING_SCOPES,
    REPO_ROOT,
    border_box,
    load_case_registry,
    load_json,
)

AXES = ("x", "y", "width", "height")

SEAT_CONTROL_DIR = REPO_ROOT / "baselines" / "seat-control"

# The pinned set. Named here so the report cannot be pointed at a second seat
# control and asked to compare two diagnostics to each other.
PINNED_SET = "chrome-148"


class ControlUnusable(Exception):
    """The seat control cannot answer the question it was asked."""


def rects_by_selector(doc: Dict[str, Any]) -> Dict[str, Dict[str, float]]:
    out: Dict[str, Dict[str, float]] = {}
    for element in doc.get("elements", []):
        selector = element.get("selector")
        rect = element.get("rect")
        if selector and isinstance(rect, dict):
            out[selector] = rect
    return out


def load_control_stamp(control_dir: Path) -> Dict[str, Any]:
    stamp_path = control_dir / "STAMP.json"
    if not stamp_path.exists():
        raise ControlUnusable(
            f"no seat control at {control_dir} — run "
            "`node tools/parity_oracle/capture_seat_control.mjs` first"
        )
    stamp = load_json(stamp_path)
    if stamp is None or stamp.get("kind") != "seat-control":
        raise ControlUnusable(f"{stamp_path} is not a seat-control stamp")
    return stamp


def compare(
    a: Dict[str, Dict[str, float]],
    b: Dict[str, Dict[str, float]],
) -> Tuple[float, int, Dict[Tuple[str, str], float]]:
    """Sum of |Δ| over axes that exceed the gate's tolerance, b relative to a."""
    total = 0.0
    count = 0
    per_axis: Dict[Tuple[str, str], float] = {}
    for selector in a.keys() & b.keys():
        for axis in AXES:
            try:
                delta = float(b[selector][axis]) - float(a[selector][axis])
            except (KeyError, TypeError, ValueError):
                continue
            per_axis[(selector, axis)] = delta
            if abs(delta) > GEOMETRY_TOLERANCE_PX:
                total += abs(delta)
                count += 1
    return total, count, per_axis


def find_layout_json(layout_root: Path, case_id: str) -> Optional[Path]:
    direct = layout_root / case_id / "layout.json"
    if direct.exists():
        return direct
    matches = sorted(layout_root.glob(f"**/{case_id}/**/layout.json"))
    return matches[0] if matches else None


def rustkit_rects(layout_path: Path) -> Dict[str, Dict[str, float]]:
    """RustKit's rects, keyed by the P0a-0 join key.

    The rect is chosen by Gate A's own `border_box`, IMPORTED and not restated.
    Which rect corresponds to Chrome's `getBoundingClientRect` is a rule, not a
    field name: the baseline is POST-transform, so on a transformed box the
    layout `border_box` and Chrome's rect measure two different things and the
    difference between them is the renderer's translate, not a defect. Gate A
    learned that (see its `border_box` docstring, and `sticky-scroll`'s
    `.overflow-content`); this report restated the extraction as
    `node["border_box"]` and un-learned it.

    Measured cost of the restatement, 2026-09-24, develop `a66c159`: `new_tab`'s
    `.ambient-glow` (`transform: translateX(-50%)` on an 800px box) was reported
    as a 400.00px delta that survived the control — the LARGEST pure-`real` axis
    on the whole 26-case board, ranked first, and entirely a phantom. Gate A
    scored the same box GREEN on the same captures. A night that picked its unit
    off this board picked a box the engine already gets bit-exact.
    """
    doc = load_json(layout_path)
    if doc is None:
        return {}
    seen: Dict[str, List[Dict[str, float]]] = {}

    def walk(node: Dict[str, Any]) -> None:
        selector = node.get("selector")
        box = border_box(node)
        # Anonymous and text boxes carry no selector and are EXCLUDED, never
        # paired positionally — same rule as Gate A.
        if selector and isinstance(box, dict):
            seen.setdefault(selector, []).append(box)
        for child in node.get("children", []) or []:
            walk(child)

    walk(doc.get("root", doc))

    # A selector that matched more than one box is an AMBIGUOUS join, and Gate A
    # refuses to compare it (`ambiguous_selector`) rather than pick one. Taking
    # the first would attribute a delta to whichever box the walk reached first
    # — a confident number for an element the receipt oracle declines to score.
    # Nothing to score is the honest answer, and it is the same answer Gate A
    # gives. No case in the 26-case set is ambiguous today, so this changes no
    # figure on the current board; it stops the two instruments from disagreeing
    # the first time one is.
    return {selector: boxes[0] for selector, boxes in seen.items() if len(boxes) == 1}


def classify(reported: float, real: float) -> str:
    """Attribute one failing axis.

    `confound` means the seat explains it: the pinned comparison fails and the
    same-seat comparison does not. Such an axis is not evidence of a RustKit
    defect and must not be worked from this seat.

    `real` means the same-seat comparison fails too — the delta survives after
    the platform is held identical on both sides, so it is box math.

    `mixed` means both fail: real, but the reported magnitude is not the
    defect's magnitude.
    """
    reported_fails = abs(reported) > GEOMETRY_TOLERANCE_PX
    real_fails = abs(real) > GEOMETRY_TOLERANCE_PX
    if reported_fails and not real_fails:
        return "confound"
    if real_fails and not reported_fails:
        return "masked"
    if real_fails and reported_fails:
        return "mixed" if abs(abs(reported) - abs(real)) > GEOMETRY_TOLERANCE_PX else "real"
    return "green"


def fixture_sha256(html_rel: str) -> Optional[str]:
    path = REPO_ROOT / html_rel
    if not path.exists():
        return None
    return hashlib.sha256(path.read_bytes()).hexdigest()


def score_case(
    case_id: str,
    scope: str,
    layout_root: Path,
    control_dir: Path,
    html_rel: str,
    stamp: Dict[str, Any],
) -> Dict[str, Any]:
    record: Dict[str, Any] = {"case_id": case_id, "scope": scope}

    # A control taken from a different fixture than the one being scored is not
    # a control. Its numbers still parse — they are simply about a page that no
    # longer exists — so the failure has to be raised here or it is never seen.
    recorded = (stamp.get("fixtures") or {}).get(case_id)
    current = fixture_sha256(html_rel)
    if recorded is None:
        record["status"] = "UNMEASURED"
        record["reason"] = "seat control does not cover this case — recapture"
        return record
    if current is None or recorded != current:
        record["status"] = "UNMEASURED"
        record["reason"] = (
            f"fixture changed since the control was captured ({html_rel}) — recapture"
        )
        return record

    pinned_doc = load_json(REPO_ROOT / "baselines" / PINNED_SET / scope / case_id / "layout-rects.json")
    control_doc = load_json(control_dir / scope / case_id / "layout-rects.json")
    if pinned_doc is None:
        record["status"] = "UNMEASURED"
        record["reason"] = "no pinned baseline"
        return record
    if control_doc is None:
        record["status"] = "UNMEASURED"
        record["reason"] = "no seat control for this case"
        return record

    pinned = rects_by_selector(pinned_doc)
    control = rects_by_selector(control_doc)

    # A control that does not cover the same elements would under-report the
    # confound on exactly the elements it is missing, which is worse than not
    # running: it would license working a root that is pure platform noise.
    missing = sorted(set(pinned) - set(control))
    extra = sorted(set(control) - set(pinned))
    if missing or extra:
        record["status"] = "UNMEASURED"
        record["reason"] = (
            f"selector sets disagree: {len(missing)} missing from control, {len(extra)} extra"
        )
        record["selector_mismatch"] = {"missing": missing[:10], "extra": extra[:10]}
        return record

    layout_path = find_layout_json(layout_root, case_id)
    if layout_path is None:
        record["status"] = "UNMEASURED"
        record["reason"] = "no RustKit layout.json"
        return record
    rustkit = rustkit_rects(layout_path)
    if not rustkit:
        record["status"] = "UNMEASURED"
        record["reason"] = "RustKit layout.json carried no selectors"
        return record

    confound_sum, confound_n, confound_axes = compare(pinned, control)
    reported_sum, reported_n, reported_axes = compare(pinned, rustkit)
    real_sum, real_n, real_axes = compare(control, rustkit)

    buckets: Dict[str, int] = {"confound": 0, "real": 0, "mixed": 0, "masked": 0}
    worst: List[Dict[str, Any]] = []
    for key in reported_axes.keys() | real_axes.keys():
        kind = classify(reported_axes.get(key, 0.0), real_axes.get(key, 0.0))
        if kind == "green":
            continue
        buckets[kind] += 1
        worst.append(
            {
                "selector": key[0],
                "axis": key[1],
                "kind": kind,
                "reported": round(reported_axes.get(key, 0.0), 4),
                "real": round(real_axes.get(key, 0.0), 4),
                "confound": round(confound_axes.get(key, 0.0), 4),
            }
        )
    worst.sort(key=lambda row: -abs(row["real"]))

    record.update(
        status="MEASURED",
        elements=len(pinned),
        confound_sum=round(confound_sum, 4),
        confound_axes=confound_n,
        reported_sum=round(reported_sum, 4),
        reported_axes=reported_n,
        real_sum=round(real_sum, 4),
        real_axes=real_n,
        # The one number this report exists to produce: how much of what Gate A
        # blames on RustKit is actually this seat.
        confound_share=(round(confound_sum / reported_sum, 4) if reported_sum else 0.0),
        buckets=buckets,
        worst_real=worst[:20],
    )
    return record


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--layout-root", required=True, type=Path)
    parser.add_argument("--control-dir", type=Path, default=SEAT_CONTROL_DIR)
    parser.add_argument("--case", action="append", dest="cases")
    parser.add_argument("--json", type=Path)
    parser.add_argument("--verbose", action="store_true")
    args = parser.parse_args()

    print("seat-control confound report — NOT A RECEIPT, NOT AN N/26.")
    print("The metric is defined against the pinned macOS set alone.\n")

    try:
        stamp = load_control_stamp(args.control_dir)
    except ControlUnusable as err:
        print(f"REFUSED: {err}")
        return 1

    print(f"control captured {stamp.get('captured_at')} on {stamp.get('platform')}")
    georgia = (stamp.get("font_resolution") or {}).get("Georgia", "?")
    print(f"seat font resolution: Georgia -> {georgia}\n")

    registry = load_case_registry()
    records: List[Dict[str, Any]] = []
    for case_id, case in sorted(registry.items()):
        if case.get("scope") in NON_GATING_SCOPES:
            continue
        if args.cases and case_id not in args.cases:
            continue
        records.append(
            score_case(
                case_id,
                case["scope"],
                args.layout_root,
                args.control_dir,
                case["html"],
                stamp,
            )
        )

    measured = [r for r in records if r["status"] == "MEASURED"]
    unmeasured = [r for r in records if r["status"] != "MEASURED"]

    measured.sort(key=lambda r: -r["reported_sum"])
    header = f"{'case':<26}{'reported':>12}{'real':>12}{'confound':>12}{'share':>8}  buckets"
    print(header)
    print("-" * len(header))
    for r in measured:
        b = r["buckets"]
        print(
            f"{r['case_id']:<26}{r['reported_sum']:>12.2f}{r['real_sum']:>12.2f}"
            f"{r['confound_sum']:>12.2f}{r['confound_share'] * 100:>7.1f}%"
            f"  real={b['real']} mixed={b['mixed']} confound={b['confound']} masked={b['masked']}"
        )

    for r in unmeasured:
        print(f"{r['case_id']:<26}UNMEASURED — {r['reason']}")

    if measured:
        reported_total = sum(r["reported_sum"] for r in measured)
        real_total = sum(r["real_sum"] for r in measured)
        confound_total = sum(r["confound_sum"] for r in measured)
        print("-" * len(header))
        share = confound_total / reported_total if reported_total else 0.0
        print(
            f"{'TOTAL':<26}{reported_total:>12.2f}{real_total:>12.2f}"
            f"{confound_total:>12.2f}{share * 100:>7.1f}%"
        )
        print(
            f"\n{len(measured)} measured, {len(unmeasured)} unmeasured. "
            f"{share * 100:.1f}% of what Gate A reports on this seat is the seat."
        )

    if args.verbose:
        for r in measured:
            if not r["worst_real"]:
                continue
            print(f"\n{r['case_id']} — worst surviving (real) deltas:")
            for row in r["worst_real"][:10]:
                print(
                    f"  {row['kind']:<8} {row['axis']:<6} real={row['real']:>10.3f} "
                    f"reported={row['reported']:>10.3f} confound={row['confound']:>9.3f} "
                    f"{row['selector'][:64]}"
                )

    if args.json:
        args.json.parent.mkdir(parents=True, exist_ok=True)
        args.json.write_text(
            json.dumps(
                {
                    "report": "seat-control-confound",
                    "not_a_receipt": True,
                    "tolerance_px": GEOMETRY_TOLERANCE_PX,
                    "pinned_set": PINNED_SET,
                    "control_stamp": stamp,
                    "cases": records,
                },
                indent=2,
            )
            + "\n", encoding="utf-8"
        )
        print(f"\nReport written to {args.json}")

    # A report that measured nothing is a failure, not a pass. Same rule as the
    # gates: unmeasured is never green.
    return 0 if measured else 1


if __name__ == "__main__":
    sys.exit(main())
