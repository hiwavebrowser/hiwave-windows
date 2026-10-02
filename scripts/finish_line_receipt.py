#!/usr/bin/env python3
"""
finish_line_receipt.py — compute `N/26 finish-line-green`, the campaign metric.

The four gates each publish an independent verdict. Nothing joined them, so
`N/26` has only ever been stated as three separate numbers in prose ("4/26
geometry-green, 1/26 paint-green") with the conjunction left to the reader.
That is not the metric. The metric is the CONJUNCTION, per
`docs/PARITY_FINISH_LINE_PLAN_2026-08-04.md` §3 and
`trench/BASELINE-parity-finish-line.md`:

    1. geometry   within 0.5px per box vs baselines/chrome-148/**/layout-rects.json
    2. paint      >= 99% of pixels within aa_tolerance (the one pinned constant)
    3. stable     across STABILITY_MIN_RUNS measured iterations
    4. discrete   zero structural failures (paint outside box, missing clip,
                  wrong solid color) — auto-fail REGARDLESS of the percentage

A case is finish-line-green only when all four are AFFIRMATIVELY MEASURED and
all four are green. This script does not measure anything itself; it reads the
gates' own reports and refuses to fill in a blank.

Three rules hold the honesty of the join, and each has a mutation-checked
guard:

**Unmeasured is never green.** A condition whose gate could not score the case
reads NOT MEASURED and the case is not counted in N. This is the baseline
file's blank-instrument-row rule applied to the conjunction: a gate that did
not run and a gate that found nothing wrong must not produce the same row.

**Conditions 2 and 4 are separate columns even though one gate produces both.**
Gate B's per-case `green` folds the percentage bar and the discrete auto-fails
together. Reading it as the paint column would report a case with a missing
clip and 99.99% of pixels within tolerance as "paint-red", which misattributes
the defect and sends the grind at the wrong family. Plan §5 calls a case that
is geometry-green but paint-red *signal, not noise* — that only holds if the
columns say which condition actually failed.

**Discrete is unmeasured when paint is unmeasured.** Both come from Gate B. A
case Gate B could not read has `discrete_failures == 0` in its record because
nothing was counted, not because nothing is wrong. Treating that zero as a
green discrete column is the exact substitution this campaign exists to end.

Non-gating by construction, on Gate C's precedent: the NUMBERS never fail a
run (this is a receipt, not a fifth gate), but a receipt that measured nothing
exits 1. "N/26 where N=0 because nothing ran" and "0/26 measured and red" are
different facts.

Usage:
    python3 scripts/finish_line_receipt.py \
        --gate-a parity-results/gate-a.json \
        --gate-b parity-results/gate-b.json \
        --aggregate parity-results/aggregate_report.json \
        --json parity-results/finish-line.json \
        --markdown parity-results/finish-line.md
"""

import argparse
import json
import sys
from pathlib import Path
from typing import Any, Dict, List, Optional, Sequence

REPO_ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(Path(__file__).resolve().parent))


# ---------------------------------------------------------------------------
# Bars — cited from their owners, never copied.
#
# Same rule the gates already follow for `aa_tolerance` and
# `STABILITY_MIN_RUNS`: one bar, one place. A receipt that copies a threshold
# can drift from the gate that enforces it and then publish a green N/26 built
# on a bar nobody is holding.
# ---------------------------------------------------------------------------


def stability_min_runs() -> int:
    from parity_lib import STABILITY_MIN_RUNS  # noqa: E402

    return int(STABILITY_MIN_RUNS)


def stability_max_variance(level: str = "pr_merge") -> float:
    """The variance budget, cited from the gate that enforces it."""
    from parity_gate import level_defaults  # noqa: E402

    return float(level_defaults(level)["max_variance"])


