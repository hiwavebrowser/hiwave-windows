"""Gate A (geometry) must fail every box it cannot honestly score.

Run: python3 scripts/tests/test_layout_oracle_gate.py

The gate's whole value is that a geometry delta is never rasterizer noise, so
every way it could quietly NOT compare a box is a way it reports a green that
means nothing. The tests below are organised around those ways:

  * a box Chrome captured and RustKit never emitted   -> missing_box
  * two RustKit boxes claiming one selector           -> ambiguous_selector
  * an anonymous or text box near a real element      -> excluded, not paired
  * a box RustKit sized that Chrome collapsed         -> phantom_box
  * a capture that never ran                          -> FAIL, not "0 cases pass"

Plus the join itself, exercised against all 26 committed Chrome baselines
rather than a hand-written fixture: unit tests were fully green on night 1
while the join silently dropped three real elements, and only the corpus
caught it.
"""
import copy
import json
import os
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))
from layout_oracle_gate import (  # noqa: E402
    GEOMETRY_TOLERANCE_PX,
    baselines_dir,
    chrome_rects_path,
    compare_case,
    gate_passes,
    load_case_registry,
    run_gate,
)

REPO_ROOT = Path(__file__).resolve().parent.parent.parent


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def rect(x, y, w, h):
    return {"x": x, "y": y, "width": w, "height": h}


def chrome_doc(*elements):
    return {"viewport": {"width": 800, "height": 600}, "elements": list(elements)}


def chrome_el(selector, tag, r):
    return {"selector": selector, "tag": tag, "rect": dict(r)}


def rk_box(selector=None, tag=None, r=None, box_type="block", children=None, **extra):
    box = {"type": box_type, "children": children or []}
    if r is not None:
        box["border_box"] = dict(r)
        # A real export also carries content_rect; the gate must ignore it.
        box["content_rect"] = dict(r)
    if selector is not None:
        box["selector"] = selector
        box["tag"] = tag or selector.rsplit(">", 1)[-1].strip().split(".")[0]
        box["element_id"] = 1
    box.update(extra)
    return box


def rk_doc(*children):
    return {"version": 1, "viewport": {"width": 800, "height": 600},
            "root": rk_box(box_type="block", r=rect(0, 0, 800, 600), children=list(children))}


def synthesize_rustkit_from_chrome(chrome):
    """A perfect RustKit capture: every Chrome element, at Chrome's rect.

    This is the identity case. It must score green on every case, which is what
    makes a later 0.6px perturbation attributable to the perturbation and not
    to the gate mis-joining real corpus data.
    """
    children = [
        rk_box(selector=el["selector"], tag=el["tag"], r=el["rect"])
        for el in chrome["elements"]
    ]
    return rk_doc(*children)


def gate_cases():
    return [
        (cid, case)
        for cid, case in sorted(load_case_registry().items())
        if case["scope"] != "holdout"
    ]


# ---------------------------------------------------------------------------
# The join, against the real corpus
# ---------------------------------------------------------------------------


def test_identity_capture_is_green_on_every_gate_case():
    """All 26 cases, every box compared, zero failures."""
    total_compared = 0
    for case_id, case in gate_cases():
        chrome = json.load(open(chrome_rects_path(case_id, case["scope"]), encoding="utf-8"))
        rustkit = synthesize_rustkit_from_chrome(chrome)
        result = compare_case(case_id, chrome, rustkit)
        assert result["green"], (
            f"{case_id}: identity capture scored red: {result['receipts'][:3]}"
        )
        assert result["compared"] == len(chrome["elements"]), (
            f"{case_id}: compared {result['compared']} of {len(chrome['elements'])}"
        )
        total_compared += result["compared"]
    assert total_compared == 1593, (
        f"the gate set is 1593 committed boxes; compared {total_compared}. "
        "A changed count means the corpus or the baselines moved."
    )


