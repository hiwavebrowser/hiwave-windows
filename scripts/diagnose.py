#!/usr/bin/env python3
"""diagnose.py <url> [--binary PATH] [--live-ms N] [--shield] [--keep DIR]

One command for "why does this page look wrong" (dev-tools T4): a headless
live capture with the event log on, then the page report: the most likely
causes first, facts below. Prints the text report; --keep DIR also keeps
report.json and events.jsonl there.

The report holds no page text and no query strings, but covering-box
selectors carry the page's id and class names.

It does not build: pass --binary, or build parity-capture first
(target/parity, else target/release).
"""
import argparse
import pathlib
import subprocess
import sys
import tempfile

REPO_ROOT = pathlib.Path(__file__).resolve().parent.parent


def find_binary():
    for profile in ("parity", "release"):
        path = REPO_ROOT / "target" / profile / "parity-capture"
        if path.exists():
            return path
    sys.exit("parity-capture is not built: cargo build --profile parity -p parity-capture")


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("url")
    ap.add_argument("--binary", type=pathlib.Path)
    ap.add_argument("--live-ms", type=int, default=25000)
    ap.add_argument("--shield", action="store_true", help="install the shield (needs a build with --shield)")
    ap.add_argument("--keep", type=pathlib.Path, help="keep report.json and events.jsonl in this directory")
    args = ap.parse_args()

    binary = args.binary or find_binary()
    if args.keep:
        args.keep.mkdir(parents=True, exist_ok=True)
        return diagnose(args, binary, args.keep)
    # The capture's files go with the temporary directory.
    with tempfile.TemporaryDirectory(prefix="diagnose-") as out:
        return diagnose(args, binary, pathlib.Path(out))


def diagnose(args, binary, out):
    cmd = [
        str(binary),
        "--url", args.url,
        "--live-ms", str(args.live_ms),
        "--timeout-ms", str(args.live_ms + 95000),
        "--event-log", str(out / "events.jsonl"),
        "--report", str(out / "report.json"),
        "--report-text", str(out / "report.txt"),
    ]
    if args.shield:
        cmd.append("--shield")
    run = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, encoding="utf-8")
    text = out / "report.txt"
    if not text.exists():
        # The capture says why on stderr.
        sys.exit(f"no report (exit {run.returncode})\nstdout: {run.stdout[-500:]}\nstderr: {run.stderr[-1500:]}")
    print(text.read_text(encoding="utf-8"), end="")
    if args.keep:
        print(f"\nkept: {out}/report.json, {out}/events.jsonl")
    return 0


if __name__ == "__main__":
    sys.exit(main())
