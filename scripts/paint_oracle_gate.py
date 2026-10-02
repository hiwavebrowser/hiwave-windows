#!/usr/bin/env python3
"""paint_oracle_gate.py — Gate B (paint) of the dual-oracle parity gate.

Plan: docs/PARITY_FINISH_LINE_PLAN_2026-08-04.md §2. Baseline and rules:
trench/BASELINE-parity-finish-line.md.

Gate B answers a different question from Gate A. Geometry can be bit-exact, so
Gate A can demand it. Paint cannot: Chrome is not bit-stable against itself on
text antialiasing, gradient dither, or resample kernels, and a gate that
demanded bit-equal pixels would spend the campaign grinding noise. So Gate B
has two halves, and the second is the one that matters:

    percentage half   >= 99% of pixels within the pinned AA tolerance
    discrete half     a paint bug that is STRUCTURAL auto-fails regardless
                      of what the percentage says

The discrete half exists because the percentage half can be bought. §1 of the
plan records master's collapsed shelf scoring 3.71% (pass) while the correct
tree scored 33.87% (fail): a small enough defect always passes a percentage,
and a large enough correct change always looks like a regression. A wrong solid
fill on a button is a real bug at 0.4% of the viewport. It fails here.

THE PINNED CONSTANT
-------------------
`aa_tolerance: 5` lives in docs/VISUAL_DIFF_POLICY.md and is READ FROM THERE,
not copied into this file. Plan §2 is explicit that exactly one number may
exist; the ±3/255 figure from earlier drafts is retired. A second literal in a
gate is how a campaign ends up with two tolerances and an argument about which
one was in force.

WHAT IS COMPARED TO WHAT
------------------------
    Chrome   baselines/chrome-148/<scope>/<case>/baseline.png
    RustKit  <root>/<case>/<viewport>/iter-<n>/capture/frame.ppm

Same discovery discipline as Gate A: only the case's REGISTRY viewport is
scored, because Chrome's frame was captured at that viewport and nowhere else.
A case captured only off-viewport is UNMEASURED, which fails, rather than
measured wrongly.

DISCRETE STRUCTURAL FAILURES IMPLEMENTED HERE
---------------------------------------------
Two of the three kinds named in plan §2 are implemented, and both are tests
performed strictly INSIDE an element's own border box:

  wrong_solid_color  Chrome paints the element's interior as one flat colour
                     and RustKit paints it as a different flat colour. The
                     shipped class this catches is #83 — form controls painting
                     white where `background: transparent` was set.

  missing_clip       An element with a border radius, whose rounded corner
                     notch — the part of the border box the arc cuts away — is
                     painted with the element's own fill in RustKit while
                     Chrome shows what is behind it. This is P1's corner-notch
                     defect.

`paint_outside_box` is specified but NOT implemented, deliberately. See
WHY PAINT_OUTSIDE_BOX IS NOT HERE at the bottom of this docstring.

Both implemented detectors refuse to fire when they cannot attribute. If
RustKit's interior is not a flat colour, `wrong_solid_color` skips rather than
reporting a gradient-vs-solid mismatch as a wrong colour — that difference is
real, and the percentage half already owns it. A discrete failure is an
auto-fail, so a misattributed one costs more than a missed one.

GEOMETRY IS A PRECONDITION OF BOTH DETECTORS
--------------------------------------------
Both detectors read RustKit's pixels at CHROME's rect. That is only a statement
about paint when RustKit put the box where Chrome put it. If the element is
displaced, every pixel read belongs to something else, and whatever the
detector concludes is a fact about the layout delta Gate A is already
reporting — laundered into an auto-fail on the paint gate.

This is not hypothetical. Measured on the 26-case corpus 2026-08-11, **62 of
62** `missing_clip` auto-fails fired on elements Gate A fails, displaced by 8px
to 384px; **zero** fired on a geometrically exact element. `css-selectors`
`div.section:nth-of-type(3)` was reported as an unclipped corner while RustKit
rounds that corner correctly — 21px higher up the page, where the box actually
is. The detector was reading the middle of the white card and calling it a
notch.

So `attributable_selectors` joins RustKit's layout dump and admits an element
to the discrete detectors only when its border box matches Chrome's rect within
Gate A's tolerance, on every axis. The constant is IMPORTED from
`layout_oracle_gate` rather than restated: two tolerances that must agree and
are written down twice will disagree.

The precondition is necessary, not sufficient, and the limit is worth stating:
an exactly-placed element can still have a displaced SIBLING painting into its
corner, which would read as its own missing clip. Closing that needs
overlap analysis this gate does not do. What it does close is the case where
the element under test is itself somewhere else.

An element that is missing from the layout dump, or duplicated in it, is also
excluded — no evidence is not the same as evidence of correctness, and Gate A
already reports both as join failures.

WHY PAINT_OUTSIDE_BOX IS NOT HERE
---------------------------------
The obvious implementation — "differing pixels that fall outside every Chrome
element box" — was measured against the corpus before being written, and it is
decoration: across all 26 gating cases, **0.00%** of the viewport lies outside
the union of Chrome's captured rects, because `body` and its block descendants
tile the page. The detector would never fire on any case and would have shipped
green forever.

The attributable version — the element's own fill appearing in the band just
outside its border box, which is #86's signature — is only sound when the
element's geometry is already known correct. Otherwise a sibling that shifted
into the gap paints the same evidence, and Gate B would auto-fail a case for a
paint bug that is really the layout delta Gate A is already reporting. That
precondition needs the RustKit layout dump joined in, which is the next unit of
work on this gate, not a thing to approximate.

Usage:
    python3 scripts/paint_oracle_gate.py --capture-root parity-results
    python3 scripts/paint_oracle_gate.py --capture-root <dir> --case bg-solid
    python3 scripts/paint_oracle_gate.py --capture-root <dir> --json out.json

Exit codes:
    0 = every discovered case is paint-green
    1 = at least one case fails, or the run measured nothing
"""