def test_one_perturbed_box_produces_exactly_one_receipt():
    """0.6px on one axis of one box, on real data, is one failing line."""
    case_id, case = gate_cases()[0]
    chrome = json.load(open(chrome_rects_path(case_id, case["scope"]), encoding="utf-8"))
    rustkit = synthesize_rustkit_from_chrome(chrome)
    target = rustkit["root"]["children"][1]
    target["border_box"]["x"] += 0.6

    result = compare_case(case_id, chrome, rustkit)
    assert not result["green"]
    assert result["geometry_failures"] == 1, result["receipts"]
    assert result["join_failures"] == 0
    line = result["receipts"][0]
    assert line.count(" · ") == 5, f"receipt is not the fixed 6-column form: {line}"
    assert target["selector"] in line
    assert " · x · " in line
    assert "+0.6" in line


# ---------------------------------------------------------------------------
# The tolerance boundary
# ---------------------------------------------------------------------------


def test_exactly_at_tolerance_passes_and_just_over_fails():
    chrome = chrome_doc(chrome_el("body > p", "p", rect(10, 10, 100, 20)))

    at = rk_doc(rk_box("body > p", "p", rect(10 + GEOMETRY_TOLERANCE_PX, 10, 100, 20)))
    assert compare_case("t", chrome, at)["green"], "0.5px is within the bar"

    over = rk_doc(rk_box("body > p", "p", rect(10 + GEOMETRY_TOLERANCE_PX + 0.001, 10, 100, 20)))
    assert not compare_case("t", chrome, over)["green"]


def test_every_axis_is_compared_independently():
    chrome = chrome_doc(chrome_el("body > p", "p", rect(10, 10, 100, 20)))
    rustkit = rk_doc(rk_box("body > p", "p", rect(11, 12, 103, 24)))
    result = compare_case("t", chrome, rustkit)
    axes = {f["axis"] for f in result["failures"]}
    assert axes == {"x", "y", "width", "height"}, axes


def test_the_border_box_is_what_joins_to_chrome():
    """content_rect is inset by padding; comparing it would fail every padded box.

    Chrome's rect is getBoundingClientRect, i.e. the border box. A gate reading
    content_rect reports a constant bogus delta on real pages, which reads as a
    layout bug and sends the night's dig at the wrong thing.
    """
    chrome = chrome_doc(chrome_el("body > div", "div", rect(0, 0, 100, 100)))
    box = rk_box("body > div", "div", rect(0, 0, 100, 100))
    box["content_rect"] = rect(20, 20, 60, 60)  # padding 20 all round
    assert compare_case("t", chrome, rk_doc(box))["green"]


def test_a_transformed_box_joins_on_its_visual_rect_not_its_layout_rect():
    """getBoundingClientRect is POST-transform; CSS transforms do not lay out.

    Scored against `border_box`, a box the renderer translates reads as a
    layout defect that isn't there — and getting its layout position RIGHT
    makes the reported delta bigger. sticky-scroll's `.overflow-content`
    (`translate(-50%, -50%)`) was 139.53px "out of place" while sitting
    exactly where it belonged.
    """
    chrome = chrome_doc(chrome_el("body > div", "div", rect(837.96875, 1051.25, 300, 300)))
    box = rk_box("body > div", "div", rect(987.96875, 1201.25, 300, 300))
    box["visual_border_box"] = rect(837.96875, 1051.25, 300, 300)
    assert compare_case("t", chrome, rk_doc(box))["green"], (
        "a box whose visual rect matches Chrome is geometry-green"
    )


def test_an_untransformed_box_still_joins_on_its_border_box():
    """The preference must not become a requirement.

    Only transformed boxes carry `visual_border_box`; every other box in the
    corpus would go unscored if its absence stopped the join.
    """
    chrome = chrome_doc(chrome_el("body > div", "div", rect(0, 0, 100, 100)))
    box = rk_box("body > div", "div", rect(0, 0, 100, 100))
    assert "visual_border_box" not in box
    assert compare_case("t", chrome, rk_doc(box))["green"]


