#!/usr/bin/env python3
"""Write THIRD_PARTY_NOTICES.md: the notices of every third-party work in the shipped binary.

  python3 scripts/third_party_notices.py            # rewrite THIRD_PARTY_NOTICES.md
  python3 scripts/third_party_notices.py --check    # exit 1 if the file is out of date
  (add --offline to pass --offline to cargo)

The crate list is `cargo tree -p hiwave-app -e normal` (default features) for both macOS
targets, HiWave's own crates left out. Licence expressions and repository URLs are each
crate's `Cargo.toml` fields. Copyright lines and licence texts are read from the crates'
published sources, which `cargo fetch` puts under ~/.cargo/registry. Texts that no crate in
the tree carries are in scripts/third_party_notices_data/.

What this script cannot find is written by hand below and must be kept current by hand:
works copied into the source that are not crates (Chart.js, the CLDR tables, code ported
into the vendored Boa), section 6, and the obligations text. A licence file below a crate's
root that the NESTED table does not know stops the run, so a new one gets read by a person.
"""
import collections
import json
import os
import pathlib
import re
import subprocess
import sys

REPO = pathlib.Path(__file__).resolve().parent.parent
DATA = pathlib.Path(__file__).resolve().parent / "third_party_notices_data"
OUT = REPO / "THIRD_PARTY_NOTICES.md"
TARGETS = ("aarch64-apple-darwin", "x86_64-apple-darwin")
OFFLINE = ["--offline"] if "--offline" in sys.argv else []

# Where a crate offers a choice (`A OR B`), the first of these it offers is used.
PREF = ["MIT", "Apache-2.0", "BSD-3-Clause", "BSD-2-Clause", "ISC", "Zlib",
        "MPL-2.0", "Unicode-3.0", "CDLA-Permissive-2.0", "CC0-1.0"]

# Crates whose root licence files name several holders or parts: reproduced whole in section 5.
FULL = {
    "dpi": ["LICENSE-LIBM-MIT"],
    "libm": ["LICENSE.txt"],
    "aws-lc-rs": ["LICENSE"],
    "aws-lc-sys": ["LICENSE"],
    "ring": ["LICENSE", "LICENSE-other-bits", "LICENSE-BoringSSL"],
}

# Licence files below a crate's root (and root files that are not a plain licence).
# value: ("carry", note) reproduces the file in section 5; ("skip", why) says why not.
NESTED = {
    ("aws-lc-sys", "aws-lc/LICENSE"): ("skip", "the same text as the crate's root `LICENSE`, apart from trailing whitespace"),
    ("aws-lc-sys", "aws-lc/third_party/fiat/LICENSE"): ("carry", "fiat-crypto field arithmetic inside AWS-LC."),
    ("ring", "third_party/fiat/LICENSE"): ("carry", "fiat-crypto field arithmetic inside ring."),
    ("ring", "src/polyfill/once_cell/LICENSE-MIT"): ("carry", "ring's copy of code from the once_cell project, `MIT OR Apache-2.0`; used under MIT."),
    ("ring", "src/polyfill/once_cell/LICENSE-APACHE"): ("skip", "the Apache-2.0 text, which is in section 4; the once_cell code is used under MIT"),
    ("regex-syntax", "src/unicode_tables/LICENSE-UNICODE"): ("carry", "Unicode data tables compiled into the crate, under the older Unicode data-files agreement (not Unicode-3.0)."),
    ("tracing-core", "src/spin/LICENSE"): ("carry", "tracing-core's copy of the spin crate's `Once`."),
    ("security-framework", "THIRD_PARTY"): ("carry", "Covers documentation comments adapted from Apple's Security framework headers, under the Apple Public Source License 2.0. Documentation only: no Apple code is compiled into the binary."),
    ("libsqlite3-sys", "sqlcipher/LICENSE"): ("skip", "SQLCipher is built only with the `sqlcipher` features, which HiWave does not enable"),
}
NESTED_NAME = re.compile(r"^(licen[sc]e|copying|notice|third[_-]?party|patents)", re.I)
SOURCE_EXT = {".rs", ".c", ".h", ".cc", ".cpp", ".py", ".js", ".go", ".S", ".s", ".pl", ".toml", ".json", ".html"}

MIT = """Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
"""
BSD3 = """Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are met:

1. Redistributions of source code must retain the above copyright notice, this
   list of conditions and the following disclaimer.

2. Redistributions in binary form must reproduce the above copyright notice,
   this list of conditions and the following disclaimer in the documentation
   and/or other materials provided with the distribution.

3. Neither the name of the copyright holder nor the names of its contributors
   may be used to endorse or promote products derived from this software
   without specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND
ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED
WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE
FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL
DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR
SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER
CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY,
OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
"""


def cargo(*args):
    return subprocess.run(["cargo", *args, "--locked", *OFFLINE], cwd=REPO, check=True,
                          capture_output=True, text=True, encoding="utf-8").stdout


