#!/usr/bin/env python3
"""test262.py: run tc39/test262 through the vendored JS engine and compare with the expectations.

    python3 scripts/test262.py                 # sync, build, run everything, compare
    python3 scripts/test262.py --area test/language/expressions
    python3 scripts/test262.py --update        # rewrite trench/test262/expectations.txt from this run

The runner is crates/rustkit-js/examples/test262.rs; this script splits the test list into chunks,
runs them in parallel, restarts a chunk after the test that took the process down, and compares.

Expectations (trench/test262/expectations.txt) list every run that does NOT pass, one per line:
`FAIL<TAB>path<TAB>mode`. A run that is not listed is expected to pass. The comparison fails when a
run that should pass does not (a regression) and when a listed run now passes (the list is stale:
rerun with --update and commit it), so the file is always the engine's measured state.

The committed file records pass / fail only. Locally the runner also tells apart a run that took the
engine down; that finer record is kept in a file outside the repository (--private-list, default
~/.local/state/hiwave/test262_private.txt), and a listed failure that now takes the engine down is a
regression when that file is present.

Memory: every runner process is held to --mem-mb (default 3072 MiB): by RLIMIT_AS where the kernel
enforces it (Linux) and by a resident-size watchdog everywhere (macOS does not enforce RLIMIT_AS).
A process over its cap is stopped and its test recorded as not passing. The jobs times the cap never
exceed --total-mem-mb (default 24576 MiB), and at most 8 jobs run by default, so a test that
allocates without bound cannot take the machine's memory.
"""

import argparse
import collections
import concurrent.futures
import os
import pathlib
import subprocess
import sys
import tempfile
import time

try:
    import resource
except ImportError:  # not on every platform; the watchdog still holds the cap
    resource = None

ROOT = pathlib.Path(__file__).resolve().parent.parent
SUITE = ROOT / "third_party" / "test262"
EXPECTATIONS = ROOT / "trench" / "test262" / "expectations.txt"
SUMMARY = ROOT / "trench" / "test262" / "summary.txt"
PRIVATE_DEFAULT = pathlib.Path.home() / ".local" / "state" / "hiwave" / "test262_private.txt"
CHUNK = 300
MAX_JOBS = 8
DEFAULT_MEM_MB = 3072
DEFAULT_TOTAL_MEM_MB = 24576
WATCH_SECONDS = 0.25
# Statuses that took the engine down. Recorded locally, folded into FAIL in the committed file.
DOWN = ("PANIC", "CRASH")


def target_dir():
    return pathlib.Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target"))


def list_tests(area, suite=None):
    suite = suite or SUITE
    base = suite / area
    out = []
    for path in sorted(base.rglob("*.js")):
        # Files a test imports or includes, not tests (INTERPRETING.md).
        if "_FIXTURE" in path.name:
            continue
        out.append(path.relative_to(suite).as_posix())
    return out


def flags_of(source):
    """The test's front-matter flags, read as the runner reads them (parse_meta in test262.rs)."""
    start = source.find("/*---")
    if start < 0:
        return []
    end = source.find("---*/", start)
    if end < 0:
        return []
    lines = source[start + 5 : end].splitlines()
    for i, line in enumerate(lines):
        if line[:1] in (" ", "\t") or not line.startswith("flags:"):
            continue
        rest = line[len("flags:") :].strip()
        if rest.startswith("["):
            return [f.strip() for f in rest.strip("[]").split(",") if f.strip()]
        flags = []
        for sub in lines[i + 1 :]:
            if sub[:1] not in (" ", "\t") and sub.strip():
                break
            item = sub.strip()
            if item.startswith("-"):
                flags.append(item[1:].strip())
        return flags
    return []


def modes_of(path, suite=None):
    """The modes the runner runs `path` in (the table in test262.rs main)."""
    try:
        source = ((suite or SUITE) / path).read_text(encoding="utf-8", errors="replace")
    except OSError:
        return ["sloppy", "strict"]
    flags = flags_of(source)
    if "raw" in flags:
        return ["raw"]
    if "module" in flags:
        return ["module"]
    if "onlyStrict" in flags:
        return ["strict"]
    if "noStrict" in flags:
        return ["sloppy"]
    return ["sloppy", "strict"]


def resident_mb(pid):
    """The process's resident size in MiB, or None when it cannot be read."""
    try:
        with open(f"/proc/{pid}/status", encoding="utf-8") as status:
            for line in status:
                if line.startswith("VmRSS:"):
                    return int(line.split()[1]) / 1024.0
    except OSError:
        pass
    try:
        out = subprocess.run(["ps", "-o", "rss=", "-p", str(pid)], capture_output=True, text=True, timeout=5, encoding="utf-8")
        return int(out.stdout.strip()) / 1024.0 if out.stdout.strip() else None
    except (OSError, ValueError, subprocess.SubprocessError):
        return None