import argparse
import json
import math
import os
import re
import sys
from pathlib import Path
from typing import Any, Dict, Iterator, List, Optional, Sequence, Tuple

sys.path.insert(0, str(Path(__file__).resolve().parent))

from parity_image import Image, UnsupportedImage, read_png, read_ppm  # noqa: E402

# Gate A owns the geometry join and its tolerance. Imported, never restated:
# the discrete detectors are only sound where Gate A would call the box exact,
# so the two must use one number and one join.
from layout_oracle_gate import (  # noqa: E402
    AXES,
    GEOMETRY_TOLERANCE_PX,
    border_box,
    find_layout_json,
    index_rustkit,
)

REPO_ROOT = Path(__file__).resolve().parent.parent

POLICY_PATH = REPO_ROOT / "docs" / "VISUAL_DIFF_POLICY.md"

# Plan §2: ">= 99% within tolerance". One constant, one place, same rule as
# Gate A's 0.5px — a per-case paint bar is how "99% everywhere" quietly becomes
# "99% except where it was inconvenient".
PAINT_PASS_FRACTION = 0.99

# An element interior smaller than this after the inset carries too few pixels
# for "is it a flat colour" to mean anything, and a 2x2 patch is uniform by
# accident constantly. Such elements are skipped by the discrete detectors and
# remain covered by the percentage half.
MIN_INTERIOR_PX = 36

# Pixels trimmed from each side of a border box before its interior is read.
# Borders, outlines and edge antialiasing all live in the outermost pixel or
# two; including them would make almost nothing look flat.
INTERIOR_INSET_PX = 2

# A corner notch smaller than this is a rounding artefact, not a testable
# region. radius >= ~4px clears it.
MIN_NOTCH_PX = 6

# Distance beyond the corner arc, in px, before a notch pixel is trusted to be
# fully outside it. The arc's own antialiasing is a blend of both sides and
# must not be read as either.
ARC_AA_MARGIN_PX = 1.0

# Mirrors Gate A. The holdout scope is canary-only until the 26-case gate set
# is green (plan §3.6): discovered and scored, never gating.
NON_GATING_SCOPES = frozenset({"holdout"})


class PolicyError(Exception):
    """The pinned tolerance could not be read unambiguously.

    Raised rather than defaulted. A gate that silently falls back to a
    hardcoded 5 when the policy file moves is a gate that stops citing the
    policy the moment the policy changes.
    """


DEFAULT_POLICY_HEADING = "default policy"

# A section whose heading says this is outside the gate set, so it is allowed
# to state its own tolerance. Today that is "### Live Sites (non-gating)",
# which says 10.
NON_GATING_HEADING_MARKER = "non-gating"


def _policy_sections(text: str) -> Iterator[Tuple[str, str]]:
    """Yield (heading, body) for each markdown section, preamble first."""
    heading = ""
    body: List[str] = []
    for line in text.splitlines():
        if line.startswith("#"):
            yield heading, "\n".join(body)
            heading = line.lstrip("#").strip()
            body = []
        else:
            body.append(line)
    yield heading, "\n".join(body)