def read(p):
    return pathlib.Path(p).read_text(encoding="utf-8", errors="replace")


def strip_head(text, marker):
    """Drop everything before the first line matching `marker` (the copyright lines)."""
    lines = text.splitlines()
    for i, line in enumerate(lines):
        if re.search(marker, line):
            return "\n".join(lines[i:]).rstrip() + "\n"
    raise SystemExit("marker not found: " + marker)


def fence(text):
    assert "````" not in text
    body = "\n".join(line.rstrip() for line in text.rstrip("\n").splitlines())  # no trailing blanks
    return "````text\n" + body + "\n````\n"


# ---- the shipped set
meta = json.loads(cargo("metadata", "--format-version", "1"))
pkgs = {(p["name"], p["version"]): p for p in meta["packages"]}
rows = {}
for target in TARGETS:
    tree = cargo("tree", "-p", "hiwave-app", "-e", "normal", "--prefix", "none",
                 "--format", "{p}|{l}|{r}", "--target", target)
    for line in tree.splitlines():
        if not line:
            continue
        head, lic, url = line.split("|")
        head = re.sub(r" \(\*\)$", "", head)
        m = re.match(r"^(\S+) v(\S+)((?: \([^)]*\))*)$", head)
        name, ver, extra = m.group(1), m.group(2), m.group(3)
        if str(REPO / "crates") + "/" in extra:
            continue  # HiWave's own
        row = rows.setdefault((name, ver), {"targets": set(), "lic": lic, "url": url,
                                            "proc": "proc-macro" in extra,
                                            "vendored": str(REPO / "third_party") + "/" in extra})
        row["targets"].add(target)


def crate_dir(name, ver=None):
    if ver is None:
        ver = max(v for (n, v) in rows if n == name)
    return pathlib.Path(pkgs[(name, ver)]["manifest_path"]).parent


def version_of(name):
    return max(v for (n, v) in rows if n == name)


def options(expr):
    expr = expr.replace(" / ", " OR ").replace("/", " OR ")
    if " AND " in expr:
        return None
    return [o.strip() for o in expr.split(" OR ")]


# ---- copyright lines out of a crate's root licence files
BOILER = re.compile(r"\[yyyy\]|\{yyyy\}|<year>|\[name of copyright owner\]|\{name of copyright owner\}|<copyright holders>|\[year\]", re.I)
CR = re.compile(r"^(?:Copyright\b|COPYRIGHT\b|©|\(c\)\s|\(C\)\s)")
NOT_NOTICE = re.compile(r"^copyright (?:notice|and related|holders?|owner|license|law|statement|and permission)|^copyright\s*$", re.I)
LICFILE = re.compile(r"^(licen[sc]e|copying|copyright|notice|unlicense)", re.I)
RETAINED = re.compile(r"Copyrights in the [\w ]+? (?:project|Project) are retained by their contributors")
PROJECT_IS = re.compile(r"This project is copyright [^.\n]*(?:\n[^.\n]*)?")


def copyright_lines(name, ver):
    out = []
    for f in sorted(crate_dir(name, ver).iterdir()):
        if not f.is_file() or not LICFILE.match(f.name):
            continue
        text = read(f)
        for rx in (RETAINED, PROJECT_IS):
            m = rx.search(text)
            if m:
                s = re.sub(r"\s+", " ", m.group(0)).strip()
                s = re.sub(r" \(given in the file AUTHORS\)$", "", s)
                if s not in out:
                    out.append(s)
        for line in text.splitlines():
            s = line.strip().strip("`").strip()
            s = re.sub(r"^[#*>/\s-]+", "", s)
            if not CR.match(s) or BOILER.search(s) or NOT_NOTICE.match(s):
                continue
            named = re.match(r"^Copyright (?:\(c\) |© )?[A-Z]", s) and len(s) < 100
            if not (re.search(r"\d{4}|authors|developers|contributors|inc\.|project", s, re.I) or named):
                continue
            s = re.sub(r"\s+", " ", s).rstrip(".")
            if s not in out:
                out.append(s)
    return out


def nested_files(name, ver):
    d = crate_dir(name, ver)
    found = []
    for root, dirs, files in os.walk(d):
        dirs[:] = sorted(x for x in dirs if x not in ("target", ".git"))
        for f in sorted(files):
            rel = (pathlib.Path(root) / f).relative_to(d)
            if not NESTED_NAME.match(f) or pathlib.Path(f).suffix in SOURCE_EXT:
                continue
            at_root = len(rel.parts) == 1
            if at_root and re.match(r"^(licen[sc]e|copying|notice)", f, re.I):
                continue  # root licence files are read by copyright_lines / FULL
            found.append(rel.as_posix())
    return found


