"""The metric is a CONJUNCTION, and the ways a join quietly stops being one.

Run: python3 scripts/tests/test_finish_line_receipt.py

`N/26 finish-line-green` is the AND of four conditions. Every failure mode this
campaign has hit is a variant of one thing: something that was not measured
being read as something that was fine. The gates each guard that for their own
verdict. The join is a new place for it to happen, and it has four new shapes:

  * a condition whose gate did not run counted as green
  * discrete read as green because Gate B counted zero failures having read
    nothing at all
  * the paint column taken from Gate B's `green`, which already includes
    discrete — so every structural failure also reads as a percentage failure
    and the receipt names the wrong defect
  * the metric quietly becoming `min()` of the per-condition columns instead
    of the per-case AND, which is a strictly larger and always-wrong number

The last one has its own test with worked numbers, because it is the exact
Goodhart substitution in §1 of the plan wearing a conjunction's clothes: four
columns at 25/26 do not make the metric 25/26.
"""
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))

import finish_line_receipt as flr  # noqa: E402
from finish_line_receipt import (  # noqa: E402
    CONDITIONS,
    build_receipt,
    conjoin,
    discrete_verdict,
    format_markdown,
    gating_case_ids,
    paint_verdict,
    receipt_is_publishable,
    stability_verdict,
)

REPO_ROOT = Path(__file__).resolve().parent.parent.parent
SCRIPT = REPO_ROOT / "scripts" / "finish_line_receipt.py"


# ---------------------------------------------------------------------------
# Builders — a green board, then one thing broken at a time.
# ---------------------------------------------------------------------------


def a_case(case_id, green=True, geometry_failures=0, join_failures=0,
           attributable=True, text_backend="coretext"):
    return {
        "case_id": case_id,
        "measured": True,
        "green": green,
        "geometry_failures": geometry_failures,
        "join_failures": join_failures,
        "compared": 40,
        # The default is a macOS capture, because that is the only kind the
        # metric is defined against. A board of unattributable captures is
        # exercised by its own test below.
        "attributable": attributable,
        "text_backend": text_backend,
        "text_metrics_font_derived": attributable,
        "text_exposed_failures": 0,
    }


def b_case(case_id, within=1.0, discrete=0, kind="missing_clip"):
    failures = [{"discrete": True, "kind": kind} for _ in range(discrete)]
    return {
        "case_id": case_id,
        "measured": True,
        # Gate B's own green is the AND of both halves — reproduced faithfully
        # here so the tests exercise the real shape the receipt has to unpick.
        "green": within >= 0.99 and discrete == 0,
        "within_fraction": within,
        "outside_tolerance_px": 0,
        "discrete_failures": discrete,
        "failures": failures,
    }


def unmeasured(case_id, reason="no_rustkit_capture"):
    return {"case_id": case_id, "measured": False, "green": False, "reason": reason}


def agg_row(case_id, viewports, runs=3, stable=True, variance=0.0):
    return {
        "case_id": case_id,
        "viewport": viewports[case_id],
        "measured_runs": runs,
        "stable": stable,
        "diff_pct_variance": variance,
    }


def green_board(ids=None):
    """Inputs under which every one of the 26 is finish-line-green."""
    ids = ids or gating_case_ids()
    vps = flr.registry_viewports()
    gate_a = {"tolerance_px": 0.5, "cases": [a_case(c) for c in ids]}
    gate_b = {
        "aa_tolerance": 5,
        "pass_fraction": 0.99,
        "cases": [b_case(c) for c in ids],
    }
    aggregate = {"results": [agg_row(c, vps) for c in ids]}
    return gate_a, gate_b, aggregate


def by_case(receipt, case_id):
    return next(c for c in receipt["cases"] if c["case_id"] == case_id)


# ---------------------------------------------------------------------------
# The control: a green board is green, and it is all 26.
# ---------------------------------------------------------------------------


def test_a_fully_green_board_scores_every_gating_case():
    receipt = build_receipt(*green_board())
    assert receipt["summary"]["total_cases"] == 26
    assert receipt["summary"]["finish_line_green"] == 26
    assert receipt["summary"]["fully_measured"] == 26


def test_the_holdout_scope_is_not_in_the_metric():
    """Canary-only until the 26 are green (plan §3.6). 26, never 32."""
    assert len(gating_case_ids()) == 26
    assert not any(c.startswith("holdout-") for c in gating_case_ids())


