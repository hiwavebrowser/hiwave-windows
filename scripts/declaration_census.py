#!/usr/bin/env python3
"""declaration_census.py — which CSS declarations in the corpus does the engine
never apply?

Plan: docs/PARITY_FINISH_LINE_PLAN_2026-08-04.md. Baseline and rules:
trench/BASELINE-parity-finish-line.md.

WHY THIS EXISTS
---------------
2026-09-21 found that `parse_length` had never parsed a `calc()` expression:
the declaration was DROPPED, silently, as a missing declaration rather than a
parse error. It cost one 187px box on `chrome_rustkit` and forty nights of
boards never named it. The digest's own note on it was:

    A grep for `calc(` across the corpus took ten seconds and would have
    found it on night 1.

This is that grep, generalised and made repeatable. It answers one question:

    which property names does the corpus author that
    `apply_style_property` has no arm for?

A dropped declaration is invisible to every other instrument the campaign
owns. Gate A sees a box in the wrong place, Gate B sees pixels, Gate C sees a
heatmap — none of them can say "because nothing in the engine reads this
property". This can, before a capture is taken and without a GPU.

WHAT IT DOES NOT CLAIM
----------------------
**Property-level only.** An arm existing is not the same as the value parsing.
`calc()` itself would NOT have been caught by this script: `height` has an arm,
and the loss was inside `parse_length`. A property that is absent here is
certainly dropped; a property that is present may still drop the value forms
the corpus uses. Value-level coverage is a separate instrument and is not
pretended at here.

It also says nothing about whether an applied property is USED by layout or
paint. `float` has no arm (so it is reported); a property that has an arm and
is then ignored downstream reads as handled. Both limits are stated in the
report so no reader has to infer them.

THE LEDGER
----------
`cases/declaration_gaps.json` records the gaps that are known, each with the
reason it is tolerated. The census is a RATCHET over it:

  * a gap not in the ledger  -> FAIL (exit 1). A newly authored property the
    engine drops is a defect arriving silently, which is the whole class this
    file exists to end.
  * a ledger entry the engine now handles -> reported as TIGHTEN-ELIGIBLE and
    exits 0. Closing a gap must never fail the PR that closed it; re-seeding
    the ledger stays a deliberate act.
  * nothing measured (no arms extracted, no corpus cases read) -> FAIL. A run
    that measured nothing and a run that found no gaps look identical in any
    report that prints only a count.

Usage:
    python3 scripts/declaration_census.py
    python3 scripts/declaration_census.py --json out.json
    python3 scripts/declaration_census.py --include-holdout

Exit codes:
    0 = every used property is handled, or its gap is in the ledger
    1 = an unledgered gap, or the run measured nothing
"""

import argparse
import json
import re
import sys
from pathlib import Path
from typing import Any, Dict, Iterable, List, Optional, Set, Tuple

REPO_ROOT = Path(__file__).resolve().parent.parent

ENGINE_SOURCE = REPO_ROOT / "crates" / "rustkit-engine" / "src" / "lib.rs"
LEDGER_PATH = REPO_ROOT / "cases" / "declaration_gaps.json"
REGISTRY_PATH = REPO_ROOT / "cases" / "registry.json"

APPLY_FN = "fn apply_style_property"

# The spine: properties whose arms cannot plausibly be missing from a working
# engine. If the extractor returns a set without these it has not read the
# match statement — a refactor moved it, or the brace walk went wrong — and the
# whole census is measuring nothing. Reported as a refusal, never as 0 gaps.
REQUIRED_ARMS = frozenset(
    {"display", "width", "height", "position", "margin-top", "padding-top", "color"}
)

# Below this the extraction is not believable either. apply_style_property
# carried 129 arms when this was written.
MIN_PLAUSIBLE_ARMS = 50

# Custom properties are not match arms and never will be: they are resolved by
# the `var()` pass before a declaration reaches apply_style_property. Reporting
# `--accent` as a dropped declaration would be a false gap in every themed page
# in the corpus. The pass's existence is asserted rather than assumed — see
# `custom_property_pass_exists`.
CUSTOM_PROPERTY_PREFIX = "--"

# ---------------------------------------------------------------------------
# Bucket taxonomy and ownership (Package Z2-M2)
# ---------------------------------------------------------------------------

