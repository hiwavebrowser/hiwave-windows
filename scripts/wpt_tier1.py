#!/usr/bin/env python3
"""wpt_tier1.py — run the Tier-1 WPT reftest seed through the real engine.

W0b of WPT Phase 0.5. The contract (docs/WPT_W0B_IMPLEMENT_PIN_2026-07-29.md):

- The manifest is the source of truth; third_party/wpt/ is a projection of it
  (scripts/wpt_sync.sh). This runner never fetches anything.
- Test and reference BOTH render through the same parity-capture binary the
  parity campaign uses, at the manifest viewport (800x600), passed explicitly
  on every call — never the binary's own 1280x800 default.
- A reftest passes when the test frame and the reference frame match. This is
  HiWave-vs-HiWave, not HiWave-vs-Chrome: WPT asks "does the engine agree with
  itself where the spec says two documents must render identically."
- <link rel="match"> inside the test file is the authority on which reference
  to use. If it disagrees with the manifest, that is an INSTRUMENT error and
  no pixels are compared — a diff against the wrong reference is not data.
- A frame with zero ink is never a match. Two white frames agree with each other
  for the worst possible reason (the engine rendered neither document), which is
  the empty-capture-scores-100 lie wearing a reftest costume. Zero ink on either
  side -> ERROR. Any ink at all is scored: a thin frame can really match or fail.
- A reftest that cannot reach the state it asserts about is not scored.
  The capture path does not watch for reftest-wait's completion signal, so its
  frame may precede the state under test — and upstream WPT reports TIMEOUT
  there, not FAIL. Those are SKIP, with the reason recorded. (Added 2026-08-06
  night 20: three of the seed's six fails were this.) A page with plain inline
  scripts is scored, with script on (--run-inline-scripts, #799).
- A page with an external script is SKIP: --html-file runs inline scripts only,
  so its frame lacks what the script does.
- A page whose local stylesheet, image or font is not on disk is an ERROR, no
  pixels compared. The sparse sync takes only listed paths, and both sides
  rendering without the same sheet can agree for the wrong reason (T14:
  css-flexbox/support/flexbox.css).
- `status` is the exact rule below. The campaign's rule (pixelmatch 0.1,
  15%) is reported beside it in `campaign_diff_pct`, and never decides a
  status: at 15% a page with a wrong line break still passes. Where it could
  not be measured, `campaign_not_measured` says why.
- A match has to go red when the page is broken on purpose. Every match is
  rendered again with all its style removed, with its area's feature forced off
  (AREA_PROBES), and once per `mutations` entry in the manifest. If any of those
  still matches, the case is UNPROVEN, with the reason, and is not in the pass
  count. Style removal alone only shows that the page reacts to its style.
- A FAIL under SMALL_FAIL_PCT is asked of Chromium under the same staging. If
  Chromium's own test and reference differ too, the case is SKIP with the reason:
  the page cannot tell us about the engine at this rule.
- A match demoted by its area probe is asked the same. If Chromium's own test and
  reference differ, its row says the page is no evidence either way. It stays UNPROVEN.
- Root-absolute references (/fonts/ahem.css) are staged to resolve against the
  WPT root, because wptserve serves / from there and file:// does not. A
  reftest whose declared font silently fails to load is not measuring its own
  assertion. The fonts themselves are MANIFEST.support_paths.
- A fail whose test declares a web font is still a FAIL — @font-face is
  unimplemented, and that is an engine gap, not a harness one — but it is
  tagged `blocked_by` so a digest can name one capability instead of counting
  N unrelated text bugs.
- The oracle must be able to go red. Every run starts by rendering a
  deliberately mismatched fixture pair (trench/wpt/negative-control/); if the
  harness cannot fail THAT, the run aborts and publishes nothing.

Exit codes: 0 = ran and wrote last-run.json (individual cases may FAIL — that
is a result, not a runner error); 1 = harness-level failure (missing binary,
sync not run, negative control did not go red).
"""

import argparse
import collections
import concurrent.futures
import json
import math
import os
import re
import subprocess
import sys
import tempfile
import threading
from datetime import datetime, timezone
from html.parser import HTMLParser
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from parity_lib import THRESHOLDS  # noqa: E402

REPO_ROOT = Path(__file__).resolve().parent.parent
MANIFEST_PATH = REPO_ROOT / "trench" / "wpt" / "MANIFEST.json"
WPT_ROOT = REPO_ROOT / "third_party" / "wpt"


def capture_bin(env=os.environ) -> Path:
    """Where the parity build lands: cargo's target dir, relative paths from the repo."""
    return REPO_ROOT / (env.get("CARGO_TARGET_DIR") or "target") / "parity" / "parity-capture"


def capture_label(path: Path) -> str:
    """The binary as last-run.json names it; a shared target dir is outside the repo."""
    return path.relative_to(REPO_ROOT).as_posix() if path.is_relative_to(REPO_ROOT) else str(path)


CAPTURE_BIN = capture_bin()
LAST_RUN_PATH = REPO_ROOT / "trench" / "wpt" / "last-run.json"
NEGATIVE_CONTROL_DIR = REPO_ROOT / "trench" / "wpt" / "negative-control"
ORACLE_DIR = REPO_ROOT / "tools" / "parity_oracle"

# The single threshold knob. WPT reftest semantics are "renders identically",
# so W0b starts at exact RGB match. If antialiasing noise ever forces a
# tolerance, it gets raised HERE, documented in last-run.json, with Pete's
# sign-off — never borrowed from the campaign's t15.
WPT_MAX_DIFF_PCT = 0.0
CAMPAIGN_PIXELMATCH_THRESHOLD = 0.1  # compare_baseline.mjs, wpt_pair_diff.mjs

STATUSES = ("PASS", "FAIL", "UNPROVEN", "SKIP", "ERROR")

# A FAIL under this is checked against Chromium before it goes to a C row.
SMALL_FAIL_PCT = 0.1
CHROMIUM_TIMEOUT_S = 600