def address_space_cap(mem_mb):
    """Applied in the child before exec: the process may map at most `mem_mb` MiB (where enforced)."""
    limit = mem_mb * 1024 * 1024

    def apply():
        if resource is not None and sys.platform.startswith("linux"):
            resource.setrlimit(resource.RLIMIT_AS, (limit, limit))

    return apply


def run_capped(argv, mem_mb, deadline_s):
    """Runs `argv` with its output in a file, stopping it when its resident size passes `mem_mb` or
    when `deadline_s` passes. Returns (stdout text, exit code, why it was stopped or None)."""
    with tempfile.TemporaryFile() as out:
        proc = subprocess.Popen(
            argv, stdout=out, stderr=subprocess.DEVNULL, preexec_fn=address_space_cap(mem_mb)
        )
        stopped = None
        began = time.monotonic()
        while True:
            try:
                proc.wait(timeout=WATCH_SECONDS)
                break
            except subprocess.TimeoutExpired:
                pass
            rss = resident_mb(proc.pid)
            if rss is not None and rss > mem_mb:
                stopped = "memory"
            elif time.monotonic() - began > deadline_s:
                stopped = "time"
            if stopped:
                proc.kill()
                proc.wait()
                break
        out.seek(0)
        return out.read().decode("utf-8", "replace"), proc.returncode, stopped


def run_chunk(binary, files, timeout_ms, mem_mb, suite=None):
    """Runs `files`, restarting after any test that takes the process down.
    Returns {(path, mode): (status, why)}."""
    suite = suite or SUITE
    results = {}
    todo = list(files)
    while todo:
        with tempfile.NamedTemporaryFile("w", suffix=".txt", delete=False) as handle:
            handle.write("\n".join(todo) + "\n")
            list_path = handle.name
        try:
            # A chunk's whole budget: every run timing out, twice over.
            deadline = len(todo) * 2 * (timeout_ms / 1000.0) * 2 + 60
            output, code, stopped = run_capped(
                [str(binary), "--root", str(suite), "--list", list_path, "--timeout-ms", str(timeout_ms)],
                mem_mb,
                deadline,
            )
        finally:
            os.unlink(list_path)
        started = None
        for line in output.splitlines():
            parts = line.split("\t")
            if parts[0] == "START" and len(parts) >= 2:
                started = parts[1]
            elif parts[0] == "RESULT" and len(parts) >= 4:
                why = parts[4] if len(parts) > 4 else ""
                results[(parts[1], parts[2])] = (parts[3], why)
        if code == 0 and stopped is None:
            break
        # The process went down in `started` (or before its first test): every mode of that test is
        # recorded as down, then the chunk goes on after it.
        if started is None:
            started = todo[0]
        why = f"runner stopped ({stopped})" if stopped else f"runner exit {code}"
        for mode in modes_of(started, suite):
            results[(started, mode)] = ("CRASH", why)
        todo = todo[todo.index(started) + 1 :]
    return results


def read_list(path):
    listed = {}
    if path.exists():
        for line in path.read_text(encoding="utf-8").splitlines():
            if not line or line.startswith("#"):
                continue
            status, test, mode = line.split("\t")
            listed[(test, mode)] = status
    return listed


def public_status(status):
    return "FAIL" if status in DOWN else status


def write_lists(not_passing, public_path, private_path):
    """The committed list (pass / fail only) and the private one (with the runs that took the engine
    down). Returns how many runs each lists."""
    public_path.parent.mkdir(parents=True, exist_ok=True)
    body = "".join(f"FAIL\t{path}\t{mode}\n" for (path, mode) in sorted(not_passing))
    public_path.write_text("# test262 runs that do not pass, measured. Rewritten by scripts/test262.py --update.\n" + body, encoding="utf-8")
    down = {k: v for k, v in not_passing.items() if v in DOWN}
    private_path.parent.mkdir(parents=True, exist_ok=True)
    private_path.write_text(
        "# Private: test262 runs that took the engine down. Not for the repository.\n"
        + "".join(f"{status}\t{path}\t{mode}\n" for (path, mode), status in sorted(down.items())), encoding="utf-8"
    )
    return len(not_passing), len(down)


def compare(not_passing, expected, private, results):
    """Returns (regressions, now passing, now down): lists of (path, mode)."""
    regressions = sorted(k for k in not_passing if k not in expected)
    fixed = sorted(k for k in expected if results.get(k, ("PASS", ""))[0] == "PASS")
    now_down = sorted(k for k, status in not_passing.items() if status in DOWN and k in expected and k not in private)
    return regressions, fixed, now_down


