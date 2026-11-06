#!/usr/bin/env python3
"""receipt.py — the provenance block a closure PR carries (Z phase, 2026-10-02).

Emits the fields the Z plan requires, from what the machine already knows
(git, cargo, the binary, the host). Nothing is typed by hand, so "same binary"
and "all tests green" are facts a reader can check, not prose.

    python3 scripts/receipt.py --package D0 --binary target/parity/parity-capture \
        [--base <sha>] [--campaign parity-baseline/parity_test_results.json] \
        [--runs <file>...] [--note "..."] [--markdown] [--host-paths]

The output goes into public PR bodies and comments, so it carries no path of
the machine that ran it: a file inside the repository prints repo-relative, any
other file prints as its basename, and absolute paths in --note, RUSTFLAGS and
the RUSTKIT_* values get the same treatment. A home directory prints as "~".
A local-path origin prints as its basename, an origin URL loses its
credentials and query, and the machine's name is left out. Nothing is written
to stderr. --host-paths keeps all of these as given, for local use only.

Runs on macOS, Windows and Linux; every field it cannot determine is printed
as null rather than guessed.
"""
import argparse, datetime, hashlib, json, os, platform, re, shutil, subprocess, sys
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


def utc_z(when):
    """An aware UTC time in the form the receipt has always printed."""
    return when.replace(tzinfo=None).isoformat() + "Z"


# A home directory itself: its last component is an account name.
HOME_DIR = re.compile(r"(?:[A-Za-z]:)?[\\/](?:Users|home)[\\/][^\\/]+[\\/]?$")


def is_home(path):
    path = str(path)
    return bool(HOME_DIR.match(path)) or os.path.normpath(path) == os.path.normpath(str(Path.home()))


def public_path(path, root):
    """Repo-relative for a file inside the repository, "~" for a home
    directory, the basename otherwise."""
    full = Path(os.path.abspath(str(path)))
    root = Path(root).resolve()
    for cand in (full, full.resolve()):
        try:
            return cand.relative_to(root).as_posix()
        except ValueError:
            pass
    return "~" if is_home(full) else full.name


# An absolute path in free text: POSIX with at least one directory (so "/60" and
# "and/or" are left alone, and the "//host/..." of a URL is not a path), a "~/"
# or "~user/" path, a Windows drive path or a UNC path. The "/" may follow an
# option word ("-L/abs", "-isystem/abs"), a ":" ("path:/abs", the entries of a
# PATH= list) or an "=", but not any other word, which is a relative path
# ("a/b/c", "rustkit-layout/src") or "${HOME}/x". A path with a space in it is
# cut at the space.
SEG = r"[\w.@+~$-]"
ABS_PATH = re.compile(
    r"(?:(?P<flag>(?<![\w-])--?[A-Za-z][\w-]*)|(?<![\w./\\~}]))(?P<posix>~?/(?:SEG+/)+SEG*)"
    r"|(?P<tilde>(?<![\w./\\~}])~[A-Za-z_][\w.-]*/(?:SEG+/?)*)"
    r"|(?<![A-Za-z])[A-Za-z]:[\\/](?:SEG+[\\/])*SEG*"
    r"|(?<![\w\\/:])\\\\SEG+(?:\\SEG+)+".replace("SEG", SEG))
# A credential carried as a URL query value.
SECRET_QUERY = re.compile(r"([?&](?:[\w-]*[_-])?(?:token|key|secret|password|passwd|pwd|auth|signature|sig)=)[^&\s#]+", re.I)


def public_text(text, root):
    def one(m):
        tok = m.group("posix") or m.group(0)
        body = tok.rstrip(".")
        rest = tok[len(body):]
        if m.group("posix"):
            return (m.group("flag") or "") + public_path(os.path.expanduser(body), root) + rest
        parts = re.split(r"[\\/]", body.rstrip("\\/"))
        if (m.group("tilde") and len(parts) == 1) or is_home(body):
            return "~" + rest
        return parts[-1] + rest
    if not text:
        return text
    text = re.sub(r"(\b[A-Za-z][\w+.-]*://)[^/\s@]*@", r"\1", text.replace("file://", ""))
    return ABS_PATH.sub(one, SECRET_QUERY.sub(r"\1REDACTED", text))