# ---------------------------------------------------------------------------
# Each condition alone must be able to sink a case.
# ---------------------------------------------------------------------------


def test_geometry_alone_sinks_a_case():
    gate_a, gate_b, aggregate = green_board()
    target = gate_a["cases"][0]["case_id"]
    gate_a["cases"][0] = a_case(target, green=False, geometry_failures=3)
    receipt = build_receipt(gate_a, gate_b, aggregate)
    assert receipt["summary"]["finish_line_green"] == 25
    assert by_case(receipt, target)["blockers"] == ["geometry"]


def test_paint_percentage_alone_sinks_a_case():
    gate_a, gate_b, aggregate = green_board()
    target = gate_b["cases"][0]["case_id"]
    gate_b["cases"][0] = b_case(target, within=0.98)
    receipt = build_receipt(gate_a, gate_b, aggregate)
    assert receipt["summary"]["finish_line_green"] == 25
    assert by_case(receipt, target)["blockers"] == ["paint"]


def test_discrete_alone_sinks_a_case():
    gate_a, gate_b, aggregate = green_board()
    target = gate_b["cases"][0]["case_id"]
    gate_b["cases"][0] = b_case(target, within=1.0, discrete=1)
    receipt = build_receipt(gate_a, gate_b, aggregate)
    assert receipt["summary"]["finish_line_green"] == 25
    assert by_case(receipt, target)["blockers"] == ["discrete"]


def test_instability_alone_sinks_a_case():
    gate_a, gate_b, aggregate = green_board()
    target = aggregate["results"][0]["case_id"]
    aggregate["results"][0]["stable"] = False
    receipt = build_receipt(gate_a, gate_b, aggregate)
    assert receipt["summary"]["finish_line_green"] == 25
    assert by_case(receipt, target)["blockers"] == ["stability"]


# ---------------------------------------------------------------------------
# Unmeasured is never green — once per condition, because each arrives by its
# own path and a guard on one says nothing about the other three.
# ---------------------------------------------------------------------------


def test_an_unmeasured_geometry_is_not_green():
    gate_a, gate_b, aggregate = green_board()
    target = gate_a["cases"][0]["case_id"]
    gate_a["cases"][0] = unmeasured(target)
    receipt = build_receipt(gate_a, gate_b, aggregate)
    case = by_case(receipt, target)
    assert not case["finish_line_green"]
    assert not case["fully_measured"]
    assert case["unmeasured_conditions"] == ["geometry"]
    assert receipt["summary"]["finish_line_green"] == 25


def test_an_unmeasured_paint_takes_discrete_down_with_it():
    """Both come from Gate B, so neither can be green when it read nothing.

    The trap is specific: `unmeasured_case` sets `discrete_failures: 0`
    because it counted none. A discrete column that reads that zero as "no
    structural failures" reports a case Gate B never opened as structurally
    clean.
    """
    gate_a, gate_b, aggregate = green_board()
    target = gate_b["cases"][0]["case_id"]
    gate_b["cases"][0] = unmeasured(target)
    assert gate_b["cases"][0].get("discrete_failures", 0) == 0

    receipt = build_receipt(gate_a, gate_b, aggregate)
    case = by_case(receipt, target)
    assert not case["discrete"]["measured"]
    assert not case["discrete"]["green"]
    assert set(case["unmeasured_conditions"]) == {"paint", "discrete"}


def test_an_unmeasured_stability_is_not_green():
    gate_a, gate_b, aggregate = green_board()
    target = aggregate["results"][0]["case_id"]
    aggregate["results"] = aggregate["results"][1:]
    receipt = build_receipt(gate_a, gate_b, aggregate)
    case = by_case(receipt, target)
    assert not case["stability"]["measured"]
    assert case["stability"]["reason"] == "absent_from_aggregate"
    assert not case["finish_line_green"]