stats = collections.Counter()
families = collections.defaultdict(list)
compound = []
carried = []
skipped = []
unknown_nested = []
for (name, ver), row in sorted(rows.items(), key=lambda kv: (kv[0][0].lower(), kv[0][1])):
    for rel in nested_files(name, ver):
        action = NESTED.get((name, rel))
        if action is None:
            unknown_nested.append(f"{name} {ver}: {rel}")
        elif action[0] == "carry":
            carried.append((name, ver, rel, action[1]))
        else:
            skipped.append((name, ver, rel, action[1]))
    lines = copyright_lines(name, ver)
    p = pkgs[(name, ver)]
    url = row["url"] or p.get("homepage") or f"https://crates.io/crates/{name}/{ver}"
    if name in FULL:
        cr = lines[0] + ", and the other holders named in its licence files, which are reproduced in full in section 5"
        stats["from_file"] += 1
    elif lines:
        if len(lines) > 6:
            raise SystemExit(f"{name} {ver}: {len(lines)} copyright lines; add it to FULL")
        cr = "; ".join(lines)
        stats["from_file"] += 1
    elif name.startswith("boa_"):
        cr = "Copyright (c) 2019 Jason Williams (from `LICENSE-MIT` in boa-dev/boa at 337a3668; the published crate carries no licence file)"
        stats["boa"] += 1
    else:
        authors = ", ".join(re.sub(r"\s*<[^>]*>", "", a) for a in p.get("authors") or [])
        cr = (f"No copyright line in the published crate; Cargo.toml authors: {authors}" if authors
              else "No copyright line and no authors field in the published crate")
        stats["no_line"] += 1
    notes = []
    if row["vendored"]:
        notes.append(f"vendored in `third_party/{name}` and modified by HiWave, see section 3")
    if row["proc"]:
        notes.append("procedural macro: runs at build time; the code it generates is compiled in, the crate itself is not linked")
    if row["targets"] == {"x86_64-apple-darwin"}:
        notes.append("x86_64 builds only")
    if name == "psl":
        notes.append("the crate's code; the Public Suffix List it compiles in is MPL-2.0 data, see section 1")
    if name == "libsqlite3-sys":
        notes.append("the binding; it compiles in SQLite itself, see section 2")
    if any(c[0] == name for c in carried):
        notes.append("a further licence file inside the crate is reproduced in section 5")
    entry = {"name": name, "ver": ver, "lic": row["lic"], "cr": cr, "url": url, "notes": notes}
    opts = options(row["lic"])
    if opts is None:
        compound.append(entry)
        continue
    fam = next((f for f in PREF if f in opts), None)
    if fam is None:
        raise SystemExit(f"{name} {ver}: licence `{row['lic']}` has no family here. "
                         "GPL, LGPL, AGPL, SSPL or no licence stops the release (docs/GATES.md).")
    families[fam].append(entry)

if unknown_nested:
    raise SystemExit("licence files below a crate root that NESTED does not know; read them and "
                     "add a carry/skip entry:\n  " + "\n  ".join(unknown_nested))
for (name, rel) in NESTED:
    if any(n == name for (n, _) in rows) and not any(c[0] == name and c[2] == rel for c in carried + skipped):
        raise SystemExit(f"NESTED names {name}: {rel}, which is no longer in the crate")


def fmt(e):
    s = f"- **{e['name']} {e['ver']}** (declared `{e['lic']}`). {e['cr']}. <{e['url']}>"
    if e["notes"]:
        s += " [" + "; ".join(e["notes"]) + "]"
    return s


APACHE = read(crate_dir("serde") / "LICENSE-APACHE")
BSD2 = strip_head(read(crate_dir("arrayref") / "LICENSE"), r"^Redistribution and use")
ISC = strip_head(read(crate_dir("libloading") / "LICENSE"), r"^Permission to use")
ZLIB = strip_head(read(crate_dir("foldhash") / "LICENSE"), r"^This software is provided")
MPL = read(REPO / "LICENSES" / "MPL-2.0.txt")
UNI_FULL = read(crate_dir("icu_calendar") / "LICENSE")
UNI_PARTS = UNI_FULL.split("\n—\n")
UNI_CR = re.search(r"^Copyright © .*Unicode, Inc\.$", UNI_PARTS[0], re.M).group(0)
UNI = UNI_PARTS[0].replace(
    UNI_CR, "[the copyright line of each component: the crate list above, and section 2 for the CLDR data]")
ICU_TRAILER = UNI_PARTS[1].strip()
CDLA = read(crate_dir("webpki-roots") / "LICENSE")
CC0 = read(DATA / "CC0-1.0.txt")
V8_BSD = read(DATA / "v8-LICENSE.v8")
PSF = read(DATA / "v8-third_party-builtins-LICENSE")
SQLITE_VER = re.search(r'#define SQLITE_VERSION\s+"([^"]+)"',
                       read(crate_dir("libsqlite3-sys") / "sqlite3/sqlite3.c")).group(1)
TOOLCHAIN = re.search(r'channel\s*=\s*"([^"]+)"', read(REPO / "rust-toolchain.toml")).group(1)