def public_repo(url):
    """An origin URL without its credentials or query; an scp-style origin
    with a path on the remote host as host:basename; a local-path origin as
    its basename."""
    if not url:
        return url
    m = re.match(r"([A-Za-z][\w+.-]*)://(?:[^/@]*@)?([^?#]*)", url)
    if m and m.group(1).lower() != "file":
        return m.group(1) + "://" + m.group(2)
    local = m.group(2) if m else url
    scp = None if m else re.match(r"(?:[\w.-]+@)?([\w.-]{2,}):(.*)", url)
    if scp and not scp.group(2).startswith(("/", "~", "\\")):
        return url
    name = re.split(r"[\\/]", (scp.group(2) if scp else local).rstrip("\\/"))[-1]
    return (scp.group(1) + ":" + name) if scp else (name or None)


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
    ap.add_argument("--host-paths", action="store_true", help="keep paths as given (local use; never post this output)")
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
    typed = Path(os.path.abspath(a.binary)) if a.binary else None
    origin = sh("git", "config", "--get", "remote.origin.url", cwd=root)
    shown = (lambda p: str(p)) if a.host_paths else (lambda p: public_path(p, root))
    text = (lambda t: t) if a.host_paths else (lambda t: public_text(t, root))
    rec = {
        "work_package": a.package,
        "base_sha": text(a.base),
        "candidate_sha": head,
        "dirty_state": dirty,
        "repo": origin if a.host_paths else public_repo(origin),
        "branch": sh("git", "rev-parse", "--abbrev-ref", "HEAD", cwd=root),
        "cargo_lock_sha256": lock_hash,
        "binary": shown(a.binary) if binary else None,
        "binary_sha256": sha256(binary) if binary else None,
        "binary_mtime_utc": utc_z(datetime.datetime.fromtimestamp(binary.stat().st_mtime, datetime.timezone.utc)) if binary and binary.exists() else None,
        "toolchain": toolchain,
        "cargo": cargo,
        "target": target,
        "profile": a.profile or next((p for p in ("parity", "release") if binary and (p in typed.parts or p in binary.parts)), None),
        "features": a.features,
        "rustflags": text(os.environ.get("RUSTFLAGS")),
        "effective_engine_flags": {k: text(v) for k, v in os.environ.items() if k.startswith("RUSTKIT_")},
        "host": platform.node() if a.host_paths else None,
        "os": platform.platform(),
        "cpu": platform.processor() or sh("sysctl", "-n", "machdep.cpu.brand_string"),
        "sccache": bool(shutil.which("sccache")),
        "chrome_version": None,
        "campaign": None,
        "raw_runs": [],
        "utc": utc_z(datetime.datetime.now(datetime.timezone.utc)),
        "note": text(a.note),
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
            rec["campaign"] = {"file": shown(a.campaign), "sha256": sha256(a.campaign), "cases": len(r),
                               "passed": c.get("passed"), "failed": c.get("failed"),
                               "mean_diff_pct": round(sum(vals) / len(vals), 4) if vals else None,
                               "timestamp": c.get("timestamp")}
        except Exception as e:
            rec["campaign"] = {"file": shown(a.campaign), "error": text(str(e))}
    for f in a.runs:
        p = Path(f)
        rec["raw_runs"].append({"file": shown(f), "sha256": sha256(p) if p.exists() else None, "bytes": p.stat().st_size if p.exists() else None})

    if a.markdown:
        print("## Receipt (`scripts/receipt.py`)\n")
        print("```json")
        print(json.dumps(rec, indent=1, sort_keys=True))
        print("```")
    else:
        print(json.dumps(rec, indent=1, sort_keys=True))


if __name__ == "__main__":
    main()
