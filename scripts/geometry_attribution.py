#!/usr/bin/env python3
"""geometry_attribution.py — non-gating attribution of Gate A's failure list.

Baseline and rules: trench/BASELINE-parity-finish-line.md. Plan:
docs/PARITY_FINISH_LINE_PLAN_2026-08-04.md §2 (the gate) and its 2026-08-12
geometry-first amendment (why the grind needs aiming at geometry roots).

Gate A scores ABSOLUTE viewport coordinates and that is right for the metric: a
box in the wrong place is wrong, wherever the error came from. But a raw failure
count is a poor aiming device, because one displaced ancestor makes every
descendant fail too. This file answers a different question — WHICH failing box
is failing at its own edge — and it answers it for the grind, never for the gate.

It cannot fail a PR. Same construction as Gate C:

    ran, published, numbers terrible   -> exit 0
    ran, published, numbers perfect    -> exit 0
    could not run / measured nothing   -> exit 1

Two independent splits, and each is a claim with a stated limit.

ROOT vs CARRIED
---------------
For each failing (box, axis), subtract the same axis's delta on the nearest
ancestor BOTH sides can see. If what remains is within Gate A's tolerance, the
box is moving exactly as much as its parent already moves: CARRIED. Otherwise it
is a ROOT — something at this box's own edge is wrong.

    flex-positioning  #check1  y  expected 207, actual 199, delta -8.0
                      its own row is -8.0 too, so the residual is 0.0: CARRIED.
                      The checkbox is laid out exactly right inside its row;
                      the 8px is two text-sized boxes above it.

**CARRIED is arithmetic, not blame.** It says the parent's delta already
accounts for this one. For x/y and for a stretched width the influence really
does run parent -> child, so the reading is usually causal. For HEIGHT it often
runs the other way — a block's height comes from its children — so a carried
height means "these two moved together", never "the parent is at fault". The
split is published per axis so that reading stays available.

TEXT-REACHABLE vs FONT-INDEPENDENT
----------------------------------
A box is text-reachable when a text measurement can move its position or size:
its subtree carries a text run, or it is a form control sized from a label,
value or placeholder, or any sibling under the same parent is a text run.

That last clause is not defensive padding, it is the one that had to be
measured. `rounded-corners` lays six `.test-box` divs in a row with nothing but
source newlines between them; each collapsed space is a real glyph with a real
advance, and on a seat with no font backend the whole row staircases by
8.0 - 4.1875 per gap. A classifier that only looked INSIDE each box called all
of them font-independent and would have aimed a night's work at inline-block
positioning. With the sibling clause the board's font-independent root count
goes from 170 to 13.

**Text-reachable is a NECESSARY condition for unreadability, not proof of it.**
A text-bearing box can still have a font-independent defect — night 13's
`fit-content` sidebars were text-bearing and their defect was a 1400px stretch.
So the font-independent count is a lower bound on what a text-less seat can
score, never an upper bound on what is real.

MEASURED FONT-SENSITIVITY (--font-probe-root), and why the heuristic needed it
-----------------------------------------------------------------------------
The three clauses above are a guess about which boxes a font can move, and on
2026-08-21 the guess was measured and found wrong on 9 of the 12 roots it was
publishing as readable. The mechanism it cannot see is the LINE BOX: an element
with inline-level children sits on a line whose height includes the strut, and
`inline_strut_descent` is `measure_text_advanced("x", ...)` — a font call. So
`backgrounds body > div:nth-of-type(4)` holds one `inline-block` child, no text
node anywhere in its subtree or among its siblings, and its height still moves
when the font metrics move. Its in-flow following siblings move with it.

That clause cannot be added from this side: RustKit's `layout.json` exports
`type` (block/inline/text/...) and no `display`, so an `inline-block` child is
indistinguishable from a block one in the dump. A fourth heuristic would need an
engine export change and would still only cover the mechanism we happened to
find — this is the third correction to the same classifier (170 -> 13 -> 12 -> 4).

So the board can MEASURE it instead. Give `--font-probe-root` a second capture
of the same corpus taken with perturbed font metrics, and a box is font-sensitive
iff its border box differs between the two captures. That asks the question the
classifier is a proxy for — *can a font measurement move this box* — and it is
blind to mechanism, so a fourth mechanism does not need a fourth clause.

Stated limits, because this is evidence and not proof:
  * It is a LOWER bound too. A perturbation the corpus happens not to be
    sensitive to leaves a real dependency unseen. Perturb more than one metric
    (advance AND ascent/descent) and take the union; one probe is weaker.
  * A box the probe capture does not contain is font-sensitive, never readable.
    Unknown is not green, the same rule the receipt and Gate B already use.
  * It says a font CAN move the box, not that the font is why it fails.

Without the flag the heuristic still runs and the report says which basis it
used. The heuristic OVER-REPORTS readable work; it must never be read as
measured.

FONT-INEXPLICABLE (the magnitude column, 2026-08-26)
----------------------------------------------------
The rule above disqualifies a root when a font CAN reach it. By 2026-08-26 it
disqualified everything: 1993 roots, **0** published as readable, while
`new_tab body > div.footer:nth-of-type(3)` sat 1280px wide against Chrome's
137.59 and moved **0.000px** under every probe. It holds text, so the rule is
right that a font can reach it; no advance width makes a box 9x too wide, so
the board was right about reachability and useless for aiming.

The second column asks the other question — not *can a font touch this box* but
*could a font produce an error this BIG*. `font_envelope_px` is the furthest the
axis moved across the whole probe set; a residual more than
`--font-envelope-factor` times that has a non-font component of at least the
difference. The two columns are published side by side and neither replaces the
other: the strict one says what a text-less seat can measure end to end, the
magnitude one says where a defect certainly exists. A root can be in both.

Its limits are on `font_inexplicable`, and the first is load-bearing: the claim
is only as strong as the probes' perturbations, so the digest must publish those
next to the number.

Usage:
    python3 scripts/geometry_attribution.py --layout-root parity-baseline/captures
    python3 scripts/geometry_attribution.py --layout-root <dir> --case rounded-corners
    python3 scripts/geometry_attribution.py --layout-root <dir> --json out.json
    python3 scripts/geometry_attribution.py --layout-root <dir> \\
        --font-probe-root <dir-captured-with-perturbed-font-metrics>
"""