def load_aa_tolerance(policy_path: Path = POLICY_PATH) -> int:
    """Read the pinned `aa_tolerance` from docs/VISUAL_DIFF_POLICY.md.

    Plan §2 requires this gate to CITE the constant rather than carry its own
    copy, and says exactly one number may exist. The file is very slightly
    less tidy than that: it states 5 in the default policy and restates 5 in
    each gating suite section, and then states 10 under "Live Sites
    (non-gating)" — a suite that is not in the 26 and never gates.

    So the rule enforced here is the rule that was actually meant: one number
    governs everything that gates. The default block supplies it, every GATING
    section must restate it, and only a section whose heading declares itself
    non-gating may differ. A gating section that drifts to its own tolerance
    raises rather than being averaged, preferred, or silently first-matched.
    """
    if not policy_path.exists():
        raise PolicyError(f"no visual diff policy at {policy_path}")

    text = policy_path.read_text(encoding="utf-8")
    pinned: Optional[int] = None
    disagreements: List[str] = []

    for heading, body in _policy_sections(text):
        values = [int(v) for v in re.findall(r'"aa_tolerance"\s*:\s*(\d+)', body)]
        if not values:
            continue
        if heading.strip().lower() == DEFAULT_POLICY_HEADING:
            if pinned is not None and pinned != values[0]:
                raise PolicyError(
                    f"{policy_path} states the default aa_tolerance twice, differently"
                )
            pinned = values[0]
        if NON_GATING_HEADING_MARKER in heading.lower():
            continue
        for value in values:
            disagreements.append(f"{heading or '<preamble>'}={value}")

    if pinned is None:
        raise PolicyError(
            f"{policy_path} has no '{DEFAULT_POLICY_HEADING}' section declaring aa_tolerance"
        )

    stated = {entry.rsplit("=", 1)[1] for entry in disagreements}
    if stated - {str(pinned)}:
        raise PolicyError(
            f"{policy_path}: gating sections disagree on aa_tolerance "
            f"(pinned {pinned}, found {sorted(disagreements)})"
        )
    return pinned


# ---------------------------------------------------------------------------
# Failure records
# ---------------------------------------------------------------------------


class PaintFailure:
    """One paint failure, in a fixed receipt format.

    Gate A's receipt is `case · box · axis · expected · actual · Δ`. Gate B
    keeps the same six columns so a dual-oracle report reads as one table:
    the third column carries the failure kind, and expected/actual carry
    colours or percentages depending on the kind.
    """

    DISCRETE_KINDS = frozenset(
        {"wrong_solid_color", "missing_clip", "paint_outside_box", "size_mismatch"}
    )

    def __init__(
        self,
        case_id: str,
        selector: Optional[str],
        kind: str,
        expected: str = "—",
        actual: str = "—",
        detail: str = "—",
    ) -> None:
        self.case_id = case_id
        self.selector = selector
        self.kind = kind
        self.expected = expected
        self.actual = actual
        self.detail = detail

    @property
    def discrete(self) -> bool:
        return self.kind in self.DISCRETE_KINDS

    def receipt(self) -> str:
        return " · ".join(
            [
                self.case_id,
                self.selector or "—",
                self.kind,
                self.expected,
                self.actual,
                self.detail,
            ]
        )

    def to_json(self) -> Dict[str, Any]:
        return {
            "case_id": self.case_id,
            "selector": self.selector,
            "kind": self.kind,
            "discrete": self.discrete,
            "expected": self.expected,
            "actual": self.actual,
            "detail": self.detail,
        }


def fmt_rgb(color: Tuple[int, int, int]) -> str:
    return "#%02x%02x%02x" % color


# ---------------------------------------------------------------------------
# Pixel helpers
# ---------------------------------------------------------------------------


def channel_delta(a: Tuple[int, int, int], b: Tuple[int, int, int]) -> int:
    return max(abs(a[0] - b[0]), abs(a[1] - b[1]), abs(a[2] - b[2]))


def count_outside_tolerance(chrome: Image, rustkit: Image, tolerance: int) -> int:
    """Pixels whose worst channel differs by more than the pinned tolerance.

    Per channel, not summed and not averaged: a pure blue-vs-red swap has a
    zero mean delta on the green channel and would survive an averaged test.
    """
    a = chrome.rgb
    b = rustkit.rgb
    bad = 0
    for i in range(0, len(a), 3):
        if (
            abs(a[i] - b[i]) > tolerance
            or abs(a[i + 1] - b[i + 1]) > tolerance
            or abs(a[i + 2] - b[i + 2]) > tolerance
        ):
            bad += 1
    return bad


