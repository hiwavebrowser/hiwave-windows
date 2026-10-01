"""The attribution board must not flatter the work it aims.

Run: python3 scripts/tests/test_geometry_attribution.py

This board exists to say WHICH failing box is failing at its own edge, and it
has two ways to lie that Gate A does not:

  * calling a box a ROOT when its parent already moved it that far — which
    aims a night at a defect that is three ancestors up
  * calling a box FONT-INDEPENDENT when a text measurement can still reach it —
    which aims a night at a defect a text-less seat cannot score at all

The second one is not hypothetical: the first version of this classifier looked
only INSIDE each box, called `rounded-corners`' inline-block staircase
font-independent, and put 170 boxes on the board where 13 belong. The rest were
collapsed source newlines between the divs.

And, as everywhere in this campaign, a board that measured nothing is not a
clean board.
"""
import json
import os
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))
from geometry_attribution import (  # noqa: E402
    annotate,
    attribute_case,
    board_ran,
    run_attribution,
)
import geometry_attribution  # noqa: E402
import layout_oracle_gate  # noqa: E402


def rect(x=0.0, y=0.0, width=10.0, height=10.0):
    return {"x": x, "y": y, "width": width, "height": height}


def chrome_doc(*pairs):
    return {"elements": [{"selector": s, "rect": r} for s, r in pairs]}


def box(selector, r, children=(), **extra):
    node = {"selector": selector, "border_box": dict(r), "children": list(children)}
    node.update(extra)
    return node


def text(value=" "):
    return {"type": "text", "text": value, "children": []}


# ---------------------------------------------------------------------------
# root vs carried
# ---------------------------------------------------------------------------


def test_a_child_moving_exactly_with_its_parent_is_carried_not_a_root():
    """flex-positioning `#check1`: 8px out, and laid out perfectly in its row."""
    chrome = chrome_doc(
        ("div.row", rect(y=197.0)),
        ("#check1", rect(y=207.0)),
    )
    rustkit = box("div.row", rect(y=189.0), [box("#check1", rect(y=199.0))])

    result = attribute_case("c", chrome, rustkit)
    by_selector = {f["selector"]: f for f in result["findings"]}
    assert by_selector["div.row"]["root"] is True, "the row itself IS a root"
    assert by_selector["#check1"]["root"] is False, "the checkbox is carried"
    assert abs(by_selector["#check1"]["residual"]) < 1e-6
    assert result["roots"] == 1 and result["carried"] == 1


def test_a_child_whose_parent_is_exact_is_a_root():
    chrome = chrome_doc(("div.row", rect(y=100.0)), ("span.a", rect(y=110.0)))
    rustkit = box("div.row", rect(y=100.0), [box("span.a", rect(y=118.0))])

    result = attribute_case("c", chrome, rustkit)
    finding = next(f for f in result["findings"] if f["selector"] == "span.a")
    assert finding["root"] is True
    assert abs(finding["residual"] - 8.0) < 1e-6


def test_each_axis_is_attributed_on_its_own():
    """A box can be carried on one axis and a root on another.

    Collapsing to one verdict per BOX would hide exactly the case the grind
    needs: a correctly-positioned box that is the wrong size, under a parent
    that is itself displaced.
    """
    chrome = chrome_doc(
        ("div.row", rect(x=0.0, y=100.0, width=200.0, height=50.0)),
        ("span.a", rect(x=10.0, y=110.0, width=100.0, height=20.0)),
    )
    rustkit = box(
        "div.row",
        rect(x=0.0, y=90.0, width=200.0, height=50.0),
        [box("span.a", rect(x=10.0, y=100.0, width=140.0, height=20.0))],
    )

    result = attribute_case("c", chrome, rustkit)
    by_axis = {f["axis"]: f for f in result["findings"] if f["selector"] == "span.a"}
    assert by_axis["y"]["root"] is False, "y moved exactly with the parent"
    assert by_axis["width"]["root"] is True, "width is 40px wrong at its own edge"


def test_a_box_with_no_comparable_ancestor_owns_its_whole_delta():
    """`html > body` has nothing above it, so nothing can be subtracted."""
    chrome = chrome_doc(("html > body", rect(height=1342.0)))
    rustkit = box("html > body", rect(height=1279.0))

    result = attribute_case("c", chrome, rustkit)
    finding = next(f for f in result["findings"] if f["axis"] == "height")
    assert finding["anchor"] is None
    assert finding["root"] is True
    assert abs(finding["residual"] - -63.0) < 1e-6


