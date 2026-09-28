"""The declaration census must never report a clean board it did not measure.

Run: python3 scripts/tests/test_declaration_census.py

The census exists because a DROPPED declaration is invisible to every other
instrument: no parse error, no phantom box, no join failure — `calc()` cost one
187px box on `chrome_rustkit` and forty nights of boards never named it. That
makes the census's own failure mode the dangerous one. Every way it could stop
reading the engine, stop reading the corpus, or quietly widen what counts as
handled is a way it prints "no dropped declarations" over a tree it never
looked at.

The tests are organised around those ways:

  * the match arms stop being extractable (refactor, brace walk, indentation)
  * value keywords inside an arm body get counted as properties
  * the corpus stops being read, or is read but yields nothing
  * a property authored only inside a CSS comment is counted
  * an inline style="…" attribute is missed
  * the ledger stops being enforced, or a closed gap fails the PR that closed it

Plus the real trees: the committed ledger must hold on this working tree AND
on origin/master's engine, because a ratchet that only holds where it was
seeded is not a ratchet.
"""
import json
import os
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))
from declaration_census import (  # noqa: E402
    CensusRefusal,
    MIN_PLAUSIBLE_ARMS,
    census_passes,
    custom_property_pass_exists,
    declarations_in_html,
    extract_fn_body,
    handled_properties,
    load_ledger,
    load_registry,
    run_census,
)

REPO_ROOT = Path(__file__).resolve().parent.parent.parent
ENGINE_SOURCE = REPO_ROOT / "crates" / "rustkit-engine" / "src" / "lib.rs"

FAILURES = []


def check(name, condition, detail=""):
    if condition:
        print(f"  PASS {name}")
    else:
        print(f"  FAIL {name} {detail}")
        FAILURES.append(name)


def expect_refusal(name, fn, because=None):
    """A refusal, AND the reason it gives.

    The message is not decoration. Two refusals can be indistinguishable by
    outcome and still mean opposite things to whoever reads the red CI: "no
    capture was read" and "captures were read and held no declaration" both
    stop the run, and only the text says which. A guard that asserts only
    `it refused` leaves the line that tells them apart free to be deleted —
    measured, as the M7 survivor of this file's first sweep.
    """
    try:
        fn()
    except CensusRefusal as refusal:
        if because is not None and because not in str(refusal):
            check(name, False, f"refused with {refusal!r}, wanted {because!r}")
            return
        print(f"  PASS {name} ({refusal})")
        return
    check(name, False, "expected a CensusRefusal, got a report")


# ---------------------------------------------------------------------------
# A synthetic engine, so the arm extractor can be tested without the real file
# ---------------------------------------------------------------------------

SPINE = ["display", "width", "height", "position", "margin-top", "padding-top", "color"]