def px_box(
    rect: Dict[str, Any], width: int, height: int, inset: int = 0
) -> Optional[Tuple[int, int, int, int]]:
    """A CSS rect as a half-open integer pixel box, clipped to the viewport.

    Returns None when nothing survives the inset or the clip.
    """
    x0 = int(math.ceil(float(rect.get("x", 0)))) + inset
    y0 = int(math.ceil(float(rect.get("y", 0)))) + inset
    x1 = int(math.floor(float(rect.get("x", 0)) + float(rect.get("width", 0)))) - inset
    y1 = int(math.floor(float(rect.get("y", 0)) + float(rect.get("height", 0)))) - inset
    x0 = max(0, x0)
    y0 = max(0, y0)
    x1 = min(width, x1)
    y1 = min(height, y1)
    if x1 <= x0 or y1 <= y0:
        return None
    return (x0, y0, x1, y1)


def iter_box(box: Tuple[int, int, int, int]) -> Iterator[Tuple[int, int]]:
    x0, y0, x1, y1 = box
    for y in range(y0, y1):
        for x in range(x0, x1):
            yield x, y


def flat_color(
    image: Image, box: Tuple[int, int, int, int], tolerance: int
) -> Optional[Tuple[int, int, int]]:
    """The image's colour over `box`, or None if the region is not flat.

    "Flat" means every pixel is within the pinned tolerance of the first one —
    the same tolerance the percentage half uses, so a region this calls flat is
    a region the gate would call unchanged.
    """
    x0, y0, x1, y1 = box
    if (x1 - x0) * (y1 - y0) < MIN_INTERIOR_PX:
        return None
    return flat_pixels(image, list(iter_box(box)), tolerance)


def flat_pixels(
    image: Image, pixels: Sequence[Tuple[int, int]], tolerance: int
) -> Optional[Tuple[int, int, int]]:
    """The image's colour over an arbitrary pixel set, or None if not flat."""
    if not pixels:
        return None
    first = image.pixel(*pixels[0])
    for x, y in pixels:
        if channel_delta(first, image.pixel(x, y)) > tolerance:
            return None
    return first


def parse_radius(value: Optional[str]) -> float:
    """The single corner radius from a computed `border-radius`, in px.

    Only the simple uniform `Npx` form is honoured. Elliptical radii, percent
    radii and per-corner shorthands return 0 and the element is skipped: the
    notch geometry below assumes a circular arc, and applying it to an ellipse
    would carve the notch in the wrong place and auto-fail a correct render.

    The exclusion is the `fullmatch` and nothing else. An earlier version also
    rejected values containing a space, a slash or a percent sign before
    matching; the mutation sweep found that check stays green when removed,
    because `([0-9.]+)px` anchored at both ends already rejects every one of
    them. It was deleted rather than kept as reassurance — a guard that cannot
    fail is a guard nobody can trust the next reader to re-derive.
    """
    if not value:
        return 0.0
    text = value.strip()
    match = re.fullmatch(r"([0-9.]+)px", text)
    if not match:
        return 0.0
    try:
        return float(match.group(1))
    except ValueError:
        return 0.0


CORNER_NAMES = ("top-left", "top-right", "bottom-left", "bottom-right")


class Corner:
    """One rounded corner, split into the two regions that decide the verdict.

    `notch` — inside the border box, inside the r x r corner square, and
    OUTSIDE the arc. Chrome cannot paint the element's fill here; the arc cut
    it away. If RustKit paints the fill here, the rounded clip is missing.

    `inside` — the same corner square, INSIDE the arc. This is where the
    element's own fill genuinely is, and it is where the fill is sampled from.

    Sampling the fill at the corner rather than over the whole interior is what
    makes this detector usable at all. Measured against the corpus, requiring a
    flat WHOLE interior left exactly ONE element in all 26 cases with any
    surface to test — cards and buttons have text and children in the middle,
    so their interiors are never flat. Their corners are.

    Both regions are shrunk away from the arc by ARC_AA_MARGIN_PX so the arc's
    own antialiased pixels, which are a blend of both sides, are read as
    neither.
    """

    __slots__ = ("name", "notch", "inside")

    def __init__(self, name: str, notch: List[Tuple[int, int]],
                 inside: List[Tuple[int, int]]):
        self.name = name
        self.notch = notch
        self.inside = inside


