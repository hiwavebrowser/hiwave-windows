#!/usr/bin/env python3
"""Instrument smoke test — constant-expectation rendering probes.

Chrome-free contracts that lock in paint invariants the page-level parity suite
can mask (it samples at threshold 20). Each probe renders a tiny fixture with
parity-capture and asserts EXACT pixel values that follow from the CSS + the
renderer's colour math — hard to fake with page-specific CSS.

Contracts:
  * sRGB gamma round-trip: a CSS colour must read back as itself (#1a1a2e ->
    (26,26,46)). This is the darks-worst/whites-fine double-encode guard.
  * Gradient endpoints: the two ends of a 2-stop gradient are the stop colours.
  * Gradient midpoint: interpolation matches Chrome's default (gamma sRGB), i.e.
    the raw-channel average of the stops — NOT a linear-light blend.

Exit 0 if every contract holds, 1 otherwise.
"""
import os
import subprocess
import sys
import tempfile

from PIL import Image

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BIN = os.path.join(REPO, "target", "release", "parity-capture.exe")
FIX = os.path.join(REPO, "parity-tests", "instrument")


def render(html_name, w, h):
    html = os.path.join(FIX, html_name)
    out = os.path.join(tempfile.gettempdir(), f"instr_{html_name}.ppm")
    r = subprocess.run(
        [BIN, "--html-file", html, "--width", str(w), "--height", str(h),
         "--dump-frame", out],
        capture_output=True, text=True, cwd=REPO,
    )
    if not os.path.exists(out):
        raise RuntimeError(f"render failed for {html_name}: {r.stdout}{r.stderr}")
    return Image.open(out).convert("RGB")


# (probe label, html, w, h, [(pixel label, (x,y), (r,g,b), tol)])
PROBES = [
    ("gamma-dark #1a1a2e round-trip", "gamma-dark.html", 64, 64, [
        ("interior", (32, 32), (26, 26, 46), 2),
    ]),
    ("gamma-mid #808080 round-trip", "gamma-mid.html", 64, 64, [
        ("interior", (32, 32), (128, 128, 128), 2),
    ]),
    ("gradient endpoints + gamma midpoint", "gradient-h.html", 400, 100, [
        ("left=red", (4, 50), (255, 0, 0), 6),
        ("right=blue", (396, 50), (0, 0, 255), 6),
        # Chrome interpolates legacy gradients in gamma sRGB: midpoint of
        # #ff0000 and #0000ff is the raw-channel average (127,0,127) — a
        # linear-light blend would read ~(188,0,188).
        ("mid=gamma-avg", (200, 50), (127, 0, 127), 8),
    ]),
]


def main():
    if not os.path.exists(BIN):
        print(f"FATAL: parity-capture not built at {BIN}", file=sys.stderr)
        return 2
    failures = 0
    for label, html, w, h, checks in PROBES:
        try:
            img = render(html, w, h)
        except RuntimeError as e:
            print(f"FAIL  {label}: {e}")
            failures += 1
            continue
        ok = True
        details = []
        for plabel, (x, y), expect, tol in checks:
            got = img.getpixel((x, y))
            within = all(abs(got[i] - expect[i]) <= tol for i in range(3))
            if not within:
                ok = False
            details.append(
                f"    {plabel:14s} @({x},{y}) got {got} expect {expect} +-{tol} "
                f"{'ok' if within else 'MISMATCH'}"
            )
        print(f"{'PASS' if ok else 'FAIL'}  {label}")
        for d in details:
            print(d)
        if not ok:
            failures += 1
    print()
    print(f"{len(PROBES) - failures}/{len(PROBES)} probes passed")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
