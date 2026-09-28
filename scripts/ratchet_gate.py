#!/usr/bin/env python3
"""Ratchet regression gate over Gate A / Gate B / stability.

Blocking teeth without pretending absolute green (design pin, Prometheus
2026-08-12): a case FAILS this gate only when it regresses against the
committed baseline — absolute red that matches the baseline is visible but
does not fail. The baseline tightens by MANUAL commit only (a tiny PR that
touches nothing but the baseline file); this script never writes it during
a gated run.

Exit codes (the workflow step must fail ONLY on 1):
  0  no regression, nothing absolutely red beyond baseline — or RATCHET OFF
     (no baseline committed), stated loudly rather than inferred
  1  REGRESSION — at least one case is worse than the committed floor
  2  absolute red exists but nothing regressed (the honest steady state)

A NEW DISCRETE FAILURE IS NOT ALWAYS A REGRESSION
-------------------------------------------------
Gate B's discrete detectors may only speak about an element whose geometry
Gate A would call exact — otherwise they read pixels belonging to another box
(paint_oracle_gate.py, GEOMETRY IS A PRECONDITION). So the set of elements the
gate is *allowed* to report on grows every time geometry improves, and a defect
that was always present appears for the first time. That is a widened
jurisdiction, not a new bug, and a ratchet that cannot tell the two apart fails
a promote for work that fixed something.

Measured, 2026-08-30: develop's `gradient-backgrounds .linear-6` corner notch
read as a REGRESSION against master's floor. The trench digest had recorded
that exact box as UNMEASURABLE on 2026-08-12 because the card was 18px out of
place. Eighteen days of geometry work brought it inside 0.5px; nothing about
the notch changed.

So Gate B publishes WHICH elements it withheld, the floor carries that set, and
a new id whose element the baseline run withheld is classified `newly_measurable`
— tighten-eligible, meaning the floor is stale and must be re-cut to carry it.
A floor with no jurisdiction recorded (schema 1) cannot answer the question, so
it keeps failing and says why: unknown is not permission.

The mirror is enforced too, and it is the more dangerous half: an id that left
the list because its element is now WITHHELD did not get fixed, and must not
make a case tighten-eligible. Otherwise a geometry regression buys a lower
floor by making the gate stop looking.

MEASURED, 2026-08-31 — WHY THE GEOMETRY BAND DEFAULTS TO ZERO
--------------------------------------------------------------
The paint band exists because Chrome is not bit-stable against itself. #167
asked whether geometry needs one too, since `settings` reads 280 failures on
master and 281 on develop and this gate compares counts with a strict `>`.

Gate A reads `layout.json` — layout output, not pixels — so the question is
whether one binary produces one layout dump. It does: 26/26 gating cases
captured 3 times on one binary gave **byte-identical** dumps, with a control
confirming a one-byte change is detected. The count is therefore a pure
function of the dump and the committed baselines, and has no run-to-run jitter
to absorb. The 280/281 is an engine delta between two trees, which is the thing
the ratchet is supposed to see.

Caveat stated rather than buried: that run is Linux/SwiftShader. macOS uses
CoreText, and this has not been measured there.

The band therefore ships as a mechanism with a default of 0 — a default of 1
would absorb no jitter and permanently hide one regressed box per case. A floor
may raise `geometry_band` explicitly, with the measurement that justifies it.
"""
from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path
from typing import Any, Dict, List, Optional, Tuple

BASELINE_DEFAULT = Path("trench/ratchet-gates-baseline.json")
# Bumped to 2 on 2026-08-31: rows carry `discrete_withheld` and the floor
# carries `geometry_band`. Nothing reads this field — it is a label on a
# committed floor, not a check. The functional signal for an old floor is
# `discrete_withheld` being absent, which compare() reports by name.
SCHEMA = 2

# Geometry's counterpart to the paint variance band, and it is DELIBERATELY
# ZERO. The band exists as a mechanism because paint has one and the asymmetry
# was raised on #167 (`settings` reads 280 on master and 281 on develop, and
# a strict `>` fails on that). It defaults off because the premise turned out
# to be measurable and was measured: Gate A reads `layout.json`, which is
# layout output and not pixels, so on one binary and one font stack the count
# is deterministic — see MEASURED, 2026-08-31 in the module docstring. A band
# defaulting to 1 would buy nothing on that evidence and would hide exactly one
# real regressed box per case, forever. Raise it in the FLOOR file
# (`geometry_band`), with the measurement that justifies it, not here.
GEOMETRY_BAND_DEFAULT = 0