def test_a_wrong_visual_rect_is_not_excused_by_a_right_layout_rect():
    """The visual rect REPLACES the layout rect for this join, it does not
    join alongside it. Falling back to `border_box` when the visual rect fails
    would hide every real transform defect — RustKit paints new_tab's `kbd`
    chips 5% large from a `:hover` rule it should not be matching, and that is
    a defect the oracle must be able to see.
    """
    chrome = chrome_doc(chrome_el("body > div", "div", rect(0, 0, 100, 100)))
    box = rk_box("body > div", "div", rect(0, 0, 100, 100))
    box["visual_border_box"] = rect(0, 0, 105, 105)
    result = compare_case("t", chrome, rk_doc(box))
    assert not result["green"]
    axes = {f["axis"] for f in result["failures"]}
    assert axes == {"width", "height"}, axes


def test_a_wrapped_inline_joins_on_its_fragment_union_not_its_one_box():
    """Chrome's rect for a wrapped inline is the union of its line fragments.

    RustKit has no inline fragment model: the inline is ONE box, one line
    tall. `article-typography`'s `pre > code` exported a height of 16.32
    against Chrome's 148.38 while its text was laid out over 152.06 — the
    board's top-ranked defect, on content that is in the right place. The
    engine emits the union alongside; this is the gate preferring it.
    """
    chrome = chrome_doc(chrome_el("pre > code", "code", rect(24, 100, 400, 148.38)))
    box = rk_box("pre > code", "code", rect(24, 100, 400, 16.32))
    box["fragment_union_border_box"] = rect(24, 100, 400, 148.38)
    assert compare_case("t", chrome, rk_doc(box))["green"]


def test_an_inline_with_one_fragment_still_joins_on_its_border_box():
    """The preference must not become a requirement.

    Only a multi-fragment inline carries the union. Every other box in the
    corpus would go unscored if its absence stopped the join — the same
    guard `visual_border_box` carries, restated because a second optional
    rect is a second way to lose the fallback.
    """
    chrome = chrome_doc(chrome_el("body > span", "span", rect(0, 0, 100, 20)))
    box = rk_box("body > span", "span", rect(0, 0, 100, 20))
    assert "fragment_union_border_box" not in box
    assert compare_case("t", chrome, rk_doc(box))["green"]


def test_a_wrong_fragment_union_is_not_excused_by_a_right_border_box():
    """The union REPLACES the layout rect for this join; it does not join
    alongside it. Falling back on failure would make every wrapped inline
    unfailable, which is worse than the 132px phantom it replaces.
    """
    chrome = chrome_doc(chrome_el("pre > code", "code", rect(0, 0, 400, 148)))
    box = rk_box("pre > code", "code", rect(0, 0, 400, 148))
    box["fragment_union_border_box"] = rect(0, 0, 400, 200)
    result = compare_case("t", chrome, rk_doc(box))
    assert not result["green"]
    assert {f["axis"] for f in result["failures"]} == {"height"}


def test_the_visual_rect_outranks_the_fragment_union():
    """Both corrections answer 'which quantity is Chrome's rect', and the
    engine composes them by transforming the union. So where both are
    present the visual rect is the composed answer and the union is an
    intermediate; a gate that preferred the union would undo the transform.
    """
    chrome = chrome_doc(chrome_el("pre > code", "code", rect(50, 50, 400, 148)))
    box = rk_box("pre > code", "code", rect(0, 0, 400, 16))
    box["fragment_union_border_box"] = rect(0, 0, 400, 148)
    box["visual_border_box"] = rect(50, 50, 400, 148)
    assert compare_case("t", chrome, rk_doc(box))["green"]


# ---------------------------------------------------------------------------
# The ways a box could go unscored
# ---------------------------------------------------------------------------


def test_a_box_rustkit_never_emitted_is_a_failure():
    chrome = chrome_doc(
        chrome_el("body > p", "p", rect(0, 0, 10, 10)),
        chrome_el("body > span", "span", rect(0, 0, 10, 10)),
    )
    result = compare_case("t", chrome, rk_doc(rk_box("body > p", "p", rect(0, 0, 10, 10))))
    kinds = [f["kind"] for f in result["failures"]]
    assert kinds == ["missing_box"], kinds
    assert result["compared"] == 1, "the box that WAS present must still be scored"