import argparse
import json
import sys
from pathlib import Path
from typing import Any, Dict, List, Optional, Tuple

sys.path.insert(0, str(Path(__file__).resolve().parent))

# The tolerance, the join, the capture discovery and Chrome's own omissions all
# come from Gate A. Restating any of them here would create a second number that
# must agree with the first and eventually will not — the same reasoning that
# put the paint gate's geometry precondition on an import (2026-08-11).
from layout_oracle_gate import (  # noqa: E402
    AXES,
    GEOMETRY_TOLERANCE_PX,
    NON_GATING_SCOPES,
    border_box,
    chrome_rects_path,
    load_case_registry,
    load_json,
    find_layout_json,
)

TEXT_SIZED_CONTROLS = ("Button", "TextInput", "TextArea", "Select")

# How many times the measured font envelope a residual must exceed before the
# board will say a font cannot account for it. Ten, because the probes are a
# finite sample of metric space and the claim has to survive the metrics they
# did not perturb. Overridable with --font-envelope-factor; the value used is
# published in the report so a board can never be read without it.
FONT_ENVELOPE_FACTOR = 10.0


def annotate(root: Dict[str, Any]) -> Tuple[
    Dict[str, Dict[str, Any]], Dict[str, Optional[str]], Dict[str, bool]
]:
    """Walk the RustKit tree once for the three things this file needs.

    Returns (boxes, parent, text_reachable) keyed by selector:
      * boxes         selector -> the box dict
      * parent        selector -> nearest ANCESTOR selector, or None
      * text_reachable selector -> whether a text measurement can move it

    Anonymous and text boxes carry no selector and are not keys; they are still
    walked, because a text child is exactly what taints its element parent and
    its element siblings.
    """
    boxes: Dict[str, Dict[str, Any]] = {}
    parent: Dict[str, Optional[str]] = {}
    text_reachable: Dict[str, bool] = {}

    def is_text_run(node: Dict[str, Any]) -> bool:
        # ANY text node, whitespace included. A collapsed space between two
        # inline-blocks has a font-dependent advance and moves the second one.
        return node.get("type") == "text" and node.get("text") is not None

    def walk(node: Dict[str, Any], ancestor: Optional[str]) -> bool:
        selector = node.get("selector")
        if selector:
            boxes.setdefault(selector, node)
            parent.setdefault(selector, ancestor)
            ancestor = selector

        control = node.get("control_type") or ""
        own = is_text_run(node) or any(
            control.startswith(kind) for kind in TEXT_SIZED_CONTROLS
        )

        children = node.get("children") or []
        # A text run anywhere in this box's child list taints EVERY element
        # child of it, not only the ones after it: line breaking and alignment
        # move boxes in both directions.
        sibling_text = any(is_text_run(child) for child in children)
        for child in children:
            own |= walk(child, ancestor)
            child_selector = child.get("selector")
            if child_selector and sibling_text:
                text_reachable[child_selector] = True

        if selector:
            text_reachable[selector] = text_reachable.get(selector, False) or own
        return own

    walk(root, None)
    return boxes, parent, text_reachable