def measured_runs(row: Dict[str, Any]) -> int:
    """Iterations of this row that produced a MEASUREMENT, cited from the gate.

    Three producers spell this three ways and one of them is a list where
    another is an int; `parity_gate.measured_runs` already reconciles all
    three and treats unknown as zero. Re-deriving it here is how the receipt
    and the gate would come to disagree about whether a case was stable.
    """
    from parity_gate import measured_runs as _measured_runs  # noqa: E402

    return int(_measured_runs(row))


def gating_case_ids() -> List[str]:
    """The 26. Cited from Gate A's own scope rule, not re-listed here."""
    from layout_oracle_gate import NON_GATING_SCOPES, load_case_registry  # noqa: E402

    registry = load_case_registry()
    return sorted(
        cid for cid, case in registry.items() if case["scope"] not in NON_GATING_SCOPES
    )


def registry_viewports() -> Dict[str, str]:
    from layout_oracle_gate import load_case_registry  # noqa: E402

    return {
        cid: f"{case['width']}x{case['height']}"
        for cid, case in load_case_registry().items()
    }


# ---------------------------------------------------------------------------
# Per-condition verdicts
# ---------------------------------------------------------------------------


def _unmeasured(reason: str, **extra: Any) -> Dict[str, Any]:
    verdict = {"measured": False, "green": False, "reason": reason}
    verdict.update(extra)
    return verdict


def geometry_verdict(case: Optional[Dict[str, Any]]) -> Dict[str, Any]:
    if case is None:
        return _unmeasured("absent_from_gate_a")
    if not case.get("measured"):
        return _unmeasured(case.get("reason") or "unmeasured")
    # A capture whose text advances came from no font is MEASURED and
    # UNATTRIBUTABLE, and the metric has no cell for that: the conjunction
    # would read its geometry column as a fact about RustKit's box math. It is
    # a fact about a 0.5em-per-character ruler. Gate A refuses to be green on
    # such a board (see its `gate_passes`); the receipt is where that refusal
    # has to stop being a number, because the receipt is the thing that gets
    # quoted as N/26.
    #
    # Deliberately UNMEASURED and not RED. Red says "RustKit got this wrong
    # here", which is a claim this board cannot support in either direction —
    # the stub can mask a defect as easily as invent one.
    if case.get("attributable") is not True:
        return _unmeasured(
            "text_metrics_not_font_derived",
            text_backend=case.get("text_backend", "unknown"),
        )
    return {
        "measured": True,
        "green": bool(case.get("green")),
        "reason": None,
        "geometry_failures": int(case.get("geometry_failures", 0)),
        "join_failures": int(case.get("join_failures", 0)),
        "compared": int(case.get("compared", 0)),
    }


def paint_verdict(
    case: Optional[Dict[str, Any]], pass_fraction: float
) -> Dict[str, Any]:
    """Condition 2 ONLY — the percentage bar, discrete deliberately excluded.

    Derived from `within_fraction` rather than Gate B's per-case `green`,
    because that flag is the AND of this condition and condition 4. See the
    module docstring: collapsing them makes every discrete failure also read
    as a paint-percentage failure and the receipt stops naming the defect.
    """
    if case is None:
        return _unmeasured("absent_from_gate_b")
    if not case.get("measured"):
        return _unmeasured(case.get("reason") or "unmeasured")
    within = case.get("within_fraction")
    if within is None:
        return _unmeasured("no_within_fraction")
    return {
        "measured": True,
        "green": float(within) >= pass_fraction,
        "reason": None,
        "within_fraction": float(within),
        "pass_fraction": pass_fraction,
        "outside_tolerance_px": case.get("outside_tolerance_px"),
    }


def discrete_verdict(case: Optional[Dict[str, Any]]) -> Dict[str, Any]:
    """Condition 4 — auto-fail regardless of percentage.

    Unmeasured tracks Gate B: a case Gate B could not read reports zero
    discrete failures because it counted none, which is not the same fact as
    having none.
    """
    if case is None:
        return _unmeasured("absent_from_gate_b")
    if not case.get("measured"):
        return _unmeasured(case.get("reason") or "unmeasured")
    count = int(case.get("discrete_failures", 0))
    kinds = sorted(
        {
            f.get("kind")
            for f in case.get("failures", [])
            if f.get("discrete") and f.get("kind")
        }
    )
    return {
        "measured": True,
        "green": count == 0,
        "reason": None,
        "discrete_failures": count,
        "kinds": kinds,
    }