TEXTS = {
    "MIT": ("MIT License", "Each component's copyright line above stands in for the `Copyright (c) <year> <holder>` line of the licence.", MIT),
    "Apache-2.0": ("Apache License 2.0", "", APACHE),
    "BSD-3-Clause": ("BSD 3-Clause License", "Each component's copyright line above stands in for the copyright line of the licence; \"the copyright holder\" in clause 3 is that component's holder (for tiny-skia: Google Inc. and Yevhenii Reizner). V8's own BSD-3-Clause notice, for the code ported into Boa, is in section 5.", BSD3),
    "BSD-2-Clause": ("BSD 2-Clause License", "", BSD2),
    "ISC": ("ISC License", "", ISC),
    "Zlib": ("zlib License", "", ZLIB),
    "MPL-2.0": ("Mozilla Public License 2.0", "The same text as `LICENSES/MPL-2.0.txt` in the repository, which also covers the HiWave files listed under \"Files under MPL-2.0\" in `NOTICE`. It also covers the Public Suffix List (section 1).", MPL),
    "Unicode-3.0": ("Unicode License v3", "", UNI),
    "CDLA-Permissive-2.0": ("Community Data License Agreement, Permissive, 2.0", "", CDLA),
    "CC0-1.0": ("Creative Commons CC0 1.0 Universal", "", CC0),
}

total = sum(len(v) for v in families.values()) + len(compound)
n_proc = sum(1 for r in rows.values() if r["proc"])
out = []
w = out.append

w("# Third-party notices")
w("")
w("<!-- Generated by scripts/third_party_notices.py. Edit the script, not this file. -->")
w("")
w("HiWave is licensed under the GNU Affero General Public License v3.0 or later")
w("(`LICENSE`; see `NOTICE`). The HiWave application also contains the")
w("third-party works listed here, each under its own")
w("licence. This file carries the copyright and permission notices those licences")
w("require to travel with the binary. It must ship with every build that users")
w("download.")
w("")
w("It describes one build: `cargo build -p hiwave-app` with default features, for")
w("`aarch64-apple-darwin` and `x86_64-apple-darwin`. How it is regenerated and")
w("checked is in section 7 and in `docs/GATES.md`.")
w("")
w("## Contents")
w("")
w("1. Licences with source or file-level obligations (MPL-2.0)")
w("2. Works that are not Cargo crates (Chart.js, Unicode CLDR data, SQLite, the Rust standard library)")
w("3. Vendored and modified source (`third_party/`: Boa, with code ported from V8 and atoi_simd)")
w(f"4. Rust crates in the dependency closure of the `hiwave` binary ({total} crates), by licence, with each licence text once")
w("5. Crates under more than one licence at once, and licence files reproduced in full")
w("6. What is in the repository but does not ship")
w("7. How this file was made, and what was not verified")
w("")
w("No component of the shipped binary is under a GPL, LGPL, AGPL or SSPL licence,")
w("and none is without a licence. Licences here that are not simple permissive ones:")
w("MPL-2.0 (section 1), the Python Software Foundation licence stack on code")
w("ported from V8 (section 3), and the Apple Public Source License 2.0 on")
w("documentation comments in one crate (security-framework, section 5).")
w("")
w("## 1. Licences with source or file-level obligations")
w("")
w("### MPL-2.0 (file-level copyleft)")
w("")
for e in families["MPL-2.0"]:
    w(fmt(e))