def measure_font_movement(
    root: Dict[str, Any], probe_root: Dict[str, Any]
) -> Dict[Tuple[str, str], float]:
    """(selector, axis) -> HOW FAR that axis moved when the font metrics moved.

    The magnitude behind `measure_font_sensitivity`, which is this with a
    `> 0` on the end. One implementation, because a boolean "did it move" and
    a float "how far" that must agree about the same join, the same fallbacks
    and the same absent-axis rule will eventually disagree if written twice —
    the reasoning that put Gate B's tolerance on an import from Gate A.

    Absent means absent: an axis this probe could not compare is left out of
    the dict rather than given 0.0, because 0.0 reads as "the font cannot move
    this box" and is the flattering direction. Both consumers apply their own
    "unknown is not green" default.

    Per axis, not per box, because the board aims work at an axis. `test11`'s
    image on `images-intrinsic` is the case that forced it: its `y` shifts with
    the font (everything above it is text) while its `height` — 160 against
    Chrome's 90, an unapplied `aspect-ratio` — does not move under any probe.
    Scoring the box as a whole would hide a readable height behind an
    unreadable y and drop a real defect off the board.

    Joined on the STRUCTURAL PATH (root-relative child indices), not on the
    selector, because the probe capture is the same tree laid out again: the
    path is stable under a metrics change while a selector can be absent from
    one side if identity stamping ever differs. A selector whose path is missing
    from the probe, or whose box carries no border box on either side, is
    reported SENSITIVE — the conservative direction, and the same rule as
    "unmeasured is never green".
    """

    def by_path(node: Dict[str, Any]) -> Dict[Tuple[int, ...], Dict[str, Any]]:
        out: Dict[Tuple[int, ...], Dict[str, Any]] = {}

        def walk(n: Dict[str, Any], path: Tuple[int, ...]) -> None:
            out[path] = n
            for index, child in enumerate(n.get("children") or []):
                walk(child, path + (index,))

        walk(node, ())
        return out

    base = by_path(root)
    probe = by_path(probe_root)

    movement: Dict[Tuple[str, str], float] = {}

    def mark(selector: str, axis: str, moved: float) -> None:
        # MAX-accumulate: one selector can key several boxes (the walk in
        # `annotate` keeps the first). If ANY of them moves on this axis, the
        # axis moves — and the envelope is the widest of them, because the
        # smaller one would understate what a font can do to this selector.
        key = (selector, axis)
        movement[key] = max(movement.get(key, 0.0), moved)

    for path, node in base.items():
        selector = node.get("selector")
        if not selector:
            continue
        counterpart = probe.get(path)
        here = border_box(node)
        there = border_box(counterpart) if counterpart is not None else None
        if here is None or there is None:
            # An axis this probe could not compare is left ABSENT, not marked.
            # "Unknown is not green" is then expressed exactly once, at the
            # consumer's `.get(..., True)` default — a rule written down twice
            # is a rule that eventually disagrees with itself.
            continue
        for axis in AXES:
            mark(
                selector,
                axis,
                abs(float(here.get(axis, 0.0)) - float(there.get(axis, 0.0))),
            )
    return movement