def stability_verdict(
    row: Optional[Dict[str, Any]], min_runs: int, max_variance: float
) -> Dict[str, Any]:
    """Condition 3, mirroring `parity_gate`'s three checks in its own order.

    `stability_unmeasured` stays a distinct reason from `unstable` — night 4's
    point, and it survives the join: "we looked once" and "we looked three
    times and it moved" send the grind to different places.
    """
    if row is None:
        return _unmeasured("absent_from_aggregate", measured_runs=0)
    runs = measured_runs(row)
    if runs < min_runs:
        return _unmeasured(
            "stability_unmeasured", measured_runs=runs, required_runs=min_runs
        )
    variance = row.get("diff_pct_variance")
    if row.get("stable") is not True:
        return {
            "measured": True,
            "green": False,
            "reason": "unstable",
            "measured_runs": runs,
            "variance": variance,
            "max_variance": max_variance,
        }
    if variance is not None and float(variance) > max_variance:
        return {
            "measured": True,
            "green": False,
            "reason": "variance",
            "measured_runs": runs,
            "variance": float(variance),
            "max_variance": max_variance,
        }
    return {
        "measured": True,
        "green": True,
        "reason": None,
        "measured_runs": runs,
        "variance": variance,
        "max_variance": max_variance,
    }


# ---------------------------------------------------------------------------
# The conjunction
# ---------------------------------------------------------------------------

CONDITIONS = ("geometry", "paint", "stability", "discrete")


def conjoin(row: Dict[str, Dict[str, Any]]) -> Dict[str, Any]:
    """All four measured AND all four green. Anything else is not green.

    Written as an explicit all() over the fixed condition tuple rather than a
    chain of ands so that adding a fifth condition cannot silently leave it
    out of the metric.
    """
    measured = all(row[c]["measured"] for c in CONDITIONS)
    green = measured and all(row[c]["green"] for c in CONDITIONS)
    blockers = [c for c in CONDITIONS if not row[c]["green"]]
    unmeasured = [c for c in CONDITIONS if not row[c]["measured"]]
    return {
        "finish_line_green": green,
        "fully_measured": measured,
        "blockers": blockers,
        "unmeasured_conditions": unmeasured,
    }


def build_receipt(
    gate_a: Dict[str, Any],
    gate_b: Dict[str, Any],
    aggregate: Optional[Dict[str, Any]],
    case_ids: Optional[Sequence[str]] = None,
) -> Dict[str, Any]:
    ids = list(case_ids) if case_ids is not None else gating_case_ids()

    a_by_case = {c["case_id"]: c for c in gate_a.get("cases", [])}
    b_by_case = {c["case_id"]: c for c in gate_b.get("cases", [])}

    pass_fraction = float(gate_b.get("pass_fraction", 0.99))
    min_runs = stability_min_runs()
    max_variance = stability_max_variance()

    stability_rows = _stability_rows(aggregate)

    cases = []
    for case_id in ids:
        row = {
            "geometry": geometry_verdict(a_by_case.get(case_id)),
            "paint": paint_verdict(b_by_case.get(case_id), pass_fraction),
            "discrete": discrete_verdict(b_by_case.get(case_id)),
            "stability": stability_verdict(
                stability_rows.get(case_id), min_runs, max_variance
            ),
        }
        record = {"case_id": case_id}
        record.update(row)
        record.update(conjoin(row))
        cases.append(record)

    green = [c for c in cases if c["finish_line_green"]]
    fully_measured = [c for c in cases if c["fully_measured"]]

    by_condition = {
        c: {
            "green": sum(1 for case in cases if case[c]["green"]),
            "measured": sum(1 for case in cases if case[c]["measured"]),
        }
        for c in CONDITIONS
    }

    return {
        "metric": "N/26 finish-line-green",
        "bars": {
            "geometry_tolerance_px": gate_a.get("tolerance_px"),
            "aa_tolerance": gate_b.get("aa_tolerance"),
            "paint_pass_fraction": pass_fraction,
            "stability_min_runs": min_runs,
            "stability_max_variance": max_variance,
        },
        "cases": cases,
        "summary": {
            "total_cases": len(cases),
            "finish_line_green": len(green),
            "fully_measured": len(fully_measured),
            "not_fully_measured": len(cases) - len(fully_measured),
            "by_condition": by_condition,
        },
    }


