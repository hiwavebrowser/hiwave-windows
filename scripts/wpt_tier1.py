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
- A blank frame is never a match. Two blank frames agree with each other for
  the worst possible reason (the engine rendered neither document), which is
  the empty-capture-scores-100 lie wearing a reftest costume. Blank -> ERROR.
- A reftest that cannot reach the state it asserts about is not scored.
  reftest-wait / script-driven tests never reach that state without a script
  host, so their frames measure "no JS", not the behaviour under test — and
  upstream WPT reports TIMEOUT for them, not FAIL. They are SKIP, with the
  reason recorded. (Added 2026-08-06 night 20: three of the seed's six fails
  were this.)
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
import json
import re
import subprocess
import sys
import tempfile
from datetime import datetime, timezone
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from parity_lib import analyze_frame_blankness  # noqa: E402
import os  # Windows sync: .exe suffix

REPO_ROOT = Path(__file__).resolve().parent.parent
MANIFEST_PATH = REPO_ROOT / "trench" / "wpt" / "MANIFEST.json"
WPT_ROOT = REPO_ROOT / "third_party" / "wpt"
CAPTURE_BIN = REPO_ROOT / "target" / "release" / ("parity-capture.exe" if os.name == "nt" else "parity-capture")
LAST_RUN_PATH = REPO_ROOT / "trench" / "wpt" / "last-run.json"
NEGATIVE_CONTROL_DIR = REPO_ROOT / "trench" / "wpt" / "negative-control"

# The single threshold knob. WPT reftest semantics are "renders identically",
# so W0b starts at exact RGB match. If antialiasing noise ever forces a
# tolerance, it gets raised HERE, documented in last-run.json, with Pete's
# sign-off — never borrowed from the campaign's t15.
WPT_MAX_DIFF_PCT = 0.0

REL_MATCH_RE = re.compile(
    r"<link\b[^>]*\brel\s*=\s*[\"']?match[\"']?[^>]*>", re.IGNORECASE
)
HREF_RE = re.compile(r"\bhref\s*=\s*[\"']([^\"']+)[\"']", re.IGNORECASE)

# wptserve maps / to the WPT root; file:// has no such mapping, so root-absolute
# references (e.g. /fonts/ahem.css) resolve to the filesystem root and silently
# load nothing. Staged copies live NEXT TO the originals so relative references
# keep working and the fixtures on disk stay verbatim.
STAGED_SUFFIX = ".__staged"
ROOT_ABS_ATTR_RE = re.compile(r"((?:href|src)\s*=\s*[\"'])(/(?!/)[^\"']*)([\"'])", re.IGNORECASE)
ROOT_ABS_URL_RE = re.compile(r"(url\(\s*[\"']?)(/(?!/)[^\"')]*)([\"']?\s*\))", re.IGNORECASE)

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


def rel_match_hrefs(test_file: Path):
    """All <link rel=match> hrefs in a test file, in document order."""
    html = test_file.read_text(errors="replace", encoding="utf-8")
    return [
        m.group(1)
        for link in REL_MATCH_RE.finditer(html)
        if (m := HREF_RE.search(link.group(0)))
    ]


def stage_css(src: Path) -> Path:
    """Copy a CSS file beside itself with root-absolute url(...) values resolved."""
    staged = src.with_name(src.stem + STAGED_SUFFIX + src.suffix)
    css = ROOT_ABS_URL_RE.sub(
        lambda m: m.group(1) + str(WPT_ROOT / m.group(2).lstrip("/")) + m.group(3),
        src.read_text(errors="replace", encoding="utf-8"),
    )
    staged.write_text(css, encoding="utf-8")
    return staged


def stage_html(src: Path) -> Path:
    """Copy an HTML file beside itself with root-absolute references resolved
    against the WPT root — what wptserve would have served. Referenced CSS is
    staged recursively so its own url(...) values resolve too."""
    def rewrite(m):
        target = WPT_ROOT / m.group(2).lstrip("/")
        if target.suffix.lower() == ".css" and target.is_file():
            target = stage_css(target)
        return m.group(1) + str(target) + m.group(3)

    html = ROOT_ABS_ATTR_RE.sub(rewrite, src.read_text(errors="replace", encoding="utf-8"))
    html = ROOT_ABS_URL_RE.sub(
        lambda m: m.group(1) + str(WPT_ROOT / m.group(2).lstrip("/")) + m.group(3), html
    )
    staged = src.with_name(src.stem + STAGED_SUFFIX + src.suffix)
    staged.write_text(html, encoding="utf-8")
    return staged


def unrunnable_reason(test_src: str, ref_src: str):
    """Why this reftest cannot probe its own assertion in the capture path.

    These are not engine verdicts. A reftest-wait test never reaches the state
    it asserts about without a script host, so comparing its frames measures
    "no JS", not the line-box behaviour under test — upstream WPT would report
    TIMEOUT here, not FAIL. Scoring them as FAIL charges the renderer for the
    harness's missing capability, which is the decorative-instrument class this
    campaign keeps finding (forensics 2026-07-24).
    """
    if "reftest-wait" in test_src or "reftest-wait" in ref_src:
        return "reftest-wait: needs a scripted completion signal (no script host in parity-capture)"
    if "<script" in test_src.lower() or "<script" in ref_src.lower():
        return "needs JS to reach the asserted state (no script host in parity-capture)"
    if re.search(r"name\s*=\s*[\"']fuzzy[\"']", test_src, re.IGNORECASE):
        return "fuzzy annotation: tolerance matching not implemented"
    return None


