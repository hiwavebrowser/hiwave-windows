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
import os
import pathlib
import re
import subprocess
import sys
import tomllib
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



def reachability() -> dict:
    """Count ComputedStyle fields that layout READS but CSS can never SET.

    A field the layout engine consumes, with no `apply_declaration` arm able to
    write it, is an implemented capability that no page can reach. That is
    invisible to every unit test - the layout tests for such a field PASS,
    because they construct the struct directly and set it by hand. Test coverage
    measures whether code RUNS, not whether a user can CAUSE it to run.

    Found on 2026-07-31: `position` was read in 24 places across rustkit-layout,
    including out-of-flow handling in flex and grid, and had no arm at all - so
    every page rendered position:static regardless of its CSS, and none of that
    layout code was reachable.

    Reports the LIST, not only the count. A count says how far there is to go;
    the list says WHICH capability is dead, which is the part someone can act on.
    Method and caveat are carried in the returned dict rather than in a commit
    message, because whoever reads this number in six weeks will not read the
    commit.
    """
    root = pathlib.Path(__file__).resolve().parent.parent
    css = root / "crates" / "rustkit-css" / "src" / "lib.rs"
    engine = root / "crates" / "rustkit-engine" / "src" / "lib.rs"
    layout_dir = root / "crates" / "rustkit-layout" / "src"
    if not (css.exists() and engine.exists() and layout_dir.is_dir()):
        return {"available": False, "reason": "expected crate paths not found"}

    css_text = css.read_text(encoding="utf-8", errors="replace")
    m = re.search(r"pub struct ComputedStyle \{(.*?)\n\}", css_text, re.S)
    if not m:
        return {"available": False, "reason": "ComputedStyle struct not found"}
    fields = set(re.findall(r"^\s*pub (\w+):", m.group(1), re.M))

    read = set()
    for path in sorted(layout_dir.rglob("*.rs")):
        t = path.read_text(encoding="utf-8", errors="replace")
        read |= set(re.findall(r"style\.([a-z_][a-z0-9_]*)", t))
    read &= fields

    # Scan the WHOLE engine file for assignments rather than trying to slice out
    # apply_declaration by signature. The signature has changed before (the fn
    # became an associated fn), and a slice that silently misses arms produces
    # FALSE UNREACHABLE entries - the failure mode being guarded against.
    eng_text = engine.read_text(encoding="utf-8", errors="replace")
    written = set(re.findall(r"(?:style|s)\.([a-z_][a-z0-9_]*)\s*=[^=]", eng_text))
    written &= fields

    unreachable = sorted(read - written)
    return {
        "available": True,
        "computed_style_fields": len(fields),
        "read_by_layout": len(read),
        "writable_by_applier": len(written),
        "unreachable_count": len(unreachable),
        "unreachable": unreachable,
        "method": (
            "Fields matching `style.<name>` anywhere in rustkit-layout, minus "
            "fields assigned as `style.<name> =` or `s.<name> =` anywhere in "
            "rustkit-engine, intersected with the real ComputedStyle field set."
        ),
        "limit": (
            "A FLOOR, NOT A PROOF. This measures WRITABILITY - whether any arm "
            "can set the field - not whether setting it changes what gets "
            "painted. A field can leave this list and still be inert, because "
            "the break can be BELOW the applier: in propagation, in which box "
            "the consumer reads from, or in a consumer nothing populates. "
            "Measured 2026-07-31: text_decoration_line left this list and an "
            "underlined <p> still painted identically to a plain one, because "
            "decoration PROPAGATES to descendants rather than inheriting and "
            "the display list reads it off the text box. Pair this number with "
            "a test that asserts the value changes the display list."
        ),
        "caveat": (
            "REGEX OVER SOURCE, NOT TYPE-CHECKED. A field written only through "
            "an alias this pattern misses is a FALSE POSITIVE. Grep any new "
            "entry individually before acting on it - the audit that produced "
            "this metric initially mis-extracted the applier body and had to be "
            "re-checked field by field."
        ),
    }

