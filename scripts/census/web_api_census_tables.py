#!/usr/bin/env python3
"""Render the web API census JSON (from the rustkit-bindings web_api_census
test) as Markdown: totals and one table per area. Usage:
    python3 scripts/census/web_api_census_tables.py docs/census/web_api_census.json
"""
import json
import sys
from collections import Counter, OrderedDict

LABEL = {"works": "works", "broken": "BROKEN", "missing": "MISSING"}


def main(path):
    rows = json.load(open(path, encoding="utf-8"))
    total = Counter(r["status"] for r in rows)
    shallow = sum(1 for r in rows if r["shallow"])
    shallow_works = sum(1 for r in rows if r["shallow"] and r["status"] == "works")
    areas = OrderedDict()
    for r in rows:
        areas.setdefault(r["area"], []).append(r)

    out = []
    out.append("## Totals\n")
    out.append(f"- APIs probed: **{len(rows)}**")
    for s in ("works", "broken", "missing"):
        out.append(f"- {s}: **{total.get(s, 0)}** ({100 * total.get(s, 0) / len(rows):.1f}%)")
    out.append(f"- of which presence-only smoke checks (marked `*`): {shallow} ({shallow_works} counted as works)\n")
    out.append("| Area | Probed | Works | Broken | Missing |")
    out.append("|---|---:|---:|---:|---:|")
    for area, rs in areas.items():
        c = Counter(r["status"] for r in rs)
        out.append(f"| {area} | {len(rs)} | {c.get('works', 0)} | {c.get('broken', 0)} | {c.get('missing', 0)} |")
    out.append(f"| **All** | **{len(rows)}** | **{total.get('works', 0)}** | **{total.get('broken', 0)}** | **{total.get('missing', 0)}** |\n")

    out.append("## Per-area results\n")
    for area, rs in areas.items():
        c = Counter(r["status"] for r in rs)
        out.append(f"### {area} ({c.get('works', 0)}/{len(rs)} work)\n")
        out.append("| API | Status | Detail |")
        out.append("|---|---|---|")
        for r in rs:
            name = r["name"].replace("|", "\\|") + (" `*`" if r["shallow"] else "")
            detail = r["detail"].replace("|", "\\|")
            out.append(f"| {name} | {LABEL[r['status']]} | {detail} |")
        out.append("")
    print("\n".join(out))


if __name__ == "__main__":
    main(sys.argv[1] if len(sys.argv) > 1 else "docs/census/web_api_census.json")