def measure_font_sensitivity(
    root: Dict[str, Any], probe_root: Dict[str, Any]
) -> Dict[Tuple[str, str], bool]:
    """(selector, axis) -> did THAT AXIS move when the font metrics moved?

    The board's original question, now derived from the magnitude rather than
    measured separately. Any movement at all disqualifies: this is the strict
    readability rule and it does not care how far the box moved.
    """
    return {
        key: distance != 0.0
        for key, distance in measure_font_movement(root, probe_root).items()
    }


def nearest_comparable(
    selector: str,
    parent: Dict[str, Optional[str]],
    chrome: Dict[str, Dict[str, float]],
    rustkit: Dict[str, Dict[str, float]],
) -> Optional[str]:
    """The nearest ancestor with a rect on BOTH sides.

    Skipping past ancestors Chrome never captured is deliberate: the comparison
    needs two rects, and an ancestor present on only one side cannot supply a
    delta. Returning None means the box is compared against nothing and its
    delta is entirely its own — which is the correct reading for `html > body`.
    """
    current = parent.get(selector)
    while current is not None and (current not in chrome or current not in rustkit):
        current = parent.get(current)
    return current


def font_dependent(finding: Dict[str, Any]) -> bool:
    """Can a font measurement move this box? The ONE place that decides.

    EITHER signal is enough to disqualify a box, because each is a lower bound
    on font-sensitivity and each misses what the other catches:

      * the heuristic misses the LINE BOX — no text node anywhere near
        `backgrounds body > div:nth-of-type(4)`, and the strut still moves it;
      * the probe misses any metric the run did not perturb. Perturbing advance
        and ascent/descent leaves `line-height: normal` alone, because it
        resolves to a fixed multiple of font-size rather than to measured
        metrics — so a text-bearing `h2` can sit perfectly still through both
        probes and still be text-driven.

    Measured on 2026-08-21 against the 26-case corpus, 1993 roots, two probes
    (advance, and ascent/descent):

        heuristic alone                     12   (9 of them provably wrong)
        two probes alone                   322   (text-bearing h2s called clean)
        either disqualifies (this rule)      4

    Taking the measurement as authoritative and letting it OVERRIDE the
    heuristic is what produced the 322, and it was the first thing tried. It
    reads as the more rigorous choice and it is the less conservative one:
    "no probe moved it" is not "no font can move it". Unknown is not green.
    """
    if bool(finding.get("text_reachable", False)):
        return True
    return bool(finding.get("font_sensitive") or False)


def font_inexplicable(
    finding: Dict[str, Any],
    factor: float = FONT_ENVELOPE_FACTOR,
    tolerance: float = GEOMETRY_TOLERANCE_PX,
) -> bool:
    """Is this root's residual too LARGE for any font metric to account for?

    A different question from `font_dependent`, and it exists because that one
    stopped aiming anything. On 2026-08-26 the strict board published **zero**
    readable roots out of 1993 while `new_tab body > div.footer:nth-of-type(3)`
    was 1280px wide against Chrome's 137.59 and moved **0.000px** under every
    font probe. No advance width produces a 9x width. The strict board is right
    that a font *can* reach that box — it holds text — and useless for deciding
    what to work on.

    So measure the font's reach instead of its presence. `font_envelope_px` is
    the furthest this axis moved across the probe set; a residual more than
    `factor` times that cannot be the font alone, and the leftover is a defect
    of at least `residual - envelope`.

    Stated limits, because this is evidence and not proof:
      * SOUND ONLY IF THE PROBES BRACKET THE REAL GAP. The envelope measures
        what a perturbation of the size actually run does. Perturb a metric by
        less than this seat differs from Chrome and the envelope understates;
        `factor` is what absorbs that, and the probe magnitudes belong in the
        digest next to the number.
      * It is a LOWER bound on real defects. A box carrying a font error AND a
        layout defect of similar size is excluded, correctly by this rule and
        wrongly by any reading of it as "these are all the defects".
      * It does NOT say the font is irrelevant to the box, only that the font
        cannot account for the residual. Both flags are published per finding
        so the two readings stay separable.
      * An axis any probe could not compare has NO envelope and is never
        published here. Unknown is not readable — the same rule as everywhere
        else on this board.
      * The floor is Gate A's tolerance, imported. A residual inside the
        tolerance is not a failure at all, and inventing a second threshold
        here would be a second number that must agree with the pinned one.
    """
    if not finding.get("root"):
        return False
    envelope = finding.get("font_envelope_px")
    if envelope is None:
        return False
    return abs(float(finding["residual"])) > max(tolerance, factor * float(envelope))


