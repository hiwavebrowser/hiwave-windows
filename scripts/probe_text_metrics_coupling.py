#!/usr/bin/env python3
"""Two-model A/B for the `line-height: normal` epic.

The flat model (`font_size * 1.2`) and the font-metrics model (Blink's
`round(ascent)+round(descent)+line_gap`) produce box trees with IDENTICAL
STRUCTURE -- same DOM, same box builder, only sizes differ. So the two RustKit
dumps join exactly on tree path, with no selector export and no fuzzy geometry.

Chrome's committed rects (baselines/chrome-148/*/*/layout-rects.json) are the
arbiter: for each box that MOVED between models, we ask whether it moved toward
or away from Chrome. Chrome is selector-keyed and RustKit is not, so the Chrome
side is joined by geometry (x/width, then nearest y) and every join reports its
confidence -- an unjoined mover is reported as unjoined, never silently dropped.

Usage:
  probe_text_metrics_coupling.py capture --out DIR [--case C ...]
  probe_text_metrics_coupling.py compare --flat DIR --metrics DIR [--case C ...]
"""
import argparse
import json
import os
import subprocess
import sys

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
CAPTURE_BIN = os.path.join(REPO, "target", "release", "parity-capture")
REGISTRY = os.path.join(REPO, "cases", "registry.json")
CHROME = os.path.join(REPO, "baselines", "chrome-148")
DEFAULT_CASES = ["image-gallery", "about", "css-selectors"]


def registry():
    return json.load(open(REGISTRY, encoding="utf-8"))["cases"]


def capture(cases, out_dir):
    reg = registry()
    os.makedirs(out_dir, exist_ok=True)
    for case in cases:
        spec = reg[case]
        dump = os.path.join(out_dir, f"{case}.layout.json")
        cmd = [
            CAPTURE_BIN,
            "--html-file", os.path.join(REPO, spec["html"]),
            "--width", str(spec["width"]),
            "--height", str(spec["height"]),
            "--dump-layout", dump,
        ]
        proc = subprocess.run(cmd, capture_output=True, text=True, cwd=REPO, encoding="utf-8")
        status = "ok" if proc.returncode == 0 else "FAILED"
        print(f"  {case:<16} {status}  -> {dump}")
        if proc.returncode != 0:
            print(proc.stdout[-800:], proc.stderr[-800:], file=sys.stderr)


def flatten(node, path=(), out=None):
    """Depth-first walk keyed by structural path. Text boxes carry their text."""
    if out is None:
        out = []
    kind = node.get("type", "?")
    rect = node.get("border_box") or node.get("rect") or {}
    out.append(
        {
            "path": path,
            "type": kind,
            "text": node.get("text", ""),
            "x": rect.get("x", 0.0),
            "y": rect.get("y", 0.0),
            "w": rect.get("width", 0.0),
            "h": rect.get("height", 0.0),
        }
    )
    for i, child in enumerate(node.get("children", [])):
        flatten(child, path + (i,), out)
    return out


def load_rk(path):
    return flatten(json.load(open(path, encoding="utf-8"))["root"])


def load_chrome(case):
    scope = registry()[case]["scope"]
    p = os.path.join(CHROME, scope, case, "layout-rects.json")
    if not os.path.exists(p):
        return []
    return [
        {"selector": e["selector"], "tag": e.get("tag", ""), **e["rect"]}
        for e in json.load(open(p, encoding="utf-8"))["elements"]
    ]


def match_chrome(box, chrome, tol=2.0):
    """Nearest Chrome element by (x, width) then y. Returns (element, confidence)."""
    best, best_cost = None, None
    for c in chrome:
        if abs(c["x"] - box["x"]) > 24 or abs(c["width"] - box["w"]) > 24:
            continue
        cost = abs(c["x"] - box["x"]) + abs(c["width"] - box["w"]) + 0.25 * abs(c["y"] - box["y"])
        if best_cost is None or cost < best_cost:
            best, best_cost = c, cost
    if best is None:
        return None, "unjoined"
    conf = "exact" if best_cost <= tol else ("near" if best_cost <= 24 else "loose")
    return best, conf