def test_the_anchor_skips_an_ancestor_chrome_never_captured():
    """Chrome's capture drops zero-size elements and a skip list of tags.

    An ancestor present on only one side cannot supply a delta, so the search
    must continue upward. Stopping there would treat every descendant of a
    wrapper Chrome omitted as its own root.
    """
    chrome = chrome_doc(("div.outer", rect(y=100.0)), ("span.a", rect(y=100.0)))
    rustkit = box(
        "div.outer",
        rect(y=110.0),
        [box("div.invisible", rect(y=110.0), [box("span.a", rect(y=110.0))])],
    )

    result = attribute_case("c", chrome, rustkit)
    finding = next(f for f in result["findings"] if f["selector"] == "span.a")
    assert finding["anchor"] == "div.outer"
    assert finding["root"] is False


def test_the_tolerance_is_gate_as_not_a_second_number():
    """Two tolerances that must agree, written down twice, will disagree.

    Asserted on the BEHAVIOUR, not on the name: the first version of this test
    checked that the module-level constant followed Gate A's and stayed green
    when the default argument was hardcoded to 0.5 — a guard satisfied by the
    import line while the code that uses it had its own number. Moving Gate A's
    constant must move what this board calls a failure.
    """
    import importlib

    original = layout_oracle_gate.GEOMETRY_TOLERANCE_PX
    chrome = chrome_doc(("div.a", rect(y=100.0)))
    rustkit = box("div.a", rect(y=110.0))
    try:
        layout_oracle_gate.GEOMETRY_TOLERANCE_PX = 50.0
        importlib.reload(geometry_attribution)
        widened = geometry_attribution.attribute_case("c", chrome, rustkit)
        assert widened["failing_axes"] == 0, (
            "a 10px delta is inside a 50px tolerance — the board must take "
            "Gate A's constant, not carry its own"
        )
    finally:
        layout_oracle_gate.GEOMETRY_TOLERANCE_PX = original
        importlib.reload(geometry_attribution)

    restored = geometry_attribution.attribute_case("c", chrome, rustkit)
    assert restored["failing_axes"] == 1
    assert geometry_attribution.GEOMETRY_TOLERANCE_PX == original


# ---------------------------------------------------------------------------
# text-reachable vs font-independent
# ---------------------------------------------------------------------------


def test_a_whitespace_only_sibling_taints_the_box_next_to_it():
    """THE measurement that changed this board from 170 boxes to 13.

    `rounded-corners` lays inline-block divs with nothing but source newlines
    between them. Each collapsed space is a glyph with an advance, and a seat
    with no font backend measures it at 8.0 where Chrome measures 4.1875. The
    boxes are empty; their positions are not font-independent.
    """
    chrome = chrome_doc(("div.a", rect(x=10.0)), ("div.b", rect(x=110.0)))
    rustkit = box(
        "root",
        rect(),
        [box("div.a", rect(x=10.0)), text("\n      "), box("div.b", rect(x=130.0))],
    )

    _, _, reachable = annotate(rustkit)
    assert reachable["div.b"] is True, "a collapsed space moves the box after it"
    assert reachable["div.a"] is True, "line breaking moves the box before it too"

    result = attribute_case("c", chrome, rustkit)
    assert result["roots"] == 1
    assert result["font_independent_roots"] == 0, (
        "an inline-block staircase is not work a text-less seat can aim at"
    )


def test_text_in_the_subtree_taints_the_ancestor():
    chrome = chrome_doc(("div.card", rect(height=100.0)))
    rustkit = box("div.card", rect(height=120.0), [box("p", rect(), [text("hello")])])

    result = attribute_case("c", chrome, rustkit)
    assert result["roots"] == 1
    assert result["font_independent_roots"] == 0


def test_a_control_sized_from_a_label_is_text_reachable_and_a_bare_checkbox_is_not():
    """A `<button>`'s label is not a child text node — it rides in control_type.

    Reading only the tree would call every button font-independent, and button
    width is measured text. A checkbox has no such string, which is why
    `#check1` is one of the few boxes this seat can honestly score.
    """
    rustkit = box(
        "form",
        rect(),
        [
            box("button.save", rect(), control_type='Button { label: "Save" }'),
            box("#check1", rect(), control_type="Checkbox { checked: false }"),
        ],
    )

    _, _, reachable = annotate(rustkit)
    assert reachable["button.save"] is True
    assert reachable["#check1"] is False