def _stability_rows(aggregate: Optional[Dict[str, Any]]) -> Dict[str, Dict[str, Any]]:
    """Each case's REGISTRY-viewport row from the aggregate.

    The exploit phase re-runs worst cases at viewports with no baseline; those
    rows read 100% and are not the case's verdict. `parity_gate` already
    filters them with `primary_viewport_filter` and the receipt cites it
    rather than re-implementing the same rule one field differently.
    """
    if not aggregate:
        return {}
    from parity_gate import primary_viewport_filter  # noqa: E402

    rows = aggregate.get("results") or aggregate.get("cases") or []
    native = registry_viewports()
    by_case: Dict[str, Dict[str, Any]] = {}
    for row in primary_viewport_filter(rows):
        case_id = row.get("case_id")
        if not case_id:
            continue
        # A case can still appear twice if the registry does not know it;
        # prefer the row at its native viewport, and otherwise the first.
        if case_id in by_case and str(row.get("viewport", "")) != native.get(case_id):
            continue
        by_case[case_id] = row
    return by_case


def receipt_is_publishable(receipt: Dict[str, Any]) -> bool:
    """A receipt that measured nothing is not a receipt.

    Gate C's rule, and for its reason: non-gating must mean "the numbers never
    fail a run", not "always exits 0". A board nobody produced does not read
    clean. One tripwire, not two — `fully_measured == 0` subsumes
    `total_cases == 0`.
    """
    return receipt["summary"]["fully_measured"] > 0


# ---------------------------------------------------------------------------
# Output
# ---------------------------------------------------------------------------


def _condition_cell(verdict: Dict[str, Any]) -> str:
    if not verdict["measured"]:
        return f"NOT MEASURED ({verdict['reason']})"
    return "green" if verdict["green"] else "RED"


def format_markdown(receipt: Dict[str, Any]) -> str:
    s = receipt["summary"]
    bars = receipt["bars"]
    lines = [
        "## Finish line — `N/26 finish-line-green`",
        "",
        f"**{s['finish_line_green']}/{s['total_cases']}** cases pass all four "
        "conditions simultaneously.",
        "",
        "This is the campaign metric: the CONJUNCTION of geometry, paint, "
        "stability and discrete-structural, not a mean and not four separate "
        "scores. A condition that could not be measured is NOT green.",
        "",
        f"- geometry: ≤ {bars['geometry_tolerance_px']}px per box, per axis",
        f"- paint: ≥ {bars['paint_pass_fraction'] * 100:.4g}% of pixels within "
        f"±{bars['aa_tolerance']}/255 per channel",
        f"- stability: {bars['stability_min_runs']} measured iterations, "
        f"variance ≤ {bars['stability_max_variance']}",
        "- discrete: zero structural failures, regardless of percentage",
        "",
        f"Fully measured on all four: **{s['fully_measured']}/{s['total_cases']}**"
        f" ({s['not_fully_measured']} not).",
        "",
        "| condition | green | measured |",
        "|---|---|---|",
    ]
    for cond in CONDITIONS:
        c = s["by_condition"][cond]
        lines.append(
            f"| {cond} | {c['green']}/{s['total_cases']} | "
            f"{c['measured']}/{s['total_cases']} |"
        )
    lines += [
        "",
        "Per-condition counts do not add up to the metric and are not meant to:"
        " a case is green only where every column is.",
        "",
        "| case | geometry | paint | stability | discrete | finish line |",
        "|---|---|---|---|---|---|",
    ]
    for case in receipt["cases"]:
        mark = "**GREEN**" if case["finish_line_green"] else ""
        lines.append(
            f"| `{case['case_id']}` | {_condition_cell(case['geometry'])} "
            f"| {_condition_cell(case['paint'])} "
            f"| {_condition_cell(case['stability'])} "
            f"| {_condition_cell(case['discrete'])} | {mark} |"
        )
    return "\n".join(lines) + "\n"