_TRANSFORMS_OFF = (
    "transform: none !important; translate: none !important; rotate: none !important; "
    "scale: none !important; perspective: none !important; transform-style: flat !important"
)
# What each coverage area asserts, forced off. The seed's cases are too mixed for
# one rule; they have their own `mutations`.
AREA_PROBES = {
    "css-overflow": "overflow: visible !important",
    "css-position-sticky": "position: static !important",
    "css-position-absolute": "position: static !important",
    "css-transforms-2d": _TRANSFORMS_OFF,
    "css-transforms-3d": _TRANSFORMS_OFF,
    "css-masking-clip-path": "clip-path: none !important",
    "css-masking-mask": "mask: none !important; -webkit-mask: none !important; mask-image: none !important",
    "css-ui-outline": "outline: none !important",
    "css-lists": "list-style: none !important; counter-reset: none !important; counter-increment: none !important",
    "css-text-basics": (
        "white-space: normal !important; text-align: start !important; text-indent: 0 !important; "
        "letter-spacing: normal !important; word-spacing: normal !important; text-transform: none !important"
    ),
}

SCRIPT_CLASS_RE = re.compile(r"""classList\b|\bclassName\b|setAttribute\(\s*["']class["']""")
SCRIPT_ELEMENT_RE = re.compile(r"<script\b.*?</script\s*>", re.IGNORECASE | re.DOTALL)
SCRIPT_TRANSFORM_RE = re.compile(r"""setAttribute\(\s*["']transform["']""")


class _PageTags(HTMLParser):
    """What a page's markup says, read the way a parser reads it: a `>` or an
    `src=` inside another attribute's quotes, a comment and script text are not tags."""

    def __init__(self):
        super().__init__(convert_charrefs=True)
        self.tags = []
        self.style_text = []
        self._in_style = False

    def handle_starttag(self, tag, attrs):
        self.tags.append((tag, {k: v for k, v in attrs if v}))
        self._in_style = tag == "style"

    def handle_endtag(self, tag):
        if tag == "style":
            self._in_style = False

    def handle_data(self, data):
        if self._in_style:
            self.style_text.append(data)


def page_tags(html: str) -> _PageTags:
    parser = _PageTags()
    parser.feed(html)
    parser.close()
    return parser


def _rel_has(attrs, token: str) -> bool:
    return token in attrs.get("rel", "").lower().split()


# wptserve maps / to the WPT root; file:// has no such mapping, so root-absolute
# references (e.g. /fonts/ahem.css) resolve to the filesystem root and silently
# load nothing. Staged copies live NEXT TO the originals so relative references
# keep working and the fixtures on disk stay verbatim.
STAGED_SUFFIX = ".__staged"
_STAGED = {}
_STAGE_LOCK = threading.Lock()
CSS_REF_RE = re.compile(
    r"@import\s+[\"']([^\"']+)[\"']|url\(\s*[\"']?([^\"')]+?)[\"']?\s*\)", re.IGNORECASE
)
ROOT_ABS_ATTR_RE = re.compile(
    r"((?<![\w-])(?:href|src)\s*=\s*)(?:([\"'])(/(?!/)[^\"']*)\2|(/(?!/)[^\s\"'>]+))", re.IGNORECASE
)
ROOT_ABS_URL_RE = re.compile(r"(url\(\s*[\"']?)(/(?!/)[^\"')]*)([\"']?\s*\))", re.IGNORECASE)
CSS_COMMENT_RE = re.compile(r"/\*.*?\*/", re.DOTALL)
STYLE_ELEMENT_RE = re.compile(r"<style\b.*?</style\s*>", re.IGNORECASE | re.DOTALL)
SHEET_LINK_RE = re.compile(r"""<link\b(?:[^>"']|"[^"]*"|'[^']*')*>""", re.IGNORECASE)
TAG_WITH_ATTRS_RE = re.compile(r"""<[a-z](?:[^>"']|"[^"]*"|'[^']*')*>""", re.IGNORECASE)
STYLE_ATTR_RE = re.compile(
    r"""(\s)style\s*=\s*(?:"[^"]*"|'[^']*'|[^\s"'>]+)|("[^"]*"|'[^']*')""", re.IGNORECASE
)

# A test that declares a web font is tagged so a digest can group its fails.
# Until n33 (2026-08-26) the tag read "@font-face unimplemented": FontLoader
# never fetched or registered anything, so every declared face fell back.
# Since n33 a TTF/OTF face loads and is what the shaper measures with (Ahem
# included). The tag is ATTRIBUTION ONLY and is deliberately kept as the same
# field so the trendline stays comparable: a fail here is a real engine
# failure measured IN the declared font, and a PASS is real if the face was
# accepted — the tag no longer implies either "blocked" or "suspect".
WEBFONT_GAP = "declares a web font (loads since n33: TTF/OTF; WOFF/WOFF2 not yet)"


def read_ppm(path: Path):
    """(width, height, pixel bytes) from a binary P6 PPM."""
    data = path.read_bytes()
    if not data.startswith(b"P6"):
        raise ValueError(f"{path}: not a P6 PPM")
    # Header: magic, whitespace/comments, width, height, maxval, single ws.
    pos, fields = 2, []
    while len(fields) < 3:
        while pos < len(data) and data[pos : pos + 1].isspace():
            pos += 1
        if data[pos : pos + 1] == b"#":
            while pos < len(data) and data[pos : pos + 1] != b"\n":
                pos += 1
            continue
        start = pos
        while pos < len(data) and not data[pos : pos + 1].isspace():
            pos += 1
        fields.append(int(data[start:pos]))
    pos += 1  # single whitespace after maxval
    width, height, _maxval = fields
    pixels = data[pos : pos + width * height * 3]
    if len(pixels) != width * height * 3:
        raise ValueError(f"{path}: truncated pixel data")
    return width, height, pixels


def compare_frames(test_ppm: Path, ref_ppm: Path):
    """(diff_pixels, diff_pct). Dimension mismatch is a full-frame diff."""
    tw, th, tp = read_ppm(test_ppm)
    rw, rh, rp = read_ppm(ref_ppm)
    if (tw, th) != (rw, rh):
        total = max(tw * th, rw * rh)
        return total, 100.0
    total = tw * th
    diff = sum(
        1
        for i in range(0, len(tp), 3)
        if tp[i : i + 3] != rp[i : i + 3]
    )
    return diff, (diff / total * 100.0) if total else 100.0


