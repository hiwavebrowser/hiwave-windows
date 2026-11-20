"""Windows chrome-board geometry: originators vs carried, per page (Atlas 1898 item 2).

For every board row (inspector excluded), layout_oracle_gate.compare_case gives the failing
(box, axis) deltas. A failing (box, axis) is CARRIED when its nearest ancestor that both sides
measured has the same axis delta within tolerance (geometry_attribution.py's rule); otherwise it
is an ORIGINATOR. Ancestry comes from Chrome's selector chain ("a > b > c" -> "a > b").

usage: board_causes.py <mac checkout scripts dir> <board_work dir> <out.json>
"""
import json
import sys
from collections import defaultdict
from pathlib import Path

sys.path.insert(0, sys.argv[1])
from layout_oracle_gate import compare_case, join_key, GEOMETRY_TOLERANCE_PX  # noqa: E402

work = Path(sys.argv[2])
out = Path(sys.argv[3])


def parent(sel):
    return sel.rsplit(" > ", 1)[0] if " > " in sel else None


rows = []
for rects in sorted(work.glob("*/*/*/layout-rects.json")):
    d = rects.parent
    page, state, size = d.parts[-3:]
    if page == "inspector":
        continue
    rk = d / "rustkit-layout.json"
    if not rk.exists():
        continue
    chrome = json.load(open(rects, encoding="utf-8"))
    rustkit = json.load(open(rk, encoding="utf-8"))
    rec = compare_case(f"{page}/{state}/{size}", chrome, rustkit)
    deltas = {}
    others = []
    for f in rec["failures"]:
        if f.get("kind") == "delta" and f.get("delta") is not None:
            deltas[(f["selector"], f["axis"])] = f
        else:
            others.append({"selector": f.get("selector"), "kind": f.get("kind")})
    # every measured (selector, axis) delta, failing or not, for the carried test
    # Chrome-measured boxes that are not failing moved within tolerance: delta ~0.
    measured = {}
    for el in chrome.get("elements", []):
        for axis in ("x", "y", "width", "height"):
            measured[(join_key(el.get("selector")), axis)] = 0.0
    findings = []
    for (sel, axis), f in deltas.items():
        anc = parent(sel)
        anc_delta = None
        while anc is not None:
            if (anc, axis) in deltas:
                anc_delta = deltas[(anc, axis)]["delta"]
                break
            if (anc, axis) in measured:
                anc_delta = measured[(anc, axis)]
                break
            anc = parent(anc)
        carried = anc_delta is not None and abs(f["delta"] - anc_delta) <= GEOMETRY_TOLERANCE_PX
        findings.append({"selector": sel, "axis": axis, "expected": f.get("expected"), "actual": f.get("actual"),
                         "delta": round(f["delta"], 2), "ancestor": anc, "ancestor_delta": anc_delta,
                         "role": "carried" if carried else "originator"})
    rows.append({"page": page, "state": state, "size": size, "geometry_failures": rec["geometry_failures"],
                 "join_failures": rec["join_failures"], "other_failures": others,
                 "originators": sum(1 for x in findings if x["role"] == "originator"),
                 "carried": sum(1 for x in findings if x["role"] == "carried"), "findings": findings})

json.dump(rows, open(out, "w", encoding="utf-8"), indent=1)
by_page = defaultdict(lambda: [0, 0, 0, 0])
for r in rows:
    b = by_page[r["page"]]
    b[0] += 1; b[1] += r["geometry_failures"]; b[2] += r["originators"]; b[3] += r["carried"]
print("page rows geometry_failures originators carried")
for p, (n, g, o, c) in sorted(by_page.items()):
    print(p, n, g, o, c)
for r in rows:
    if r["state"] == "default" and r["size"] == "1280x800":
        print(f"\n== {r['page']} default 1280x800: {r['geometry_failures']} diffs, {r['originators']} originators, "
              f"{r['carried']} carried, other {len(r['other_failures'])}")
        for x in sorted((x for x in r["findings"] if x["role"] == "originator"), key=lambda x: -abs(x["delta"]))[:25]:
            print(f"  {x['axis']:6} {x['delta']:+8.2f}  exp {x['expected']}  act {x['actual']}  {x['selector'][-110:]}")