# The engine's lib tests run in chunks of whole modules, one process per chunk. On a hosted
# runner the GPU is the software adapter (WARP), and one process that runs the whole engine suite
# ends partway through (exit 2173, a RaiseException in the D3D12 runtime after state builds up on
# its device; tracked separately). The same tests run, serially as before; the listed count is
# checked against the count that ran, so a chunk that dies or a test that is dropped shows.
ENGINE = "rustkit-engine"
ENGINE_DIR = Path(__file__).resolve().parent.parent / "crates" / ENGINE
CHUNK_TESTS = 150
# Windows caps a command line at 32767 characters; test names go on it.
CHUNK_CHARS = 24000


def chunk_env(base: dict[str, str] | None = None) -> dict[str, str]:
    """The environment for a test executable run directly: cargo's own, i.e. `base` (the process
    environment by default) plus the `[env]` table of .cargo/config.toml for what `base` lacks.
    Without it a test thread gets Rust's 2 MB default stack, not RUST_MIN_STACK."""
    env = dict(os.environ if base is None else base)
    config = Path(__file__).resolve().parent.parent / ".cargo" / "config.toml"
    table = tomllib.loads(config.read_text(encoding="utf-8")).get("env", {}) if config.exists() else {}
    for name, value in table.items():
        if isinstance(value, dict):
            if value.get("force"):
                env[name] = str(value["value"])
            else:
                env.setdefault(name, str(value["value"]))
        else:
            env.setdefault(name, str(value))
    return env


def engine_executables() -> tuple[int, list[tuple[str, str]]]:
    """Build the engine's test targets as the workspace run builds them; return (exit, [(kind, exe)])."""
    proc = subprocess.run(
        ["cargo", "test", "-p", ENGINE, "--features", "headless", "--no-run",
         "--message-format=json"],
        stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True, encoding="utf-8",
        errors="replace")
    exes = []
    for line in (proc.stdout or "").splitlines():
        if not line.startswith("{"):
            continue
        msg = json.loads(line)
        if msg.get("reason") != "compiler-artifact" or not msg.get("executable"):
            continue
        # Only the engine's own test targets ("path+file:///.../crates/rustkit-engine#0.1.0").
        if not msg.get("profile", {}).get("test") or f"/{ENGINE}#" not in msg.get("package_id", ""):
            continue
        kind = "lib" if "lib" in msg["target"]["kind"] else msg["target"]["name"]
        exes.append((kind, msg["executable"]))
    return proc.returncode, exes


def chunks(names: list[str]) -> list[list[str]]:
    """Whole top-level modules per chunk, up to CHUNK_TESTS tests (a bigger module goes alone)."""
    modules: dict[str, list[str]] = {}
    for name in names:
        modules.setdefault(name.split("::", 1)[0] if "::" in name else "(crate root)", []).append(name)
    out: list[list[str]] = []
    for group in modules.values():
        if out and len(out[-1]) + len(group) <= CHUNK_TESTS \
                and sum(len(n) + 1 for n in out[-1] + group) <= CHUNK_CHARS:
            out[-1].extend(group)
        else:
            out.append(list(group))
    return out


def name_failures(label: str, body: str) -> None:
    """Print what failed in a test run's output to the job log (the output itself is not there):
    each failed test, a stack overflow's thread, and the test a process was in when it died."""
    for name in re.findall(r"^test (\S+) \.\.\. FAILED", body, re.M):
        print(f"  {label}: FAILED {name}", file=sys.stderr)
        # Its panic message: the first lines of its captured output block.
        block = body.split(f"---- {name} stdout ----", 1)
        if len(block) == 2:
            for line in [l for l in block[1].strip().splitlines() if l.strip()][:6]:
                print(f"      {line[:300]}", file=sys.stderr)
    if "has overflowed its stack" in body:
        at = body.rfind("thread '", 0, body.index("has overflowed"))
        print(f"  {label}: " + body[at:].splitlines()[0], file=sys.stderr)
    started = re.findall(r"^test (\S+) \.\.\. ?(.*)$", body, re.M)
    if started and not started[-1][1].strip():
        print(f"  {label}: the process ended during {started[-1][0]}", file=sys.stderr)