def has_ink(ppm: Path) -> bool:
    """Is any pixel of this frame not white?"""
    _width, _height, pixels = read_ppm(ppm)
    return pixels.count(0xFF) != len(pixels)


def rel_match_hrefs(test_file: Path):
    """All <link rel=match> hrefs in a test file, in document order."""
    page = page_tags(test_file.read_text(errors="replace", encoding="utf-8"))
    return [
        attrs["href"]
        for tag, attrs in page.tags
        if tag == "link" and _rel_has(attrs, "match") and attrs.get("href")
    ]


def _staged_once(src: Path, make) -> Path:
    """A reference or sheet many cases share is written once a run: cases run at
    the same time, and rewriting it would truncate what another capture is reading."""
    with _STAGE_LOCK:
        if src not in _STAGED:
            _STAGED[src] = make(src)
        return _STAGED[src]


def stage_css(src: Path) -> Path:
    return _staged_once(src, _stage_css)


def stage_html(src: Path) -> Path:
    return _staged_once(src, _stage_html)


def _stage_css(src: Path) -> Path:
    """Copy a CSS file beside itself with root-absolute url(...) values resolved."""
    staged = src.with_name(src.stem + STAGED_SUFFIX + src.suffix)
    css = ROOT_ABS_URL_RE.sub(
        lambda m: m.group(1) + str(WPT_ROOT / m.group(2).lstrip("/")) + m.group(3),
        src.read_text(errors="replace", encoding="utf-8"),
    )
    staged.write_text(css, encoding="utf-8")
    return staged


def _stage_html(src: Path) -> Path:
    """Copy an HTML file beside itself with root-absolute references resolved
    against the WPT root — what wptserve would have served. Referenced CSS is
    staged recursively so its own url(...) values resolve too."""
    def rewrite(m):
        target = WPT_ROOT / (m.group(3) or m.group(4)).lstrip("/")
        if target.suffix.lower() == ".css" and target.is_file():
            target = _stage_css_locked(target)
        quote = m.group(2) or '"'
        return m.group(1) + quote + str(target) + quote

    html = ROOT_ABS_ATTR_RE.sub(rewrite, src.read_text(errors="replace", encoding="utf-8"))
    html = ROOT_ABS_URL_RE.sub(
        lambda m: m.group(1) + str(WPT_ROOT / m.group(2).lstrip("/")) + m.group(3), html
    )
    staged = src.with_name(src.stem + STAGED_SUFFIX + src.suffix)
    staged.write_text(html, encoding="utf-8")
    return staged


def _stage_css_locked(src: Path) -> Path:
    """stage_css from inside stage_html, which already holds the lock."""
    if src not in _STAGED:
        _STAGED[src] = _stage_css(src)
    return _STAGED[src]


def _is_local(url: str) -> bool:
    return not (re.match(r"[a-z][a-z0-9+.-]*:", url, re.IGNORECASE) or url.startswith(("//", "#")))


def _local_target(url: str, base_dir: Path, wpt_root: Path) -> Path:
    path = url.split("#")[0].split("?")[0]
    return wpt_root / path.lstrip("/") if path.startswith("/") else base_dir / path


def local_resources(html_file: Path, wpt_root: Path):
    """(url, path) for each local subresource of a page, on disk or not, in the order found.

    Only what the page loads: <link rel=stylesheet>, src=, @import and url() in
    <style> and style=, following linked sheets. <link rel=help|author|match> are
    not resources.
    """
    found = []
    seen_urls = set()
    seen_sheets = set()

    def note(url, base_dir):
        if not _is_local(url):
            return None
        target = _local_target(url, base_dir, wpt_root)
        if (url, target) not in seen_urls:
            seen_urls.add((url, target))
            found.append((url, target))
        return target if target.is_file() else None

    def scan_css(css, base_dir):
        for m in CSS_REF_RE.finditer(CSS_COMMENT_RE.sub("", css)):
            target = note(m.group(1) or m.group(2), base_dir)
            if target and target.suffix.lower() == ".css" and target not in seen_sheets:
                seen_sheets.add(target)
                scan_css(target.read_text(errors="replace", encoding="utf-8"), target.parent)

    page = page_tags(html_file.read_text(errors="replace", encoding="utf-8"))
    for tag, attrs in page.tags:
        if tag == "link" and _rel_has(attrs, "stylesheet") and attrs.get("href"):
            target = note(attrs["href"], html_file.parent)
            if target and target not in seen_sheets:
                seen_sheets.add(target)
                scan_css(target.read_text(errors="replace", encoding="utf-8"), target.parent)
    for _tag, attrs in page.tags:
        if attrs.get("src"):
            note(attrs["src"], html_file.parent)
    scan_css("\n".join(page.style_text), html_file.parent)
    for _tag, attrs in page.tags:
        if attrs.get("style"):
            scan_css(attrs["style"], html_file.parent)
    return found


def missing_resources(html_file: Path, wpt_root: Path):
    """Local subresources of a page that are not on disk, in the order found."""
    return list(dict.fromkeys(
        url for url, target in local_resources(html_file, wpt_root) if not target.is_file()
    ))


def unrunnable_reason(test_src: str, ref_src: str):
    """Why this reftest cannot probe its own assertion in the capture path.

    These are not engine verdicts. The capture path takes its frame after load
    and does not watch for reftest-wait's completion signal, so comparing those
    frames may measure the wrong moment — upstream WPT would report TIMEOUT
    here, not FAIL. Scoring them as FAIL charges the renderer for the harness's
    missing capability, which is the decorative-instrument class this campaign
    keeps finding (forensics 2026-07-24).
    """
    if "reftest-wait" in test_src or "reftest-wait" in ref_src:
        return "reftest-wait: the capture path does not watch for the completion signal"
    if re.search(r"name\s*=\s*[\"']?fuzzy\b", test_src, re.IGNORECASE):
        return "fuzzy annotation: tolerance matching not implemented"
    for src in (test_src, ref_src):
        if any(tag == "script" and attrs.get("src") for tag, attrs in page_tags(src).tags):
            return "external script: --html-file runs inline scripts only"
    return None


