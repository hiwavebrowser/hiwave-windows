#!/usr/bin/env python3
"""Derive Chrome's resolved `line-height: normal` from the committed baselines.

Chrome's getComputedStyle reports `normal` literally, so the resolved value is
only observable through geometry: for a leaf block box with one line of text and
no vertical padding/border, the border-box height IS the line box height.

This is the A/B instrument for the text-metrics lane. It needs no live Chrome --
baselines/chrome-148/*/*/{computed-styles,layout-rects}.json already carry
Chrome's per-element truth for all 32 cases.

Usage: python3 scripts/probe_normal_lineheight.py
"""
import glob
import json
import os
import statistics
from collections import defaultdict

BASE = os.path.join(os.path.dirname(__file__), "..", "baselines", "chrome-148")

# RustKit's current model: rustkit-css LineHeight::Normal => font_size * 1.2
RUSTKIT_NORMAL_RATIO = 1.2


def px(styles, prop):
    val = styles.get(prop, "0px")
    try:
        return float(val[:-2])
    except ValueError:
        return 0.0


def leaf_selectors(selectors):
    """A selector is a leaf if no other selector is a strict descendant of it."""
    leaf = dict.fromkeys(selectors, True)
    for s in selectors:
        prefix = s + " > "
        for t in selectors:
            if t is not s and t.startswith(prefix):
                leaf[s] = False
                break
    return leaf


def collect():
    samples = defaultdict(list)
    for style_path in sorted(glob.glob(os.path.join(BASE, "*", "*", "computed-styles.json"))):
        rect_path = os.path.join(os.path.dirname(style_path), "layout-rects.json")
        if not os.path.exists(rect_path):
            continue
        case = os.path.basename(os.path.dirname(style_path))
        elements = json.load(open(style_path, encoding="utf-8"))["elements"]
        rects = {e["selector"]: e["rect"] for e in json.load(open(rect_path, encoding="utf-8"))["elements"]}
        leaf = leaf_selectors([e["selector"] for e in elements])

        for el in elements:
            styles, sel = el["styles"], el["selector"]
            if styles.get("line-height") != "normal" or not leaf.get(sel):
                continue
            if styles.get("display") not in ("block", "list-item"):
                continue
            rect = rects.get(sel)
            if not rect:
                continue
            # Vertical padding/border would make height != line box height.
            if any(
                px(styles, p)
                for p in (
                    "padding-top",
                    "padding-bottom",
                    "border-top-width",
                    "border-bottom-width",
                )
            ):
                continue
            font_size, height = px(styles, "font-size"), rect["height"]
            if font_size <= 0 or height <= 0:
                continue
            # Ratios outside this band mean the box is not a single line box.
            if not (1.0 <= height / font_size <= 1.45):
                continue
            family = (styles.get("font-family") or "").split(",")[0].strip().strip('"')
            samples[(round(font_size, 2), family)].append((height, case))
    return samples


def main():
    samples = collect()
    print("Chrome `line-height: normal`, derived from committed rects")
    print("(leaf block boxes, single line, no vertical padding/border)\n")
    header = f"{'font-size':>9} {'family':<15} {'n':>3} {'chrome':>7} {'ratio':>7} {'rustkit':>8} {'error':>7}"
    print(header)
    print("-" * len(header))

    worst = 0.0
    for (font_size, family), vals in sorted(samples.items(), key=lambda kv: -len(kv[1])):
        heights = [h for h, _ in vals]
        chrome = statistics.median(heights)
        rustkit = RUSTKIT_NORMAL_RATIO * font_size
        error = rustkit - chrome
        worst = max(worst, abs(error))
        spread = "" if max(heights) - min(heights) < 0.51 else "  (SPREAD)"
        print(
            f"{font_size:>9} {family[:15]:<15} {len(vals):>3} {chrome:>7.2f} "
            f"{chrome / font_size:>7.4f} {rustkit:>8.2f} {error:>+7.2f}{spread}"
        )

    print(f"\nWorst per-line error under the flat-1.2 model: {worst:.2f}px")
    print("Every one of these errors compounds down the page: N lines => N x error of drift.")


if __name__ == "__main__":
    main()