def test_font_independent_roots_are_a_subset_of_roots():
    """A carried box is never counted as aimable work, however text-free."""
    chrome = chrome_doc(("div.row", rect(y=100.0)), ("div.inner", rect(y=100.0)))
    rustkit = box("div.row", rect(y=140.0), [box("div.inner", rect(y=140.0))])

    result = attribute_case("c", chrome, rustkit)
    assert result["roots"] == 1
    assert result["font_independent_roots"] == 1, "the row is the root, and it is text-free"
    assert result["carried"] == 1


# ---------------------------------------------------------------------------
# non-gating, and the blank-row rule
# ---------------------------------------------------------------------------


def test_a_board_that_measured_nothing_is_not_a_clean_board():
    with tempfile.TemporaryDirectory() as empty:
        report = run_attribution(Path(empty))
    assert report["summary"]["measured"] == 0
    assert not board_ran(report), "no captures is DID NOT RUN, not zero defects"
    assert all(not c["measured"] for c in report["cases"])
    assert all(c["roots"] == 0 for c in report["cases"])


def test_catastrophic_numbers_still_publish():
    """Non-gating means the NUMBERS cannot fail a PR — not that it always
    exits 0. A board that stopped being produced would otherwise look exactly
    like a board with nothing to report."""
    chrome = chrome_doc(("html > body", rect(height=1000.0)))
    rustkit = box("html > body", rect(height=1.0))
    result = attribute_case("c", chrome, rustkit)
    report = {
        "cases": [result],
        "summary": {
            "measured": 1,
            "total_cases": 1,
            "unmeasured": 0,
            "failing_axes": result["failing_axes"],
            "roots": result["roots"],
            "carried": result["carried"],
            "font_independent_roots": result["font_independent_roots"],
        },
    }
    assert result["roots"] >= 1
    assert board_ran(report) is True


def test_the_holdout_scope_is_excluded_by_default():
    with tempfile.TemporaryDirectory() as empty:
        gating = run_attribution(Path(empty))
        with_holdout = run_attribution(Path(empty), include_non_gating=True)
    assert gating["summary"]["total_cases"] == 26
    assert with_holdout["summary"]["total_cases"] == 32


def test_an_unmeasured_case_reports_a_reason_rather_than_a_zero():
    with tempfile.TemporaryDirectory() as empty:
        report = run_attribution(Path(empty), case_ids=["bg-pure"])
    case = report["cases"][0]
    assert case["measured"] is False
    assert case["reason"], "a case that could not be scored must say why"


# ---------------------------------------------------------------------------
# measured font-sensitivity (--font-probe-root)
#
# The heuristic above is a guess about which boxes a font can move, and on
# 2026-08-21 it was measured wrong on 9 of the 12 roots it published as
# readable. The mechanism it cannot see is the line box: an element with
# inline-level children carries the font-derived strut in its height, and
# `layout.json` exports no `display` for the classifier to notice that.
# These guards hold the measured path and — more importantly — hold the rule
# that the measurement DISQUALIFIES and never rehabilitates.
# ---------------------------------------------------------------------------


def test_a_box_the_probe_moves_is_font_dependent_even_with_no_text_anywhere():
    """The defect that forced this path: `backgrounds body > div:nth-of-type(4)`
    has one inline-block child, no text node in its subtree or among its
    siblings, and its height still moves when the font metrics move."""
    chrome = chrome_doc(("div.row", rect(height=126.0)))
    rustkit = box("div.row", rect(height=127.12))
    probe = box("div.row", rect(height=127.92))

    heuristic = attribute_case("c", chrome, rustkit)
    assert heuristic["font_independent_roots"] == 1, "the heuristic sees no text"

    measured = attribute_case("c", chrome, rustkit, probe_docs=[probe])
    assert measured["roots"] == 1
    assert measured["font_independent_roots"] == 0, (
        "the probe moved it, so it is not work a text-less seat can aim at"
    )
    assert measured["font_basis"] == "measured"