BUCKETS: Dict[str, Dict[str, Any]] = {
    "interactive": {
        "title": "Interactive & Behavioral (Static No-Op)",
        "owner": "engine",
        "description": "User interaction, cursor, and selection affordances with 0 static render effect",
        "properties": {"cursor", "pointer-events", "user-select"},
    },
    "paint": {
        "title": "Visual Paint & Compositing (Non-Flow Paint)",
        "owner": "renderer",
        "description": "Visual decoration, shadows, filters, and outlines that do not affect layout flow",
        "properties": {"outline", "outline-offset", "text-shadow", "backdrop-filter", "accent-color", "resize"},
    },
    "structure": {
        "title": "Structure, Markers & Shaping",
        "owner": "layout",
        "description": "List markers, font variant glyph shaping, multi-column and scrollbars",
        "properties": {"list-style", "font-variant", "scrollbar-width", "scrollbar-color", "column-count", "float"},
    },
}


def bucket_for_property(name: str) -> Tuple[str, Dict[str, Any]]:
    """Determine the semantic bucket and owner for a CSS property name."""
    name_clean = name.strip().lower()
    for b_id, b_meta in BUCKETS.items():
        if name_clean in b_meta["properties"]:
            return b_id, b_meta
    # Heuristic classifications for newly authored properties
    if any(k in name_clean for k in ("hover", "focus", "pointer", "cursor", "touch", "user-")):
        return "interactive", BUCKETS["interactive"]
    if any(k in name_clean for k in ("shadow", "filter", "outline", "color", "paint", "mask", "clip")):
        return "paint", BUCKETS["paint"]
    return "structure", BUCKETS["structure"]


class CensusRefusal(Exception):
    """The run could not honestly measure. Never reported as a clean board."""



# ---------------------------------------------------------------------------
# The engine side: which property names have an arm
# ---------------------------------------------------------------------------


def extract_fn_body(source: str, signature: str) -> str:
    """The body of the first function whose line contains `signature`.

    Brace-walked rather than regexed: the match arms contain both braces and
    string literals with braces in them, and a regex that stops at the first
    `}` reads about four arms and calls it a function.
    """
    start = source.find(signature)
    if start == -1:
        raise CensusRefusal(f"{signature} not found in the engine source")
    depth = 0
    started = False
    for index in range(start, len(source)):
        char = source[index]
        if char == "{":
            depth += 1
            started = True
        elif char == "}":
            depth -= 1
            if started and depth == 0:
                return source[start : index + 1]
    raise CensusRefusal(f"{signature} never closes — brace walk ran off the end")


def handled_properties(source: str) -> Set[str]:
    """Property names `apply_style_property` has a match arm for.

    An arm is one or more string literals separated by `|` and followed by
    `=>`; the nested matches inside an arm body (on VALUES: "flex", "none",
    "center") are excluded by requiring the arm to sit at the match's own
    indentation, which is 8 to 12 columns in this file.
    """
    body = extract_fn_body(source, APPLY_FN)
    arms = re.findall(r'\n\s{8,12}((?:"[A-Za-z-]+"\s*\|\s*)*"[A-Za-z-]+")\s*=>', body)
    props: Set[str] = set()
    for arm in arms:
        props.update(name.lower() for name in re.findall(r'"([A-Za-z-]+)"', arm))
    return props


def custom_property_pass_exists(source: str) -> bool:
    """Is there a pass that resolves `--x` / `var(--x)` before application?

    The census excludes custom properties from the gap list. That exclusion is
    only honest while the pass it assumes is really there, so it is checked
    rather than believed.
    """
    return 'starts_with("--")' in source and "var(" in source


# ---------------------------------------------------------------------------
# The corpus side: which property names are authored
# ---------------------------------------------------------------------------

COMMENT_RE = re.compile(r"/\*.*?\*/", re.S)
STYLE_BLOCK_RE = re.compile(r"<style[^>]*>(.*?)</style>", re.S | re.I)
INLINE_STYLE_RE = re.compile(r"""\sstyle\s*=\s*"([^"]*)\"""", re.I)
RULE_BODY_RE = re.compile(r"\{([^{}]*)\}")
PROPERTY_RE = re.compile(r"^-{0,2}[A-Za-z][A-Za-z0-9-]*$")