def gate(test_src: str, ref_src: str, absent):
    """Why a case stops before any pixels, as a case update; None to go on.

    A missing resource comes first: it is the instrument's fault on any page, and
    a SKIP would hide it for as long as the page stays unscored.
    """
    if absent:
        return {
            "status": "ERROR",
            "reason": f"local resources not on disk {sorted(set(absent))} — "
                      f"add them to MANIFEST.support_paths; pixels not compared",
        }
    unrunnable = unrunnable_reason(test_src, ref_src)
    return {"status": "SKIP", "reason": unrunnable} if unrunnable else None


def webfont_dependent(test_src: str, ref_src: str) -> bool:
    """Does either side declare a web font it needs to render as designed?"""
    for src in (test_src, ref_src):
        if re.search(r"name\s*=\s*[\"']?flags\b[^>]*ahem", src, re.IGNORECASE):
            return True
        if re.search(r"href\s*=\s*[\"']?/fonts/", src, re.IGNORECASE):
            return True
        if "@font-face" in src:
            return True
    return False


def capture_args(html_file: Path, out_ppm: Path, viewport):
    """The parity-capture call. Script stays off unless the page has one, and
    with it on a script that throws fails the capture (an ERROR, not a frame)."""
    cmd = [
        str(CAPTURE_BIN),
        "--html-file", str(html_file),
        "--width", str(viewport["width"]),
        "--height", str(viewport["height"]),
        "--dump-frame", str(out_ppm),
    ]
    if "<script" in html_file.read_text(errors="replace", encoding="utf-8").lower():
        cmd.append("--run-inline-scripts")
    return cmd


def capture_failure(stdout: str, stderr: str) -> str:
    """parity-capture's own `error` from its JSON line; stderr's tail is a timestamped log."""
    lines = stdout.strip().splitlines()
    try:
        error = json.loads(lines[-1]).get("error") if lines else None
    except (ValueError, AttributeError):
        error = None
    if error:
        return str(error)
    tail = (stderr or stdout or "").strip().splitlines()
    return tail[-1] if tail else "no output"


def capture(html_file: Path, out_ppm: Path, viewport):
    """Render through the engine. Returns None on success, reason on failure."""
    cmd = capture_args(html_file, out_ppm, viewport)
    try:
        proc = subprocess.run(cmd, capture_output=True, text=True, timeout=60, encoding="utf-8")
    except subprocess.TimeoutExpired:
        return "capture timeout (60s)"
    if proc.returncode != 0:
        return f"capture exit {proc.returncode}: {capture_failure(proc.stdout, proc.stderr)}"
    if not out_ppm.exists() or out_ppm.stat().st_size == 0:
        return "capture produced no frame"
    return None


def campaign_diff(test_ppm: Path, ref_ppm: Path):
    """The campaign's pixelmatch on this pair: `(diff_pct, None)`, or `(None, why)`
    when it could not be measured. Reported beside the status, never used to set it."""
    if not (ORACLE_DIR / "node_modules").is_dir():
        return None, "tools/parity_oracle/node_modules is missing"
    try:
        proc = subprocess.run(
            ["node", "wpt_pair_diff.mjs", str(test_ppm), str(ref_ppm)],
            cwd=ORACLE_DIR, capture_output=True, text=True, timeout=60, encoding="utf-8"
        )
    except FileNotFoundError:
        return None, "node not found"
    except subprocess.TimeoutExpired:
        return None, "wpt_pair_diff.mjs timeout (60s)"
    except OSError as e:
        return None, f"node did not start: {type(e).__name__}"
    if proc.returncode != 0:
        tail = (proc.stderr or proc.stdout or "").strip().splitlines()
        return None, f"wpt_pair_diff.mjs exit {proc.returncode}: {tail[-1][:200] if tail else 'no output'}"
    try:
        diff_pct = json.loads(proc.stdout)["diff_pct"]
    except ValueError:
        return None, "wpt_pair_diff.mjs printed no JSON"
    except (KeyError, TypeError):
        return None, "wpt_pair_diff.mjs JSON has no diff_pct"
    if isinstance(diff_pct, bool) or not isinstance(diff_pct, (int, float)) or not math.isfinite(diff_pct):
        return None, "wpt_pair_diff.mjs diff_pct is not a number"
    return round(diff_pct, 4), None


def campaign_counts(cases):
    """Scored cases against the campaign threshold; an unmeasured one is neither."""
    limit = THRESHOLDS["default"]
    scored = [c for c in cases if c["status"] in ("PASS", "FAIL", "UNPROVEN")]
    measured = [c["campaign_diff_pct"] for c in scored if c["campaign_diff_pct"] is not None]
    return {
        "within": sum(1 for d in measured if d <= limit),
        "over": sum(1 for d in measured if d > limit),
        "not_measured": len(scored) - len(measured),
    }


def unproven_passes(cases):
    """Matches that also matched when the page was broken on purpose, each with its reason."""
    return {c["id"]: c["reason"] for c in cases if c["status"] == "UNPROVEN"}


def count_statuses(cases):
    return {s: sum(1 for c in cases if c["status"] == s) for s in STATUSES}


def pass_rate(counts):
    """An unproven case was scored, and is not a pass."""
    scored = counts["PASS"] + counts["FAIL"] + counts["UNPROVEN"]
    return round(counts["PASS"] / scored, 4) if scored else None


def per_area(cases):
    areas = {}
    for c in cases:
        areas.setdefault(c["area"], []).append(c)
    return {name: count_statuses(areas[name]) for name in sorted(areas)}


def oracle_record():
    """What decided each case: the reftest's own reference. Chromium only decides
    whether a small FAIL is a SKIP."""
    return {
        "kind": "reftest: test frame vs reference frame, both through parity-capture",
        "status_rule": {"name": "exact RGB match", "max_diff_pct": WPT_MAX_DIFF_PCT},
        "campaign_rule": {
            "name": "pixelmatch, reported only, never sets a status",
            "pixelmatch_threshold": CAMPAIGN_PIXELMATCH_THRESHOLD,
            "threshold_pct": THRESHOLDS["default"],
        },
        "small_fail_rule": {
            "name": "Chromium's own test vs reference, exact RGB; a difference makes the FAIL a SKIP",
            "under_pct": SMALL_FAIL_PCT,
        },
    }