def attribute_case(
    case_id: str,
    chrome_doc: Dict[str, Any],
    rustkit_doc: Dict[str, Any],
    tolerance: float = GEOMETRY_TOLERANCE_PX,
    probe_docs: Optional[List[Dict[str, Any]]] = None,
    envelope_factor: float = FONT_ENVELOPE_FACTOR,
) -> Dict[str, Any]:
    """Split one case's failing axes into root/carried and reachable/independent."""
    root_node = rustkit_doc.get("root", rustkit_doc)
    boxes, parent, text_reachable = annotate(root_node)

    # When a probe capture is present the sensitivity is MEASURED and replaces
    # the heuristic outright. Both are kept on every finding so a board can be
    # re-read later without re-running, and so the gap between them stays
    # visible — it was 9 of 12 the night the probe was built.
    font_sensitive: Optional[Dict[Tuple[str, str], bool]] = None
    # The envelope is the widest movement across the probe set, and it is only
    # KNOWN where EVERY probe could compare the axis. A probe that could not is
    # missing evidence, not evidence of stillness, so one silent probe leaves
    # the axis unpublished rather than admitted on the strength of the others.
    font_envelope: Optional[Dict[Tuple[str, str], float]] = None
    if probe_docs:
        font_sensitive = {}
        font_envelope = {}
        seen: Dict[Tuple[str, str], int] = {}
        for probe in probe_docs:
            for key, distance in measure_font_movement(
                root_node, probe.get("root", probe)
            ).items():
                font_sensitive[key] = font_sensitive.get(key, False) or distance != 0.0
                font_envelope[key] = max(font_envelope.get(key, 0.0), distance)
                seen[key] = seen.get(key, 0) + 1
        font_envelope = {
            key: distance
            for key, distance in font_envelope.items()
            if seen.get(key, 0) == len(probe_docs)
        }

    chrome: Dict[str, Dict[str, float]] = {}
    for element in chrome_doc.get("elements", []):
        rect = element.get("rect") or {}
        if element.get("selector") and all(axis in rect for axis in AXES):
            chrome[element["selector"]] = rect

    rustkit: Dict[str, Dict[str, float]] = {}
    for selector, box in boxes.items():
        rect = border_box(box)
        if rect is not None and all(axis in rect for axis in AXES):
            rustkit[selector] = rect

    findings: List[Dict[str, Any]] = []
    for selector in sorted(set(chrome) & set(rustkit)):
        anchor = nearest_comparable(selector, parent, chrome, rustkit)
        for axis in AXES:
            delta = float(rustkit[selector][axis]) - float(chrome[selector][axis])
            if abs(delta) <= tolerance:
                continue
            inherited = (
                float(rustkit[anchor][axis]) - float(chrome[anchor][axis])
                if anchor is not None
                else 0.0
            )
            residual = delta - inherited
            heuristic = bool(text_reachable.get(selector, False))
            # A selector the probe never saw counts as sensitive, not readable.
            measured = (
                bool(font_sensitive.get((selector, axis), True))
                if font_sensitive is not None
                else None
            )
            finding = {
                "case_id": case_id,
                "selector": selector,
                "anchor": anchor,
                "axis": axis,
                "expected": float(chrome[selector][axis]),
                "actual": float(rustkit[selector][axis]),
                "delta": delta,
                "residual": residual,
                "root": abs(residual) > tolerance,
                "text_reachable": heuristic,
                "font_sensitive": measured,
                "font_basis": "measured" if measured is not None else "heuristic",
                "font_envelope_px": (
                    font_envelope.get((selector, axis))
                    if font_envelope is not None
                    else None
                ),
            }
            finding["font_inexplicable"] = font_inexplicable(
                finding, envelope_factor, tolerance
            )
            findings.append(finding)

    roots = [f for f in findings if f["root"]]
    return {
        "case_id": case_id,
        "measured": True,
        "compared": len(set(chrome) & set(rustkit)),
        "failing_axes": len(findings),
        "roots": len(roots),
        "carried": len(findings) - len(roots),
        "font_independent_roots": sum(1 for f in roots if not font_dependent(f)),
        "font_inexplicable_roots": sum(1 for f in roots if f["font_inexplicable"]),
        "font_basis": "measured" if font_sensitive is not None else "heuristic",
        "findings": findings,
    }