def declarations_in_html(html: str) -> List[str]:
    """Every property name authored in a case, lowercased, with duplicates.

    Both sources count: `<style>` blocks and `style="…"` attributes. Comments
    are stripped first — a property named only inside `/* … */` is not
    authored, and counting it would invent a gap the page does not have.
    """
    text = COMMENT_RE.sub(" ", html)
    css_chunks = list(STYLE_BLOCK_RE.findall(text))
    # An inline style attribute is a declaration list with no braces around it.
    css_chunks.extend("{" + value + "}" for value in INLINE_STYLE_RE.findall(text))

    names: List[str] = []
    for chunk in css_chunks:
        for rule_body in RULE_BODY_RE.findall(chunk):
            for declaration in rule_body.split(";"):
                if ":" not in declaration:
                    continue
                name = declaration.split(":", 1)[0].strip().lower()
                if PROPERTY_RE.match(name):
                    names.append(name)
    return names


def load_registry(include_holdout: bool) -> List[Tuple[str, Dict[str, Any]]]:
    with open(REGISTRY_PATH, encoding="utf-8") as handle:
        cases = json.load(handle)["cases"]
    return [
        (case_id, case)
        for case_id, case in sorted(cases.items())
        if include_holdout or case["scope"] != "holdout"
    ]


# ---------------------------------------------------------------------------
# The ledger
# ---------------------------------------------------------------------------


def load_ledger() -> Dict[str, str]:
    if not LEDGER_PATH.exists():
        return {}
    with open(LEDGER_PATH, encoding="utf-8") as handle:
        return json.load(handle)["known_gaps"]


# ---------------------------------------------------------------------------
# Census
# ---------------------------------------------------------------------------


def run_census(
    engine_source: str,
    cases: Iterable[Tuple[str, Dict[str, Any]]],
    ledger: Dict[str, str],
    read_case: Optional[Any] = None,
) -> Dict[str, Any]:
    """Score one tree. Raises CensusRefusal where it cannot measure."""
    handled = handled_properties(engine_source)
    if len(handled) < MIN_PLAUSIBLE_ARMS:
        raise CensusRefusal(
            f"only {len(handled)} match arms extracted "
            f"(< {MIN_PLAUSIBLE_ARMS}) — the extractor is not reading the match"
        )
    missing_spine = sorted(REQUIRED_ARMS - handled)
    if missing_spine:
        raise CensusRefusal(
            "the extracted arms are missing properties no engine can be without: "
            + ", ".join(missing_spine)
        )
    if not custom_property_pass_exists(engine_source):
        raise CensusRefusal(
            "no custom-property/var() pass found — the census excludes `--x` "
            "declarations on the assumption that one resolves them"
        )

    reader = read_case or (
        lambda case: (REPO_ROOT / case["html"]).read_text(
            encoding="utf-8", errors="replace"
        )
    )

    used: Dict[str, Dict[str, Any]] = {}
    measured_cases: List[str] = []
    unreadable: List[str] = []
    for case_id, case in cases:
        try:
            html = reader(case)
        except OSError:
            unreadable.append(case_id)
            continue
        measured_cases.append(case_id)
        for name in declarations_in_html(html):
            if name.startswith(CUSTOM_PROPERTY_PREFIX):
                continue
            entry = used.setdefault(name, {"declarations": 0, "cases": set()})
            entry["declarations"] += 1
            entry["cases"].add(case_id)

    if not measured_cases:
        raise CensusRefusal("no corpus case was read — this receipt measured nothing")
    if not used:
        raise CensusRefusal(
            f"{len(measured_cases)} cases read and not one declaration found — "
            "the HTML parse is measuring nothing"
        )

    gaps = []
    for name in sorted(used):
        if name in handled:
            continue
        gaps.append(
            {
                "property": name,
                "declarations": used[name]["declarations"],
                "cases": sorted(used[name]["cases"]),
                "ledgered": name in ledger,
                "note": ledger.get(name, ""),
            }
        )

    used_names = set(used)
    tighten_eligible = sorted(
        name for name in ledger if name in handled or name not in used_names
    )

    # Bucket grouping and ranking (Package Z2-M2)
    bucket_map: Dict[str, Dict[str, Any]] = {
        b_id: {
            "key": b_id,
            "title": b_meta["title"],
            "owner": b_meta["owner"],
            "description": b_meta["description"],
            "declarations": 0,
            "cases": set(),
            "properties": [],
        }
        for b_id, b_meta in BUCKETS.items()
    }

    for gap in gaps:
        b_id, b_meta = bucket_for_property(gap["property"])
        b_entry = bucket_map.setdefault(
            b_id,
            {
                "key": b_id,
                "title": b_meta["title"],
                "owner": b_meta["owner"],
                "description": b_meta["description"],
                "declarations": 0,
                "cases": set(),
                "properties": [],
            },
        )
        b_entry["declarations"] += gap["declarations"]
        b_entry["cases"].update(gap["cases"])
        gap_with_bucket = dict(gap, bucket=b_id, owner=b_meta["owner"])
        b_entry["properties"].append(gap_with_bucket)

    ranked_buckets = sorted(bucket_map.values(), key=lambda b: (-b["declarations"], b["key"]))
    for b in ranked_buckets:
        b["case_count"] = len(b["cases"])
        b["cases"] = sorted(b["cases"])
        b["property_count"] = len(b["properties"])

    top_three_buckets = ranked_buckets[:3]

    return {
        "engine_arms": len(handled),
        "cases_measured": measured_cases,
        "cases_unreadable": unreadable,
        "properties_used": len(used),
        "gaps": gaps,
        "unledgered_gaps": [gap["property"] for gap in gaps if not gap["ledgered"]],
        "tighten_eligible": tighten_eligible,
        "buckets": ranked_buckets,
        "top_three_buckets": top_three_buckets,
        "limits": [
            "property-level only: an arm existing does not mean the value parses "
            "(calc() lived inside `height`, which has an arm)",
            "an arm that applies a property layout or paint then ignores reads as "
            "handled",
        ],
    }