def corners(
    rect: Dict[str, Any], radius: float, width: int, height: int
) -> List[Corner]:
    box = px_box(rect, width, height)
    if box is None or radius <= 0:
        return []
    x0, y0, x1, y1 = box
    r = min(radius, (x1 - x0) / 2.0, (y1 - y0) / 2.0)
    if r <= 0:
        return []

    centers = [
        (x0 + r, y0 + r),
        (x1 - r, y0 + r),
        (x0 + r, y1 - r),
        (x1 - r, y1 - r),
    ]
    spans = [
        (x0, y0, int(math.ceil(x0 + r)), int(math.ceil(y0 + r))),
        (int(math.floor(x1 - r)), y0, x1, int(math.ceil(y0 + r))),
        (x0, int(math.floor(y1 - r)), int(math.ceil(x0 + r)), y1),
        (int(math.floor(x1 - r)), int(math.floor(y1 - r)), x1, y1),
    ]

    out = []
    outer = r + ARC_AA_MARGIN_PX
    inner = r - ARC_AA_MARGIN_PX
    for name, (cx, cy), span in zip(CORNER_NAMES, centers, spans):
        sx0, sy0, sx1, sy1 = span
        notch: List[Tuple[int, int]] = []
        inside: List[Tuple[int, int]] = []
        for y in range(max(y0, sy0), min(y1, sy1)):
            for x in range(max(x0, sx0), min(x1, sx1)):
                distance = math.hypot(x + 0.5 - cx, y + 0.5 - cy)
                if distance > outer:
                    notch.append((x, y))
                elif distance < inner:
                    inside.append((x, y))
        out.append(Corner(name, notch, inside))
    return out


def corner_notch_pixels(
    rect: Dict[str, Any], radius: float, width: int, height: int
) -> List[Tuple[int, int]]:
    """Every notch pixel of every corner. Kept for callers that want the total."""
    return [p for corner in corners(rect, radius, width, height) for p in corner.notch]


# ---------------------------------------------------------------------------
# Discrete structural detectors
# ---------------------------------------------------------------------------


def detect_wrong_solid_color(
    case_id: str,
    chrome: Image,
    rustkit: Image,
    elements: Sequence[Dict[str, Any]],
    tolerance: int,
) -> List[PaintFailure]:
    failures = []
    for element in elements:
        box = px_box(element.get("rect") or {}, chrome.width, chrome.height,
                     inset=INTERIOR_INSET_PX)
        if box is None:
            continue
        want = flat_color(chrome, box, tolerance)
        if want is None:
            continue
        got = flat_color(rustkit, box, tolerance)
        if got is None:
            # RustKit did not paint a flat colour here. That is a real
            # difference and the percentage half scores it; calling it a wrong
            # SOLID colour would put a name on it the evidence does not
            # support.
            continue
        if channel_delta(want, got) > tolerance:
            failures.append(
                PaintFailure(
                    case_id,
                    element.get("selector"),
                    "wrong_solid_color",
                    fmt_rgb(want),
                    fmt_rgb(got),
                    f"Δ{channel_delta(want, got)} over {(box[2]-box[0])*(box[3]-box[1])}px",
                )
            )
    return failures


def detect_missing_clip(
    case_id: str,
    chrome: Image,
    rustkit: Image,
    elements: Sequence[Dict[str, Any]],
    styles: Dict[str, Dict[str, str]],
    tolerance: int,
) -> List[PaintFailure]:
    failures = []
    for element in elements:
        selector = element.get("selector")
        style = styles.get(selector or "")
        if not style:
            continue
        radius = parse_radius(style.get("border-radius"))
        if radius <= 0:
            continue
        rect = element.get("rect") or {}
        for corner in corners(rect, radius, chrome.width, chrome.height):
            if len(corner.notch) < MIN_NOTCH_PX or len(corner.inside) < MIN_NOTCH_PX:
                continue

            # The element's own fill, sampled where it genuinely is: inside the
            # arc, at this corner. Not flat there means the corner carries a
            # gradient or content and the notch paint cannot be attributed to a
            # single fill — so the detector declines rather than guesses.
            fill = flat_pixels(rustkit, corner.inside, tolerance)
            if fill is None:
                continue

            if any(
                channel_delta(fill, rustkit.pixel(x, y)) > tolerance
                for x, y in corner.notch
            ):
                continue  # RustKit did clip: the notch is not the fill.
            if any(
                channel_delta(fill, chrome.pixel(x, y)) <= tolerance
                for x, y in corner.notch
            ):
                # Chrome shows the same colour behind the corner, so a square
                # corner is indistinguishable from a round one here. No
                # evidence either way, and a guess would be an auto-fail.
                continue

            failures.append(
                PaintFailure(
                    case_id,
                    selector,
                    "missing_clip",
                    f"radius {radius:g}px {corner.name}",
                    f"fill {fmt_rgb(fill)} across all {len(corner.notch)} notch px",
                    "corner not clipped",
                )
            )
    return failures