def test_a_case_missing_from_a_gate_report_is_unmeasured_not_absent():
    """26 rows always. A case that fell out of a report is a NOT MEASURED row.

    If the receipt iterated the gate's cases instead of the registry's, a
    capture failure that dropped 20 cases would produce `6/6` — a perfect
    score computed over the survivors.
    """
    gate_a, gate_b, aggregate = green_board()
    gate_a["cases"] = gate_a["cases"][:6]
    gate_b["cases"] = gate_b["cases"][:6]
    receipt = build_receipt(gate_a, gate_b, aggregate)
    assert receipt["summary"]["total_cases"] == 26
    assert receipt["summary"]["finish_line_green"] == 6
    dropped = by_case(receipt, gating_case_ids()[10])
    assert dropped["geometry"]["reason"] == "absent_from_gate_a"
    assert dropped["paint"]["reason"] == "absent_from_gate_b"


def test_gates_that_did_not_run_at_all_produce_no_green_and_no_receipt():
    receipt = build_receipt({}, {}, None)
    assert receipt["summary"]["total_cases"] == 26
    assert receipt["summary"]["finish_line_green"] == 0
    assert receipt["summary"]["fully_measured"] == 0
    assert not receipt_is_publishable(receipt)


# ---------------------------------------------------------------------------
# Attribution: the paint column is the percentage, and nothing else.
# ---------------------------------------------------------------------------


def test_a_structural_failure_does_not_masquerade_as_a_paint_percentage_failure():
    """A missing clip on an otherwise pixel-clean case: paint green, discrete RED.

    Gate B's own `green` is False here (it ANDs both halves). Reading that
    flag as the paint column would report a case whose percentage is 99.99%
    as paint-red, and the grind would go looking for a rasterizer bug that
    does not exist. Which column is red IS the receipt.
    """
    gate_a, gate_b, aggregate = green_board()
    target = gate_b["cases"][0]["case_id"]
    gate_b["cases"][0] = b_case(target, within=0.9999, discrete=17)
    assert gate_b["cases"][0]["green"] is False

    case = by_case(build_receipt(gate_a, gate_b, aggregate), target)
    assert case["paint"]["green"] is True
    assert case["discrete"]["green"] is False
    assert case["discrete"]["discrete_failures"] == 17
    assert case["discrete"]["kinds"] == ["missing_clip"]
    assert case["blockers"] == ["discrete"]


def test_the_percentage_bar_is_gate_bs_and_a_case_exactly_on_it_passes():
    gate_a, gate_b, aggregate = green_board()
    ids = gating_case_ids()
    gate_b["pass_fraction"] = 0.99
    gate_b["cases"][0] = b_case(ids[0], within=0.99)
    gate_b["cases"][1] = b_case(ids[1], within=0.9899)
    receipt = build_receipt(gate_a, gate_b, aggregate)
    assert by_case(receipt, ids[0])["paint"]["green"] is True
    assert by_case(receipt, ids[1])["paint"]["green"] is False


# ---------------------------------------------------------------------------
# The Goodhart guard: four columns at 25/26 is not a metric of 25/26.
# ---------------------------------------------------------------------------


def test_the_metric_is_the_per_case_and_not_the_best_column():
    """Break each condition on a DIFFERENT case: columns 25/26, metric 22/26.

    This is the substitution the campaign exists to catch, in the receipt
    itself. A join that reported `min(column_greens)` would print 25/26 here
    and every number after it would be wrong in the flattering direction.
    """
    ids = gating_case_ids()
    gate_a, gate_b, aggregate = green_board()
    gate_a["cases"][0] = a_case(ids[0], green=False, geometry_failures=1)
    gate_b["cases"][1] = b_case(ids[1], within=0.5)
    gate_b["cases"][2] = b_case(ids[2], within=1.0, discrete=4)
    aggregate["results"][3]["stable"] = False

    receipt = build_receipt(gate_a, gate_b, aggregate)
    s = receipt["summary"]
    for cond in CONDITIONS:
        assert s["by_condition"][cond]["green"] == 25, cond
    assert s["finish_line_green"] == 22
    assert min(s["by_condition"][c]["green"] for c in CONDITIONS) == 25


def test_conjoin_requires_every_condition_in_the_tuple():
    """A condition dropped from the AND must be able to fail the case.

    Guards against the conjunction being written as a hand-rolled chain that
    silently omits one; each condition is knocked out in turn against an
    otherwise-green row.
    """
    for missing in CONDITIONS:
        row = {
            c: {"measured": True, "green": c != missing, "reason": None}
            for c in CONDITIONS
        }
        assert conjoin(row)["finish_line_green"] is False, missing
    all_green = {c: {"measured": True, "green": True} for c in CONDITIONS}
    assert conjoin(all_green)["finish_line_green"] is True