def test_a_box_no_probe_moves_stays_font_independent():
    """`images-intrinsic` test1's image: 102 against 100, a border the natural
    size never gained, and it does not move under any font perturbation."""
    chrome = chrome_doc(("img.test-img", rect(width=102.0)))
    rustkit = box("img.test-img", rect(width=100.0))
    probe = box("img.test-img", rect(width=100.0))

    result = attribute_case("c", chrome, rustkit, probe_docs=[probe])
    assert result["font_independent_roots"] == 1
    finding = result["findings"][0]
    assert finding["font_sensitive"] is False
    assert finding["font_basis"] == "measured"


def test_the_probe_cannot_rehabilitate_a_text_reachable_box():
    """EITHER signal disqualifies. Letting the measurement override the
    heuristic took the board from 12 roots to 322 — `line-height: normal`
    resolves to a fixed multiple of font-size, so a text-bearing box can sit
    perfectly still through every metrics probe and still be text-driven."""
    chrome = chrome_doc(("h2.title", rect(y=100.0)))
    rustkit = box("h2.title", rect(y=140.0), [text("Heading")])
    probe = box("h2.title", rect(y=140.0), [text("Heading")])

    result = attribute_case("c", chrome, rustkit, probe_docs=[probe])
    finding = result["findings"][0]
    assert finding["text_reachable"] is True
    assert finding["font_sensitive"] is False, "no probe moved it"
    assert result["font_independent_roots"] == 0, (
        "a text-bearing box is never rehabilitated by a probe that missed it"
    )


def test_sensitivity_is_per_axis_not_per_box():
    """`images-intrinsic` test11: its `y` moves with the font because
    everything above it is text, while its `height` — 160 against Chrome's 90,
    an unapplied aspect-ratio — does not. Scoring the box as a whole hides a
    readable height behind an unreadable y."""
    chrome = chrome_doc(("img.test-img", rect(y=1971.0, height=90.0)))
    rustkit = box("img.test-img", rect(y=2004.2, height=160.0))
    probe = box("img.test-img", rect(y=2010.0, height=160.0))

    result = attribute_case("c", chrome, rustkit, probe_docs=[probe])
    by_axis = {f["axis"]: f for f in result["findings"]}
    assert by_axis["y"]["font_sensitive"] is True
    assert by_axis["height"]["font_sensitive"] is False
    assert result["font_independent_roots"] == 1, "the height survives, the y does not"


def test_probes_are_unioned_not_replaced():
    """One perturbation is a weak lower bound. The descent probe alone left 322
    roots on the board; the advance probe catches a different set, and a box is
    sensitive if ANY probe moves it."""
    chrome = chrome_doc(("div.a", rect(y=100.0)))
    rustkit = box("div.a", rect(y=140.0))
    still = box("div.a", rect(y=140.0))
    moves = box("div.a", rect(y=143.0))

    assert attribute_case("c", chrome, rustkit, probe_docs=[still])[
        "font_independent_roots"
    ] == 1
    assert attribute_case("c", chrome, rustkit, probe_docs=[still, moves])[
        "font_independent_roots"
    ] == 0, "the second probe moved it; the union decides"
    assert attribute_case("c", chrome, rustkit, probe_docs=[moves, still])[
        "font_independent_roots"
    ] == 0, "probe order cannot change the verdict"


def test_a_box_the_probe_never_saw_is_sensitive_not_readable():
    """Unknown is not green — the rule the receipt and Gate B already use."""
    chrome = chrome_doc(("div.a", rect(y=100.0)), ("div.b", rect(y=200.0)))
    rustkit = box("div.a", rect(y=140.0), [box("div.b", rect(y=260.0))])
    truncated = box("div.a", rect(y=140.0))  # div.b's path is absent

    result = attribute_case("c", chrome, rustkit, probe_docs=[truncated])
    by_sel = {f["selector"]: f for f in result["findings"]}
    assert by_sel["div.b"]["font_sensitive"] is True, (
        "a box the probe could not join is withheld, never admitted"
    )


def test_the_basis_is_reported_and_a_missing_probe_says_heuristic():
    """A board read later must not be able to mistake a guess for a measurement."""
    chrome = chrome_doc(("div.a", rect(y=100.0)))
    rustkit = box("div.a", rect(y=140.0))

    guessed = attribute_case("c", chrome, rustkit)
    assert guessed["font_basis"] == "heuristic"
    assert guessed["findings"][0]["font_sensitive"] is None
    assert guessed["findings"][0]["font_basis"] == "heuristic"


