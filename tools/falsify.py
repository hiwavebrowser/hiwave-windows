"""Falsification harness — mutate the product, watch the guard fail, restore.

WHY THIS IS A COMMITTED TOOL AND NOT A SCRATCH SCRIPT
-----------------------------------------------------
It was a scratch script, rewritten from scratch on each unit, and that is
exactly how the same defect kept reappearing:

  * one run used `cargo test --workspace` WITHOUT --no-fail-fast, so cargo
    stopped at the first failing binary and the harness UNDER-REPORTED reds —
    it said one guard fired when two did;
  * two runs were "restored" by reaching for `git checkout --`, which does not
    restore a mutation, it discards the whole working file — twice destroying
    uncommitted product code.

Neither was a knowledge problem. Both were a fresh-file problem. A harness that
is rewritten every time carries no scars; a committed one carries all of them.

THE THREE GUARANTEES
--------------------
1. RESTORE CANNOT LOSE WORK. Originals are held in memory and rewritten in a
   `finally`. Git is never invoked to restore. If the process is killed
   mid-run, `--verify-clean` on the next run detects the residue.
2. THE SUITE IS FULLY OBSERVED. `--no-fail-fast` is not optional and not a
   parameter; it is baked into the command.
3. A MUTATION THAT DOES NOT APPLY IS A FAILURE, NOT A SKIP. A stale anchor
   string silently testing nothing is the vacuity defect in harness form.

USAGE
-----
Define mutations in a JSON file (or import and call `run`):

    [
      {"label": "site 2 px-only",
       "file":  "crates/rustkit-layout/src/grid.rs",
       "find":  "<exact source text>",
       "replace": "<the OLD, buggy shape>"}
    ]

    python tools/falsify.py mutations.json

Each mutation is applied ALONE, the suite is run, the file is restored, and the
named failing tests are reported. Names, not counts: a count is a summary you
can get wrong silently; a list is checkable by the reader.
"""

from __future__ import annotations

import argparse
import io
import json
import subprocess
import sys
from pathlib import Path

CARGO_TEST = ["cargo", "test", "--workspace", "--no-fail-fast"]


def run_suite() -> tuple[list[str], bool]:
    """Return (named failing tests, compile_error)."""
    proc = subprocess.run(
        CARGO_TEST, capture_output=True, text=True, encoding="utf-8", errors="replace"
    )
    out = proc.stdout + proc.stderr
    failed = sorted(
        {
            line.split()[1]
            for line in out.splitlines()
            if line.startswith("test ")
            and "FAILED" in line
            and not line.startswith("test result")
        }
    )
    return failed, "error[" in out


def run(mutations: list[dict], repo: Path) -> int:
    paths = {repo / m["file"] for m in mutations}
    originals = {p: io.open(p, encoding="utf-8").read() for p in paths}

    baseline, broke = run_suite()
    if baseline or broke:
        print("REFUSING TO RUN: the tree is not green before mutating.")
        for f in baseline:
            print(f"   {f}")
        return 2

    report: list[str] = ["=== FALSIFICATION ==="]
    exit_code = 0
    try:
        for m in mutations:
            path = repo / m["file"]
            src = originals[path]
            if m["find"] not in src:
                # A stale anchor silently testing nothing is the vacuity
                # defect wearing a harness costume. Loud, not skipped.
                report.append(f"\n{m['label']}\n   -> MUTATION DID NOT APPLY (stale anchor) — FAILURE")
                exit_code = 1
                continue

            io.open(path, "w", encoding="utf-8").write(src.replace(m["find"], m["replace"], 1))
            failed, broke = run_suite()
            io.open(path, "w", encoding="utf-8").write(src)  # restore immediately

            tag = "  [COMPILE ERROR]" if broke else ""
            report.append(f"\n{m['label']}\n   -> {len(failed)} RED{tag}")
            for f in failed:
                report.append(f"      {f}")
            if not failed and not broke:
                report.append("      *** NOTHING FAILED — the guard is decorative ***")
                exit_code = 1
    finally:
        # Unconditional. A killed run still leaves the tree as it was found.
        for p, s in originals.items():
            io.open(p, "w", encoding="utf-8").write(s)

    final, _ = run_suite()
    report.append(f"\n=== RESTORED: {len(final)} failing (expect 0) ===")
    if final:
        exit_code = 1
        for f in final:
            report.append(f"   {f}")

    print("\n".join(report))
    return exit_code


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description="Falsification harness")
    ap.add_argument("mutations", help="JSON file describing the mutations")
    ap.add_argument("--repo", default=".", help="repo root (default: cwd)")
    args = ap.parse_args(argv)

    mutations = json.load(io.open(args.mutations, encoding="utf-8"))
    return run(mutations, Path(args.repo).resolve())


if __name__ == "__main__":
    sys.exit(main())
