#!/usr/bin/env python3
"""wpt_coverage.py: one command from a fresh clone to the per-area WPT table.

Syncs the manifest's paths at the pin, builds parity-capture on the parity
profile, runs the runner. The table is the runner's own output, and
trench/wpt/last-run.json carries it under `areas`.

    python3 scripts/wpt_coverage.py [--no-sync] [--no-build] [runner args]

--no-sync and --no-build are for a seat that has done those steps its own way.
"""

import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent


def steps(args):
    no_sync = "--no-sync" in args
    no_build = "--no-build" in args
    rest = [a for a in args if a not in ("--no-sync", "--no-build")]
    planned = []
    if not no_sync:
        planned.append(["bash", "scripts/wpt_sync.sh"])
    if not no_build:
        planned.append(["cargo", "build", "--profile", "parity", "-p", "parity-capture"])
    planned.append([sys.executable, "scripts/wpt_tier1.py", *rest])
    return planned


def main():
    for cmd in steps(sys.argv[1:]):
        code = subprocess.run(cmd, cwd=REPO).returncode
        if code != 0:
            return code
    return 0


if __name__ == "__main__":
    sys.exit(main())