def test_two_boxes_under_one_selector_taint_it_if_either_moves():
    """`annotate` keys boxes by selector and keeps the FIRST. If a later box
    under the same selector moves, the selector moves — dropping the
    OR-accumulation would let the first box's stillness speak for both."""
    chrome = chrome_doc(("div.dup", rect(y=100.0)))
    rustkit = box(
        "div.dup", rect(y=140.0), [box("div.dup", rect(y=140.0))]
    )
    # The box that MOVES is walked FIRST and the still one second, so a plain
    # last-write-wins assignment would end on False. Ordering it the other way
    # round lets the bug pass, which is how this guard first stayed green.
    probe = box("div.dup", rect(y=147.0), [box("div.dup", rect(y=140.0))])

    result = attribute_case("c", chrome, rustkit, probe_docs=[probe])
    assert result["findings"][0]["font_sensitive"] is True, (
        "one box under this selector moved, so the selector is font-sensitive"
    )


def test_a_case_missing_one_of_its_probes_falls_back_rather_than_half_measuring():
    """All the probe roots or none. Half a union is weaker evidence wearing the
    same label as a full one."""
    both = [{"root": "p1"}, {"root": "p2"}]
    assert geometry_attribution.complete_probe_set(both, 2) == both
    assert geometry_attribution.complete_probe_set(both[:1], 2) == [], (
        "one probe of two is not the union the other cases were scored against"
    )
    assert geometry_attribution.complete_probe_set([], 0) == []


def test_a_mixed_board_is_not_reported_as_measured():
    """One case falling back to the heuristic makes the headline a mix of two
    strengths of evidence, and a mix must not be published as measured."""
    measured = {"case_id": "a", "measured": True, "font_basis": "measured"}
    guessed = {"case_id": "b", "measured": True, "font_basis": "heuristic"}
    assert geometry_attribution.board_font_basis([measured, measured]) == "measured"
    assert geometry_attribution.board_font_basis([measured, guessed]) == "heuristic"
    assert geometry_attribution.board_font_basis([]) == "heuristic"


# ---------------------------------------------------------------------------
# the magnitude column (font-INEXPLICABLE)
#
# The strict column asks whether a font CAN reach a box and by 2026-08-26 it
# disqualified all 1993 roots. This one asks whether a font could produce an
# error that BIG, and it must not become a second way to flatter the work: the
# lies available to it are admitting a box no probe could compare, taking a
# narrower envelope than the probes measured, and letting a carried box in.
# ---------------------------------------------------------------------------


def test_a_residual_far_larger_than_the_font_envelope_is_published():
    """`new_tab body > div.footer:nth-of-type(3)`: 1280px against Chrome's
    137.59, and 0.000px of movement under every probe. It holds text, so the
    strict column is right to withhold it; no advance width makes a box 9x too
    wide, so a board that publishes nothing here is not aiming anything."""
    chrome = chrome_doc(("div.footer", rect(width=137.59)))
    rustkit = box("div.footer", rect(width=1280.0), [text("Privacy")])
    probe = box("div.footer", rect(width=1280.0), [text("Privacy")])

    result = attribute_case("c", chrome, rustkit, probe_docs=[probe])
    finding = next(f for f in result["findings"] if f["axis"] == "width")
    assert finding["text_reachable"] is True, "the strict column still withholds it"
    assert finding["font_envelope_px"] == 0.0
    assert finding["font_inexplicable"] is True
    assert result["font_inexplicable_roots"] == 1
    assert result["font_independent_roots"] == 0, "the two columns are independent"


def test_a_residual_inside_the_envelope_is_not_published():
    """The whole claim is a comparison of magnitudes. A box 8px out that a
    probe already moves 4px has no demonstrated non-font component."""
    chrome = chrome_doc(("h2.title", rect(y=100.0)))
    rustkit = box("h2.title", rect(y=108.0), [text("Heading")])
    probe = box("h2.title", rect(y=112.0), [text("Heading")])

    result = attribute_case("c", chrome, rustkit, probe_docs=[probe])
    finding = next(f for f in result["findings"] if f["axis"] == "y")
    assert finding["root"] is True and finding["font_envelope_px"] == 4.0
    assert finding["font_inexplicable"] is False
    assert result["font_inexplicable_roots"] == 0