# ---------------------------------------------------------------------------
# Bars are cited from their owners, never copied.
# ---------------------------------------------------------------------------


def test_the_stability_run_count_is_read_from_parity_lib():
    import parity_lib

    original = parity_lib.STABILITY_MIN_RUNS
    try:
        parity_lib.STABILITY_MIN_RUNS = 5
        gate_a, gate_b, aggregate = green_board()  # rows carry measured_runs=3
        receipt = build_receipt(gate_a, gate_b, aggregate)
        assert receipt["bars"]["stability_min_runs"] == 5
        assert receipt["summary"]["finish_line_green"] == 0
        case = by_case(receipt, gating_case_ids()[0])
        assert case["stability"]["reason"] == "stability_unmeasured"
    finally:
        parity_lib.STABILITY_MIN_RUNS = original


def test_the_variance_budget_is_read_from_the_gate_that_enforces_it():
    import parity_gate

    original = parity_gate.level_defaults
    try:
        parity_gate.level_defaults = lambda level: dict(
            original(level), max_variance=0.0
        )
        gate_a, gate_b, aggregate = green_board()
        aggregate["results"][0]["diff_pct_variance"] = 0.05
        receipt = build_receipt(gate_a, gate_b, aggregate)
        assert receipt["bars"]["stability_max_variance"] == 0.0
        assert by_case(receipt, aggregate["results"][0]["case_id"])["stability"][
            "reason"
        ] == "variance"
    finally:
        parity_gate.level_defaults = original


def test_the_paint_tolerance_is_reported_from_gate_bs_report_not_a_local_copy():
    gate_a, gate_b, aggregate = green_board()
    gate_b["aa_tolerance"] = 11
    gate_b["pass_fraction"] = 0.5
    receipt = build_receipt(gate_a, gate_b, aggregate)
    assert receipt["bars"]["aa_tolerance"] == 11
    assert receipt["bars"]["paint_pass_fraction"] == 0.5


# ---------------------------------------------------------------------------
# Stability evidence: measured, not attempted; native viewport, not exploit.
# ---------------------------------------------------------------------------


def test_looked_once_and_looked_thrice_and_it_moved_are_different_reasons():
    gate_a, gate_b, aggregate = green_board()
    ids = gating_case_ids()
    aggregate["results"][0]["measured_runs"] = 1
    aggregate["results"][1]["stable"] = False
    receipt = build_receipt(gate_a, gate_b, aggregate)
    assert by_case(receipt, ids[0])["stability"]["reason"] == "stability_unmeasured"
    assert by_case(receipt, ids[1])["stability"]["reason"] == "unstable"


def test_an_unknown_run_count_is_zero_runs_not_one():
    vps = flr.registry_viewports()
    row = {"case_id": "x", "viewport": vps[gating_case_ids()[0]], "stable": True}
    verdict = stability_verdict(row, min_runs=3, max_variance=0.1)
    assert verdict["measured_runs"] == 0
    assert verdict["reason"] == "stability_unmeasured"


def test_an_exploit_viewport_row_is_not_stability_evidence():
    """The exploit phase re-runs worst cases at viewports with no baseline.

    Those rows read 100% and must never stand in for the case's verdict at its
    registry viewport — `parity_gate.primary_viewport_filter`'s whole job,
    cited here rather than re-derived.
    """
    gate_a, gate_b, aggregate = green_board()
    target = gating_case_ids()[0]
    for row in aggregate["results"]:
        if row["case_id"] == target:
            row["viewport"] = "3000x3000"
    receipt = build_receipt(gate_a, gate_b, aggregate)
    assert by_case(receipt, target)["stability"]["reason"] == "absent_from_aggregate"


def test_the_aggregates_cases_list_is_read_when_results_is_absent():
    gate_a, gate_b, aggregate = green_board()
    aggregate = {"cases": aggregate["results"]}
    receipt = build_receipt(gate_a, gate_b, aggregate)
    assert receipt["summary"]["by_condition"]["stability"]["green"] == 26


# ---------------------------------------------------------------------------
# Publishing: the numbers never fail, a receipt that did not run does.
# ---------------------------------------------------------------------------