def unmeasured_case(case_id: str, reason: str) -> Dict[str, Any]:
    """A case with no capture is UNMEASURED, never "nothing to attribute"."""
    return {
        "case_id": case_id,
        "measured": False,
        "reason": reason,
        "compared": 0,
        "failing_axes": 0,
        "roots": 0,
        "carried": 0,
        "font_independent_roots": 0,
        "font_inexplicable_roots": 0,
        "findings": [],
    }


def complete_probe_set(
    found: List[Dict[str, Any]], expected: int
) -> List[Dict[str, Any]]:
    """ALL of the probe roots resolved for this case, or none of them.

    A case that resolved only some would be scored against a weaker union than
    its neighbours, and the board's headline would mix two strengths of
    evidence under one number. Falling back to the heuristic for that case is
    the honest outcome, and `font_basis` then says so.
    """
    if expected and len(found) != expected:
        return []
    return found


def board_font_basis(measured_cases: List[Dict[str, Any]]) -> str:
    """"measured" only where EVERY measured case had its full probe set.

    One case falling back to the heuristic makes the board's headline a mix of
    two strengths of evidence, and a mix must not be published as measured. An
    empty board is "heuristic": it measured nothing, so it certainly did not
    measure this.
    """
    if not measured_cases:
        return "heuristic"
    if all(case.get("font_basis") == "measured" for case in measured_cases):
        return "measured"
    return "heuristic"