def test_a_duplicate_selector_is_reported_not_first_matched():
    """Two boxes claiming one selector: scoring either one is a coin flip.

    First-matching would score the correct twin and call the case green while a
    second, wrong box sat unexamined in the tree.
    """
    chrome = chrome_doc(chrome_el("body > p", "p", rect(0, 0, 10, 10)))
    rustkit = rk_doc(
        rk_box("body > p", "p", rect(0, 0, 10, 10)),      # correct
        rk_box("body > p", "p", rect(0, 0, 999, 999)),    # wrong
    )
    result = compare_case("t", chrome, rustkit)
    assert not result["green"]
    assert [f["kind"] for f in result["failures"]] == ["ambiguous_selector"]
    assert result["compared"] == 0, "an ambiguous selector must not be scored at all"


def test_anonymous_and_text_boxes_are_excluded_never_paired():
    """The Option on identity is load-bearing.

    Here an anonymous box and a text box sit at the wrong place in the tree,
    adjacent to the one real element. Pairing either of them positionally would
    manufacture a geometry failure on a case that is actually correct.
    """
    chrome = chrome_doc(chrome_el("body > p", "p", rect(10, 10, 100, 20)))
    rustkit = rk_doc(
        rk_box(box_type="anonymous_block", r=rect(500, 500, 3, 3)),
        {"type": "text", "text": "hello", "rect": rect(600, 600, 7, 7), "children": []},
        rk_box("body > p", "p", rect(10, 10, 100, 20)),
    )
    result = compare_case("t", chrome, rustkit)
    assert result["green"], result["receipts"]
    assert result["compared"] == 1
    assert result["rustkit_identified"] == 1
    assert result["rustkit_boxes"] == 4, "the unidentified boxes are still counted"


def test_a_box_chrome_would_have_captured_but_did_not_is_a_phantom():
    """RustKit gave size to something Chrome collapsed to zero."""
    chrome = chrome_doc(chrome_el("body > p", "p", rect(0, 0, 10, 10)))
    rustkit = rk_doc(
        rk_box("body > p", "p", rect(0, 0, 10, 10)),
        rk_box("body > div.ghost", "div", rect(0, 0, 200, 50)),
    )
    result = compare_case("t", chrome, rustkit)
    assert not result["green"]
    assert [f["kind"] for f in result["failures"]] == ["phantom_box"]


def test_chromes_own_omissions_are_not_phantoms():
    """Mirror capture_baseline.mjs, or the gate invents failures on every page.

    Chrome drops zero-size elements and a fixed tag list before writing
    layout-rects.json. RustKit emits both. Neither is a defect.
    """
    chrome = chrome_doc(chrome_el("body > p", "p", rect(0, 0, 10, 10)))
    rustkit = rk_doc(
        rk_box("body > p", "p", rect(0, 0, 10, 10)),
        rk_box("html", "html", rect(0, 0, 800, 600)),          # skipped tag
        rk_box("body > style", "style", rect(0, 0, 800, 20)),  # skipped tag
        rk_box("body > i.empty", "i", rect(40, 40, 0, 0)),     # zero-size
    )
    result = compare_case("t", chrome, rustkit)
    assert result["green"], result["receipts"]


# ---------------------------------------------------------------------------
# A run that measured nothing
# ---------------------------------------------------------------------------


def test_a_run_with_no_captures_fails_rather_than_reporting_all_green():
    """"PASS: all 0 cases" is how a broken pipeline turns green."""
    with tempfile.TemporaryDirectory() as empty:
        report = run_gate(Path(empty))
        assert report["summary"]["measured"] == 0
        assert report["summary"]["unmeasured"] == len(gate_cases())
        assert not gate_passes(report)
        assert all(not c["green"] for c in report["cases"])