def _case_map(report: Optional[Dict[str, Any]]) -> Dict[str, Dict[str, Any]]:
    if not report:
        return {}
    return {c.get("case_id"): c for c in report.get("cases", []) if c.get("case_id")}


def discrete_ids(case: Dict[str, Any]) -> List[str]:
    """Stable identity for a discrete failure: kind::selector.

    Production Gate B rows carry discrete failures inside `failures[]`,
    flagged `"discrete": true` (only attributable ones — the geometry
    precondition already ran inside the gate). The percentage-bar entry in
    the same list carries `"discrete": false` and is not an id.
    """
    out = []
    for f in case.get("failures", []) or []:
        if f.get("discrete"):
            out.append(f"{f.get('kind', '?')}::{f.get('selector', '?')}")
    return sorted(out)


def selector_of(discrete_id: str) -> str:
    """The selector half of a `kind::selector` id.

    Split on the FIRST separator only. A selector may itself contain `::`
    (a pseudo-element), and splitting on the last one would silently rename
    the element the ratchet is about to make a decision about.
    """
    _, sep, selector = discrete_id.partition("::")
    return selector if sep else ""


def withheld_selectors(case: Optional[Dict[str, Any]]) -> Optional[List[str]]:
    """Which elements Gate B declined to speak about, or None for UNKNOWN.

    None is not the empty set and must never be coerced into one. A gate run
    that never opened a case, and a gate run that examined every element of
    it, are different facts; a floor that cannot answer the question is the
    first, and answering it "nothing was withheld" turns every newly visible
    defect into a false regression report — or, read the other way, would let
    a real one through.
    """
    if not case:
        return None
    value = case.get("discrete_withheld_selectors")
    if value is None:
        return None
    return sorted(value)


def snapshot(
    gate_a: Optional[Dict[str, Any]],
    gate_b: Optional[Dict[str, Any]],
    max_variance: float,
    geometry_band: int = GEOMETRY_BAND_DEFAULT,
) -> Dict[str, Any]:
    """One ratchet row per case present in either gate report."""
    amap, bmap = _case_map(gate_a), _case_map(gate_b)
    rows: Dict[str, Any] = {}
    for cid in sorted(set(amap) | set(bmap)):
        a, b = amap.get(cid), bmap.get(cid)
        # Production schema (pinned by test_production_schema_excerpt):
        # geometry_failures / join_failures are COUNTS, not lists; the
        # per-case paint fraction is `within_fraction`.
        rows[cid] = {
            "geometry_measured": bool(a and a.get("measured")),
            "geometry_green": bool(a and a.get("green")),
            "geometry_fail_count": int((a or {}).get("geometry_failures") or 0),
            "join_fail_count": int((a or {}).get("join_failures") or 0),
            "paint_measured": bool(b and b.get("measured")),
            "paint_green": bool(b and b.get("green")),
            "paint_pct": float((b or {}).get("within_fraction") or 0.0),
            "discrete_fail_ids": discrete_ids(b or {}),
            # The jurisdiction the discrete detectors had in THIS run. Carried
            # into the floor so a later run can tell a newly BROKEN element
            # from a newly MEASURABLE one — see compare().
            "discrete_withheld": withheld_selectors(b),
        }
    return {
        "schema": SCHEMA,
        "max_variance": max_variance,
        "geometry_band": geometry_band,
        "cases": rows,
    }