w(f"- **Public Suffix List**, compiled into the `psl {version_of('psl')}` crate (`src/list.rs`,")
w("  generated from the list). The list is maintained by Mozilla and its")
w("  contributors and carries this notice: \"This Source Code Form is subject to the")
w("  terms of the Mozilla Public License, v. 2.0. If a copy of the MPL was not")
w("  distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.\"")
w("  <https://publicsuffix.org/list/public_suffix_list.dat>,")
w("  <https://github.com/publicsuffix/list>. The `psl` crate's own code is")
w("  `MIT/Apache-2.0` and is listed in section 4.")
w("")
w("What MPL-2.0 requires of us for these:")
w("")
w("- **Tell recipients where the source is** (section 3.2(a)). The two crates are")
w("  used exactly as published on crates.io; their source is at the URLs above and")
w(f"  at `https://crates.io/crates/adblock/{version_of('adblock')}` and")
w(f"  `https://crates.io/crates/option-ext/{version_of('option-ext')}`. The Public Suffix List is")
w("  used as the `psl` crate compiled it; the list's source form is at the")
w(f"  publicsuffix.org URL above and the compiled form at `https://crates.io/crates/psl/{version_of('psl')}`.")
w("  This file is that notice.")
w("- **Keep their files under MPL-2.0** (section 3.1). If HiWave ever changes a file")
w("  of either crate, or the list, the changed file must be published under")
w("  MPL-2.0. Vendoring a patched copy the way Boa is vendored would trigger this.")
w("- **Do not restrict the recipient's rights in that source** (section 3.2(b)). The")
w("  binary may be distributed under other terms (HiWave's own AGPL-3.0-or-later, or the")
w("  commercial licence), but those terms cannot limit what MPL-2.0 grants over")
w("  this source.")
w("- **Leave their notices in place** (section 3.4).")
w("")
w("The full text of MPL-2.0 is in section 4.")
w("")
w("## 2. Works that are not Cargo crates")
w("")
w("### Chart.js 4.4.0 (MIT)")
w("")
w("`crates/hiwave-app/src/ui/chart.umd.min.js`, the minified UMD build from the")
w("`chart.js@4.4.0` npm package, embedded in the binary with `include_str!` and")
w("injected into the built-in report page.")
w("")
w("- **Chart.js 4.4.0**. Copyright (c) 2014-2022 Chart.js Contributors (upstream")
w("  `LICENSE.md` at tag v4.4.0; the bundled file's own banner reads")
w("  \"(c) 2023 Chart.js Contributors, Released under the MIT License\").")
w("  <https://github.com/chartjs/Chart.js>")
w("- **@kurkle/color 0.3.2**, bundled inside that build. Copyright (c) 2018-2021")
w("  Jukka Kurkela (upstream `LICENSE.md` at tag v0.3.2; banner in the bundled")
w("  file: \"(c) 2023 Jukka Kurkela, Released under the MIT License\").")
w("  <https://github.com/kurkle/color>")
w("")
w("Licence text: MIT, in section 4.")
w("")
w("### Unicode CLDR 48 data (Unicode-3.0)")
w("")
w("Copyright (c) 1991-2025 Unicode, Inc. All rights reserved. Distributed under")
w("the Unicode License v3. <https://cldr.unicode.org>,")
w("<https://github.com/unicode-org/cldr-json> (release 48.0.0),")
w("<https://www.unicode.org/license.txt>")
w("")
w("- `crates/rustkit-bindings/src/likely_subtags.txt`: the likely-subtags table")
w("  (`common/supplemental/likelySubtags.xml`, read from cldr-json 48.0.0")
w("  `cldr-core/supplemental/likelySubtags.json`). Data only, re-encoded by")
w("  `scripts/cldr_likely_subtags.py`; embedded with `include_str!` in")
w("  `web_intl_likely.rs`.")
w("- `crates/rustkit-bindings/src/web_intl.js`: the first-day-of-week and weekend")
w("  tables (`WEEK_FIRST`, `WEEKEND`, `WEEKEND_FRI_SAT`) from CLDR 48 supplemental")
w("  `weekData`.")
w("- The same file's English plural, list and relative-time patterns (the sections")
w("  marked \"CLDR en\") follow CLDR's `en` locale data.")
w("")
w("Licence text: Unicode License v3, in section 4. Several crates in section 4")
w("that declare only MIT or Apache-2.0 also embed tables generated from the")
w("Unicode Character Database (the `unicode-*` crates, `regex-syntax`, `idna`'s")
w("data); where such a crate ships a Unicode licence file it is reproduced in")
w("section 5.")
w("")
w("### WHATWG HTML named character references (BSD-3-Clause)")
w("")
w("Copyright © WHATWG (Apple, Google, Mozilla, Microsoft).")
w("<https://html.spec.whatwg.org/entities.json>,")
w("<https://html.spec.whatwg.org/multipage/acknowledgements.html#ipr>")
w("")
w("- `crates/rustkit-html/src/entities/named.rs`: the HTML named character")
w("  references table, generated from `entities.json` by")
w("  `scripts/generate_html_entities.py` (the source's SHA-256 is recorded in the")
w("  file's header). Data only; the licence as received is kept beside it in")
w("  `crates/rustkit-html/src/entities/WHATWG-LICENSE.txt`.")
w("")
w("Licence text: BSD 3-Clause License, in section 4; \"the copyright holder\" in")
w("clause 3 is WHATWG (Apple, Google, Mozilla, Microsoft).")
w("")
w(f"### SQLite {SQLITE_VER} (public domain)")
w("")
w(f"`libsqlite3-sys {version_of('libsqlite3-sys')}` is built with its `bundled` feature (through")
w(f"`rusqlite`), which compiles the SQLite {SQLITE_VER} amalgamation (`sqlite3/sqlite3.c`)")
w("into the binary. SQLite is in the public domain and asks for no notice.")
w("<https://www.sqlite.org/copyright.html>. Its source files say:")
w("")
w("> The author disclaims copyright to this source code. In place of a legal")
w("> notice, here is a blessing:")
w(">")
w("> May you do good and not evil.")
w("> May you find forgiveness for yourself and forgive others.")
w("> May you share freely, never taking more than you give.")
w("")
w("### The Rust standard library (MIT OR Apache-2.0)")
w("")
w(f"The binary links Rust's `std`, `core` and `alloc` from the toolchain pinned in")
w(f"`rust-toolchain.toml` ({TOOLCHAIN}). `cargo tree` does not list them. \"Copyrights in")
w("the Rust Standard Library are retained by their contributors\" (The Rust")
w("Project Developers, <https://thanks.rust-lang.org>); it is licensed")
w("`Apache-2.0 OR MIT` and used here under MIT (text in section 4). The standard")
w("library has dependencies and in-tree exceptions of its own (for example")
w("`backtrace`, `hashbrown`, `libc`, `compiler_builtins`, Unicode data). They are")
w("listed in the `COPYRIGHT-library.html` file that ships with the toolchain")
w("(`$(rustc --print sysroot)/share/doc/rust/`) and at")
w("<https://github.com/rust-lang/rust/blob/master/COPYRIGHT>; they are not")
w("reproduced here.")
w("")
w("## 3. Vendored and modified source")
w("")
w("`third_party/boa_engine`, `third_party/boa_parser` and `third_party/boa_ast`")
w("started as the crates.io 0.22.0 releases of the Boa JavaScript engine (upstream")
w("`boa-dev/boa` at `337a3668`) and are wired in through `[patch.crates-io]`.")
w("")
w("**All three are modified by HiWave.** The changes are marked `HIWAVE PATCH (...)`")
w("in the source and each patch is described in")
w("`third_party/boa_engine/PATCHES.md` (which also covers the `boa_parser` and")
w("`boa_ast` changes): a cross-thread interrupt and its follow-ups, recursion")
w("and native-stack limits, V8's sort order, evaluation-order and")
w("temporal-dead-zone fixes, interpreter statistics and performance paths, among")
w("others. They are not the software Boa published; do not report their")
w("behaviour upstream as Boa's.")
w("")
w("Upstream licenses Boa `Unlicense OR MIT`; HiWave uses it under MIT. The")
w("published crates contain no licence file, so the notice is given here:")
w("")
w("    Copyright (c) 2019 Jason Williams")
w("")
w("(from `LICENSE-MIT` in <https://github.com/boa-dev/boa> at `337a3668`). The")
w("crates are listed again with the other MIT crates in section 4.")
w("")
w("### Code ported from V8 into the vendored Boa")
w("")
w("`third_party/boa_engine/src/builtins/array/v8_timsort.rs` (added by HiWave) is")
w("a Rust port of V8's sorting built-ins:")
w("")
w("- `third_party/v8/builtins/array-sort.tq` (`Array.prototype.sort`). V8 keeps")
w("  that file under the Python licence stack, because it is derived from")
w("  CPython's `Objects/listobject.c`. Its header reads: \"Copyright (c) 2001,")
w("  2002, 2003, 2004, 2005, 2006, 2007, 2008, 2009, 2010, 2011, 2012, 2013,")
w("  2014, 2015, 2016, 2017, 2018, 2019, 2020, 2021, 2022, 2023, 2024, 2025,")
w("  2026 Python Software Foundation; All Rights Reserved\" (V8 `main`, read")
w("  2026-11-14). Licence: `third_party/v8/builtins/LICENSE` in V8, the Python")
w("  Software Foundation License Version 2 with the BeOpen, CNRI and CWI")
w("  agreements, reproduced in full in section 5.")
w("- `src/builtins/typed-array-sort.tq` (typed-array sort). \"Copyright 2019 the")
w("  V8 project authors. All rights reserved.\" BSD-3-Clause; V8's `LICENSE.v8`")
w("  is reproduced in full in section 5.")
w("")
w("Summary of changes, as the Python licence asks of a derivative work: the")
w("algorithm was rewritten in Rust from V8's Torque version (itself a port of")
w("CPython's list sort) to work on Boa's value vectors with a fallible")
w("comparator, and arrays shorter than 8 elements go straight to binary")
w("insertion to match Chrome's measured behaviour. <https://github.com/v8/v8>")
w("")
w("The \"join stack\" change described in `PATCHES.md` matches V8's observable")
w("behaviour and ports no V8 code.")
w("")
w("### Code Boa itself copied")
w("")
w("`third_party/boa_engine/src/builtins/date/utils.rs` (`mod fast_atoi`, present")
w("in upstream Boa) says it is copied from `atoi_simd`'s `src/fallback.rs`.")
w("atoi_simd is `MIT OR Apache-2.0`: Copyright (c) 2022 Dmitry Rodionov (upstream")
w("`LICENSE-MIT`). <https://github.com/RoDmitry/atoi_simd>. Used under MIT, text")
w("in section 4.")
w("")
w("## 4. Rust crates in the dependency closure of the `hiwave` binary")
w("")
w("Every crate `cargo tree -p hiwave-app -e normal` reports for")
w("`aarch64-apple-darwin` and `x86_64-apple-darwin`, HiWave's own crates left out.")
w(f"{total - n_proc} of them are compiled and linked into the binary. The other {n_proc} are")
w("procedural macros (marked in their entries): they run at build time and are")
w("not linked, but the code they generate is, so they are listed too.")
w("")
w("Where a crate offers a choice of licences (`A OR B`), HiWave uses it under the")
w("first of these that it offers: " + ", ".join(PREF[:6]) + ".")
w("The expression the crate declares is shown with each entry. Copyright lines are")
w("copied from the licence files inside each published crate; where a crate ships")
w("none, the entry says so and gives the `authors` field of its `Cargo.toml`")
w("instead.")
w("")
for fam in [f for f in PREF if f in families]:
    title, note, text = TEXTS[fam]
    n = len(families[fam])
    w(f"### {title} (`{fam}`): {n} crate{'' if n == 1 else 's'}")
    w("")
    for e in families[fam]:
        w(fmt(e))
    w("")
    if fam == "Unicode-3.0":
        w("For the ICU4X crates above (repository `unicode-org/icu4x`) the licence file adds:")
        w("")
        for line in ICU_TRAILER.splitlines():
            w("> " + line)
        w("")
    w(f"#### Text of the {title}")
    w("")
    if note:
        w(note)
        w("")
    w(fence(text))

