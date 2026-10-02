#!/usr/bin/env python3
"""Run parity-capture on an ad-hoc HTML file and print the layout tree.

Usage: python3 scripts/run_repro_capture.py <html-file> [width] [height]
Writes frame.ppm + layout.json next to the HTML file, then prints the
layout tree via dump_layout_tree.
"""
import subprocess
import sys
from pathlib import Path
import os  # Windows sync: .exe suffix

REPO_ROOT = Path(__file__).parent.parent


def main():
    html = Path(sys.argv[1]).resolve()
    width = sys.argv[2] if len(sys.argv) > 2 else "1280"
    height = sys.argv[3] if len(sys.argv) > 3 else "800"
    layout = html.with_suffix(".layout.json")
    frame = html.with_suffix(".frame.ppm")

    cmd = [
        str(REPO_ROOT / "target" / "release" / ("parity-capture.exe" if os.name == "nt" else "parity-capture")),
        "--html-file", str(html),
        "--width", width,
        "--height", height,
        "--dump-frame", str(frame),
        "--dump-layout", str(layout),
    ]
    r = subprocess.run(cmd, capture_output=True, text=True, timeout=60, cwd=REPO_ROOT, encoding="utf-8")
    if r.returncode != 0:
        print("CAPTURE FAILED:", r.stderr[:500])
        sys.exit(1)

    dump = subprocess.run(
        [sys.executable, str(REPO_ROOT / "scripts" / "dump_layout_tree.py"), str(layout), "6"],
        capture_output=True, text=True, encoding="utf-8"
    )
    print(dump.stdout)


if __name__ == "__main__":
    main()
