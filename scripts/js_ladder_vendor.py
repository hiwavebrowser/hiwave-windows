#!/usr/bin/env python3
"""Vendor js-ladder rung 1 (MDN learning-area, CC0) fixtures.

Reads websuite/js-ladder/01-mdn/manifest.json. For each fixture, downloads the
entry page from mdn/learning-area at a pinned commit into
websuite/js-ladder/01-mdn/<id>/index.html, plus the relative scripts,
stylesheets and images it references (and the url() refs of those
stylesheets), keeping their paths relative to the page.

    python3 scripts/js_ladder_vendor.py            # pin to current main
    python3 scripts/js_ladder_vendor.py --commit <sha>

The pinned commit is written back into the manifest as "commit".
"""
import json
import posixpath
import re
import sys
import urllib.request
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
RUNG_DIR = REPO_ROOT / "websuite" / "js-ladder" / "01-mdn"
MANIFEST = RUNG_DIR / "manifest.json"
API = "https://api.github.com/repos/mdn/learning-area/commits/main"
RAW = "https://raw.githubusercontent.com/mdn/learning-area/{commit}/{path}"

HTML_REF = re.compile(r'''(?:src|href)\s*=\s*["']([^"'#?]+)["']''', re.I)
CSS_REF = re.compile(r'''url\(\s*["']?([^"')#?]+)["']?\s*\)''', re.I)


def fetch(url: str) -> bytes:
    req = urllib.request.Request(url, headers={"User-Agent": "hiwave-js-ladder-vendor"})
    with urllib.request.urlopen(req, timeout=30) as r:
        return r.read()


def local_ref(ref: str) -> bool:
    return not re.match(r"^[a-z][a-z0-9+.-]*:|^//", ref, re.I)


def vendor_fixture(commit: str, fx: dict) -> list:
    src = fx["src"]
    base = posixpath.dirname(src)
    out_dir = RUNG_DIR / fx["id"]
    out_dir.mkdir(parents=True, exist_ok=True)
    written = []

    def get(rel: str, dest: Path) -> bytes:
        data = fetch(RAW.format(commit=commit, path=posixpath.join(base, rel)))
        dest.parent.mkdir(parents=True, exist_ok=True)
        dest.write_bytes(data)
        written.append(str(dest.relative_to(RUNG_DIR)))
        return data

    html = get(posixpath.basename(src), out_dir / "index.html").decode("utf-8", "replace")
    # "extra": files only script references (e.g. the gallery's createElement'd imgs)
    queue = [r for r in HTML_REF.findall(html) if local_ref(r)] + fx.get("extra", [])
    seen = set()
    while queue:
        rel = posixpath.normpath(queue.pop(0))
        if rel in seen:
            continue
        seen.add(rel)
        if rel.startswith("..") or rel.startswith("/"):
            print(f"  {fx['id']}: skip out-of-dir ref {rel}", file=sys.stderr)
            continue
        try:
            data = get(rel, out_dir / rel)
        except Exception as e:  # a dead link in the example stays dead here too
            print(f"  {fx['id']}: {rel}: {e}", file=sys.stderr)
            continue
        if rel.endswith(".css"):
            css_dir = posixpath.dirname(rel)
            for u in CSS_REF.findall(data.decode("utf-8", "replace")):
                if local_ref(u):
                    queue.append(posixpath.join(css_dir, u))
    return written


def main() -> int:
    manifest = json.loads(MANIFEST.read_text(encoding="utf-8"))
    commit = None
    if "--commit" in sys.argv:
        commit = sys.argv[sys.argv.index("--commit") + 1]
    if not commit:
        commit = json.loads(fetch(API))["sha"]
    manifest["commit"] = commit
    for fx in manifest["fixtures"]:
        files = vendor_fixture(commit, fx)
        print(f"{fx['id']}: {len(files)} file(s)")
    MANIFEST.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    print(f"pinned mdn/learning-area @ {commit}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
