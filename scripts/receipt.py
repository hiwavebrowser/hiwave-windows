#!/usr/bin/env python3
"""receipt.py — the provenance block a closure PR carries (Z phase, 2026-10-02).

Emits the fields the Z plan requires, from what the machine already knows
(git, cargo, the binary, the host). Nothing is typed by hand, so "same binary"
and "all tests green" are facts a reader can check, not prose.

    python3 scripts/receipt.py --package D0 --binary target/parity/parity-capture \
        [--base <sha>] [--campaign parity-baseline/parity_test_results.json] \
        [--runs <file>...] [--note "..."] [--markdown]

Runs on macOS, Windows and Linux; every field it cannot determine is printed
as null rather than guessed.
"""
import argparse, datetime, hashlib, json, os, platform, shutil, subprocess, sys
from pathlib import Path


def sh(*cmd, cwd=None):
    try:
        return subprocess.run(cmd, capture_output=True, text=True, cwd=cwd, timeout=60, encoding="utf-8").stdout.strip() or None
    except Exception:
        return None


def sha256(path):
    try:
        h = hashlib.sha256()
        with open(path, "rb") as f:
            for chunk in iter(lambda: f.read(1 << 20), b""):
                h.update(chunk)
        return h.hexdigest()
    except Exception:
        return None


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--package", required=True)
    ap.add_argument("--binary", help="the measured binary (parity-capture)")
    ap.add_argument("--base", help="base SHA the candidate is measured against")
    ap.add_argument("--campaign", help="parity_test results JSON for the candidate")
    ap.add_argument("--runs", nargs="*", default=[], help="raw run/A-B files to hash and list")
    ap.add_argument("--profile", help="cargo profile used (parity|release)")
    ap.add_argument("--features", default=None)
    ap.add_argument("--note", default=None)
    ap.add_argument("--markdown", action="store_true")
    a = ap.parse_args()

    root = Path(sh("git", "rev-parse", "--show-toplevel") or ".")
    head = sh("git", "rev-parse", "HEAD", cwd=root)
    dirty = bool(sh("git", "status", "--porcelain", "--untracked-files=no", cwd=root))
    toolchain = sh("rustc", "-V")
    cargo = sh("cargo", "-V")
    target = sh("rustc", "-vV")
    target = next((l.split(":", 1)[1].strip() for l in (target or "").splitlines() if l.startswith("host:")), None)
    lock_hash = sha256(root / "Cargo.lock")
    binary = Path(a.binary).resolve() if a.binary else None
    rec = {
        "work_package": a.package,
        "base_sha": a.base,
        "candidate_sha": head,
        "dirty_state": dirty,
        "repo": sh("git", "config", "--get", "remote.origin.url", cwd=root),
        "branch": sh("git", "rev-parse", "--abbrev-ref", "HEAD", cwd=root),
        "cargo_lock_sha256": lock_hash,
        "binary": str(binary) if binary else None,
        "binary_sha256": sha256(binary) if binary else None,
        "binary_mtime_utc": datetime.datetime.utcfromtimestamp(binary.stat().st_mtime).isoformat() + "Z" if binary and binary.exists() else None,
        "toolchain": toolchain,
        "cargo": cargo,
        "target": target,
        "profile": a.profile or ("parity" if binary and "parity" in binary.parts else "release" if binary and "release" in binary.parts else None),
        "features": a.features,
        "rustflags": os.environ.get("RUSTFLAGS"),
        "effective_engine_flags": {k: v for k, v in os.environ.items() if k.startswith("RUSTKIT_")},
        "host": platform.node(),
        "os": platform.platform(),
        "cpu": platform.processor() or sh("sysctl", "-n", "machdep.cpu.brand_string"),
        "sccache": bool(shutil.which("sccache")),
        "chrome_version": None,
        "campaign": None,
        "raw_runs": [],
        "utc": datetime.datetime.utcnow().isoformat() + "Z",
        "note": a.note,
    }
    meta = root / "baselines" / "metadata.json"
    if meta.exists():
        try:
            m = json.load(open(meta, encoding="utf-8"))
            rec["chrome_version"] = m.get("chrome_version") or m.get("chrome") or m.get("version")
        except Exception:
            pass
    if a.campaign and Path(a.campaign).exists():
        try:
            c = json.load(open(a.campaign, encoding="utf-8"))
            r = c.get("results", [])
            vals = [x["pixel"]["diffPercent"] for x in r if x.get("pixel") and x["pixel"].get("diffPercent") is not None]
            rec["campaign"] = {"file": a.campaign, "sha256": sha256(a.campaign), "cases": len(r),
                               "passed": c.get("passed"), "failed": c.get("failed"),
                               "mean_diff_pct": round(sum(vals) / len(vals), 4) if vals else None,
                               "timestamp": c.get("timestamp")}
        except Exception as e:
            rec["campaign"] = {"file": a.campaign, "error": str(e)}
    for f in a.runs:
        p = Path(f)
        rec["raw_runs"].append({"file": f, "sha256": sha256(p) if p.exists() else None, "bytes": p.stat().st_size if p.exists() else None})

    if a.markdown:
        print("## Receipt (`scripts/receipt.py`)\n")
        print("```json")
        print(json.dumps(rec, indent=1, sort_keys=True))
        print("```")
    else:
        print(json.dumps(rec, indent=1, sort_keys=True))


if __name__ == "__main__":
    main()