def develop_sha_at(cwd: Path):
    """The develop commit this head is on; None without origin/develop, never HEAD."""
    proc = subprocess.run(
        ["git", "merge-base", "HEAD", "origin/develop"],
        cwd=cwd, capture_output=True, text=True, encoding="utf-8"
    )
    return proc.stdout.strip() if proc.returncode == 0 and proc.stdout.strip() else None


def run_pair(test_file: Path, ref_file: Path, viewport, workdir: Path, stage: bool = True):
    """Render both sides and compare. Returns a partial case dict."""
    test_ppm = workdir / "test.ppm"
    ref_ppm = workdir / "ref.ppm"

    if stage:
        test_file = stage_html(test_file)
        ref_file = stage_html(ref_file)

    for label, src, dst in (("test", test_file, test_ppm),
                            ("ref", ref_file, ref_ppm)):
        reason = capture(src, dst, viewport)
        if reason:
            return {"status": "ERROR", "reason": f"{label}: {reason}"}

    # Zero-ink gate. Two white frames "match" because the engine rendered
    # neither document — that is a refusal, not a pass.
    blanks = [label for label, ppm in (("test", test_ppm), ("ref", ref_ppm)) if not has_ink(ppm)]
    if blanks:
        return {
            "status": "ERROR",
            "reason": f"zero-ink frame ({'+'.join(blanks)}) — render refusal, not a match",
        }

    diff_pixels, diff_pct = compare_frames(test_ppm, ref_ppm)
    campaign_pct, campaign_why = campaign_diff(test_ppm, ref_ppm)
    # compare_frames is total TODAY (dimension mismatch = full-frame diff),
    # but `diff_pct` is a nullable field everywhere rows round-trip through
    # JSON, and the guard test treats every consumer alike — rightly, since
    # a refactor that routes a loaded row through here would make the
    # ordering below silently compare None. Fail loud instead.
    if diff_pct is None:
        return {
            "status": "ERROR",
            "reason": "comparator returned no diff_pct — refusing to grade",
        }
    status = "PASS" if diff_pct <= WPT_MAX_DIFF_PCT else "FAIL"
    return {
        "status": status,
        "reason": None,
        "diff_pct": round(diff_pct, 4),
        "diff_pixels": diff_pixels,
        "campaign_diff_pct": campaign_pct,
        "campaign_not_measured": campaign_why,
    }


def style_removed(html: str) -> str:
    """The page with no author style: no <style>, no sheet links, no style= attributes."""
    html = STYLE_ELEMENT_RE.sub("", html)
    html = SHEET_LINK_RE.sub(
        lambda m: "" if _rel_has(page_tags(m.group(0)).tags[0][1], "stylesheet") else m.group(0), html
    )
    return TAG_WITH_ATTRS_RE.sub(
        lambda t: STYLE_ATTR_RE.sub(lambda m: m.group(2) or m.group(1) or "", t.group(0)), html
    )


def apply_mutation(html: str, mutation):
    """The page with this mutation in, or None if the page does not say `find`."""
    if mutation["find"] not in html:
        return None
    return html.replace(mutation["find"], mutation["replace"])


def area_mutated(html: str, area):
    """The page with its area's feature forced off, or None if the area has no probe.
    The rule goes last so a same-specificity !important on the page does not beat it."""
    declarations = AREA_PROBES.get(area)
    if declarations is None:
        return None
    rules = "".join(f"{selector} {{ {declarations} }}\n" for selector in ("*", "*::before", "*::after"))
    return f"{html}\n<style>\n{rules}</style>\n"


def area_probe_reason_prefix(area) -> str:
    return f"still matches under the {area} probe"


def probe_blind_spot(html: str):
    """What on this page the probes cannot break, if anything: a reader of an
    UNPROVEN row should know the probe may have missed the asserted feature."""
    scripts = "\n".join(SCRIPT_ELEMENT_RE.findall(html))
    if SCRIPT_TRANSFORM_RE.search(scripts) or any("transform" in attrs for _tag, attrs in page_tags(html).tags):
        return "the page sets an SVG transform attribute, which removing style may not touch"
    if SCRIPT_CLASS_RE.search(scripts):
        return "the page toggles a class from script, so a probe may not reach what it asserts"
    return None


def _probe_matches(name, html, test_file, ref_ppm, viewport, workdir):
    """(still matches the reference, why it could not tell). The page is gone after."""
    page = test_file.with_name(f"{test_file.stem}.__probe-{name}{test_file.suffix}")
    page.write_text(html, encoding="utf-8")
    frame = workdir / f"probe-{name}.ppm"
    try:
        why = capture(stage_html(page), frame, viewport)
    finally:
        with _STAGE_LOCK:
            staged = _STAGED.pop(page, None)
        page.unlink()
        if staged:
            staged.unlink(missing_ok=True)
    if why:
        return False, why
    return compare_frames(frame, ref_ppm)[0] == 0, None


def prove_pass(entry, test_file: Path, ref_ppm: Path, viewport, workdir: Path):
    """Why this match proves nothing, or None if every probe went red."""
    html = test_file.read_text(errors="replace", encoding="utf-8")
    matches, why = _probe_matches("unstyled", style_removed(html), test_file, ref_ppm, viewport, workdir)
    if why:
        return f"style-removed probe rendered no frame: {why}"
    if matches:
        return "still matches with all its style removed"
    area = entry.get("area")
    forced_off = area_mutated(html, area)
    if forced_off is not None:
        matches, why = _probe_matches("area", forced_off, test_file, ref_ppm, viewport, workdir)
        if why:
            return f"{area} probe rendered no frame: {why}"
        if matches:
            return f"{area_probe_reason_prefix(area)} ({AREA_PROBES[area]})"
    for i, mutation in enumerate(entry.get("mutations", [])):
        mutated = apply_mutation(html, mutation)
        if mutated is None:
            return f"mutation does not apply, page has no {mutation['find']!r}"
        matches, why = _probe_matches(f"m{i}", mutated, test_file, ref_ppm, viewport, workdir)
        if why:
            return f"mutation {mutation['find']!r} rendered no frame: {why}"
        if matches:
            return f"still matches with {mutation['find']!r} changed to {mutation['replace']!r}"
    return None