def test_a_single_missing_capture_does_not_pass_by_omission():
    """One case captured perfectly, one never captured, is not a green run."""
    cases = gate_cases()
    present, absent = cases[0][0], cases[1][0]
    with tempfile.TemporaryDirectory() as root:
        chrome = json.load(open(chrome_rects_path(present, cases[0][1]["scope"]), encoding="utf-8"))
        out = Path(root) / present
        out.mkdir(parents=True)
        with open(out / "layout.json", "w", encoding="utf-8") as handle:
            json.dump(synthesize_rustkit_from_chrome(chrome), handle)

        report = run_gate(Path(root), case_ids=[present, absent])
        assert report["summary"]["measured"] == 1
        assert report["summary"]["green"] == 1
        assert not gate_passes(report)
        missing = [c for c in report["cases"] if c["case_id"] == absent][0]
        assert missing["reason"] == "no_rustkit_capture"


def test_only_the_registry_viewport_capture_is_scored():
    """A swarm run writes several viewports per case; the baselines have one.

    Chrome's rects were captured at the registry viewport only. Scoring a
    1920x1080 dump against 800x600 baselines reports a page-wide geometry
    catastrophe that is entirely instrument mismatch — and, worse, one that
    LOOKS like the campaign's own target defect class. A case captured only
    off-viewport is unmeasured (which fails), never measured wrongly.
    """
    case_id, case = gate_cases()[0]
    native = f"{case['width']}x{case['height']}"
    chrome = json.load(open(chrome_rects_path(case_id, case["scope"]), encoding="utf-8"))
    good = synthesize_rustkit_from_chrome(chrome)
    shifted = copy.deepcopy(good)
    for child in shifted["root"]["children"]:
        child["border_box"]["x"] += 400  # what a wrong-viewport dump looks like

    with tempfile.TemporaryDirectory() as root:
        run = Path(root) / "run-1" / case_id
        for viewport, tree in (("3840x2160", shifted), (native, good)):
            out = run / viewport / "iter-1"
            out.mkdir(parents=True)
            with open(out / "layout.json", "w", encoding="utf-8") as handle:
                json.dump(tree, handle)

        report = run_gate(Path(root), case_ids=[case_id])
        assert report["cases"][0]["green"], report["cases"][0]["receipts"][:3]

    # Off-viewport alone is a refusal, not a score.
    with tempfile.TemporaryDirectory() as root:
        out = Path(root) / "run-1" / case_id / "3840x2160" / "iter-1"
        out.mkdir(parents=True)
        with open(out / "layout.json", "w", encoding="utf-8") as handle:
            json.dump(shifted, handle)

        report = run_gate(Path(root), case_ids=[case_id])
        assert report["cases"][0]["reason"] == "no_native_viewport_capture"
        assert not gate_passes(report)


def test_gate_passes_refuses_any_report_that_measured_nothing():
    """The tripwires are tested on the predicate, not only through run_gate.

    Routed through run_gate these two branches are unreachable — an unmeasured
    case already carries green=False, so `red == 0` catches the same runs and
    both tripwires mutate GREEN. That is the definition of decoration, and it
    was true of this file until the mutation sweep said so.

    They are kept because they defend a DIFFERENT failure than the one red
    counts: a future refactor of how case records are built (or a caller
    assembling a summary itself) can produce measured=0 with red=0, and
    "PASS: all 0 cases" is how a broken pipeline turns green. Asserting them
    against the predicate is what makes them load-bearing rather than ornament.
    """
    def report(**summary):
        base = {"total_cases": 26, "measured": 26, "unmeasured": 0, "green": 26,
                "red": 0, "geometry_failures": 0, "join_failures": 0,
                "attributable": True, "text_exposed_failures": 0,
                "text_backends": ["coretext"], "unattributable_cases": 0}
        base.update(summary)
        return {"summary": base}

    assert gate_passes(report()), "a genuinely green run must still pass"
    assert not gate_passes(report(total_cases=0, measured=0, green=0)), \
        "zero cases discovered is a pipeline bug, not a pass"
    assert not gate_passes(report(measured=0, unmeasured=26)), \
        "26 cases none of which were measured is not a pass"


# ---------------------------------------------------------------------------
# Text-metric provenance: a capture whose advances came from no font
# ---------------------------------------------------------------------------


def _rk_doc_with(backend, derived, *children):
    doc = rk_doc(*children)
    if backend is not None:
        doc["text_backend"] = backend
    if derived is not None:
        doc["text_metrics_font_derived"] = derived
    return doc