def run_tests() -> tuple[int, str, dict]:
    code, out = run(["cargo", "test", "--workspace", "--exclude", ENGINE, "--",
                     "--test-threads=1"])
    worst = code
    texts = [out]
    if code != 0:
        name_failures("workspace", out)
    build_code, exes = engine_executables()
    info = {"listed": 0, "ran": 0, "processes": 0, "build_exit": build_code}
    env = chunk_env()
    if build_code != 0 or not exes:
        return worst or build_code or 1, "\n".join(texts), info
    for kind, exe in exes:
        listing = subprocess.run([exe, "--list", "--format", "terse"], cwd=ENGINE_DIR, env=env,
                                 stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True,
                                 encoding="utf-8", errors="replace")
        names = [line[: -len(": test")] for line in listing.stdout.splitlines()
                 if line.endswith(": test")]
        info["listed"] += len(names)
        groups = chunks(names) if kind == "lib" else [[]]
        for group in groups:
            cmd = [exe, "--test-threads=1"] + (["--exact", *group] if group else [])
            proc = subprocess.run(cmd, cwd=ENGINE_DIR, env=env, stdout=subprocess.PIPE,
                                  stderr=subprocess.STDOUT, text=True, encoding="utf-8",
                                  errors="replace")
            info["processes"] += 1
            body = proc.stdout or ""
            # The line cargo would print, so the parser below attributes the block to the engine.
            texts.append(f"     Running {kind} ({exe})\n{body}")
            for m in RESULT_RE.finditer(body):
                info["ran"] += int(m.group(2)) + int(m.group(3)) + int(m.group(4))
            if proc.returncode != 0:
                print(f"engine chunk failed (exit {proc.returncode}): {kind}, "
                      f"{len(group) or 'all'} tests from {group[0] if group else exe}",
                      file=sys.stderr)
                name_failures(kind, body)
                worst = worst or proc.returncode
    doc_code, doc_out = run(["cargo", "test", "-p", ENGINE, "--features", "headless", "--doc"])
    texts.append(doc_out)
    if doc_code != 0:
        name_failures("doc", doc_out)
    worst = worst or doc_code
    if info["ran"] != info["listed"]:
        print(f"engine tests listed {info['listed']}, ran {info['ran']}", file=sys.stderr)
        worst = worst or 1
    return worst, "\n".join(texts), info


def collect(commit: str, branch: str) -> dict:
    build_code, build_out = run(["cargo", "build", "--workspace"])
    build_warnings = len(re.findall(r"^warning:", build_out, re.M))

    # Engine tests share a GPU guard. Hundreds of parallel tests can spend
    # its entire 120-second timeout waiting behind other valid tests; the
    # sync gate therefore runs serially on Windows, including hosted runners.
    test_code, test_out, engine_chunks = run_tests()

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
            "engine_chunks": engine_chunks,
        },
        "per_crate": dict(sorted(per_crate.items())),
        "crates_with_no_tests": empty,
        "reachability": reachability(),
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
    r = m.get("reachability", {})
    if r.get("available"):
        lines.append("")
        lines.append("### Reachability")
        lines.append("")
        lines.append(
            f"- {r['unreachable_count']} of {r['read_by_layout']} ComputedStyle "
            f"fields read by layout CANNOT be set from CSS"
        )
        if r["unreachable"]:
            lines.append(f"- dead capabilities: `{'`, `'.join(r['unreachable'])}`")
        lines.append(f"- limit: {r['limit']}")
        lines.append(f"- caveat: {r['caveat']}")

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