# ---------------------------------------------------------------------------
# Attribution: which elements the discrete detectors may speak about
# ---------------------------------------------------------------------------


def attributable_selectors(
    elements: Sequence[Dict[str, Any]],
    rustkit_layout: Dict[str, Any],
) -> set:
    """Selectors whose RustKit border box is where Chrome's rect is.

    Only these may reach a discrete detector — see GEOMETRY IS A PRECONDITION
    OF BOTH DETECTORS in the module docstring. An element that is absent from
    the layout dump, or claimed by more than one box, is excluded: Gate A
    reports both as join failures, and neither is evidence that the paint at
    Chrome's rect belongs to this element.
    """
    root = rustkit_layout.get("root", rustkit_layout)
    index, _identified, _total = index_rustkit(root)

    admitted = set()
    for element in elements:
        selector = element.get("selector")
        if not selector:
            continue
        candidates = index.get(selector, [])
        if len(candidates) != 1:
            continue
        actual = border_box(candidates[0][1])
        expected = element.get("rect") or {}
        if actual is None:
            continue
        if all(
            expected.get(axis) is not None
            and actual.get(axis) is not None
            and abs(float(actual[axis]) - float(expected[axis]))
            <= GEOMETRY_TOLERANCE_PX
            for axis in AXES
        ):
            admitted.add(selector)
    return admitted


# ---------------------------------------------------------------------------
# Case scoring
# ---------------------------------------------------------------------------


def compare_case(
    case_id: str,
    chrome: Image,
    rustkit: Image,
    elements: Sequence[Dict[str, Any]],
    styles: Dict[str, Dict[str, str]],
    tolerance: int,
    attributable: set,
    pass_fraction: float = PAINT_PASS_FRACTION,
) -> Dict[str, Any]:
    if chrome.size != rustkit.size:
        # docs/VISUAL_DIFF_POLICY.md: strict_size / fail_on_size_mismatch.
        # Scaling one side to fit would make every subsequent number a
        # comparison between two images that were never the same picture.
        failure = PaintFailure(
            case_id,
            None,
            "size_mismatch",
            f"{chrome.width}x{chrome.height}",
            f"{rustkit.width}x{rustkit.height}",
            "frames are not the same size",
        )
        return {
            "case_id": case_id,
            "measured": True,
            "green": False,
            "total_px": chrome.width * chrome.height,
            "outside_tolerance_px": None,
            "within_fraction": None,
            "discrete_examined": 0,
            "discrete_unattributable": 0,
            # None, not []: attribution never ran on a size mismatch, so "which
            # elements were withheld" is UNKNOWN. An empty list would assert
            # that none were, and a downstream ratchet would read every future
            # discrete failure here as newly broken.
            "discrete_withheld_selectors": None,
            "discrete_failures": 1,
            "failures": [failure.to_json()],
            "receipts": [failure.receipt()],
        }

    total = chrome.width * chrome.height
    bad = count_outside_tolerance(chrome, rustkit, tolerance)
    within = (total - bad) / total if total else 0.0

    failures: List[PaintFailure] = []
    if within < pass_fraction:
        failures.append(
            PaintFailure(
                case_id,
                None,
                "paint_below_bar",
                f">={pass_fraction * 100:.4g}%",
                f"{within * 100:.4f}%",
                f"{bad}/{total}px outside ±{tolerance}",
            )
        )

    # Only geometrically exact elements reach the discrete detectors. Anything
    # else and the detector is reading pixels that belong to another box.
    scoped = [e for e in elements if e.get("selector") in attributable]
    # The withheld SET, not just its size. A downstream ratchet cannot tell a
    # newly BROKEN element from a newly MEASURABLE one without knowing which
    # elements this run declined to speak about: every geometry fix enlarges
    # this gate's jurisdiction, so a discrete failure can appear on an element
    # that was silently skipped last time and was defective all along.
    withheld_selectors = sorted(
        {
            e["selector"]
            for e in elements
            if e.get("selector") and e["selector"] not in attributable
        }
    )
    withheld = sum(
        1
        for e in elements
        if e.get("selector") and e.get("selector") not in attributable
    )

    failures.extend(
        detect_wrong_solid_color(case_id, chrome, rustkit, scoped, tolerance)
    )
    failures.extend(
        detect_missing_clip(case_id, chrome, rustkit, scoped, styles, tolerance)
    )

    return {
        "case_id": case_id,
        "measured": True,
        "green": not failures,
        "total_px": total,
        "outside_tolerance_px": bad,
        "within_fraction": within,
        "discrete_examined": len(scoped),
        "discrete_unattributable": withheld,
        "discrete_withheld_selectors": withheld_selectors,
        "discrete_failures": sum(1 for f in failures if f.discrete),
        "failures": [f.to_json() for f in failures],
        "receipts": [f.receipt() for f in failures],
    }