def test_a_stub_shaper_capture_can_never_be_green_even_with_zero_failures():
    """The property that matters, and the one red does not give.

    On the Linux trench seat the stub shaper's numbers disagree with Chrome, so
    every case is red and it LOOKS as though red is protecting the receipt. It
    is not. Red is a coincidence of this corpus: change the font stack, or
    score a page with no words on it, and a capture measured with a
    0.5em-per-character ruler scores zero failures and the gate prints PASS.
    """
    chrome = chrome_doc(chrome_el("body > div", "div", rect(0, 0, 100, 50)))
    identity = [rk_box(selector="body > div", tag="div", r=rect(0, 0, 100, 50))]

    stub = compare_case("c", chrome, _rk_doc_with("stub-0.5em", False, *identity))
    assert stub["geometry_failures"] == 0, "the fixture is the identity case"
    assert stub["green"], "zero failures is still zero failures"
    assert not stub["attributable"], (
        "a capture whose advances came from no font must not be attributable "
        "however few boxes it failed"
    )

    real = compare_case("c", chrome, _rk_doc_with("coretext", True, *identity))
    assert real["attributable"], "a Core Text capture is attributable"

    # And the predicate, not only the record: this is the half that stops a
    # green stub board being quoted as an N/26.
    def summary_of(records):
        measured = [r for r in records if r["measured"]]
        return {"summary": {
            "total_cases": len(records), "measured": len(measured), "unmeasured": 0,
            "green": sum(1 for r in records if r["green"]), "red": 0,
            "geometry_failures": 0, "join_failures": 0, "text_exposed_failures": 0,
            "text_backends": sorted({r["text_backend"] for r in measured}),
            "unattributable_cases": sum(1 for r in measured if not r["attributable"]),
            "attributable": all(r["attributable"] for r in measured) and bool(measured),
        }}

    assert not gate_passes(summary_of([stub])), \
        "an all-green stub board is not a pass"
    assert gate_passes(summary_of([real])), \
        "an all-green Core Text board is still a pass"


def test_a_capture_that_declares_no_provenance_is_not_trusted():
    """Absent is not True.

    The field was added on 2026-10-01; every capture taken before it is silent
    about its shaper. A gate that reads silence as "a font produced this"
    re-opens the hole for exactly the captures most likely to predate the fix.
    """
    chrome = chrome_doc(chrome_el("body > div", "div", rect(0, 0, 100, 50)))
    identity = [rk_box(selector="body > div", tag="div", r=rect(0, 0, 100, 50))]

    silent = compare_case("c", chrome, _rk_doc_with(None, None, *identity))
    assert silent["green"], "the fixture is the identity case"
    assert not silent["attributable"], "a capture that does not say is not trusted"
    assert silent["text_backend"] == "unknown"
    assert silent["text_metrics_font_derived"] is None, (
        "None and False are different findings: 'did not say' and 'said no'"
    )

    # A non-boolean in the field is also not a yes.
    for bogus in ("true", 1, {}, []):
        doc = rk_doc(*identity)
        doc["text_metrics_font_derived"] = bogus
        assert not compare_case("c", chrome, doc)["attributable"], (
            f"{bogus!r} in the provenance field must not read as True"
        )