def run_case(entry, viewport):
    """One manifest entry, start to finish; returns its row."""
    case = {
        "id": entry["id"],
        "area": entry.get("area") or "seed",
        "status": None,
        "reason": None,
        "diff_pct": None,
        "diff_pixels": None,
        "campaign_diff_pct": None,
        "campaign_not_measured": None,
        "rel_match": None,
        "blocked_by": None,
        "maps_to": entry.get("maps_to"),
        "chromium_diff_pixels": None,
        "chromium_not_measured": None,
    }
    test_file = WPT_ROOT / entry["path"]
    ref_file = WPT_ROOT / entry["ref"] if entry.get("ref") else None

    if entry.get("kind") != "reftest":
        case.update(status="SKIP", reason=f"kind={entry.get('kind')} not run in W0b")
        return case
    if not test_file.exists():
        case.update(status="ERROR", rel_match="missing",
                    reason="test file absent after --check passed (instrument)")
        return case
    if ref_file is None or not ref_file.exists():
        case.update(status="SKIP", rel_match="missing", reason="missing ref")
        return case
    # rel=match is the authority; the manifest ref is a candidate.
    hrefs = rel_match_hrefs(test_file)
    if not hrefs:
        case.update(status="ERROR", rel_match="missing",
                    reason="test has no <link rel=match> — not a reftest at this pin")
        return case
    resolved = [
        (test_file.parent / h).resolve().relative_to(WPT_ROOT.resolve())
        for h in hrefs
    ]
    if ref_file.resolve().relative_to(WPT_ROOT.resolve()) not in resolved:
        case.update(
            status="ERROR", rel_match="mismatch",
            reason=f"manifest ref {entry['ref']} not among rel=match "
                   f"targets {[str(r) for r in resolved]} — instrument error, "
                   f"pixels not compared",
        )
        return case
    case["rel_match"] = "ok"
    test_src = test_file.read_text(errors="replace", encoding="utf-8")
    ref_src = ref_file.read_text(errors="replace", encoding="utf-8")
    absent = missing_resources(test_file, WPT_ROOT) + missing_resources(ref_file, WPT_ROOT)
    stop = gate(test_src, ref_src, absent)
    if stop:
        case.update(stop)
        return case
    if webfont_dependent(test_src, ref_src):
        case["blocked_by"] = WEBFONT_GAP
    with tempfile.TemporaryDirectory(prefix="wpt-") as td:
        case.update(run_pair(test_file, ref_file, viewport, Path(td)))
        if case["status"] == "PASS":
            why = prove_pass(entry, test_file, Path(td) / "ref.ppm", viewport, Path(td))
            if why:
                blind = probe_blind_spot(test_src)
                case.update(status="UNPROVEN", reason=f"{why}; {blind}" if blind else why)
    return case


def chromium_pair_diffs(pairs, viewport):
    """Chromium's own test against its own reference, exact RGB, for each pair:
    `(pixels differing by id, browser version, None)`, or `(None, None, why)`."""
    if not (ORACLE_DIR / "node_modules").is_dir():
        return None, None, "tools/parity_oracle/node_modules is missing"
    try:
        proc = subprocess.run(
            ["node", "wpt_pair_chromium.mjs", str(viewport["width"]), str(viewport["height"])],
            input=json.dumps(pairs), cwd=ORACLE_DIR, capture_output=True, text=True, timeout=CHROMIUM_TIMEOUT_S, encoding="utf-8"
        )
    except FileNotFoundError:
        return None, None, "node not found"
    except subprocess.TimeoutExpired:
        return None, None, f"wpt_pair_chromium.mjs timeout ({CHROMIUM_TIMEOUT_S}s)"
    except OSError as e:
        return None, None, f"node did not start: {type(e).__name__}"
    if proc.returncode != 0:
        tail = (proc.stderr or proc.stdout or "").strip().splitlines()
        return None, None, f"wpt_pair_chromium.mjs exit {proc.returncode}: {tail[-1][:200] if tail else 'no output'}"
    try:
        answer = json.loads(proc.stdout)
        return answer["diffs"], answer["version"], None
    except (ValueError, KeyError, TypeError):
        return None, None, "wpt_pair_chromium.mjs printed no usable JSON"


def _ask_chromium(asked, entries, viewport, oracle):
    """Put Chromium's answer on each case asked about; returns its version. A case
    with no answer says why in `chromium_not_measured`."""
    by_id = {e["id"]: e for e in entries}
    pairs = [
        {
            "id": c["id"],
            "test": str(stage_html(WPT_ROOT / by_id[c["id"]]["path"])),
            "ref": str(stage_html(WPT_ROOT / by_id[c["id"]]["ref"])),
        }
        for c in asked
    ]
    diffs, version, why = oracle(pairs, viewport)
    for case in asked:
        if diffs is None:
            case["chromium_not_measured"] = why
        elif case["id"] not in diffs:
            case["chromium_not_measured"] = "the oracle gave no answer for this case"
        else:
            case["chromium_diff_pixels"] = diffs[case["id"]]
    return version


def recheck_small_fails(cases, entries, viewport, oracle=chromium_pair_diffs):
    """A FAIL under SMALL_FAIL_PCT that Chromium's own test and reference also fail
    under the same staging says nothing about the engine: SKIP, with the reason. A
    FAIL with no answer stays a FAIL and says why."""
    small = [
        c for c in cases
        if c["status"] == "FAIL" and c["diff_pct"] is not None and c["diff_pct"] < SMALL_FAIL_PCT
    ]
    if not small:
        return
    version = _ask_chromium(small, entries, viewport, oracle)
    for case in small:
        if case.get("chromium_diff_pixels"):
            case.update(
                status="SKIP",
                reason=f"Chromium {version}'s own test and reference differ by "
                       f"{case['chromium_diff_pixels']} px under the same staging; not an engine verdict",
            )


