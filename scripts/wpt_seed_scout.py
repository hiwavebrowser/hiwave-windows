#!/usr/bin/env python3
"""wpt_seed_scout.py — propose new MANIFEST entries by READING tests, not listings.

W0a seeded 14 entries from pinned directory *listings*, and said so in its own
known-gaps section: "the test->ref BINDING is not verified... growing the seed is
a W0b task with the tree actually checked out". This is that task, done the way
the runner already demands — WPT's authority for a reference is the test file's
own `<link rel=match href=...>`, so this scout fetches each candidate test at the
pinned SHA and reads it, instead of pattern-matching `-ref.html` neighbours.

For every candidate it reports one of:
  CANDIDATE  — reftest with a rel=match whose target exists at the pin, and
               which the W0b runner can actually render (no JS, no reftest-wait,
               no fuzzy annotation). These are printed as manifest-ready entries.
  UNRUNNABLE — a real reftest the runner would only ever SKIP. Named, not seeded:
               padding the manifest with permanent skips inflates the list
               without moving the denominator, which is the WPT-shaped version
               of a metric that cannot go red.
  NOT-REFTEST— no rel=match (testharness.js, manual, or a reference file).

Nothing is written to MANIFEST.json. The output is a proposal for a human (or
the next session) to paste, because a manifest that grows itself is a manifest
nobody read.

    python3 scripts/wpt_seed_scout.py --dir css/css-text/overflow-wrap [--dir ...]
    python3 scripts/wpt_seed_scout.py --bucket 1A [--limit 40] [--json out.json]
"""

import argparse
import json
import re
import sys
import urllib.error
import urllib.request
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
MANIFEST = REPO_ROOT / "trench" / "wpt" / "MANIFEST.json"
API = "https://api.github.com/repos/web-platform-tests/wpt/git/trees"
RAW = "https://raw.githubusercontent.com/web-platform-tests/wpt"

# Bucket -> directories, following trench/WPT_TIER1_SUBSET.md's shopping list.
BUCKET_DIRS = {
    "1A": [
        "css/css-text/overflow-wrap",
        "css/css-text/word-break",
        "css/css-text/line-break",
    ],
    "1B": ["css/css-inline"],
    "1C": ["css/css-flexbox"],
}

REL_MATCH_RE = re.compile(
    r"<link[^>]*rel=[\"']?match[\"']?[^>]*href=[\"']?([^\"'> ]+)", re.IGNORECASE
)
FUZZY_RE = re.compile(r"name=[\"']fuzzy[\"']")


def get_json(url):
    return json.load(urllib.request.urlopen(url, timeout=60))


def list_dir(pin, path):
    """Return the blob names directly under `path` at `pin` (one level, no recursion)."""
    sha = pin
    for part in path.split("/"):
        node = get_json(f"{API}/{sha}")
        match = [t for t in node["tree"] if t["path"] == part and t["type"] == "tree"]
        if not match:
            return None
        sha = match[0]["sha"]
    node = get_json(f"{API}/{sha}")
    return [t["path"] for t in node["tree"] if t["type"] == "blob"]


def fetch_text(pin, path, limit=8192):
    with urllib.request.urlopen(f"{RAW}/{pin}/{path}", timeout=60) as r:
        return r.read(limit).decode("utf-8", errors="replace")