def compare(
    baseline: Dict[str, Any], current: Dict[str, Any]
) -> Tuple[List[str], List[str], List[str], List[str], List[str]]:
    """-> (regressions, absolute, tighten_eligible, newly_measurable,
    newly_unmeasurable).

    Every list is human lines except tighten_eligible, which is case ids.
    """
    regressions: List[str] = []
    absolute: List[str] = []
    tighten: List[str] = []
    newly_measurable: List[str] = []
    newly_unmeasurable: List[str] = []
    variance = float(baseline.get("max_variance", 0.1))
    geometry_band = int(baseline.get("geometry_band", GEOMETRY_BAND_DEFAULT))
    base_cases: Dict[str, Any] = baseline.get("cases", {})

    for cid, base in sorted(base_cases.items()):
        cur = current["cases"].get(cid)
        if cur is None or (
            (base.get("geometry_measured") and not cur["geometry_measured"])
            or (base.get("paint_measured") and not cur["paint_measured"])
        ):
            regressions.append(
                f"{cid}: measured in baseline, UNMEASURED now — instrument regression"
            )
            continue

        if cur["geometry_fail_count"] > base.get("geometry_fail_count", 0) + geometry_band:
            regressions.append(
                f"{cid}: geometry failures {base.get('geometry_fail_count', 0)}"
                f" -> {cur['geometry_fail_count']}"
                + (f" (above band {geometry_band})" if geometry_band else "")
            )
        if cur["join_fail_count"] > base.get("join_fail_count", 0) + geometry_band:
            regressions.append(
                f"{cid}: join failures {base.get('join_fail_count', 0)}"
                f" -> {cur['join_fail_count']}"
                + (f" (above band {geometry_band})" if geometry_band else "")
            )
        # A discrete failure the floor does not carry is not automatically a
        # regression. Gate B only speaks about geometrically exact elements, so
        # every geometry fix enlarges its jurisdiction and can surface a defect
        # that was there all along and was being withheld. Measured 2026-08-30:
        # develop's `gradient-backgrounds .linear-6` notch, unmeasurable on
        # 2026-08-12 because the card was 18px out of place, read as a
        # REGRESSION against master's floor while nothing had regressed.
        base_withheld = base.get("discrete_withheld")
        case_newly_measurable = False
        new_ids = sorted(
            set(cur["discrete_fail_ids"]) - set(base.get("discrete_fail_ids", []))
        )
        for nid in new_ids:
            if base_withheld is None:
                # UNKNOWN, so the question cannot be answered FOR the floor.
                # Fail loud and name the remedy: an unanswerable floor is not
                # licence to downgrade a possible regression.
                regressions.append(
                    f"{cid}: NEW discrete failure {nid}"
                    " (floor predates discrete_withheld — re-seed to classify)"
                )
            elif selector_of(nid) in set(base_withheld):
                newly_measurable.append(
                    f"{cid}: {nid} — element was WITHHELD in the baseline run"
                )
                case_newly_measurable = True
            else:
                regressions.append(f"{cid}: NEW discrete failure {nid}")
        floor = base.get("paint_pct", 0.0) - variance
        if cur["paint_pct"] < floor:
            regressions.append(
                f"{cid}: paint {base.get('paint_pct', 0.0):.5f}"
                f" -> {cur['paint_pct']:.5f} (below variance band {variance})"
            )
        for col in ("geometry_green", "paint_green"):
            if base.get(col) and not cur[col]:
                regressions.append(f"{cid}: {col} flipped green -> red")

        red = (not cur["geometry_green"]) or (not cur["paint_green"]) or cur[
            "discrete_fail_ids"
        ]
        if red:
            absolute.append(
                f"{cid}: geo_fails={cur['geometry_fail_count']}"
                f" paint={cur['paint_pct']:.5f} discrete={len(cur['discrete_fail_ids'])}"
            )
        # The mirror of newly_measurable, and it is the dangerous direction.
        # An id can leave the list because the defect was FIXED, or because
        # geometry moved the element out of Gate B's jurisdiction and the
        # detector stopped being allowed to speak about it. Only the first is
        # an improvement. Counting the second would invite a floor commit that
        # bakes in a zero bought by not looking — and the geometry band added
        # above makes that reachable, because a small geometry regression can
        # now hold while it silently withdraws an element.
        cur_withheld = cur.get("discrete_withheld")
        gone = set(base.get("discrete_fail_ids", [])) - set(cur["discrete_fail_ids"])
        if cur_withheld is None:
            # UNKNOWN jurisdiction: no id can be shown to have been fixed.
            fixed, withdrawn = set(), gone
        else:
            withheld_now = set(cur_withheld)
            fixed = {g for g in gone if selector_of(g) not in withheld_now}
            withdrawn = gone - fixed
        for wid in sorted(withdrawn):
            newly_unmeasurable.append(
                f"{cid}: {wid} — gone because the element is now WITHHELD,"
                " not because it was fixed"
            )

        improved = (
            cur["geometry_fail_count"] < base.get("geometry_fail_count", 0)
            or cur["paint_pct"] > base.get("paint_pct", 0.0) + variance
            or bool(fixed and not new_ids)
        )
        # A newly measurable id makes the case tighten-eligible rather than a
        # regression (ratified 2026-08-30). "Tighten" here means the floor is
        # STALE and must be re-cut to carry a defect it could not see, which is
        # the opposite direction from the usual stale-low case — so these are
        # printed in their own block and never folded into the improvement line.
        if improved or case_newly_measurable:
            tighten.append(cid)

    return regressions, absolute, tighten, newly_measurable, newly_unmeasurable


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--gate-a", type=Path, required=True)
    ap.add_argument("--gate-b", type=Path, required=True)
    ap.add_argument("--baseline", type=Path, default=BASELINE_DEFAULT)
    ap.add_argument(
        "--max-variance",
        type=float,
        default=0.1,
        help="paint %% band reused from the stability bar — do not invent a second",
    )
    ap.add_argument(
        "--geometry-band",
        type=int,
        default=GEOMETRY_BAND_DEFAULT,
        help="geometry/join failure-count band written into a seed. Default 0: "
        "Gate A reads layout.json, which is deterministic on one binary, so a "
        "nonzero band hides a real box rather than absorbing jitter. Raise it "
        "only with a measurement behind it.",
    )
    ap.add_argument("--engine-sha", help="engine commit the captures were produced from")
    ap.add_argument("--receipt-run", help="CI run id/url the gate reports came from")
    ap.add_argument("--stability-runs", type=int,
                    help="iterations behind the captures (seed law: >= 3)")
    ap.add_argument(
        "--write-seed",
        type=Path,
        help="write a baseline snapshot from the current reports and exit 0 "
        "(seeding is a separate, manual, baseline-only commit)",
    )
    args = ap.parse_args()

    def load(p: Path) -> Optional[Dict[str, Any]]:
        return json.load(open(p, encoding="utf-8")) if p.exists() else None

    gate_a, gate_b = load(args.gate_a), load(args.gate_b)
    for name, rep, p in (("A", gate_a, args.gate_a), ("B", gate_b, args.gate_b)):
        if rep is None:
            print(f"RATCHET: Gate {name} report missing at {p} — it did not run.")
            print("A gate that did not run cannot be ratcheted. Failing loud.")
            return 1

    current = snapshot(gate_a, gate_b, args.max_variance, args.geometry_band)

    if args.write_seed:
        # Provenance travels WITH the floor (configs are receipts): a reader
        # of the committed baseline must not need archaeology to learn which
        # engine, which run, and how many iterations stand behind it.
        import datetime
        current["provenance"] = {
            "engine_sha": args.engine_sha,
            "receipt_run": args.receipt_run,
            "stability_runs": args.stability_runs,
            "captured_at": datetime.datetime.now(datetime.timezone.utc)
            .isoformat(timespec="seconds"),
        }
        missing = [k for k, v in current["provenance"].items() if v in (None, "")]
        if missing:
            print(f"RATCHET seed REFUSED — provenance missing: {', '.join(missing)}. "
                  "A floor without provenance is a number without a receipt.")
            return 1
        if args.stability_runs is not None and args.stability_runs < 3:
            print(f"RATCHET seed REFUSED — stability_runs={args.stability_runs} < 3 "
                  "(seed law: honest macOS receipt, N>=3).")
            return 1
        args.write_seed.parent.mkdir(parents=True, exist_ok=True)
        with open(args.write_seed, "w", encoding="utf-8") as fh:
            json.dump(current, fh, indent=2, sort_keys=True)
        print(f"RATCHET: seed written to {args.write_seed} "
              f"({len(current['cases'])} cases). Commit it in a baseline-only PR.")
        return 0

    if not args.baseline.exists():
        print(
            f"RATCHET OFF — no baseline committed at {args.baseline}. "
            "This run measured everything and gated nothing; seed with "
            "--write-seed from an honest macOS receipt and commit it."
        )
        return 0

    baseline = json.load(open(args.baseline, encoding="utf-8"))
    regressions, absolute, tighten, newly_measurable, newly_unmeasurable = compare(
        baseline, current
    )

    if newly_measurable:
        print(f"RATCHET newly-measurable ({len(newly_measurable)}):")
        for line in newly_measurable:
            print(f"  {line}")
        print("  These are NOT regressions. Gate B may now speak about an element")
        print("  it withheld when the floor was cut; the defect was always there.")
        print("  Re-cut the floor so it carries them.")
    if newly_unmeasurable:
        print(f"RATCHET newly-UNMEASURABLE ({len(newly_unmeasurable)}):")
        for line in newly_unmeasurable:
            print(f"  {line}")
        print("  These are NOT improvements and do not make a case tighten-eligible.")
    if tighten:
        print(f"RATCHET tighten-eligible ({len(tighten)}): {', '.join(tighten)}")
        print("  (manual baseline-only PR — this script never writes during a run)")
    if regressions:
        print(f"RATCHET REGRESSION ({len(regressions)}):")
        for r in regressions:
            print(f"  {r}")
        return 1
    if absolute:
        print(f"RATCHET holds: {len(absolute)} case(s) absolutely red, none worse "
              "than the committed floor:")
        for a in absolute:
            print(f"  {a}")
        return 2
    print("RATCHET holds: no regression, no absolute red beyond baseline.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