w("## 5. Crates under more than one licence at once, and licence files in full")
w("")
w("These declare an `AND` expression: every licence named applies to some part of")
w("the crate.")
w("")
for e in compound:
    w(fmt(e))
w("")
w("- **dpi**: Apache-2.0 for the crate (text in section 4) and MIT for the code it")
w("  takes from libm; its `LICENSE-LIBM-MIT` follows in full.")
w("- **unicode-ident**: used under MIT for its code and Unicode-3.0 for the Unicode")
w("  Character Database tables it embeds; both texts are in section 4.")
w("- **aws-lc-rs**, **aws-lc-sys** and **ring**: their licence files follow in full,")
w("  because they attribute individual parts (AWS-LC, BoringSSL, OpenSSL, SSLeay,")
w("  fiat-crypto and others) that a summary would lose.")
w("")
w("Also reproduced in full below:")
w("")
w("- **libm** (listed under MIT in section 4): one licence, but its `LICENSE.txt`")
w("  names several holders for parts ported from musl and FreeBSD.")
w("- **Licence files below a crate's root.** Every crate in section 4 was searched")
w("  for `LICENSE*`, `COPYING*`, `NOTICE*`, `THIRD_PARTY*` and `PATENTS*` files")
w("  below its root. Those found are reproduced here, except:")
for (name, ver, rel, why) in skipped:
    w(f"  - `{name} {ver}`: `{rel}` is {why}." if why.startswith("the ") else f"  - `{name} {ver}`: `{rel}`. {why}.")
