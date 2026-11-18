#!/usr/bin/env python3
"""Write crates/rustkit-bindings/src/likely_subtags.txt from CLDR's likelySubtags.json.

  python3 scripts/cldr_likely_subtags.py <cldr-json>/cldr-core/supplemental/likelySubtags.json

One line per CLDR entry, sorted by key: `key=ScrpRG` when the likely language is
the key's own, `key=lang-ScrpRG` otherwise (RG is two letters or three digits).
The lookup is web_intl_likely.rs; the algorithm over it is in web_intl.js.
"""
import json
import pathlib
import sys

src = json.load(open(sys.argv[1], encoding="utf-8"))["supplemental"]
version = src["version"]["_cldrVersion"]
lines = []
for key, value in src["likelySubtags"].items():
    lang, script, region = value.split("-")
    assert len(script) == 4 and len(region) in (2, 3), (key, value)
    own = key.split("-")[0] == lang
    lines.append((key, f"{key}={'' if own else lang + '-'}{script}{region}"))
lines = [line for _, line in sorted(lines)]  # by key alone: the lookup is a binary search
header = f"""# CLDR {version} likely subtags (common/supplemental/likelySubtags.xml, read from cldr-json {version}.0.0).
# Data only, re-encoded by scripts/cldr_likely_subtags.py: key=ScrpRG, or key=lang-ScrpRG when the language differs.
# Copyright (c) 1991-2025 Unicode, Inc. All rights reserved. Distributed under the Unicode License v3
# (SPDX: Unicode-3.0), https://www.unicode.org/license.txt
# The full notice and licence text ship in THIRD_PARTY_NOTICES.md at the repository root.
"""
out = pathlib.Path(__file__).resolve().parent.parent / "crates/rustkit-bindings/src/likely_subtags.txt"
out.write_text(header + "\n".join(lines) + "\n", encoding="utf-8")
print(f"CLDR {version}: {len(lines)} entries, {out.stat().st_size} bytes")
