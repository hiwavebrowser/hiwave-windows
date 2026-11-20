"""Copy the repo files that synced engine crates embed from OUTSIDE their own crate.

An engine refresh copies crates/rustkit-* verbatim from hiwave-macos. Some crates pull
fixtures from elsewhere in the macOS tree with include_bytes!/include_str!, for example

    include_bytes!("../../../websuite/js-ladder/01-mdn/32-image-gallery/images/pic4.jpg")
    include_str!("../../../../hiwave-app/src/ui/inspector_rustkit.html")
    include_str!(concat!("../../../fixtures/consent-overlay/", $name))

hiwave-windows does not carry all of those trees, and the ones it carries go stale. This
script finds every literal include whose target leaves its crate (or, for the concat!
form, every tracked file in that directory on the macOS commit) and writes it as a raw git
blob (no line-ending conversion) from the macOS checkout when it is:

  - missing: always copied;
  - present outside crates/ and different from the macOS blob: refreshed. A copy that
    differs only in line endings (core.autocrlf) is not different.

An existing file under crates/ is never overwritten: synced crates own their files (with
Windows' patches applied after the copy), and hiwave-app's UI pages are a Windows fork.

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


def out_of_crate_targets() -> dict[str, Path]:
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
                rel = target.relative_to(REPO.resolve()).as_posix()
                out[rel + "/" if is_dir else rel] = src
    return out


def same_content(a: bytes, b: bytes) -> bool:
    """Equal up to CRLF vs LF (a checkout with core.autocrlf rewrites line endings)."""
    return a == b or a.replace(b"\r\n", b"\n") == b.replace(b"\r\n", b"\n")


def main() -> int:
    if len(sys.argv) != 2:
        print(__doc__)
        return 2
    mac = Path(sys.argv[1])
    commit = git(mac, "rev-parse", "HEAD").stdout.decode().strip()
    added, refreshed, bad = [], [], []
    for rel, src in sorted(out_of_crate_targets().items()):
        if rel.endswith("/"):
            names = git(mac, "ls-tree", "-r", "--name-only", commit, rel).stdout.decode().split()
        else:
            names = [rel]
        for name in names:
            dest = REPO / name
            if dest.exists() and name.startswith("crates/"):
                continue  # owned by the crate copy or a Windows fork
            r = git(mac, "cat-file", "blob", f"{commit}:{name}")
            if r.returncode != 0:
                if not dest.exists():
                    bad.append((name, src.relative_to(REPO.resolve()).as_posix()))
                continue  # present here, not on the Mac: nothing to follow
            if dest.exists():
                if same_content(dest.read_bytes(), r.stdout):
                    continue
                refreshed.append(name)
            else:
                added.append(name)
            dest.parent.mkdir(parents=True, exist_ok=True)
            dest.write_bytes(r.stdout)
        if rel.endswith("/") and not names and not (REPO / rel).exists():
            bad.append((rel, src.relative_to(REPO.resolve()).as_posix()))
    print(f"engine fixtures from hiwave-macos {commit[:8]}: {len(added)} added, {len(refreshed)} refreshed")
    for name in refreshed:
        print(f"  refreshed: {name}")
    for rel, src in bad:
        print(f"  ERROR: {rel} (included by {src}) is not tracked on {commit[:8]}")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