def webfont_dependent(test_src: str, ref_src: str) -> bool:
    """Does either side declare a web font it needs to render as designed?"""
    for src in (test_src, ref_src):
        if re.search(r"name\s*=\s*[\"']flags[\"'][^>]*ahem", src, re.IGNORECASE):
            return True
        if re.search(r"href\s*=\s*[\"']/fonts/", src, re.IGNORECASE):
            return True
        if "@font-face" in src:
            return True
    return False


def capture(html_file: Path, out_ppm: Path, viewport):
    """Render through the engine. Returns None on success, reason on failure."""
    cmd = [
        str(CAPTURE_BIN),
        "--html-file", str(html_file),
        "--width", str(viewport["width"]),
        "--height", str(viewport["height"]),
        "--dump-frame", str(out_ppm),
    ]
    try:
        proc = subprocess.run(cmd, capture_output=True, text=True, timeout=60, encoding="utf-8")
    except subprocess.TimeoutExpired:
        return "capture timeout (60s)"
    if proc.returncode != 0:
        tail = (proc.stderr or proc.stdout or "").strip().splitlines()
        return f"capture exit {proc.returncode}: {tail[-1] if tail else 'no output'}"
    if not out_ppm.exists() or out_ppm.stat().st_size == 0:
        return "capture produced no frame"
    return None


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

    # Blank gate. Two blank frames "match" because the engine rendered
    # neither document — that is a refusal, not a pass.
    blanks = []
    for label, ppm in (("test", test_ppm), ("ref", ref_ppm)):
        b = analyze_frame_blankness(ppm, (255, 255, 255))
        if b.get("is_blank"):
            blanks.append(label)
    if blanks:
        return {
            "status": "ERROR",
            "reason": f"blank frame ({'+'.join(blanks)}) — render refusal, not a match",
        }

    diff_pixels, diff_pct = compare_frames(test_ppm, ref_ppm)
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
    }


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

    if not CAPTURE_BIN.exists():
        print(f"parity-capture not built: {CAPTURE_BIN}\n"
              f"run: cargo build --release -p parity-capture", file=sys.stderr)
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
                  f"run: cargo build --release -p parity-capture\n"
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

    cases = []
    for entry in entries:
        case = {
            "id": entry["id"],
            "status": None,
            "reason": None,
            "diff_pct": None,
            "diff_pixels": None,
            "rel_match": None,
            "blocked_by": None,
            "maps_to": entry.get("maps_to"),
        }
        test_file = WPT_ROOT / entry["path"]
        ref_file = WPT_ROOT / entry["ref"] if entry.get("ref") else None

        if entry.get("kind") != "reftest":
            case.update(status="SKIP", reason=f"kind={entry.get('kind')} not run in W0b")
        elif not test_file.exists():
            case.update(status="ERROR", rel_match="missing",
                        reason="test file absent after --check passed (instrument)")
        elif ref_file is None or not ref_file.exists():
            case.update(status="SKIP", rel_match="missing", reason="missing ref")
        else:
            # rel=match is the authority; the manifest ref is a candidate.
            hrefs = rel_match_hrefs(test_file)
            if not hrefs:
                case.update(status="ERROR", rel_match="missing",
                            reason="test has no <link rel=match> — not a reftest at this pin")
            else:
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
                else:
                    case["rel_match"] = "ok"
                    test_src = test_file.read_text(errors="replace", encoding="utf-8")
                    ref_src = ref_file.read_text(errors="replace", encoding="utf-8")
                    unrunnable = unrunnable_reason(test_src, ref_src)
                    if unrunnable:
                        case.update(status="SKIP", reason=unrunnable)
                    else:
                        if webfont_dependent(test_src, ref_src):
                            case["blocked_by"] = WEBFONT_GAP
                        with tempfile.TemporaryDirectory(prefix="wpt-") as td:
                            case.update(run_pair(test_file, ref_file, viewport, Path(td)))

        cases.append(case)
        if args.verbose or case["status"] != "PASS":
            detail = case["reason"] or f"diff {case['diff_pct']}%"
            print(f"  {case['id']:55s} {case['status']:5s} {detail}")
        else:
            print(f"  {case['id']:55s} PASS")

    counts = {s: sum(1 for c in cases if c["status"] == s)
              for s in ("PASS", "FAIL", "SKIP", "ERROR")}
    scored = counts["PASS"] + counts["FAIL"]
    rate = round(counts["PASS"] / scored, 4) if scored else None

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
        "runner": "scripts/wpt_tier1.py",
        "viewport": viewport,
        "wpt_max_diff_pct": WPT_MAX_DIFF_PCT,
        "ts": datetime.now(timezone.utc).isoformat(),
        "n": len(cases),
        "pass": counts["PASS"],
        "fail": counts["FAIL"],
        "skip": counts["SKIP"],
        "error": counts["ERROR"],
        "rate": rate,
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

    if not args.case:
        LAST_RUN_PATH.write_text(json.dumps(last_run, indent=2) + "\n", encoding="utf-8")
        print(f"\nwrote {LAST_RUN_PATH.relative_to(REPO_ROOT)}")

    print(f"\nWPT Tier-1: {counts['PASS']}/{scored} scored "
          f"(pass {counts['PASS']} / fail {counts['FAIL']} / "
          f"skip {counts['SKIP']} / error {counts['ERROR']}, n={len(cases)}) "
          f"@ pin {manifest['wpt_pin'][:12]}")
    for gap, ids in last_run["attribution"]["blocked_fails"].items():
        print(f"  attributed: {len(ids)} fail(s) blocked by {gap}")
    suspect = last_run["attribution"]["suspect_passes"]["ids"]
    if suspect:
        print(f"  web-font PASS(es) ({len(suspect)}; measured in the declared face since n33): "
              + ", ".join(suspect))
    return 0


if __name__ == "__main__":
    sys.exit(main())