def compare(cases, flat_dir, metrics_dir, top=20):
    for case in cases:
        fa = load_rk(os.path.join(flat_dir, f"{case}.layout.json"))
        me = load_rk(os.path.join(metrics_dir, f"{case}.layout.json"))
        chrome = load_chrome(case)

        print(f"\n=== {case} ===")
        if len(fa) != len(me):
            print(f"  !! box count differs: flat {len(fa)} vs metrics {len(me)} "
                  "-- the models changed TREE STRUCTURE (wrapping), not just sizes.")
        by_path = {b["path"]: b for b in me}

        movers = []
        for b in fa:
            m = by_path.get(b["path"])
            if not m:
                continue
            dy, dh = m["y"] - b["y"], m["h"] - b["h"]
            if abs(dy) < 0.01 and abs(dh) < 0.01:
                continue
            c, conf = match_chrome(b, chrome)
            if c:
                improve_y = abs(b["y"] - c["y"]) - abs(m["y"] - c["y"])
                improve_h = abs(b["h"] - c["height"]) - abs(m["h"] - c["height"])
            else:
                improve_y = improve_h = None
            movers.append({**b, "dy": dy, "dh": dh, "chrome": c, "conf": conf,
                           "iy": improve_y, "ih": improve_h})

        movers.sort(key=lambda m: -(abs(m["dy"]) + abs(m["dh"])))
        worse = [m for m in movers if m["iy"] is not None and (m["iy"] + m["ih"]) < -0.5]

        print(f"  {len(movers)} boxes moved between models; "
              f"{len(worse)} moved AWAY from Chrome (sum of |dy|,|dh| improvements < -0.5px)")
        hdr = (f"  {'path':<18} {'type':<12} {'dy':>7} {'dh':>7} "
               f"{'->Chrome y':>10} {'->Chrome h':>10} {'join':<8} text")
        print(hdr)
        print("  " + "-" * (len(hdr) - 2))
        for m in movers[:top]:
            path = ".".join(str(p) for p in m["path"][-4:])
            iy = f"{m['iy']:+.2f}" if m["iy"] is not None else "  --"
            ih = f"{m['ih']:+.2f}" if m["ih"] is not None else "  --"
            sel = (m["chrome"]["selector"].split(" > ")[-1] if m["chrome"] else "")
            label = (m["text"][:28] or sel)
            print(f"  {path:<18} {m['type']:<12} {m['dy']:>+7.2f} {m['dh']:>+7.2f} "
                  f"{iy:>10} {ih:>10} {m['conf']:<8} {label}")

        if worse:
            print("\n  WORSE UNDER METRICS (the coupling -- these hold the wrong constant):")
            for m in sorted(worse, key=lambda m: m["iy"] + m["ih"])[:top]:
                sel = m["chrome"]["selector"] if m["chrome"] else "?"
                print(f"    {sel}  dy={m['dy']:+.2f} dh={m['dh']:+.2f} "
                      f"net={m['iy'] + m['ih']:+.2f} ({m['conf']}) {m['text'][:24]}")


def main():
    ap = argparse.ArgumentParser()
    sub = ap.add_subparsers(dest="cmd", required=True)
    c = sub.add_parser("capture")
    c.add_argument("--out", required=True)
    c.add_argument("--case", action="append")
    d = sub.add_parser("compare")
    d.add_argument("--flat", required=True)
    d.add_argument("--metrics", required=True)
    d.add_argument("--case", action="append")
    d.add_argument("--top", type=int, default=20)
    a = ap.parse_args()

    cases = a.case or DEFAULT_CASES
    if a.cmd == "capture":
        capture(cases, a.out)
    else:
        compare(cases, a.flat, a.metrics, a.top)


if __name__ == "__main__":
    main()