def print_receipt(receipt: Dict[str, Any]) -> None:
    s = receipt["summary"]
    print("Finish line — N/26 finish-line-green")
    print(
        f"  metric:     {s['finish_line_green']}/{s['total_cases']}"
        " cases pass all four conditions"
    )
    print(
        f"  measured:   {s['fully_measured']}/{s['total_cases']} scored on all four"
        f"  ({s['not_fully_measured']} not fully measured)"
    )
    for cond in CONDITIONS:
        c = s["by_condition"][cond]
        print(
            f"    {cond:<10} {c['green']}/{s['total_cases']} green,"
            f" {c['measured']}/{s['total_cases']} measured"
        )
    print()
    for case in receipt["cases"]:
        if case["finish_line_green"]:
            print(f"  GREEN {case['case_id']}")
            continue
        detail = ", ".join(
            f"{c}={_condition_cell(case[c])}" for c in case["blockers"]
        )
        print(f"  RED   {case['case_id']}: {detail}")


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Compute N/26 finish-line-green from the gate reports"
    )
    parser.add_argument("--gate-a", type=Path, required=True)
    parser.add_argument("--gate-b", type=Path, required=True)
    parser.add_argument(
        "--aggregate",
        type=Path,
        help="parity_aggregate report carrying the stability evidence",
    )
    parser.add_argument("--json", type=Path)
    parser.add_argument("--markdown", type=Path)
    args = parser.parse_args()

    def load(path: Optional[Path]) -> Optional[Dict[str, Any]]:
        if path is None or not path.exists():
            return None
        with open(path, encoding="utf-8") as handle:
            return json.load(handle)

    gate_a = load(args.gate_a)
    gate_b = load(args.gate_b)
    aggregate = load(args.aggregate)

    # A missing gate report is not an empty one. `{}` here would score every
    # case `absent_from_gate_*`, which is honest, but saying so is better than
    # making the reader infer it from 26 identical reasons.
    for name, report, path in (
        ("A", gate_a, args.gate_a),
        ("B", gate_b, args.gate_b),
    ):
        if report is None:
            print(f"Gate {name} report not found at {path} — it did not run.")
    if aggregate is None:
        print(
            f"No aggregate report at {args.aggregate} — stability is unmeasured"
            " for every case, which is not the same as stable."
        )

    receipt = build_receipt(gate_a or {}, gate_b or {}, aggregate)
    print_receipt(receipt)

    if args.json:
        args.json.parent.mkdir(parents=True, exist_ok=True)
        with open(args.json, "w", encoding="utf-8") as handle:
            json.dump(receipt, handle, indent=2)
    if args.markdown:
        args.markdown.parent.mkdir(parents=True, exist_ok=True)
        with open(args.markdown, "w", encoding="utf-8") as handle:
            handle.write(format_markdown(receipt))

    if not receipt_is_publishable(receipt):
        print(
            "\nThis receipt measured nothing on all four conditions. That is"
            " not 0/26 — it is a receipt that did not run."
        )
        return 1
    print("\nReceipt published. This is a measurement, not a gate: it blocks nothing.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