def classify(pin, path, _unused=None):
    """Read one candidate test and decide whether it can be seeded."""
    name = path.rsplit("/", 1)[-1]
    rec = {"path": path, "id": None, "ref": None, "verdict": None, "reason": None}
    try:
        src = fetch_text(pin, path)
    except Exception as e:
        rec["verdict"], rec["reason"] = "ERROR", f"fetch failed: {e}"
        return rec

    m = REL_MATCH_RE.search(src)
    if not m:
        rec["verdict"], rec["reason"] = "NOT-REFTEST", "no <link rel=match>"
        return rec

    href = m.group(1)
    if href.startswith("/"):
        ref = href.lstrip("/")
    else:
        # normalise ./ and ../ textually — these are WPT-relative paths, not
        # local files, so the filesystem must not be consulted.
        parts = []
        for p in f"{Path(path).parent.as_posix()}/{href}".split("/"):
            if p == "..":
                if parts:
                    parts.pop()
            elif p not in ("", "."):
                parts.append(p)
        ref = "/".join(parts)

    # Existence is decided by fetching the ref at the pin, NOT by looking it up
    # in the test's own directory listing: WPT keeps many refs in a sibling
    # `reference/` directory, and a same-directory-only check called 126 present
    # files absent on this scout's first run. A tree walk per candidate would be
    # correct too, and slower; the fetch is needed anyway for the skip predicates.
    rec["ref"] = ref
    try:
        ref_src = fetch_text(pin, ref)
    except urllib.error.HTTPError as e:
        if e.code == 404:
            rec["verdict"], rec["reason"] = "UNRUNNABLE", f"rel=match target absent at pin: {ref}"
        else:
            rec["verdict"], rec["reason"] = "ERROR", f"ref fetch failed: {e}"
        return rec
    except Exception as e:
        rec["verdict"], rec["reason"] = "ERROR", f"ref fetch failed: {e}"
        return rec

    if "reftest-wait" in src or "reftest-wait" in ref_src:
        rec["verdict"], rec["reason"] = "UNRUNNABLE", "reftest-wait (needs scripted completion signal)"
    elif "<script" in src or "<script" in ref_src:
        rec["verdict"], rec["reason"] = "UNRUNNABLE", "needs JS (no script host in parity-capture path)"
    elif FUZZY_RE.search(src):
        rec["verdict"], rec["reason"] = "UNRUNNABLE", "fuzzy annotation (tolerance matching not implemented)"
    else:
        rec["verdict"] = "CANDIDATE"

    stem = name[: -len(".html")] if name.endswith(".html") else name
    rec["id"] = f"{path.split('/')[1]}/{'/'.join(path.split('/')[2:-1])}/{stem}".replace("//", "/")
    return rec


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--dir", action="append", default=[], help="WPT-relative directory to scout")
    ap.add_argument("--bucket", action="append", default=[], choices=sorted(BUCKET_DIRS), help="scout a Tier-1 bucket's directories")
    ap.add_argument("--limit", type=int, default=60, help="max candidate tests to READ per directory")
    ap.add_argument("--json", default=None, help="write the full proposal to this file")
    args = ap.parse_args()

    manifest = json.loads(MANIFEST.read_text(encoding="utf-8"))
    pin = manifest["wpt_pin"]
    seeded = {e["path"] for e in manifest["entries"]}

    dirs = list(args.dir)
    for b in args.bucket:
        dirs.extend(BUCKET_DIRS[b])
    if not dirs:
        print("nothing to scout: pass --dir or --bucket", file=sys.stderr)
        return 2

    proposals = []
    for d in dirs:
        names = list_dir(pin, d)
        if names is None:
            print(f"MISSING DIR {d}", file=sys.stderr)
            continue
        existing = {f"{d}/{n}" for n in names}
        candidates = [
            f"{d}/{n}"
            for n in sorted(names)
            if n.endswith(".html")
            and not n.endswith("-ref.html")
            and "-manual" not in n
            and f"{d}/{n}" not in seeded
        ]
        truncated = len(candidates) > args.limit
        if truncated:
            candidates = candidates[: args.limit]
        print(f"\n=== {d}: {len(names)} files, reading {len(candidates)} candidate tests"
              + (f"  [TRUNCATED at --limit {args.limit}]" if truncated else ""))
        with ThreadPoolExecutor(max_workers=8) as pool:
            recs = list(pool.map(lambda p: classify(pin, p, existing), candidates))
        for r in recs:
            r["dir"] = d
            r["truncated_scan"] = truncated
            proposals.append(r)
            if r["verdict"] != "NOT-REFTEST":
                print(f"  {r['verdict']:<11} {r['path']}  {r['reason'] or '-> ' + str(r['ref'])}")

    ok = [r for r in proposals if r["verdict"] == "CANDIDATE"]
    unrun = [r for r in proposals if r["verdict"] == "UNRUNNABLE"]
    notref = [r for r in proposals if r["verdict"] == "NOT-REFTEST"]
    err = [r for r in proposals if r["verdict"] == "ERROR"]
    print(f"\nscouted {len(proposals)} | CANDIDATE {len(ok)} | UNRUNNABLE {len(unrun)} "
          f"| not-reftest {len(notref)} | error {len(err)}")
    if any(r["truncated_scan"] for r in proposals):
        print("NOTE: at least one directory was truncated by --limit — this scan is a sample, not a census.")

    if ok:
        print("\n--- manifest-ready entries (paste into MANIFEST.json entries, set tier by hand) ---")
        print(json.dumps([{"id": r["id"], "tier": "?", "kind": "reftest",
                           "path": r["path"], "ref": r["ref"]} for r in ok], indent=2))

    if args.json:
        Path(args.json).write_text(json.dumps({"pin": pin, "proposals": proposals}, indent=2) + "\n", encoding="utf-8")
        print(f"\nwrote {args.json}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