def test_an_axis_one_probe_could_not_compare_has_no_envelope():
    """Unknown is not readable, the same rule the strict column and Gate B use.

    A probe that dropped the box measures nothing about it; taking the max over
    the probes that DID see it would publish a box on partial evidence, and the
    partial evidence is the flattering half."""
    chrome = chrome_doc(("div.a", rect(width=100.0)))
    rustkit = box("div.a", rect(width=900.0), [text("x")])
    saw_it = box("div.a", rect(width=900.0), [text("x")])
    missed_it = {"selector": "root", "children": []}

    result = attribute_case("c", chrome, rustkit, probe_docs=[saw_it, missed_it])
    finding = next(f for f in result["findings"] if f["axis"] == "width")
    assert finding["font_envelope_px"] is None
    assert finding["font_inexplicable"] is False, (
        "800px out and still withheld — one blind probe is missing evidence"
    )


def test_the_envelope_is_the_widest_probe_not_the_first():
    """Two probes, and the second moves the box ten times further. Taking the
    first (or the smallest) would understate what a font can do to this axis
    and publish a box the wider probe already explains."""
    chrome = chrome_doc(("div.a", rect(y=100.0)))
    rustkit = box("div.a", rect(y=180.0), [text("x")])
    small = box("div.a", rect(y=181.0), [text("x")])
    wide = box("div.a", rect(y=270.0), [text("x")])

    result = attribute_case("c", chrome, rustkit, probe_docs=[small, wide])
    finding = next(f for f in result["findings"] if f["axis"] == "y")
    assert finding["font_envelope_px"] == 90.0, "the widest probe sets the envelope"
    assert finding["font_inexplicable"] is False
    only_small = attribute_case("c", chrome, rustkit, probe_docs=[small])
    assert only_small["findings"][0]["font_envelope_px"] == 1.0
    assert only_small["findings"][0]["font_inexplicable"] is True


def test_the_envelope_is_per_axis_not_per_box():
    """`images-intrinsic` test11 is the shape: a box whose `y` rides on a page
    of text while its `height` is an unapplied aspect-ratio that no font can
    touch. One envelope per box would hide the readable axis behind the other."""
    chrome = chrome_doc(("img.test", rect(y=100.0, height=90.0)))
    rustkit = box("img.test", rect(y=2000.0, height=160.0), [text("x")])
    probe = box("img.test", rect(y=2400.0, height=160.0), [text("x")])

    result = attribute_case("c", chrome, rustkit, probe_docs=[probe])
    by_axis = {f["axis"]: f for f in result["findings"]}
    assert by_axis["y"]["font_envelope_px"] == 400.0
    assert by_axis["y"]["font_inexplicable"] is False
    assert by_axis["height"]["font_envelope_px"] == 0.0
    assert by_axis["height"]["font_inexplicable"] is True


def test_a_carried_box_is_never_font_inexplicable():
    """The column aims work at a box's own edge. A box moving exactly with its
    parent has no own-edge error to explain, however large its delta."""
    chrome = chrome_doc(("div.row", rect(y=100.0)), ("span.a", rect(y=110.0)))
    rustkit = box(
        "div.row", rect(y=1100.0), [box("span.a", rect(y=1110.0), [text("x")])]
    )
    probe = box(
        "div.row", rect(y=1100.0), [box("span.a", rect(y=1110.0), [text("x")])]
    )

    result = attribute_case("c", chrome, rustkit, probe_docs=[probe])
    child = next(
        f for f in result["findings"] if f["selector"] == "span.a" and f["axis"] == "y"
    )
    assert child["root"] is False and abs(child["residual"]) < 1e-6
    assert child["font_inexplicable"] is False
    assert result["font_inexplicable_roots"] == 1, "only the row itself"

    # Inside `attribute_case` the tolerance floor already holds this, because a
    # carried box is BY DEFINITION inside the tolerance. The `root` gate is
    # load-bearing on exactly one path — a direct call at a tolerance smaller
    # than the one the finding's `root` flag was computed at — so that is what
    # this asserts. A sweep found the integration assertion above could not
    # reach the gate at all.
    carried_at_a_coarser_tolerance = {
        "root": False,
        "residual": 4.0,
        "font_envelope_px": 0.0,
    }
    assert geometry_attribution.font_inexplicable(
        carried_at_a_coarser_tolerance, tolerance=0.5
    ) is False, "the finding's own root verdict wins over a re-supplied tolerance"