def FILLER(i):
    """A plausible property name with no digits in it — CSS has none."""
    return "filler-" + chr(ord("a") + i // 26) + chr(ord("a") + i % 26)


def fake_engine(props=None, *, custom_pass=True, indent=12):
    """A source string shaped like apply_style_property.

    `props` defaults to the spine plus enough filler to clear
    MIN_PLAUSIBLE_ARMS, so a test that removes one arm is removing it from an
    otherwise believable extraction.
    """
    names = list(props if props is not None else SPINE)
    if props is None:
        names += [FILLER(i) for i in range(MIN_PLAUSIBLE_ARMS + 5 - len(names))]
    pad = " " * indent
    arms = "\n".join(
        f'{pad}"{name}" => {{\n{pad}    match value {{\n'
        f'{pad}        "flex" => style.x = 1,\n'
        f'{pad}        "none" => style.x = 0,\n'
        f"{pad}        _ => {{}}\n{pad}    }}\n{pad}}}"
        for name in names
    )
    var_pass = ""
    if custom_pass:
        var_pass = (
            "fn resolve_vars(&self, decl: &Declaration) {\n"
            '    if decl.property.starts_with("--") { return; }\n'
            '    let _ = "var(";\n'
            "}\n"
        )
    return (
        var_pass
        + "    fn apply_style_property(&self, style: &mut ComputedStyle, "
        "property: &str, value: &str) {\n"
        "        match property {\n"
        f"{arms}\n"
        "            _ => {}\n"
        "        }\n"
        "    }\n"
    )


def case(case_id, html, scope="micro"):
    return (case_id, {"html": f"/dev/null/{case_id}", "scope": scope, "_html": html})


def reader(case_dict):
    return case_dict["_html"]


def census(engine, cases, ledger=None):
    return run_census(engine, cases, ledger or {}, read_case=reader)


# ---------------------------------------------------------------------------
# 1. The engine side
# ---------------------------------------------------------------------------


def test_arm_extraction():
    props = handled_properties(fake_engine())
    check("extracts the arms it was given", {"display", "height"} <= props)
    check(
        "value keywords inside an arm body are not properties",
        "flex" not in props and "none" not in props,
        sorted(p for p in props if p in ("flex", "none")),
    )


def test_missing_arm_is_reported_as_a_gap():
    """The mutation this whole file exists for: an arm disappears."""
    without_height = [p for p in SPINE if p != "height"] + [
        FILLER(i) for i in range(MIN_PLAUSIBLE_ARMS + 5)
    ]
    cases = [case("c", "<style>div { height: 4px }</style>")]
    # `height` is spine, so its removal is refused outright rather than
    # reported — the refusal IS the report for a property this central.
    expect_refusal("a missing spine arm refuses the whole run", lambda: census(
        fake_engine(without_height), cases
    ))

    without_filler = [p for p in SPINE] + [
        FILLER(i) for i in range(MIN_PLAUSIBLE_ARMS + 5) if i != 3
    ]
    cases = [case("c", "<style>div { %s: 4px }</style>" % FILLER(3))]
    report = census(fake_engine(without_filler), cases)
    check(
        "a non-spine arm that disappears is reported as a gap",
        report["unledgered_gaps"] == [FILLER(3)],
        report["unledgered_gaps"],
    )
    check("and the census fails on it", not census_passes(report))


def test_unreadable_engine_refuses():
    expect_refusal(
        "a source with no apply_style_property refuses",
        lambda: census("fn something_else() {}", [case("c", "<style>a{color:red}</style>")]),
    )
    expect_refusal(
        "a source with too few arms refuses",
        lambda: census(
            fake_engine(SPINE), [case("c", "<style>a{color:red}</style>")]
        ),
    )
    expect_refusal(
        "an unclosed function refuses rather than reading half a match",
        lambda: extract_fn_body("fn apply_style_property(x) { match y {", "fn apply_style_property"),
    )


def test_custom_property_pass_is_checked_not_assumed():
    check(
        "the real engine has a var()/custom-property pass",
        custom_property_pass_exists(ENGINE_SOURCE.read_text(encoding="utf-8")),
    )
    expect_refusal(
        "no var() pass means the `--x` exclusion is not safe, so refuse",
        lambda: census(
            fake_engine(custom_pass=False), [case("c", "<style>a{--x:1;color:red}</style>")]
        ),
    )
    report = census(fake_engine(), [case("c", "<style>a{--accent:#fff;color:red}</style>")])
    check(
        "custom properties are never reported as dropped declarations",
        "--accent" not in report["unledgered_gaps"],
    )


# ---------------------------------------------------------------------------
# 2. The corpus side
# ---------------------------------------------------------------------------


def test_declaration_parsing():
    html = """
      <style>
        /* a whole rule, commented out: .b { column-count: 2 } */
        .a { color: red; /* backdrop-filter: blur(2px); */ height: 4px }
      </style>
      <div style="float: left; width: 3px"></div>
    """
    names = declarations_in_html(html)
    check("reads style blocks", "color" in names and "height" in names)
    check("reads inline style attributes", "float" in names and "width" in names)
    # Both comment shapes, because they fail differently: a commented-out rule
    # still offers RULE_BODY_RE a `{ … }` to find, while a commented-out
    # declaration sits inside a body that is otherwise live. The first sweep's
    # fixture had only a comment OUTSIDE any rule, which no parse would have
    # counted, so it asserted nothing.
    check(
        "a declaration commented out inside a live rule is not authored",
        "backdrop-filter" not in names,
        names,
    )
    check(
        "a whole rule commented out is not authored",
        "column-count" not in names,
        names,
    )


def test_empty_runs_refuse():
    expect_refusal(
        "no case read at all refuses, and says THAT",
        lambda: census(fake_engine(), []),
        because="no corpus case was read",
    )
    expect_refusal(
        "cases read but no declaration found refuses, and says THAT instead",
        lambda: census(fake_engine(), [case("c", "<p>no css here</p>")]),
        because="not one declaration found",
    )


def test_holdout_scope_is_opt_in():
    gating = {case_id for case_id, _ in load_registry(False)}
    with_holdout = {case_id for case_id, _ in load_registry(True)}
    check("26 gating cases", len(gating) == 26, len(gating))
    check("holdout is excluded by default", with_holdout > gating)


# ---------------------------------------------------------------------------
# 3. The ledger ratchet
# ---------------------------------------------------------------------------


def test_ledger_ratchet():
    cases = [case("c", "<style>div { widget-gizmo: 4px }</style>")]
    report = census(fake_engine(), cases)
    check("an unledgered gap fails", not census_passes(report))

    report = census(fake_engine(), cases, {"widget-gizmo": "known, and why"})
    check("a ledgered gap passes", census_passes(report))
    check(
        "and keeps its reason in the report",
        report["gaps"][0]["note"] == "known, and why",
    )

    # The PR that closes a gap must not be the PR that goes red, so this is
    # checked on a corpus with no OTHER gap in it — otherwise the assertion
    # passes or fails on the unrelated one.
    handled_only = [case("c", "<style>div { display: block }</style>")]
    report = census(fake_engine(), handled_only, {"display": "stale entry"})
    check(
        "a ledger entry the tree now handles is tighten-eligible, not a failure",
        census_passes(report) and report["tighten_eligible"] == ["display"],
        (census_passes(report), report["tighten_eligible"]),
    )


def test_committed_ledger_holds_on_the_real_trees():
    ledger = load_ledger()
    check("the ledger is committed and non-empty", bool(ledger))

    working = run_census(
        ENGINE_SOURCE.read_text(encoding="utf-8"), load_registry(False), ledger
    )
    check(
        "the committed ledger holds on this working tree",
        census_passes(working),
        working["unledgered_gaps"],
    )
    check(
        "and it measured the whole gating corpus",
        len(working["cases_measured"]) == 26 and not working["cases_unreadable"],
        working["cases_unreadable"],
    )

    master = subprocess.run(
        ["git", "show", "origin/master:crates/rustkit-engine/src/lib.rs"],
        cwd=REPO_ROOT,
        capture_output=True,
        text=True, encoding="utf-8"
    )
    if master.returncode != 0:
        print("  SKIP origin/master not fetched on this seat")
        return
    on_master = run_census(master.stdout, load_registry(False), ledger)
    check(
        "the committed ledger holds on origin/master's engine too",
        census_passes(on_master),
        on_master["unledgered_gaps"],
    )


def test_the_two_layout_gaps_stay_named():
    """column-count and float are layout defects, not tolerated noise.

    They are in the ledger so the ratchet can hold, and the ledger entry is the
    only place their measured cost is written down. An entry that loses its
    reasoning is the beginning of a gap becoming folklore.
    """
    ledger = load_ledger()
    for prop in ("column-count", "float"):
        check(f"{prop} is ledgered", prop in ledger)
        check(
            f"{prop}'s entry says it is a layout defect",
            "LAYOUT DEFECT" in ledger.get(prop, ""),
            ledger.get(prop, "")[:60],
        )


def main():
    print(__doc__.strip().splitlines()[0])
    for fn in (
        test_arm_extraction,
        test_missing_arm_is_reported_as_a_gap,
        test_unreadable_engine_refuses,
        test_custom_property_pass_is_checked_not_assumed,
        test_declaration_parsing,
        test_empty_runs_refuse,
        test_holdout_scope_is_opt_in,
        test_ledger_ratchet,
        test_committed_ledger_holds_on_the_real_trees,
        test_the_two_layout_gaps_stay_named,
    ):
        print(f"\n{fn.__name__}")
        fn()
    print()
    if FAILURES:
        print(f"FAILED: {len(FAILURES)} — " + ", ".join(FAILURES))
        return 1
    print("All declaration-census guards pass.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