def unmeasured_case(case_id: str, reason: str) -> Dict[str, Any]:
    """A case the gate could not score. NOT a pass — same rule as Gate A.

    A frame that never rendered and a frame that is perfect look identical to a
    gate that skips missing files.
    """
    return {
        "case_id": case_id,
        "measured": False,
        "green": False,
        "reason": reason,
        "total_px": 0,
        "outside_tolerance_px": None,
        "within_fraction": None,
        "discrete_examined": 0,
        "discrete_unattributable": 0,
        # UNKNOWN, not empty — this case was never opened. See the note on the
        # size-mismatch return above.
        "discrete_withheld_selectors": None,
        "discrete_failures": 0,
        "failures": [],
        "receipts": [],
    }


# ---------------------------------------------------------------------------
# Discovery
# ---------------------------------------------------------------------------


def baselines_dir() -> Path:
    return REPO_ROOT / "baselines" / os.environ.get("PARITY_BASELINE_SET", "chrome-148")


def load_case_registry() -> Dict[str, Dict[str, Any]]:
    with open(REPO_ROOT / "cases" / "registry.json", encoding="utf-8") as handle:
        return json.load(handle)["cases"]


def load_styles(path: Path) -> Dict[str, Dict[str, str]]:
    """Selector -> computed styles, from the committed Chrome capture."""
    if not path.exists():
        return {}
    with open(path, encoding="utf-8") as handle:
        document = json.load(handle)
    return {
        element["selector"]: element.get("styles", {})
        for element in document.get("elements", [])
        if element.get("selector")
    }


def find_frame(
    capture_root: Path, case_id: str, native_viewport: str
) -> Tuple[Optional[Path], Optional[str]]:
    """Locate a case's RustKit frame. Returns (path, refusal_reason).

    Same two capture layouts Gate A handles, and the same refusal: only the
    registry viewport is accepted, because Chrome's frame exists at that
    viewport and nowhere else. Comparing an 1920x1080 render against an 800x600
    baseline would report a page-wide paint catastrophe that is purely an
    instrument mismatch.
    """
    direct = capture_root / case_id / "frame.ppm"
    if direct.exists():
        return direct, None

    matches = sorted(capture_root.glob(f"**/{case_id}/**/frame.ppm"))
    if not matches:
        return None, "no_rustkit_capture"
    native = [p for p in matches if native_viewport in p.parts]
    if not native:
        return None, "no_native_viewport_capture"
    return min(native, key=lambda p: (len(p.parts), str(p))), None