def run_attribution(
    layout_root: Path,
    case_ids: Optional[List[str]] = None,
    include_non_gating: bool = False,
    tolerance: float = GEOMETRY_TOLERANCE_PX,
    font_probe_roots: Optional[List[Path]] = None,
    envelope_factor: float = FONT_ENVELOPE_FACTOR,
) -> Dict[str, Any]:
    registry = load_case_registry()
    cases: List[Dict[str, Any]] = []

    for case_id, case in sorted(registry.items()):
        if case_ids is not None and case_id not in case_ids:
            continue
        if case["scope"] in NON_GATING_SCOPES and not include_non_gating:
            continue

        chrome_doc = load_json(chrome_rects_path(case_id, case["scope"]))
        if chrome_doc is None:
            cases.append(unmeasured_case(case_id, "no_chrome_baseline"))
            continue
        layout_path, refusal = find_layout_json(
            layout_root, case_id, f"{case['width']}x{case['height']}"
        )
        if layout_path is None:
            cases.append(unmeasured_case(case_id, refusal or "no_rustkit_capture"))
            continue
        rustkit_doc = load_json(layout_path)
        if rustkit_doc is None:
            cases.append(unmeasured_case(case_id, "unreadable_rustkit_capture"))
            continue

        # The probe is discovered with Gate A's own capture-layout rules and at
        # the case's REGISTRY viewport, so a probe captured off-viewport cannot
        # be silently joined against an on-viewport base. A case with no probe
        # falls back to the heuristic for that case alone, and says so.
        probe_docs: List[Dict[str, Any]] = []
        for probe_root in font_probe_roots or []:
            probe_path, _ = find_layout_json(
                probe_root, case_id, f"{case['width']}x{case['height']}"
            )
            probe_doc = load_json(probe_path) if probe_path is not None else None
            if probe_doc is not None:
                probe_docs.append(probe_doc)

        probe_docs = complete_probe_set(probe_docs, len(font_probe_roots or []))

        cases.append(
            attribute_case(
                case_id,
                chrome_doc,
                rustkit_doc,
                tolerance,
                probe_docs,
                envelope_factor,
            )
        )

    measured = [c for c in cases if c["measured"]]
    return {
        "board": "geometry-attribution",
        "gating": False,
        "tolerance_px": tolerance,
        "layout_root": str(layout_root),
        "font_probe_roots": [str(p) for p in (font_probe_roots or [])],
        "font_envelope_factor": envelope_factor,
        "cases": cases,
        "summary": {
            "total_cases": len(cases),
            "measured": len(measured),
            "unmeasured": len(cases) - len(measured),
            "failing_axes": sum(c["failing_axes"] for c in cases),
            "roots": sum(c["roots"] for c in cases),
            "carried": sum(c["carried"] for c in cases),
            "font_independent_roots": sum(c["font_independent_roots"] for c in cases),
            "font_inexplicable_roots": sum(
                c.get("font_inexplicable_roots", 0) for c in cases
            ),
            "font_basis": board_font_basis(measured),
        },
    }


def board_ran(report: Dict[str, Any]) -> bool:
    """Did this board measure anything?

    The ONLY thing that makes this file exit non-zero. Its numbers never do —
    "non-gating" has to mean the numbers cannot fail a PR, not "always exits 0",
    or a board that stopped being produced would look exactly like a clean one.
    """
    return report["summary"]["measured"] > 0


