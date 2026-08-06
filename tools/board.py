"""Emit the repo's state block, measured at the moment of running.

WHY THIS EXISTS
---------------
A state line is a measurement, not a memory. I claimed "zero open PRs from this
seat" in four consecutive broadcasts without once checking the author field --
#33 was mine the whole time. Another seat then copied that line into a board
summary, over the top of his own correct `gh pr list` result, because a peer
had stated it flatly. One unchecked claim, propagated, in the sentence readers
skim.

Writing it down as a rule did not work; the fleet had that rule and broke it
three times in one night. So this makes the correct form CHEAPER than the wrong
one: running this is less effort than recalling the numbers, and its output is
already in the agreed shape.

WHAT IT DOES NOT DO
-------------------
It reports what it measured and nothing else. There is no cached mode and no
"since last run" shortcut, because a stale board that looks fresh is the exact
defect this replaces. If a measurement is too slow to take, the honest output
is to say so -- see --skip-tests, which prints an explicit not-measured line
rather than a remembered number.

BRANCH NAMES ARE MANDATORY. Under the develop/master model a bare SHA is
ambiguous: "windows is at X" no longer identifies a state. Every tip printed
here is qualified by its branch.
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path


def sh(args: list[str], cwd: Path) -> tuple[int, str]:
    p = subprocess.run(args, cwd=str(cwd), capture_output=True, text=True,
                       encoding="utf-8", errors="replace")
    return p.returncode, (p.stdout or "").strip()


def short(repo: Path, ref: str) -> str:
    code, out = sh(["git", "rev-parse", "--short", ref], repo)
    return out if code == 0 and out else "ABSENT"


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description="Emit a measured board block")
    ap.add_argument("--repo", default=".", help="repo root (default: cwd)")
    ap.add_argument("--name", default=None, help="repo label (default: dir name)")
    ap.add_argument("--skip-tests", action="store_true",
                    help="do not run the suite; print an explicit not-measured line")
    args = ap.parse_args(argv)

    repo = Path(args.repo).resolve()
    name = args.name or repo.name
    now = datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")

    sh(["git", "fetch", "-q", "origin"], repo)

    develop = short(repo, "origin/develop")
    master = short(repo, "origin/master")
    _, porcelain = sh(["git", "status", "--porcelain"], repo)
    dirty = len([l for l in porcelain.splitlines() if l.strip()])

    # Author field included DELIBERATELY: omitting it is the exact defect this
    # tool exists to prevent.
    code, raw = sh(["gh", "pr", "list", "--state", "open",
                    "--json", "number,author,baseRefName,title"], repo)
    if code == 0 and raw:
        prs = [f"#{p['number']} @{p['author']['login']} -> {p['baseRefName']}"
               for p in json.loads(raw)]
    else:
        prs = ["(gh unavailable - NOT MEASURED)"]

    if args.skip_tests:
        suite = "NOT RE-MEASURED THIS RUN (do not quote a remembered value)"
    else:
        code, _ = sh(["cargo", "test", "--workspace", "--no-fail-fast"], repo)
        suite = f"cargo test --workspace --no-fail-fast -> EXIT {code}"

    print(f"BOARD - measured at {now}")
    print(f"  {name}  develop {develop} / master {master}")
    print(f"  working tree      {dirty} modified")
    print(f"  open PRs          {', '.join(prs) if prs else 'none'}")
    print(f"  suite             {suite}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