def _run(gate_a, gate_b, aggregate, tmp):
    paths = {}
    for name, payload in (("a", gate_a), ("b", gate_b), ("agg", aggregate)):
        if payload is None:
            paths[name] = Path(tmp) / f"missing-{name}.json"
            continue
        paths[name] = Path(tmp) / f"{name}.json"
        paths[name].write_text(json.dumps(payload), encoding="utf-8")
    out = Path(tmp) / "receipt.json"
    md = Path(tmp) / "receipt.md"
    proc = subprocess.run(
        [
            sys.executable,
            str(SCRIPT),
            "--gate-a",
            str(paths["a"]),
            "--gate-b",
            str(paths["b"]),
            "--aggregate",
            str(paths["agg"]),
            "--json",
            str(out),
            "--markdown",
            str(md),
        ],
        capture_output=True,
        text=True, encoding="utf-8"
    )
    return proc, out, md


def test_process_exits_zero_when_the_numbers_are_catastrophic():
    """0/26 is a measurement. It is not an error and must not read as one."""
    gate_a, gate_b, aggregate = green_board()
    for case in gate_a["cases"]:
        case["green"] = False
        case["geometry_failures"] = 99
    with tempfile.TemporaryDirectory() as tmp:
        proc, out, _ = _run(gate_a, gate_b, aggregate, tmp)
        assert proc.returncode == 0, proc.stdout + proc.stderr
        assert json.loads(out.read_text(encoding="utf-8"))["summary"]["finish_line_green"] == 0


def test_process_exits_nonzero_when_it_measured_nothing():
    with tempfile.TemporaryDirectory() as tmp:
        proc, _, _ = _run(None, None, None, tmp)
        assert proc.returncode == 1
        assert "did not run" in proc.stdout


def test_a_missing_aggregate_is_named_rather_than_silently_unstable():
    gate_a, gate_b, _ = green_board()
    with tempfile.TemporaryDirectory() as tmp:
        proc, out, _ = _run(gate_a, gate_b, None, tmp)
        assert "stability is unmeasured" in proc.stdout
        receipt = json.loads(out.read_text(encoding="utf-8"))
        assert receipt["summary"]["finish_line_green"] == 0
        assert receipt["summary"]["by_condition"]["geometry"]["green"] == 26


def test_the_markdown_says_the_metric_is_not_the_columns():
    gate_a, gate_b, aggregate = green_board()
    md = format_markdown(build_receipt(gate_a, gate_b, aggregate))
    assert "26/26" in md
    assert "conjunction" in md.lower()
    assert "not green" in md.lower()


def test_the_markdown_prints_a_row_for_every_gating_case():
    gate_a, gate_b, aggregate = green_board()
    md = format_markdown(build_receipt(gate_a, gate_b, aggregate))
    for case_id in gating_case_ids():
        assert f"`{case_id}`" in md, case_id


# ---------------------------------------------------------------------------
# Unit-level: the two verdicts most likely to be quietly rewritten.
# ---------------------------------------------------------------------------


def test_paint_verdict_declines_a_record_with_no_fraction():
    verdict = paint_verdict({"case_id": "x", "measured": True}, 0.99)
    assert not verdict["measured"]
    assert verdict["reason"] == "no_within_fraction"


def test_discrete_verdict_reports_the_kinds_it_found():
    record = {
        "case_id": "x",
        "measured": True,
        "green": False,
        "within_fraction": 1.0,
        "discrete_failures": 2,
        "failures": [
            {"discrete": True, "kind": "wrong_solid_color"},
            {"discrete": True, "kind": "missing_clip"},
            {"discrete": False, "kind": "aa_noise"},
        ],
    }
    verdict = discrete_verdict(record)
    assert verdict["kinds"] == ["missing_clip", "wrong_solid_color"]


# ---------------------------------------------------------------------------
# The lane. A receipt nothing runs is not a receipt.
#
# Night 5 shipped Gates A/B/C with `continue-on-error: true` and no
# `if: always()`, so an earlier failing step SKIPPED all three on every run of
# the PR that introduced them — an advisory cycle that collected nothing, on
# exactly the PRs where the board is most useful. The step existing in the YAML
# was never the same fact as the step running. These assert the conditions that
# make it run, not that someone typed its name.
# ---------------------------------------------------------------------------