def jobs_within_budget(jobs, mem_mb, total_mb):
    return max(1, min(jobs, MAX_JOBS, total_mb // mem_mb))


def summary_lines(results, area):
    counts = collections.Counter("PASS" if status == "PASS" else "FAIL" for status, _ in results.values())
    per_area = collections.defaultdict(collections.Counter)
    for (path, _mode), (status, _why) in results.items():
        per_area[area_of(path)]["PASS" if status == "PASS" else "not"] += 1
    total = sum(counts.values())
    lines = [f"test262 @ {(ROOT / 'trench' / 'test262' / 'PIN').read_text(encoding="utf-8").strip()}  area={area}"]
    lines.append(f"runs {total}  pass {counts['PASS']} ({100.0 * counts['PASS'] / total:.2f}%)  not passing {counts['FAIL']}")
    for name in sorted(per_area):
        c = per_area[name]
        n = c["PASS"] + c["not"]
        lines.append(f"  {name:55} {c['PASS']:6}/{n:<6} {100.0 * c['PASS'] / n:6.2f}%")
    return lines


def area_of(path):
    parts = path.split("/")
    return "/".join(parts[:3]) if len(parts) > 3 else "/".join(parts[:2])


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--area", default="test", help="directory under the suite to run (default: test)")
    parser.add_argument("--jobs", type=int, default=max(1, min(MAX_JOBS, (os.cpu_count() or 2) // 2)))
    parser.add_argument("--timeout-ms", type=int, default=10000)
    parser.add_argument("--mem-mb", type=int, default=DEFAULT_MEM_MB, help="memory cap per runner process, MiB")
    parser.add_argument(
        "--total-mem-mb", type=int, default=DEFAULT_TOTAL_MEM_MB, help="jobs x --mem-mb never exceeds this, MiB"
    )
    parser.add_argument("--private-list", type=pathlib.Path, default=PRIVATE_DEFAULT)
    parser.add_argument("--update", action="store_true", help="rewrite the expectations from this run")
    parser.add_argument("--no-build", action="store_true")
    parser.add_argument("--no-sync", action="store_true")
    parser.add_argument("--show", type=int, default=40, help="how many differing runs to print")
    args = parser.parse_args()
    if args.private_list.resolve().is_relative_to(ROOT.resolve()):
        sys.exit("--private-list must be outside the repository")

    if not args.no_sync:
        subprocess.run([str(ROOT / "scripts" / "test262_sync.sh")], check=True)
    if not args.no_build:
        subprocess.run(
            ["cargo", "build", "--release", "-p", "rustkit-js", "--example", "test262"], cwd=ROOT, check=True
        )
    binary = target_dir() / "release" / "examples" / "test262"

    files = list_tests(args.area)
    if not files:
        sys.exit(f"no tests under {args.area}")
    jobs = jobs_within_budget(args.jobs, args.mem_mb, args.total_mem_mb)
    chunks = [files[i : i + CHUNK] for i in range(0, len(files), CHUNK)]
    results = {}
    with concurrent.futures.ThreadPoolExecutor(max_workers=jobs) as pool:
        for done, part in enumerate(pool.map(lambda c: run_chunk(binary, c, args.timeout_ms, args.mem_mb), chunks), 1):
            results.update(part)
            print(f"\r{done}/{len(chunks)} chunks", end="", file=sys.stderr, flush=True)
    print(file=sys.stderr)

    lines = summary_lines(results, args.area)
    print("\n".join(lines))
    not_passing = {key: status for key, (status, _why) in results.items() if status != "PASS"}
    whole = args.area == "test"
    if args.update:
        if not whole:
            sys.exit("--update rewrites the whole list: run it without --area")
        listed, down = write_lists(not_passing, EXPECTATIONS, args.private_list)
        SUMMARY.write_text("\n".join(lines) + "\n", encoding="utf-8")
        print(f"wrote {EXPECTATIONS.relative_to(ROOT)} ({listed} runs), {SUMMARY.relative_to(ROOT)}")
        print(f"wrote the private list ({down} runs) to {args.private_list}")
        return

    in_area = lambda k: whole or k[0].startswith(args.area + "/")
    expected = {k: v for k, v in read_list(EXPECTATIONS).items() if in_area(k)}
    private = {k: v for k, v in read_list(args.private_list).items() if in_area(k)}
    if not args.private_list.exists():
        print(f"(no private list at {args.private_list}: failures that now take the engine down are not told apart)")
    regressions, fixed, now_down = compare(not_passing, expected, private, results)
    if not args.private_list.exists():
        now_down = []
    for label, keys in (
        ("REGRESSION (expected to pass)", regressions),
        ("NOW TAKES THE ENGINE DOWN (listed as failing)", now_down),
        ("NOW PASSES (listed as not passing)", fixed),
    ):
        if keys:
            print(f"\n{label}: {len(keys)}")
            for path, mode in keys[: args.show]:
                status, why = results.get((path, mode), ("PASS", ""))
                print(f"  {status:8} {mode:7} {path}  {why}")
    if regressions or fixed or now_down:
        sys.exit(1)
    print("\nno flip against the expectations")


if __name__ == "__main__":
    main()