def run_gate(
    capture_root: Path,
    case_ids: Optional[Sequence[str]] = None,
    include_non_gating: bool = False,
    tolerance: Optional[int] = None,
    pass_fraction: float = PAINT_PASS_FRACTION,
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

    cases = []
    for case_id, case in selected:
        base = baselines_dir() / case["scope"] / case_id
        baseline_png = base / "baseline.png"
        rects_path = base / "layout-rects.json"
        if not baseline_png.exists():
            cases.append(unmeasured_case(case_id, "no_chrome_baseline"))
            continue

        viewport = f"{case['width']}x{case['height']}"
        frame_path, refusal = find_frame(capture_root, case_id, viewport)
        if frame_path is None:
            cases.append(unmeasured_case(case_id, refusal or "no_rustkit_capture"))
            continue

        # The layout dump is not optional. Without it the discrete detectors
        # cannot tell "RustKit failed to clip this corner" from "RustKit put
        # this box 384px away", and every capture that writes a frame writes a
        # layout.json beside it. A case that has one but not the other is a
        # broken capture, and reporting it UNMEASURED is the same refusal Gate
        # A makes — not a pass by omission.
        layout_path, layout_refusal = find_layout_json(capture_root, case_id, viewport)
        if layout_path is None:
            # Gate A's refusal names the frame it was looking for; here the
            # frame was found and the layout beside it was not, so say that.
            if layout_refusal == "no_rustkit_capture":
                layout_refusal = "no_rustkit_layout"
            cases.append(unmeasured_case(case_id, layout_refusal or "no_rustkit_layout"))
            continue
        try:
            with open(layout_path, encoding="utf-8") as handle:
                rustkit_layout = json.load(handle)
        except (OSError, ValueError) as exc:
            cases.append(unmeasured_case(case_id, f"unreadable_rustkit_layout: {exc}"))
            continue

        try:
            chrome = read_png(baseline_png)
            rustkit = read_ppm(frame_path)
        except UnsupportedImage as exc:
            cases.append(unmeasured_case(case_id, f"unreadable_capture: {exc}"))
            continue

        elements: List[Dict[str, Any]] = []
        if rects_path.exists():
            with open(rects_path, encoding="utf-8") as handle:
                elements = json.load(handle).get("elements", [])
        styles = load_styles(base / "computed-styles.json")

        record = compare_case(
            case_id,
            chrome,
            rustkit,
            elements,
            styles,
            tolerance,
            attributable_selectors(elements, rustkit_layout),
            pass_fraction,
        )
        record["scope"] = case["scope"]
        cases.append(record)

    measured = [c for c in cases if c["measured"]]
    green = [c for c in cases if c["green"]]

    return {
        "gate": "B-paint",
        "aa_tolerance": tolerance,
        "pass_fraction": pass_fraction,
        "capture_root": str(capture_root),
        "cases": cases,
        "summary": {
            "total_cases": len(cases),
            "measured": len(measured),
            "unmeasured": len(cases) - len(measured),
            "green": len(green),
            "red": len(cases) - len(green),
            "discrete_failures": sum(c["discrete_failures"] for c in cases),
            "discrete_examined": sum(c["discrete_examined"] for c in cases),
            "discrete_unattributable": sum(
                c["discrete_unattributable"] for c in cases
            ),
        },
    }


def gate_passes(report: Dict[str, Any]) -> bool:
    """A run that measured nothing is a FAIL.

    Identical tripwire to Gate A's, for the identical reason: "PASS: all 0
    cases" is how a broken pipeline reports success.
    """
    if report["summary"]["measured"] == 0:
        return False
    return report["summary"]["red"] == 0


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------


def print_report(report: Dict[str, Any], verbose: bool = False) -> None:
    summary = report["summary"]
    print("Gate B — paint")
    print(
        f"  tolerance:  ±{report['aa_tolerance']}/255 per channel"
        f"  (pinned in {POLICY_PATH.relative_to(REPO_ROOT)})"
    )
    print(f"  bar:        >= {report['pass_fraction'] * 100:.4g}% of pixels within it")
    print(f"  captures:   {report['capture_root']}")
    print(
        f"  cases:      {summary['green']}/{summary['total_cases']} paint-green"
        f"  ({summary['unmeasured']} unmeasured)"
    )
    print(f"  discrete:   {summary['discrete_failures']} structural auto-fails")
    print(
        f"  attributed: {summary['discrete_examined']} elements examined,"
        f" {summary['discrete_unattributable']} withheld"
        f" (geometry not within {GEOMETRY_TOLERANCE_PX}px — Gate A owns those)"
    )
    print()

    for case in report["cases"]:
        if not case["measured"]:
            print(f"  UNMEASURED {case['case_id']}: {case['reason']}")
            continue
        mark = "GREEN" if case["green"] else "RED  "
        within = case["within_fraction"]
        pct = "—" if within is None else f"{within * 100:.4f}%"
        print(
            f"  {mark} {case['case_id']}: {pct} within tolerance,"
            f" {case['discrete_failures']} discrete"
        )
        receipts = case["receipts"] if verbose else case["receipts"][:5]
        for line in receipts:
            print(f"        {line}")
        hidden = len(case["receipts"]) - len(receipts)
        if hidden > 0:
            print(f"        … {hidden} more (use --verbose)")


def main() -> int:
    parser = argparse.ArgumentParser(description="Gate B — paint oracle")
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
        help="Also score the holdout scope (canary-only, plan §3.6)",
    )
    parser.add_argument("--json", type=Path, help="Write the full report here")
    parser.add_argument("--verbose", action="store_true")
    args = parser.parse_args()

    try:
        report = run_gate(
            args.capture_root,
            case_ids=args.cases,
            include_non_gating=args.include_non_gating,
        )
    except PolicyError as exc:
        print(f"Gate B: FAIL — {exc}", file=sys.stderr)
        return 1

    print_report(report, verbose=args.verbose)

    if args.json:
        args.json.parent.mkdir(parents=True, exist_ok=True)
        with open(args.json, "w", encoding="utf-8") as handle:
            json.dump(report, handle, indent=2)
        print(f"\nReport written to {args.json}")

    passed = gate_passes(report)
    print(f"\nGate B: {'PASS' if passed else 'FAIL'}")
    return 0 if passed else 1


if __name__ == "__main__":
    sys.exit(main())
