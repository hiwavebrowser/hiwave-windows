"""Copy the repo files that synced engine crates embed from OUTSIDE their own crate.

An engine refresh copies crates/rustkit-* verbatim from hiwave-macos. Some crates pull
fixtures from elsewhere in the macOS tree with include_bytes!/include_str!, for example

    include_bytes!("../../../websuite/js-ladder/01-mdn/32-image-gallery/images/pic4.jpg")
    include_str!("../../../../hiwave-app/src/ui/inspector_rustkit.html")
    include_str!(concat!("../../../fixtures/consent-overlay/", $name))

hiwave-windows does not carry those trees, so the crate fails to compile here. This
script finds every literal include whose target leaves its crate and is missing, then
copies that file (or, for the concat! form, every tracked file in that directory) as a
raw git blob (no line-ending conversion) from the macOS checkout. It never overwrites a
file that exists.

    python scripts/sync_engine_fixtures.py <path-to-hiwave-macos-checkout>

Run it after the crate copy and before the first build. Exits 1 if a missing target is
not tracked on the macOS commit (the include is then wrong, not the sync).
"""
from __future__ import annotations

import re
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
INCLUDE_FILE = re.compile(r'include_(?:bytes|str)!\(\s*"([^"]+)"')
# The file name is a macro argument, so the directory is the unit.
INCLUDE_DIR = re.compile(r'include_(?:bytes|str)!\(\s*concat!\(\s*"([^"]+/)"')


def git(mac: Path, *args: str) -> subprocess.CompletedProcess:
    return subprocess.run(["git", "-C", str(mac), *args], capture_output=True)


def missing_targets() -> dict[str, Path]:
    """repo-relative target (trailing '/' = directory) -> the source file that includes it."""
    out: dict[str, Path] = {}
    crates = (REPO / "crates").resolve()
    for src in crates.rglob("*.rs"):
        parts = src.relative_to(crates).parts
        if "target" in parts:
            continue
        crate_root = crates / parts[0]
        try:
            text = src.read_text(encoding="utf-8")
        except (OSError, UnicodeDecodeError):
            continue
        for pattern, is_dir in ((INCLUDE_FILE, False), (INCLUDE_DIR, True)):
            for m in pattern.finditer(text):
                target = (src.parent / m.group(1)).resolve()
                if target.is_relative_to(crate_root):
                    continue  # inside its own crate: copied with the crate
                if not target.exists():
                    rel = target.relative_to(REPO.resolve()).as_posix()
                    out[rel + "/" if is_dir else rel] = src
    return out


def main() -> int:
    if len(sys.argv) != 2:
        print(__doc__)
        return 2
    mac = Path(sys.argv[1])
    commit = git(mac, "rev-parse", "HEAD").stdout.decode().strip()
    written, bad = 0, []
    for rel, src in sorted(missing_targets().items()):
        if rel.endswith("/"):
            names = git(mac, "ls-tree", "-r", "--name-only", commit, rel).stdout.decode().split()
        else:
            names = [rel]
        blobs = []
        for name in names:
            r = git(mac, "cat-file", "blob", f"{commit}:{name}")
            if r.returncode != 0:
                blobs = []
                break
            blobs.append((name, r.stdout))
        if not blobs:
            bad.append((rel, src.relative_to(REPO.resolve()).as_posix()))
            continue
        for name, data in blobs:
            dest = REPO / name
            dest.parent.mkdir(parents=True, exist_ok=True)
            dest.write_bytes(data)
            written += 1
    print(f"engine fixtures from hiwave-macos {commit[:8]}: {written} file(s) written")
    for rel, src in bad:
        print(f"  ERROR: {rel} (included by {src}) is not tracked on {commit[:8]}")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
