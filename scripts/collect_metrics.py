#!/usr/bin/env python3
"""Collect build/test metrics for hiwave-windows.

hiwave-macos collects parity metrics by running the pixel-capture harness
(scripts/parity_test.py) and recording an average pixel diff. That metric is
NOT collectable on a GitHub-hosted Windows runner: parity capture needs a real
GPU adapter, and when capture yields nothing the macOS pipeline defaults the
per-case diff to 100.0 rather than erroring -- so a GPU-less runner would
publish a confident-looking "100% diff" that is an artefact of the harness,
not a measurement of the renderer.

So this collects what IS true headless: does the workspace build, and do the
tests pass. Those are real numbers a Windows runner can stand behind.

Usage:
    python scripts/collect_metrics.py --output metrics.json
    python scripts/collect_metrics.py --input metrics.json --format markdown
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path

# "test result: ok. 44 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out"
RESULT_RE = re.compile(
    r"test result: (\w+)\. (\d+) passed; (\d+) failed; (\d+) ignored"
)
# "     Running unittests src\lib.rs (target\debug\deps\rustkit_text-abc123.exe)"
RUNNING_RE = re.compile(r"Running .*?\(.*?[\\/]deps[\\/]([A-Za-z0-9_]+?)-[0-9a-f]+")
DOCTEST_RE = re.compile(r"^\s*Doc-tests (\S+)")


def run(cmd: list[str]) -> tuple[int, str]:
    # stderr is merged into stdout by the PIPE, not concatenated afterwards.
    # cargo prints "Running <binary>" on stderr and "test result:" on stdout,
    # so appending one to the other loses the interleaving that attributes a
    # result block to the binary that produced it -- every crate then lands
    # in a single bucket.
    proc = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                          text=True, encoding="utf-8", errors="replace")
    return proc.returncode, proc.stdout or ""


def collect(commit: str, branch: str) -> dict:
    build_code, build_out = run(["cargo", "build", "--workspace"])
    build_warnings = len(re.findall(r"^warning:", build_out, re.M))

    test_code, test_out = run(["cargo", "test", "--workspace"])

    # Attribute each "test result:" block to the binary that produced it.
    current = None
    per_crate: dict[str, dict[str, int]] = {}
    totals = {"passed": 0, "failed": 0, "ignored": 0}
    for line in test_out.splitlines():
        m = RUNNING_RE.search(line)
        if m:
            current = m.group(1)
            continue
        m = DOCTEST_RE.match(line)
        if m:
            current = m.group(1) + " (doc)"
            continue
        m = RESULT_RE.search(line)
        if m:
            _ok, p, f, i = m.group(1), int(m.group(2)), int(m.group(3)), int(m.group(4))
            key = current or "unattributed"
            bucket = per_crate.setdefault(key, {"passed": 0, "failed": 0, "ignored": 0})
            bucket["passed"] += p
            bucket["failed"] += f
            bucket["ignored"] += i
            totals["passed"] += p
            totals["failed"] += f
            totals["ignored"] += i

    # A crate with zero tests is worth surfacing: it is the shape of the
    # rustkit-text gap (compiled green, ran nothing, looked fine).
    empty = sorted(k for k, v in per_crate.items()
                   if v["passed"] == 0 and v["failed"] == 0 and not k.endswith("(doc)"))

    return {
        "timestamp": datetime.now(timezone.utc).isoformat(),
        "commit": commit,
        "branch": branch,
        "platform": "windows",
        "build": {
            "ok": build_code == 0,
            "exit_code": build_code,
            "warnings": build_warnings,
        },
        "tests": {
            "ok": test_code == 0,
            "exit_code": test_code,
            "passed": totals["passed"],
            "failed": totals["failed"],
            "ignored": totals["ignored"],
            "total": totals["passed"] + totals["failed"],
        },
        "per_crate": dict(sorted(per_crate.items())),
        "crates_with_no_tests": empty,
        "not_collected": {
            "parity_pixel_diff": (
                "requires a GPU adapter; not collectable on a hosted Windows "
                "runner. Deliberately omitted rather than emitting the "
                "harness's 100.0 default as if it were a measurement."
            )
        },
    }


def to_markdown(m: dict) -> str:
    b, t = m["build"], m["tests"]
    lines = [
        "## Windows Metrics",
        "",
        "| Metric | Value |",
        "|--------|-------|",
        f"| Build | {'PASS' if b['ok'] else 'FAIL'} ({b['warnings']} warnings) |",
        f"| Tests passed | **{t['passed']}** |",
        f"| Tests failed | {t['failed']} |",
        f"| Tests ignored | {t['ignored']} |",
        f"| Commit | `{m['commit'][:8]}` |",
        f"| Branch | {m['branch']} |",
        "",
    ]
    if m["crates_with_no_tests"]:
        lines += [
            "<details><summary>Crates running zero tests "
            f"({len(m['crates_with_no_tests'])})</summary>",
            "",
            # ASCII only: this string is written to a redirected stdout pipe,
            # which on Windows is the locale codepage, not UTF-8.
            "These compile but execute no tests - the same shape as the "
            "`rustkit-text` parity gap.",
            "",
        ]
        lines += [f"- `{c}`" for c in m["crates_with_no_tests"]]
        lines += ["", "</details>", ""]
    lines += [
        "<details><summary>Per-crate results</summary>",
        "",
        "| Crate | passed | failed | ignored |",
        "|-------|--------|--------|---------|",
    ]
    for name, v in m["per_crate"].items():
        lines.append(f"| {name} | {v['passed']} | {v['failed']} | {v['ignored']} |")
    lines += [
        "",
        "</details>",
        "",
        f"> Not collected: parity pixel diff - "
        f"{m['not_collected']['parity_pixel_diff']}",
    ]
    return "\n".join(lines)


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--output")
    ap.add_argument("--input", help="render an existing metrics.json instead of collecting")
    ap.add_argument("--format", choices=["json", "markdown"], default="json")
    ap.add_argument("--commit", default="")
    ap.add_argument("--branch", default="")
    a = ap.parse_args()

    if a.input:
        metrics = json.loads(Path(a.input).read_text(encoding="utf-8"))
    else:
        metrics = collect(a.commit, a.branch)

    if a.output:
        Path(a.output).write_text(json.dumps(metrics, indent=2), encoding="utf-8")

    if a.format == "markdown":
        sys.stdout.write(to_markdown(metrics) + "\n")
    elif not a.output:
        sys.stdout.write(json.dumps(metrics, indent=2) + "\n")

    # Collection succeeding is the contract here, not the tests passing --
    # the workflow records red builds as data rather than losing the run.
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