WORKFLOW = REPO_ROOT / ".github" / "workflows" / "parity.yml"


def _workflow():
    """Parse parity.yml, and NEVER skip when the parser is missing.

    The first draft of this file wrapped the import in
    `except ImportError: return None`, and every lane test below opened with
    `if workflow is None: return`. On this seat pyyaml is installed and they
    all ran; on a bare CI runner they would have returned immediately and
    printed `ok`. A guard that reports success having checked nothing is the
    decoration this campaign exists to refuse, and the escape hatch made all
    five of them exactly that — on the runner where they matter most.

    Caught because the guard suite's first CI run went red on this import in
    a NEIGHBOURING file, which had the same hatch one level up. Hard failure
    is the correct behaviour: if the lane cannot be parsed, the lane is
    unverified, and unverified is not green.
    """
    import yaml  # noqa: E402 - a missing parser must fail, never skip

    with open(WORKFLOW, encoding="utf-8") as handle:
        return yaml.safe_load(handle)


def _receipt_steps(workflow, job):
    return [
        s
        for s in workflow["jobs"][job]["steps"]
        if "finish_line_receipt.py" in str(s.get("run", ""))
    ]


def test_both_aggregate_lanes_compute_the_metric():
    workflow = _workflow()
    for job in ("pr-aggregate", "nightly-aggregate"):
        steps = _receipt_steps(workflow, job)
        assert len(steps) == 1, f"{job} runs the receipt {len(steps)} times"


def test_the_receipt_runs_even_after_an_earlier_step_failed():
    """`continue-on-error` stops a step failing the job. It does not run it."""
    workflow = _workflow()
    for job in ("pr-aggregate", "nightly-aggregate"):
        step = _receipt_steps(workflow, job)[0]
        assert step.get("if") == "always()", f"{job}: receipt is skippable"
        assert step.get("continue-on-error") is True, f"{job}: receipt can fail the job"


def test_the_receipt_is_fed_all_three_inputs():
    """Missing --aggregate would score stability unmeasured on all 26 forever."""
    workflow = _workflow()
    for job in ("pr-aggregate", "nightly-aggregate"):
        run = _receipt_steps(workflow, job)[0]["run"]
        for flag in ("--gate-a", "--gate-b", "--aggregate", "--json", "--markdown"):
            assert flag in run, f"{job}: receipt missing {flag}"


def test_the_guard_suite_runs_in_ci():
    """Every guard in this campaign was hand-run once and never again.

    Asserted on the step's own command rather than the job's name, and on the
    glob rather than a list, because a hand-maintained list of guard files is
    a place for a new guard to be silently left out.
    """
    workflow = _workflow()
    runs = [
        str(s.get("run", ""))
        for s in workflow["jobs"]["script-guards"]["steps"]
    ]
    body = "\n".join(runs)
    assert "scripts/tests/test_*.py" in body
    assert "python3" in body


def test_the_guard_job_installs_the_yaml_parser_the_lane_guards_need():
    """Every lane guard in scripts/tests/ parses parity.yml. Nothing else does.

    pyyaml is not in the runner image, and it IS on the trench seat, so this
    dependency is invisible locally and fatal in CI — which is exactly how it
    was found. Asserted here so removing the install is caught at review
    rather than by a red job.
    """
    workflow = _workflow()
    body = "\n".join(
        str(s.get("run", "")) for s in workflow["jobs"]["script-guards"]["steps"]
    )
    assert "pyyaml" in body


def test_no_guard_file_skips_itself_when_the_yaml_parser_is_missing():
    """The hatch that made five of this file's own guards vacuous in CI.

    `try: import yaml / except ImportError: return` reads like defensive
    portability and behaves like deletion: on the one machine where the lane
    guards matter, they print `ok` having parsed nothing. Missing parser must
    be a hard failure in every guard file, not just this one.

    Checked on the parse tree, not the text: this file's own docstrings
    discuss the hatch by name, and a grep-based version of this test failed on
    its own prose.
    """
    import ast

    def imports_yaml(node):
        return any(
            isinstance(n, ast.Import) and any(a.name == "yaml" for a in n.names)
            for n in ast.walk(node)
        )

    def catches_import_error(handler):
        names = []
        if isinstance(handler.type, ast.Name):
            names = [handler.type.id]
        elif isinstance(handler.type, ast.Tuple):
            names = [e.id for e in handler.type.elts if isinstance(e, ast.Name)]
        return {"ImportError", "ModuleNotFoundError"} & set(names)

    for path in sorted((REPO_ROOT / "scripts" / "tests").glob("test_*.py")):
        tree = ast.parse(path.read_text(encoding="utf-8"))
        for node in ast.walk(tree):
            if not isinstance(node, ast.Try):
                continue
            if not any(imports_yaml(stmt) for stmt in node.body):
                continue
            for handler in node.handlers:
                assert not catches_import_error(
                    handler
                ), f"{path.name} skips its lane guards when pyyaml is missing"


