#!/usr/bin/env python3
"""Verify a frozen livesuite snapshot is actually offline-renderable.

The freezer's ONE claim is that rendering the snapshot touches no network.
If that claim is false the whole lane is measuring the weather again, and
the failure is silent: a page that quietly fetches a live asset scores
differently on every run and nobody sees why.

So the check is mechanical and runs in CI, not "I looked at it once".

Distinguishes FETCHED references (img/script/link/css-url — the renderer
goes and gets these) from NAVIGATION targets (anchor href — a link's
destination is correctly left absolute; rewriting it would change what the
page means).
"""
import json
import re
import sys
from pathlib import Path

FETCH_PATTERNS = [
    (r'<(?:img|script|iframe|source)\b[^>]*\ssrc="([^"]+)"', "src"),
    (r'url\((["\']?)(https?://[^)"\']+)\1\)', "css-url"),
]


def fetched_refs(html: str):
    out = []
    for m in re.finditer(FETCH_PATTERNS[0][0], html, re.I):
        out.append(("src", m.group(1)))
    for m in re.finditer(r"<link\b[^>]*>", html, re.I):
        tag = m.group(0)
        if re.search(r'rel="[^"]*(stylesheet|icon|preload)', tag, re.I):
            href = re.search(r'href="([^"]+)"', tag, re.I)
            if href:
                out.append(("link", href.group(1)))
    for m in re.finditer(FETCH_PATTERNS[1][0], html, re.I):
        out.append(("css-url", m.group(2)))
    return out


def verify(snapshot: Path):
    problems = []
    manifest_path = snapshot / "manifest.json"
    if not manifest_path.exists():
        return [f"{snapshot.name}: no manifest.json"]
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))

    # A non-200 freeze is usually an interstitial (consent wall, bot
    # challenge, login gate). Scoring one as if it were the site is the
    # false-green this lane exists to prevent.
    if manifest.get("http_status") != 200:
        problems.append(
            f"{snapshot.name}: froze HTTP {manifest.get('http_status')} — "
            "likely an interstitial, not the page"
        )

    # Review is a human act. An unreviewed snapshot must not score, because
    # only a human can tell "the real page" from "a plausible-looking wall".
    if manifest.get("review", {}).get("status") != "REVIEWED":
        problems.append(
            f"{snapshot.name}: review status is "
            f"{manifest.get('review', {}).get('status')!r}, not REVIEWED"
        )

    html_path = snapshot / "index.html"
    if not html_path.exists():
        return problems + [f"{snapshot.name}: no index.html"]
    html = html_path.read_text(errors="replace", encoding="utf-8")

    remote = [(k, u) for k, u in fetched_refs(html) if u.startswith("http")]
    for kind, url in remote[:10]:
        problems.append(f"{snapshot.name}: {kind} still remote at render time: {url[:100]}")

    # Every local reference must exist on disk. A rewritten path pointing at
    # nothing renders as a hole that looks exactly like an engine bug.
    for kind, url in fetched_refs(html):
        if url.startswith("http") or url.startswith("data:"):
            continue
        if not (snapshot / url.split("?")[0]).exists():
            problems.append(f"{snapshot.name}: {kind} points at missing local file: {url[:80]}")

    return problems


def main():
    root = Path(sys.argv[1] if len(sys.argv) > 1 else "livesuite")
    if not root.exists():
        print(f"no livesuite root at {root} — nothing to verify")
        return 0
    snapshots = [d for d in sorted(root.iterdir()) if d.is_dir()]
    if not snapshots:
        print(f"{root} has no snapshots")
        return 0

    all_problems = []
    for snap in snapshots:
        probs = verify(snap)
        all_problems += probs
        print(f"{'FAIL' if probs else 'ok  '}  {snap.name}")
        for p in probs:
            print(f"        {p}")

    print(f"\n{len(snapshots) - len({p.split(':')[0] for p in all_problems})}/{len(snapshots)} snapshots offline-clean")
    return 1 if all_problems else 0


if __name__ == "__main__":
    sys.exit(main())