def test_the_floor_is_gate_as_tolerance_not_a_second_number():
    """With a zero envelope the factor multiplies to nothing, so something has
    to stop a 0.4px wobble being published as a defect. That something is the
    pinned tolerance, imported — a second threshold invented here is a second
    number that must agree with the first and eventually will not."""
    tol = layout_oracle_gate.GEOMETRY_TOLERANCE_PX
    still = box("div.a", rect(y=100.0 + tol * 1.5), [text("x")])
    chrome = chrome_doc(("div.a", rect(y=100.0)))
    result = attribute_case(
        "c", chrome, box("div.a", rect(y=100.0 + tol * 1.5), [text("x")]),
        probe_docs=[still],
    )
    finding = next(f for f in result["findings"] if f["axis"] == "y")
    assert finding["root"] is True and finding["font_envelope_px"] == 0.0
    assert finding["font_inexplicable"] is True

    inside = geometry_attribution.font_inexplicable(
        {"root": True, "residual": tol * 0.9, "font_envelope_px": 0.0}
    )
    assert inside is False, "a residual inside the tolerance is not a failure at all"


def test_two_boxes_under_one_selector_take_the_widest_envelope():
    """`annotate` keeps the first box for a selector; the movement map keys on
    the selector, so a second box under the same key must widen the envelope
    rather than be dropped. The strict column has the same rule with OR."""
    chrome = chrome_doc(("div.dup", rect(y=100.0)))
    rustkit = box(
        "root-wrapper",
        rect(),
        [box("div.dup", rect(y=200.0), [text("x")]), box("div.dup", rect(y=300.0))],
    )
    probe = box(
        "root-wrapper",
        rect(),
        [box("div.dup", rect(y=200.0), [text("x")]), box("div.dup", rect(y=390.0))],
    )

    result = attribute_case("c", chrome, rustkit, probe_docs=[probe])
    finding = next(f for f in result["findings"] if f["axis"] == "y")
    assert finding["font_envelope_px"] == 90.0
    assert finding["font_inexplicable"] is False


def test_the_factor_reaches_the_finding():
    """A factor nobody can change is a constant with a flag in front of it."""
    chrome = chrome_doc(("div.a", rect(y=100.0)))
    rustkit = box("div.a", rect(y=140.0), [text("x")])
    probe = box("div.a", rect(y=145.0), [text("x")])

    lenient = attribute_case("c", chrome, rustkit, probe_docs=[probe], envelope_factor=2.0)
    strict = attribute_case("c", chrome, rustkit, probe_docs=[probe], envelope_factor=50.0)
    assert lenient["findings"][0]["font_inexplicable"] is True
    assert strict["findings"][0]["font_inexplicable"] is False


def test_a_board_with_no_probe_publishes_no_magnitude_claim():
    """The claim is measured or it is not made. Without a probe there is no
    envelope, and an absent envelope must not read as zero."""
    chrome = chrome_doc(("div.a", rect(width=10.0)))
    rustkit = box("div.a", rect(width=900.0), [text("x")])

    result = attribute_case("c", chrome, rustkit)
    finding = next(f for f in result["findings"] if f["axis"] == "width")
    assert finding["font_envelope_px"] is None
    assert finding["font_inexplicable"] is False
    assert result["font_inexplicable_roots"] == 0


def test_sensitivity_is_still_any_movement_at_all_after_the_refactor():
    """The strict column is now derived from the magnitude. It must keep its own
    rule: ANY movement disqualifies, however small — a 0.0001px shift is still
    a font reaching the box."""
    base = box("div.a", rect(y=100.0))
    probe = box("div.a", rect(y=100.0001))
    moved = geometry_attribution.measure_font_sensitivity(base, probe)
    assert moved[("div.a", "y")] is True
    assert moved[("div.a", "x")] is False
    distance = geometry_attribution.measure_font_movement(base, probe)
    assert distance[("div.a", "x")] == 0.0
    assert 0.0 < distance[("div.a", "y")] < 0.001


def test_the_report_is_json_serialisable():
    """It is published as an artifact; a report that cannot be written is not
    a report."""
    chrome = chrome_doc(("div.a", rect(y=10.0)))
    rustkit = box("div.a", rect(y=30.0))
    json.dumps(attribute_case("c", chrome, rustkit))


if __name__ == "__main__":
    for name, fn in sorted(globals().items()):
        if name.startswith("test_") and callable(fn):
            fn()
            print(f"ok  {name}")
    print("PASS: the attribution board does not flatter the work it aims")