def is_area_probe_demotion(case) -> bool:
    return case["status"] == "UNPROVEN" and case["reason"].startswith(area_probe_reason_prefix(case["area"]))


def recheck_area_probes(cases, entries, viewport, oracle=chromium_pair_diffs):
    """A match demoted by its area probe, on a page Chromium's own test and reference
    do not match on: the page is no evidence for the engine or against it. It stays
    UNPROVEN and its reason says so."""
    probed = [c for c in cases if is_area_probe_demotion(c)]
    if not probed:
        return
    version = _ask_chromium(probed, entries, viewport, oracle)
    for case in probed:
        if case["chromium_diff_pixels"]:
            case["reason"] += (
                f"; Chromium {version}'s own test and reference differ by {case['chromium_diff_pixels']} px "
                "under the same staging, so this page is not evidence either way"
            )


def area_probe_chromium_counts(cases):
    """What Chromium said about the area-probe demotions, as one count per answer."""
    probed = [c for c in cases if is_area_probe_demotion(c)]
    pixels = [c["chromium_diff_pixels"] for c in probed]
    return {
        "asked": len(probed),
        "differ": sum(1 for p in pixels if p),
        "match": sum(1 for p in pixels if p == 0),
        "not_measured": sum(1 for p in pixels if p is None),
    }


def chromium_gap_lines(cases):
    """One console line per question Chromium did not answer, and why."""
    gaps = collections.Counter(
        ("area probe" if is_area_probe_demotion(c) else "small FAIL", c["chromium_not_measured"])
        for c in cases if c["chromium_not_measured"]
    )
    return [f"{question} not checked in Chromium: {why} ({n})" for (question, why), n in sorted(gaps.items())]


def remove_staged():
    """Every staged copy under the WPT root, this run's and any an interrupted run
    left. The fixtures on disk stay verbatim."""
    with _STAGE_LOCK:
        _STAGED.clear()
    for staged in WPT_ROOT.rglob(f"*{STAGED_SUFFIX}*"):
        if staged.is_file():
            staged.unlink()


def run_cases(entries, viewport, jobs, verbose):
    """Every entry to its final row. The rechecks read staged pages, so those go last."""
    cases = []
    try:
        with concurrent.futures.ThreadPoolExecutor(max_workers=max(1, jobs)) as pool:
            for case in pool.map(lambda entry: run_case(entry, viewport), entries):
                cases.append(case)
                if verbose or case["status"] != "PASS":
                    detail = case["reason"] or f"diff {case['diff_pct']}%"
                    print(f"  {case['id']:55s} {case['status']:8s} {detail}", flush=True)
                else:
                    print(f"  {case['id']:55s} PASS", flush=True)
        recheck_small_fails(cases, entries, viewport)
        recheck_area_probes(cases, entries, viewport)
    finally:
        remove_staged()
    return cases