def print_report(report: Dict[str, Any], verbose: bool = False) -> None:
    summary = report["summary"]
    print("Geometry attribution — NON-GATING. These numbers cannot fail a PR.")
    print(f"  tolerance:  {report['tolerance_px']}px per box, per axis (Gate A's)")
    print(f"  layouts:    {report['layout_root']}")
    print(
        f"  cases:      {summary['measured']}/{summary['total_cases']} measured"
        f"  ({summary['unmeasured']} unmeasured)"
    )
    print(
        f"  failing:    {summary['failing_axes']} axes ="
        f" {summary['roots']} root + {summary['carried']} carried"
    )
    if summary.get("font_basis") == "measured":
        print(
            f"  of the roots, {summary['font_independent_roots']} are font-independent"
            f" — MEASURED against {len(report.get('font_probe_roots') or [])} font probe(s)"
        )
    else:
        print(
            f"  of the roots, {summary['font_independent_roots']} are font-independent"
            " — HEURISTIC, and it OVER-REPORTS: measured 2026-08-21, 9 of the 12"
        )
        print(
            "    roots it called readable moved when the font metrics moved."
            " Pass --font-probe-root to measure instead of guessing."
        )
    if summary.get("font_basis") == "measured":
        print(
            f"  of the roots, {summary.get('font_inexplicable_roots', 0)} have a residual"
            f" more than {report.get('font_envelope_factor', FONT_ENVELOPE_FACTOR):g}x"
            " the measured font envelope —"
        )
        print(
            "    a font cannot account for those, whether or not it can reach them."
        )
    print()
    print(
        f"  {'case':24s} {'fail':>6s} {'root':>6s} {'carried':>8s} {'font-free':>10s}"
        f" {'font-inexpl':>12s}"
    )
    for case in sorted(
        report["cases"], key=lambda c: (-c["font_independent_roots"], c["case_id"])
    ):
        if not case["measured"]:
            print(f"  UNMEASURED {case['case_id']}: {case['reason']}")
            continue
        print(
            f"  {case['case_id']:24s} {case['failing_axes']:6d} {case['roots']:6d}"
            f" {case['carried']:8d} {case['font_independent_roots']:10d}"
            f" {case.get('font_inexplicable_roots', 0):12d}"
        )

    print()
    print("  font-independent roots, worst first — the work a text-less seat can aim at:")
    aimable = [
        f
        for case in report["cases"]
        for f in case["findings"]
        if f["root"] and not font_dependent(f)
    ]
    aimable.sort(key=lambda f: -abs(f["residual"]))
    shown = aimable if verbose else aimable[:20]
    for f in shown:
        print(
            f"    {f['case_id']} · {f['selector']} · {f['axis']} ·"
            f" {f['expected']:.4f} · {f['actual']:.4f} · {f['residual']:+.4f}"
        )
    if not aimable:
        print("    (none — every root on this board is downstream of a text measurement)")
    hidden = len(aimable) - len(shown)
    if hidden > 0:
        print(f"    … {hidden} more (use --verbose)")

    if report["summary"].get("font_basis") != "measured":
        return
    print()
    print(
        "  font-INEXPLICABLE roots, worst first — residual, then the envelope it"
        " has to beat:"
    )
    oversized = [
        f
        for case in report["cases"]
        for f in case["findings"]
        if f.get("font_inexplicable")
    ]
    oversized.sort(key=lambda f: -(abs(f["residual"]) - float(f["font_envelope_px"])))
    shown = oversized if verbose else oversized[:20]
    for f in shown:
        print(
            f"    {f['case_id']} · {f['selector']} · {f['axis']} ·"
            f" {f['expected']:.4f} · {f['actual']:.4f} · {f['residual']:+.4f}"
            f" · envelope {f['font_envelope_px']:.4f}px"
        )
    if not oversized:
        print("    (none — every root is inside the font envelope this probe set measured)")
    hidden = len(oversized) - len(shown)
    if hidden > 0:
        print(f"    … {hidden} more (use --verbose)")


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Non-gating attribution of Gate A's geometry failures"
    )
    parser.add_argument(
        "--layout-root",
        type=Path,
        required=True,
        help="Directory holding RustKit layout.json captures",
    )
    parser.add_argument(
        "--case", action="append", dest="cases", help="Limit to a case id (repeatable)"
    )
    parser.add_argument(
        "--include-non-gating",
        action="store_true",
        help="Also attribute the holdout scope",
    )
    parser.add_argument("--tolerance", type=float, default=GEOMETRY_TOLERANCE_PX)
    parser.add_argument(
        "--font-probe-root",
        type=Path,
        action="append",
        dest="font_probe_roots",
        help=(
            "A second capture of the same corpus taken with PERTURBED font "
            "metrics. Font-sensitivity is then measured (a box that moves "
            "between the two captures is font-dependent) instead of guessed."
        ),
    )
    parser.add_argument(
        "--font-envelope-factor",
        type=float,
        default=FONT_ENVELOPE_FACTOR,
        help=(
            "How many times the MEASURED font envelope a root's residual must "
            "exceed before the board reports that a font cannot account for it. "
            "Needs --font-probe-root; the value used is written into the report."
        ),
    )
    parser.add_argument("--json", type=Path, help="Write the full report here")
    parser.add_argument("--verbose", action="store_true")
    args = parser.parse_args()

    report = run_attribution(
        args.layout_root,
        case_ids=args.cases,
        include_non_gating=args.include_non_gating,
        tolerance=args.tolerance,
        font_probe_roots=args.font_probe_roots,
        envelope_factor=args.font_envelope_factor,
    )
    print_report(report, verbose=args.verbose)

    if args.json:
        args.json.parent.mkdir(parents=True, exist_ok=True)
        with open(args.json, "w", encoding="utf-8") as handle:
            json.dump(report, handle, indent=2)
        print(f"\nReport written to {args.json}")

    if not board_ran(report):
        print("\nAttribution: DID NOT RUN — it measured nothing. This is not a pass.")
        return 1
    print("\nAttribution: published (non-gating — it cannot fail this PR)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
