#!/usr/bin/env python3
"""wpt_import.py: choose the coverage reftests in trench/wpt/MANIFEST.json (T15).

The manifest stays the source of truth and this only writes it: run it by hand
when an area changes, against a WPT tree at the pin that holds the areas below
plus /common, /resources, /images and /fonts. The run itself never needs it.

An area takes the reftests the runner can score, sorted by path, evenly spread
down to its cap so a selection is not just the 001s. The same tree gives the
same entries, and the seed (entries with no `area`) is never touched.

    python3 scripts/wpt_import.py <wpt tree>
"""

import json
import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import wpt_tier1  # noqa: E402

# `include` is a regex on the path inside `dir`. `row` is the Z plan row a FAIL here goes to.
AREAS = {
    "css-overflow": {
        "dir": "css/css-overflow", "row": "C2", "cap": 40,
        "include": r"^(?!line-clamp/|scroll-markers/|column-|.*scroll-marker|.*scroll-button)[^/]+$",
    },
    "css-position-sticky": {
        "dir": "css/css-position", "row": "C7", "cap": 30, "include": r"^sticky/",
    },
    "css-position-absolute": {
        "dir": "css/css-position", "row": "C15", "cap": 30,
        "include": r"^(static-position/|[^/]*(absolute|containing-block|abspos)[^/]*$)",
    },
    "css-transforms-2d": {
        "dir": "css/css-transforms", "row": "C10", "cap": 40,
        "include": r"^(translate/|scale/|rotate/|skew[XY]?/|matrix/|transform-origin/"
                   r"|(?!.*(3d|perspective|preserve|backface))[^/]*transform[^/]*$)",
    },
    "css-transforms-3d": {
        "dir": "css/css-transforms", "row": "C10", "cap": 30,
        "include": r"^(?!.*(hit-test|animation))[^/]*(3d|perspective|preserve|backface)[^/]*$",
    },
    "css-masking-clip-path": {
        "dir": "css/css-masking", "row": "C8", "cap": 30, "include": r"^clip-path/",
    },
    "css-masking-mask": {
        "dir": "css/css-masking", "row": "C8", "cap": 30, "include": r"^mask-image/",
    },
    "css-ui-outline": {
        "dir": "css/css-ui", "row": "C5", "cap": 30, "include": r"^outline[^/]*$",
    },
    "css-lists": {
        "dir": "css/css-lists", "row": "C6", "cap": 30, "include": r"^(?!counter-|.*-counter)[^/]+$",
    },
    "css-text-basics": {
        "dir": "css/css-text", "row": "C6", "cap": 40,
        "include": r"^(white-space|text-align|text-indent|letter-spacing|word-spacing|text-transform)/",
    },
}

NOT_A_TEST = re.compile(r"(^|/)(reference|references|support|resources)/|-ref\b|-notref\b|\.tentative\.|\.__")


def candidates(root: Path, area):
    """Test-page paths of an area, sorted."""
    base = root / area["dir"]
    include = re.compile(area["include"])
    found = []
    for page in base.rglob("*.html"):
        inside = page.relative_to(base).as_posix()
        if not NOT_A_TEST.search(inside) and include.search(inside):
            found.append(page.relative_to(root).as_posix())
    return sorted(found)


def usable_ref(page: Path, root: Path):
    """The page's reference, relative to the root, if the runner can score it."""
    src = page.read_text(errors="replace", encoding="utf-8")
    tags = wpt_tier1.page_tags(src).tags
    links = [attrs for tag, attrs in tags if tag == "link"]
    if any(wpt_tier1._rel_has(attrs, "mismatch") for attrs in links):
        return None
    hrefs = wpt_tier1.rel_match_hrefs(page)
    if len(hrefs) != 1 or wpt_tier1.unrunnable_reason(src, ""):
        return None
    ref = (page.parent / hrefs[0]).resolve()
    if not ref.is_file() or ref.suffix != ".html":
        return None
    if wpt_tier1.unrunnable_reason(ref.read_text(errors="replace", encoding="utf-8"), ""):
        return None
    if wpt_tier1.missing_resources(page, root) or wpt_tier1.missing_resources(ref, root):
        return None
    return ref.relative_to(root.resolve()).as_posix()


def spread(items, cap):
    """Up to `cap` items, evenly spaced, the first one always in."""
    if len(items) <= cap:
        return list(items)
    return [items[i * len(items) // cap] for i in range(cap)]


def support_of(pages, root: Path):
    """Everything those pages load from disk, the pages themselves left out."""
    root = root.resolve()
    found = set()
    for rel in pages:
        for _url, target in wpt_tier1.local_resources(root / rel, root):
            found.add(target.resolve().relative_to(root).as_posix())
    return sorted(found - set(pages))


def area_entries(root: Path):
    root = root.resolve()
    entries = []
    for name, area in AREAS.items():
        usable = []
        for rel in candidates(root, area):
            ref = usable_ref(root / rel, root)
            if ref:
                usable.append((rel, ref))
        chosen = spread(usable, area["cap"])
        print(f"{name}: {len(usable)} usable, {len(chosen)} taken")
        for rel, ref in chosen:
            entries.append({
                "id": rel.removeprefix("css/").removesuffix(".html"),
                "path": rel,
                "ref": ref,
                "kind": "reftest",
                "tier": "2",
                "area": name,
                "maps_to": area["row"],
            })
    return entries


def main():
    root = Path(sys.argv[1])
    manifest = json.loads(wpt_tier1.MANIFEST_PATH.read_text(encoding="utf-8"))
    seed = [e for e in manifest["entries"] if not e.get("area")]
    seed_ids = {e["id"] for e in seed}
    seen = set()
    added = []
    for entry in area_entries(root):
        if entry["id"] not in seed_ids and entry["id"] not in seen:
            seen.add(entry["id"])
            added.append(entry)
    pages = [p for e in seed + added for p in (e["path"], e["ref"])]
    paths = set(manifest["support_paths"]["paths"]) | set(support_of(pages, root))
    manifest["support_paths"]["paths"] = sorted(paths - set(pages))
    manifest["entries"] = seed + added
    wpt_tier1.MANIFEST_PATH.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    print(f"manifest: {len(seed)} seed + {len(added)} coverage, {len(paths)} support files")


if __name__ == "__main__":
    main()