def negative_control(viewport) -> bool:
    """Prove the oracle can go red. True = it failed the mismatched pair."""
    test = NEGATIVE_CONTROL_DIR / "control.html"
    notref = NEGATIVE_CONTROL_DIR / "control-notref.html"
    if not test.exists() or not notref.exists():
        print("HARNESS ERROR: negative-control fixtures missing", file=sys.stderr)
        return False
    with tempfile.TemporaryDirectory(prefix="wpt-negctl-") as td:
        result = run_pair(test, notref, viewport, Path(td), stage=False)
    if result["status"] != "FAIL":
        print(
            f"HARNESS ERROR: negative control returned {result['status']} "
            f"({result.get('reason')}) — a harness that cannot fail a deliberate "
            f"mismatch publishes nothing",
            file=sys.stderr,
        )
        return False
    print(f"negative control: FAIL as required "
          f"(diff {result['diff_pct']}%) — oracle can go red")
    return True


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--case", help="run a single manifest id")
    parser.add_argument("--area", help="run one area of the manifest (`seed` is the 30 of W0b)")
    parser.add_argument("--jobs", type=int, default=min(4, os.cpu_count() or 1),
                        help="cases rendered at once (default 4)")
    parser.add_argument("--verbose", "-v", action="store_true")
    parser.add_argument("--allow-stale", action="store_true",
                        help="skip the binary-freshness guard (records the run as suspect)")
    args = parser.parse_args()

    manifest = json.loads(MANIFEST_PATH.read_text(encoding="utf-8"))
    viewport = manifest["default_viewport"]
    entries = manifest["entries"]
    if args.case:
        entries = [e for e in entries if e["id"] == args.case]
        if not entries:
            print(f"no manifest entry with id {args.case}", file=sys.stderr)
            return 1
    if args.area:
        entries = [e for e in entries if (e.get("area") or "seed") == args.area]
        if not entries:
            print(f"no manifest entries in area {args.area}", file=sys.stderr)
            return 1

    if not CAPTURE_BIN.exists():
        print(f"parity-capture not built: {CAPTURE_BIN}\n"
              f"run: cargo build --profile parity -p parity-capture", file=sys.stderr)
        return 1

    # n24 lesson (2026-08-12): this runner once scored a stale binary as a
    # confirmed null result. The artifact it consumes must be newer than the
    # sources it was built from, or the run is not a measurement.
    if not args.allow_stale:
        newest_src, newest_path = 0.0, None
        roots = [REPO_ROOT / "crates", REPO_ROOT / "Cargo.lock"]
        for root in roots:
            candidates = root.rglob("*.rs") if root.is_dir() else [root]
            for f in candidates:
                m = f.stat().st_mtime
                if m > newest_src:
                    newest_src, newest_path = m, f
        if CAPTURE_BIN.stat().st_mtime < newest_src:
            print(f"parity-capture is STALE: {newest_path} is newer than the binary.\n"
                  f"run: cargo build --profile parity -p parity-capture\n"
                  f"(or pass --allow-stale to record a knowingly-suspect run)",
                  file=sys.stderr)
            return 1

    check = subprocess.run(
        [str(REPO_ROOT / "scripts" / "wpt_sync.sh"), "--check"],
        capture_output=True, text=True, encoding="utf-8"
    )
    if check.returncode != 0:
        print("wpt_sync.sh --check failed — sync before running:\n"
              + check.stdout + check.stderr, file=sys.stderr)
        return 1

    if not negative_control(viewport):
        return 1

    hiwave_sha = subprocess.run(
        ["git", "rev-parse", "HEAD"], cwd=REPO_ROOT,
        capture_output=True, text=True, encoding="utf-8"
    ).stdout.strip()

    cases = run_cases(entries, viewport, args.jobs, args.verbose)
    rechecked = [c for c in cases if c["chromium_diff_pixels"] is not None]
    for case in rechecked:
        print(f"  chromium: {case['id']} differs by {case['chromium_diff_pixels']} px -> {case['status']}")

    counts = count_statuses(cases)
    scored = counts["PASS"] + counts["FAIL"] + counts["UNPROVEN"]
    rate = pass_rate(counts)
    areas = per_area(cases)

    # Whether THIS run has the shape the pin calls presumptively dishonest:
    # everything green, nothing red, on the full seed. Computed, not asserted
    # — a field hardcoded to true says the same thing after a 6-FAIL run as
    # after a suspicious one, which makes it decoration rather than a check.
    all_green = (
        counts["PASS"] == len(cases)
        and counts["FAIL"] == 0
        and counts["ERROR"] == 0
    )

    last_run = {
        "schema": 1,
        "wpt_pin": manifest["wpt_pin"],
        "hiwave_git_sha": hiwave_sha,
        "develop_sha": develop_sha_at(REPO_ROOT),
        "oracle": oracle_record(),
        "capture_binary": capture_label(CAPTURE_BIN),
        "runner": "scripts/wpt_tier1.py",
        "viewport": viewport,
        "wpt_max_diff_pct": WPT_MAX_DIFF_PCT,
        "ts": datetime.now(timezone.utc).isoformat(),
        "n": len(cases),
        "pass": counts["PASS"],
        "fail": counts["FAIL"],
        "unproven": counts["UNPROVEN"],
        "skip": counts["SKIP"],
        "error": counts["ERROR"],
        "rate": rate,
        "campaign_rule_counts": campaign_counts(cases),
        "areas": areas,
        "unproven_passes": unproven_passes(cases),
        "area_probe_chromium": area_probe_chromium_counts(cases),
        "attribution": {
            "_comment": "Fails grouped by a named engine capability gap. Attribution only — these cases stay in the denominator. A gap here is a lane, not an excuse.",
            "blocked_fails": {
                gap: sorted(c["id"] for c in cases
                            if c["status"] == "FAIL" and c.get("blocked_by") == gap)
                for gap in sorted({c["blocked_by"] for c in cases
                                   if c["status"] == "FAIL" and c.get("blocked_by")})
            },
            "suspect_passes": {
                "_comment": "PASSes on tests that declare a web font. Before n33 the font never loaded, so these were the dangerous direction (a green case not measuring its own assertion); since n33 the face loads (TTF/OTF) and a PASS here is measured in the declared font. Kept as the same field for trendline continuity.",
                "ids": sorted(c["id"] for c in cases
                              if c["status"] == "PASS" and c.get("blocked_by")),
            },
        },
        "cases": cases,
        "honesty": {
            "all_green": all_green,
            "all_green_suspect": all_green,
            "negative_control": "FAIL as required (checked every run; run aborts otherwise)",
            "note": (
                "SUSPECT: this run is all-green on the full seed, which the W0b "
                "exit criteria treat as presumptively a lying harness rather than "
                "a passing engine. Verify the oracle before quoting this rate."
                if all_green else
                "Not suspect: this run contains red "
                f"({counts['FAIL']} FAIL, {counts['ERROR']} ERROR), so the oracle "
                "demonstrably distinguishes pass from fail on real cases. A run "
                "with pass==n and fail==0 and error==0 would flip this field."
            ),
        },
    }

    if not args.case and not args.area:
        LAST_RUN_PATH.write_text(json.dumps(last_run, indent=2) + "\n", encoding="utf-8")
        print(f"\nwrote {LAST_RUN_PATH.relative_to(REPO_ROOT)}")

    print(f"\nWPT Tier-1: {counts['PASS']}/{scored} scored "
          f"(pass {counts['PASS']} / fail {counts['FAIL']} / unproven {counts['UNPROVEN']} / "
          f"skip {counts['SKIP']} / error {counts['ERROR']}, n={len(cases)}) "
          f"@ pin {manifest['wpt_pin'][:12]}")
    print(f"  {'area':28s} {'pass':>5s} {'fail':>5s} {'unprv':>5s} {'skip':>5s} {'error':>5s}")
    for name, row in areas.items():
        print(f"  {name:28s} {row['PASS']:5d} {row['FAIL']:5d} {row['UNPROVEN']:5d} {row['SKIP']:5d} {row['ERROR']:5d}")
    held = last_run["campaign_rule_counts"]
    print(f"  campaign rule, reported only: {held['within']} within "
          f"{THRESHOLDS['default']}%, {held['over']} over, {held['not_measured']} not measured")
    for why in sorted({c["campaign_not_measured"] for c in cases if c["campaign_not_measured"]}):
        print(f"  campaign rule not measured: {why}")
    if last_run["unproven_passes"]:
        print(f"  unproven: {len(last_run['unproven_passes'])} (see last-run.json)")
    split = last_run["area_probe_chromium"]
    print(f"  area probes in Chromium: {split['asked']} asked, {split['differ']} differ, "
          f"{split['match']} match, {split['not_measured']} not measured")
    for line in chromium_gap_lines(cases):
        print(f"  {line}")
    for gap, ids in last_run["attribution"]["blocked_fails"].items():
        print(f"  attributed: {len(ids)} fail(s) blocked by {gap}")
    suspect = last_run["attribution"]["suspect_passes"]["ids"]
    if suspect:
        print(f"  web-font PASS(es) ({len(suspect)}; measured in the declared face since n33): "
              + ", ".join(suspect))
    return 0


if __name__ == "__main__":
    sys.exit(main())