def test_this_very_guard_file_is_matched_by_the_ci_glob():
    """The glob must actually match this file, not merely look like it would."""
    import glob as _glob

    matched = _glob.glob(str(REPO_ROOT / "scripts" / "tests" / "test_*.py"))
    assert str(Path(__file__).resolve()) in {str(Path(m).resolve()) for m in matched}


def test_a_stub_shaper_board_produces_no_n_over_26_at_all():
    """The receipt is what gets quoted, so it is where the refusal must land.

    Gate A refuses to be GREEN on a capture whose text advances came from no
    font, but it still prints per-case geometry counts -- those are useful
    mechanics. The receipt is different: its whole output is the number people
    repeat. A board of stub captures must therefore yield an N/26 of zero with
    every case NOT FULLY MEASURED, not a confident conjunction over a column
    that measures a 0.5em-per-character ruler.
    """
    gate_a, gate_b, aggregate = green_board()
    gate_a = {
        "tolerance_px": 0.5,
        "cases": [
            a_case(c["case_id"], attributable=False, text_backend="stub-0.5em")
            for c in gate_a["cases"]
        ],
    }
    receipt = build_receipt(gate_a, gate_b, aggregate)

    assert receipt["summary"]["finish_line_green"] == 0, (
        "a board measured against a fixed ruler cannot contribute a single "
        "finish-line-green case"
    )
    assert receipt["summary"]["fully_measured"] == 0, \
        "every case on such a board is unmeasured on geometry"
    for case in receipt["cases"]:
        geom = case["geometry"]
        assert geom["measured"] is False
        assert geom["green"] is False
        assert geom["reason"] == "text_metrics_not_font_derived", (
            "the reason has to name the cause, or a future reader rediscovers "
            f"the stub from scratch; got {geom['reason']!r}"
        )
        assert geom["text_backend"] == "stub-0.5em", \
            "and it has to name the backend, so the refusal is checkable"

    # A gate-a.json written BEFORE the provenance field existed carries no
    # `attributable` key at all, and that is the likeliest shape to meet in the
    # wild: every report on disk from the 57 nights before 2026-10-01. Absent
    # must refuse exactly as False does. Written as its own board because the
    # builder above always sets the key, and a guard that only ever sees the
    # key set cannot tell `is not True` from `is False`.
    silent_a = {"tolerance_px": 0.5, "cases": []}
    for case in green_board()[0]["cases"]:
        row = dict(case)
        del row["attributable"]
        del row["text_metrics_font_derived"]
        del row["text_backend"]
        silent_a["cases"].append(row)
    silent = build_receipt(silent_a, gate_b, aggregate)
    assert silent["summary"]["finish_line_green"] == 0, (
        "a report that does not say which shaper produced it must refuse: "
        "absent is not a yes, and every pre-2026-10-01 report is absent"
    )
    for case in silent["cases"]:
        assert case["geometry"]["reason"] == "text_metrics_not_font_derived"
        assert case["geometry"]["text_backend"] == "unknown", \
            "a report that did not say must be reported as not having said"

    # The control: the SAME board with font-derived captures is fully green. If
    # this does not hold, the assertions above pass for the wrong reason.
    control = build_receipt(*green_board())
    assert control["summary"]["finish_line_green"] == 26, \
        "the control board must be fully green or the refusal proves nothing"


if __name__ == "__main__":
    for name, fn in sorted(globals().items()):
        if name.startswith("test_") and callable(fn):
            fn()
            print(f"ok  {name}")
    print("PASS: the metric is the conjunction, and unmeasured is never green")