def census_passes(report: Dict[str, Any]) -> bool:
    return not report["unledgered_gaps"]


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------


def print_report(report: Dict[str, Any], bounded: bool = False) -> None:
    print(
        f"Declaration census — {report['engine_arms']} engine arms, "
        f"{report['properties_used']} properties authored across "
        f"{len(report['cases_measured'])} cases"
    )
    if report["cases_unreadable"]:
        print("  UNREADABLE: " + ", ".join(report["cases_unreadable"]))
    print()

    # Top Three Buckets with Owners (Package Z2-M2)
    top_buckets = report.get("top_three_buckets", [])
    if top_buckets:
        print("  TOP THREE BUCKETS WITH OWNERS (Package Z2-M2):")
        for i, b in enumerate(top_buckets, 1):
            props_summary = ", ".join(
                f"{p['property']} ({p['declarations']})" for p in b["properties"]
            )
            print(
                f"    {i}. {b['title']} — owner: {b['owner']}\n"
                f"       decls={b['declarations']:2d}, cases={b['case_count']:2d}, properties={b['property_count']}: {props_summary}"
            )
        print()

    if not bounded:
        if not report["gaps"]:
            print("  no dropped declarations in the corpus")
        for gap in report["gaps"]:
            mark = "ledgered" if gap["ledgered"] else "NEW GAP"
            _, b_meta = bucket_for_property(gap["property"])
            print(
                f"  {mark:8s} {gap['property']:20s} [{b_meta['owner']:8s}] "
                f"decls={gap['declarations']:4d} cases={len(gap['cases']):2d} "
                f"{', '.join(gap['cases'][:4])}"
            )
            if gap["note"]:
                print(f"           {gap['note']}")
        if report["tighten_eligible"]:
            print()
            print("  TIGHTEN-ELIGIBLE (ledger entries this tree no longer needs):")
            for name in report["tighten_eligible"]:
                print(f"    {name}")
        print()
    for limit in report["limits"]:
        print(f"  limit: {limit}")


def main(argv: Optional[List[str]] = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--json", help="write the full report here")
    parser.add_argument(
        "--include-holdout",
        action="store_true",
        help="also census the non-gating holdout cases",
    )
    parser.add_argument(
        "--bounded",
        action="store_true",
        help="bound output strictly to the top three buckets with owners (Z2-M2)",
    )
    args = parser.parse_args(argv)

    try:
        report = run_census(
            ENGINE_SOURCE.read_text(encoding="utf-8"),
            load_registry(args.include_holdout),
            load_ledger(),
        )
    except CensusRefusal as refusal:
        print(f"Declaration census REFUSED: {refusal}", file=sys.stderr)
        print("This is not a pass. Nothing was measured.", file=sys.stderr)
        return 1

    print_report(report, bounded=args.bounded)

    if args.json:
        serialisable = dict(report)
        with open(args.json, "w", encoding="utf-8") as handle:
            json.dump(serialisable, handle, indent=2)
        print(f"\nReport written to {args.json}")

    if not census_passes(report):
        print(
            "\nDeclaration census: FAIL — "
            + ", ".join(report["unledgered_gaps"])
            + "\nEither the engine should apply it, or "
            "cases/declaration_gaps.json should say why it does not.",
        )
        return 1
    print("\nDeclaration census: PASS")
    return 0


if __name__ == "__main__":
    sys.exit(main())