def test_text_exposure_claims_downward_and_sideways_but_never_ancestry():
    """The classifier has to exclude the USELESS relation, not a useful one.

    The first version of this guard asserted that a box whose text sits two
    levels below is NOT claimed, and the mutation sweep showed the assertion
    never fired: the fixture had no such box. Widening `own` from "has a text
    child" to "has text anywhere beneath" left every test green, which means
    the narrower rule was a choice no guard held — and the narrower rule was
    also wrong. A box two levels above its text still takes its content size
    from that text.

    What must stay excluded is ANCESTRY: some box ABOVE this one contains
    words. That is true of nearly every box on a page with text on it, so a
    column that claims it reads 100% and informs nobody.
    """
    chrome = chrome_doc(
        chrome_el("body > div.deep", "div", rect(0, 0, 100, 50)),
        chrome_el("body > div.deep > div.mid", "div", rect(0, 0, 100, 50)),
        chrome_el("body > div.after", "div", rect(0, 60, 100, 50)),
        chrome_el("body > div.quiet", "div", rect(0, 120, 100, 50)),
        chrome_el("body > div.quiet > div.inner", "div", rect(0, 120, 50, 50)),
    )
    # `div.quiet` precedes the text that its PARENT (the synthetic root) holds,
    # so neither it nor `div.inner` has a downward or sideways relation while
    # the root above them does. That is the ancestry-only case.
    doc = rk_doc(
        rk_box(selector="body > div.deep", tag="div", r=rect(9, 0, 100, 50), children=[
            rk_box(selector="body > div.deep > div.mid", tag="div", r=rect(9, 0, 100, 50),
                   children=[rk_box(box_type="text", rect=rect(0, 0, 10, 10))]),
        ]),
        rk_box(selector="body > div.after", tag="div", r=rect(9, 60, 100, 50)),
        rk_box(selector="body > div.quiet", tag="div", r=rect(9, 120, 100, 50), children=[
            rk_box(selector="body > div.quiet > div.inner", tag="div", r=rect(9, 120, 50, 50)),
        ]),
    )
    result = compare_case("c", chrome, doc)
    by_sel = {f["selector"]: f["text_exposure"] for f in result["failures"] if f["axis"] == "x"}

    assert by_sel["body > div.deep > div.mid"] == "own", \
        "a box with a text child is sized by a text measurement"
    assert by_sel["body > div.deep"] == "own", (
        "and so is a box whose text sits two levels below — it takes its "
        "content size from that text through the box between them"
    )
    assert by_sel["body > div.after"] == "flow", \
        "a preceding sibling subtree carrying text hands its advance along"
    assert by_sel["body > div.quiet"] == "flow", \
        "the relation is the preceding SUBTREE's text, not a preceding text box"
    assert by_sel["body > div.quiet > div.inner"] is None, (
        "a box whose only text is in an ANCESTOR is not claimed: that relation "
        "is true of almost every box and classifying it would make the column "
        "read 100% and mean nothing"
    )


def test_the_exposure_count_never_silently_corrects_the_failure_count():
    """`geometry_failures` must stay exactly what every prior receipt measured.

    The ratchet carries committed per-case floors and the digest carries fifty
    nights of counts. Netting exposure out of the headline number would make
    all of them incomparable while looking like an improvement.
    """
    chrome = chrome_doc(chrome_el("body > div", "div", rect(0, 0, 100, 50)))
    doc = rk_doc(rk_box(selector="body > div", tag="div", r=rect(9, 0, 100, 50),
                        children=[rk_box(box_type="text", rect=rect(0, 0, 10, 10))]))
    result = compare_case("c", chrome, doc)
    assert result["geometry_failures"] == 1, "one perturbed axis is one failure"
    assert result["text_exposed_failures"] == 1, "and it is text-exposed"
    assert len(result["receipts"]) == 1, "the receipt schema is unchanged"


def test_an_unknown_case_filter_discovers_nothing_and_fails():
    """The reachable route into the zero-cases tripwire."""
    with tempfile.TemporaryDirectory() as empty:
        report = run_gate(Path(empty), case_ids=["no-such-case"])
    assert report["summary"]["total_cases"] == 0
    assert not gate_passes(report)


def test_the_holdout_scope_does_not_gate():
    """Canary-only until the 26 are green (plan §3.6)."""
    with tempfile.TemporaryDirectory() as empty:
        gating = run_gate(Path(empty))
        with_holdout = run_gate(Path(empty), include_non_gating=True)
    assert gating["summary"]["total_cases"] == 26
    assert with_holdout["summary"]["total_cases"] == 32


if __name__ == "__main__":
    assert baselines_dir().exists(), f"no baselines at {baselines_dir()}"
    for name, fn in sorted(globals().items()):
        if name.startswith("test_") and callable(fn):
            fn()
            print(f"ok  {name}")
    print("PASS: Gate A fails every box it cannot honestly score")
