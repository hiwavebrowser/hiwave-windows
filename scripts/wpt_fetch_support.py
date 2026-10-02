#!/usr/bin/env python3
"""wpt_fetch_support.py — materialise MANIFEST.support_paths into third_party/wpt/.

wpt_sync.sh does this as part of its sparse checkout, but this seat's Bash
allowlist has no `git clone`, so the working tree here is produced by direct
fetch at the pinned SHA. Same pin, same destination, same --check contract.

    python3 scripts/wpt_fetch_support.py [--check]
"""

import argparse
import json
import sys
import urllib.request
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
MANIFEST_PATH = REPO_ROOT / "trench" / "wpt" / "MANIFEST.json"
DEST = REPO_ROOT / "third_party" / "wpt"
RAW = "https://raw.githubusercontent.com/web-platform-tests/wpt"


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--check", action="store_true", help="verify presence only; no network")
    args = ap.parse_args()

    manifest = json.loads(MANIFEST_PATH.read_text(encoding="utf-8"))
    pin = manifest["wpt_pin"]
    paths = manifest.get("support_paths", {}).get("paths", [])
    if not paths:
        print("no support_paths in manifest — nothing to do")
        return 0

    missing = 0
    for rel in paths:
        dst = DEST / rel
        if args.check:
            if not dst.is_file():
                print(f"MISSING {rel}")
                missing += 1
            continue
        dst.parent.mkdir(parents=True, exist_ok=True)
        url = f"{RAW}/{pin}/{rel}"
        try:
            data = urllib.request.urlopen(url, timeout=30).read()
        except Exception as exc:  # noqa: BLE001 — the reason belongs in the output
            print(f"FETCH FAILED {rel}: {exc}", file=sys.stderr)
            missing += 1
            continue
        dst.write_bytes(data)
        print(f"  {rel}  {len(data)} bytes")

    if missing:
        print(f"FAIL: {missing}/{len(paths)} support paths absent", file=sys.stderr)
        return 1
    print(f"OK: {len(paths)} support paths present at {DEST}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