w("- **V8's licences**, for the code ported into the vendored Boa (section 3).")
w("")
blocks = []
for name, files in FULL.items():
    for f in files:
        blocks.append((name, version_of(name), f, ""))
blocks += carried
blocks.sort(key=lambda b: (list(FULL).index(b[0]) if b[0] in FULL else 99, b[0]))
for (name, ver, f, note) in blocks:
    assert (name, ver) in rows, (name, ver)
    w(f"#### {name} {ver}: `{f}`")
    w("")
    if note:
        w(note)
        w("")
    w(fence(read(crate_dir(name, ver) / f)))
w("#### V8: `LICENSE.v8`")
w("")
w("For the typed-array sort ported from `src/builtins/typed-array-sort.tq` (section 3).")
w("")
w(fence(V8_BSD))
w("#### V8: `third_party/v8/builtins/LICENSE` (Python Software Foundation licence stack)")
w("")
w("For the array sort ported from `third_party/v8/builtins/array-sort.tq` (section 3).")
w("")
w(fence(PSF))

w("## 6. In the repository, not in the shipped binary")
w("")
w("Listed so the next inventory does not have to rediscover them. None of these")
w("is compiled into `hiwave` or copied into an app bundle.")
w("")
w("- **Ahem test font** (`crates/rustkit-text/tests/fixtures/Ahem.{ttf,woff,woff2}`),")
w("  from web-platform-tests; used by tests only.")
w("- **Noto Sans** (`baselines/common/fonts/NotoSans-*.ttf`, SIL Open Font License")
w("  1.1); used by the parity baselines only. HiWave bundles no fonts: text is")
w("  drawn with the fonts installed on the system.")
w("- **Test pages and images** under `fixtures/`, `websuite/`, `static-web-suite/`,")
w("  `cases/`, `parity-*`, `goldens/` and `baselines/`, including the MDN")
w("  learning-area gallery image a `rustkit-image` unit test embeds")
w("  (`websuite/js-ladder/01-mdn/32-image-gallery/images/pic4.jpg`, inside")
w("  `#[cfg(test)]`).")
w("- **Node packages** under `tools/parity_oracle/node_modules` (Playwright,")
w("  pixelmatch, pngjs): developer tooling.")
w("- **Build-time and test-time crates**: `[build-dependencies]` and")
w("  `[dev-dependencies]` across the workspace, and the crates only")
w("  `hiwave-mcp`, `hiwave-smoke`, `parity-capture`, `rustkit-bench` and")
w("  `rustkit-test` use.")
w("- **Crates in `Cargo.lock` that this build does not enable**: optional and")
w("  target-gated dependencies such as wgpu's Vulkan and GLES back ends, the")
w("  quinn family, and other-platform crates. They are in the lock file and not")
w("  in the `cargo tree` output for this package on macOS.")
w("- **Filter lists** (EasyList, EasyPrivacy): not bundled. A running HiWave")
w("  downloads them from `easylist.to` into the user's cache")
w("  (`crates/hiwave-shield/src/filter_lists.rs`).")
w("- **System frameworks** (WebKit, AppKit, CoreText, Metal and the rest): part of")
w("  macOS, linked dynamically, not redistributed.")
w("")
w("## 7. How this file was made, and what was not verified")
w("")
w("`scripts/third_party_notices.py` writes this file. To regenerate it, and to")
w("check that it is current (exit 1 and a diff summary when it is not):")
w("")
w("```bash")
w("python3 scripts/third_party_notices.py")
w("python3 scripts/third_party_notices.py --check")
w("```")
w("")
w("The crate list is the output of this, for both targets, with the lines whose")
w("path is under `crates/` (HiWave's own) removed:")
w("")
w("```bash")
w("cargo tree -p hiwave-app -e normal --prefix none --format '{p}|{l}|{r}' --target <target> --locked")
w("```")
w("")
w("It is valid for `-p hiwave-app` with default features on the two macOS")
w("targets. `-e normal` leaves out build- and dev-dependencies, which are not")
w("linked. A build with other features, of another package, or of the whole")
w("workspace at once (which unifies features across members) can link more.")
w("")
w("Licence expressions and repository URLs are the `license` and `repository`")
w("fields of each crate's `Cargo.toml`. Copyright lines are collected from the")
w("`LICENSE*`, `COPYING*`, `COPYRIGHT*`, `NOTICE*` and `UNLICENSE*` files at the")
w("root of each crate's published source (`~/.cargo/registry/src`). Licence")
w("files below a crate's root are found by walking each crate's directory; the")
w("script stops on one it has not been told about. The works in sections 2 and 3")
w("were found by hand: by listing every `include_str!` and `include_bytes!` in the")
w("workspace, and by searching `crates/`, `third_party/`, `scripts/` and `tools/`")
w("for attribution comments (\"Copyright\", \"SPDX\", \"ported from\", \"derived")
w("from\", \"based on\", \"adapted from\", \"copied from\", \"taken from\"). The script")
w("does not repeat that search.")
w("")
w("Not verified:")
w("")
w(f"- Copyright lines are what each crate's own licence files say ({stats['from_file']} crates).")
w(f"  {stats['no_line']} crates ship no copyright line; their entries give the `authors` field")
w("  instead, which names authors, not necessarily copyright holders. Their")
w("  upstream repositories were not read, except Boa's.")
w("- Licence expressions are the crates' own declarations. No crate's source was")
w("  audited file by file for code under a licence other than the declared one;")
w("  only files named like a licence were looked for.")
w("- Which licence to use where a crate offers a choice is a convention chosen for")
w("  this file (stated in section 4), not something a lawyer reviewed. The same")
w("  goes for whether pointing at upstream URLs satisfies MPL-2.0 section 3.2(a),")
w("  and for how the Python licence applies to a port of a port.")
w("- The vendored Boa was searched for attribution comments, not compared line by")
w("  line with other engines. The V8 and Python notices were read from V8's")
w("  `main` branch, not from the revision the port was made from.")
w("- The Rust standard library's own third-party notices are pointed at, not")
w("  reproduced.")
w("- The list is for the macOS binary. The Windows and Linux ports resolve other")
w("  platform crates and need their own files.")
w("- HiWave has no packaging step in this repository; whatever builds the")
w("  downloadable app must copy this file into it.")

text = "\n".join(out).rstrip("\n") + "\n"
summary = (f"{total} crates ({n_proc} proc-macro); " + ", ".join(f"{f} {len(families[f])}" for f in PREF if f in families)
           + f"; AND-expressions {len(compound)}; copyright from file {stats['from_file']}, "
           f"none {stats['no_line']}, boa {stats['boa']}; nested carried {len(carried)}, skipped {len(skipped)}")
if "--check" in sys.argv:
    old = OUT.read_text(encoding="utf-8") if OUT.exists() else ""
    if old == text:
        print("THIRD_PARTY_NOTICES.md is current: " + summary)
        sys.exit(0)
    entry = re.compile(r"^- \*\*(\S+ \S+)\*\*", re.M)
    have, want = set(entry.findall(old)), set(entry.findall(text))
    for c in sorted(want - have):
        print("missing: " + c)
    for c in sorted(have - want):
        print("stale: " + c)
    print("THIRD_PARTY_NOTICES.md is out of date; run python3 scripts/third_party_notices.py")
    sys.exit(1)
OUT.write_text(text, encoding="utf-8")
print("wrote THIRD_PARTY_NOTICES.md: " + summary)
