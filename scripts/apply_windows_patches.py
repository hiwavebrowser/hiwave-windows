"""Apply the Windows-local patches to freshly copied RustKit engine crates.

An engine refresh copies crates/rustkit-* verbatim from hiwave-macos. A small
number of differences are Windows-specific and live only in this repo (rule
#528: platform-specific changes are not sent upstream). This script re-applies
them after the copy, so a refresh is mechanical and a reviewer can check the
residual with one command:

    python scripts/apply_windows_patches.py [--check]

Each patch is idempotent. It looks for its own marker first and skips when the
patch is already there. If the marker is absent AND the upstream text it needs
to edit has moved, it stops with an error instead of silently doing nothing:
that means upstream changed the code and a person must look.

--check reports the state without writing anything (exit 1 if a patch is
missing).

The patches (all cfg(windows), so macOS and Linux behaviour is unchanged):

1. rustkit-layout/src/text.rs: FontFamilyChain.
   - sans_serif() leads with Arial. Chrome's default sans-serif on Windows is
     Arial, not Segoe UI (Segoe UI is what system-ui resolves to).
   - from_css_value() skips -apple-system and BlinkMacSystemFont. Chrome on
     Windows does not know these macOS-only names, so a stack such as
     "-apple-system, BlinkMacSystemFont, sans-serif" falls through to
     sans-serif. A 14px normal line is 16px in Arial and 19px in Segoe UI.
   - tests for both.
2. rustkit-layout/src/lib.rs: two strut-descent tests compared a raw fractional
   descent with a line box that rounds to whole pixels, within 0.5. Both now
   allow one pixel. Arial's 3.453 descent rounds up to 4.
(A third patch, removing the ENGINE_INIT test mutex from rustkit-engine, was
dropped in refresh #10: hiwave-macos #415 fixed the lock inversion upstream.)

Then the patch FILES in scripts/windows-patches/*.patch, in name order. Each
is a `git format-patch` of an upstream PR that Windows needs before it lands
(the file's own header names the PR). Applied with `git apply`; a patch that
no longer applies AND whose marker (its first added test) is already in the
tree counts as landed upstream, and the script says so: that is the cue to
delete the file. Any other failure to apply is an error.
"""
from __future__ import annotations

import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
TEXT = REPO / "crates" / "rustkit-layout" / "src" / "text.rs"
LIB = REPO / "crates" / "rustkit-layout" / "src" / "lib.rs"
MATRIX = REPO / "crates" / "rustkit-engine" / "src" / "nested_layout_matrix_tests.rs"
FLEX_AR = REPO / "crates" / "rustkit-engine" / "src" / "flex_aspect_ratio_tests.rs"
BS = chr(92)  # a backslash, built here so no literal backslash sits in this source


def read(path: Path) -> tuple[str, str]:
    raw = path.read_bytes().decode("utf-8")
    nl = "\r\n" if "\r\n" in raw else "\n"
    return raw.replace("\r\n", "\n"), nl


def write(path: Path, text: str, nl: str) -> None:
    path.write_bytes(text.replace("\n", nl).encode("utf-8"))


def edit(text: str, name: str, marker: str, old: str, new: str, state: list,
         count: int = 1) -> str:
    """Replace `old` with `new` once (or exactly `count` times), unless `marker`
    shows the patch is already in. Anything else is an error: upstream moved."""
    if marker in text:
        state.append((name, "already applied"))
        return text
    if text.count(old) != count:
        state.append((name, f"MISSING: upstream moved, apply by hand (found {text.count(old)}, want {count})"))
        return text
    state.append((name, "applied"))
    return text.replace(old, new)


def patch_text(text: str, state: list) -> str:
    quote = "'" + BS + "''"  # the three characters  '\''  as written in the Rust source
    # 1. (retired in refresh #22) sans_serif(): Arial first on Windows is upstream: hiwave-macos
    #    #1351 gives Windows its own sans_serif() leading with Arial (develop b60da7f5).
    # 2. from_css_value(): on Windows `BlinkMacSystemFont` is an unknown family too. Upstream skips
    #    `-apple-system` everywhere (CB7b) and maps `BlinkMacSystemFont` to system-ui, which is what
    #    Chrome on macOS does; Chrome for Testing 148 on Windows (win64) treats it as unknown:
    #    "BlinkMacSystemFont" alone is the default serif (67.266 px for "Hello world" at 14px, as
    #    serif), "BlinkMacSystemFont, sans-serif" is Arial (69.250), system-ui is Segoe UI (70.906).
    #    docs/diagnostics/2026-11-18/font_alias_win148.json.
    text = edit(
        text, "text.rs skips BlinkMacSystemFont on Windows",
        'cfg!(windows) && f.eq_ignore_ascii_case("BlinkMacSystemFont")',
        '''            .filter(|f| !f.eq_ignore_ascii_case("-apple-system"))
''',
        '''            .filter(|f| !f.eq_ignore_ascii_case("-apple-system"))
            // Windows: Chrome there does not know `BlinkMacSystemFont` either (macOS's name for the
            // system font), so it is skipped too (Chrome for Testing 148 win64:
            // docs/diagnostics/2026-11-18/font_alias_win148.json).
            .filter(|f| !(cfg!(windows) && f.eq_ignore_ascii_case("BlinkMacSystemFont")))
''', state)
    # 3. tests.
    text = edit(
        text, "text.rs sans-serif test expectation",
        '#[cfg(windows)]\n        assert_eq!(sans.primary, "Arial");',
        '''        #[cfg(target_os = "macos")]
        assert_eq!(sans.primary, "Helvetica");
        #[cfg(not(target_os = "macos"))]
        assert_eq!(sans.primary, "Segoe UI");
''',
        '''        #[cfg(target_os = "macos")]
        assert_eq!(sans.primary, "Helvetica");
        #[cfg(windows)]
        assert_eq!(sans.primary, "Arial");
        #[cfg(all(not(target_os = "macos"), not(windows)))]
        assert_eq!(sans.primary, "Segoe UI");
''', state)
    text = edit(
        text, "text.rs alias tests: BlinkMacSystemFont",
        "Chrome on Windows does not know `BlinkMacSystemFont`",
        '''        let blink = FontFamilyChain::from_css_value("BlinkMacSystemFont");
        assert_eq!(blink.primary, system.primary);
    }
''',
        '''        let blink = FontFamilyChain::from_css_value("BlinkMacSystemFont");
        #[cfg(not(windows))]
        assert_eq!(blink.primary, system.primary);
        // Chrome on Windows does not know `BlinkMacSystemFont`: alone it is the default serif.
        #[cfg(windows)]
        assert_eq!(blink.primary, FontFamilyChain::serif().primary);
    }

    #[cfg(windows)]
    #[test]
    fn macos_only_aliases_fall_through_on_windows() {
        // Chrome for Testing 148 win64 (docs/diagnostics/2026-11-18/font_alias_win148.json).
        let c = FontFamilyChain::from_css_value("-apple-system, BlinkMacSystemFont, sans-serif");
        assert_eq!(c.primary, "Arial");
        let c = FontFamilyChain::from_css_value("BlinkMacSystemFont, sans-serif");
        assert_eq!(c.primary, "Arial");
        // A real family after the aliases still wins.
        let c = FontFamilyChain::from_css_value("-apple-system, 'Segoe UI', sans-serif");
        assert_eq!(c.primary, "Segoe UI");
        // system-ui is not one of them: it stays Segoe UI.
        let c = FontFamilyChain::from_css_value("system-ui, sans-serif");
        assert_eq!(c.primary, "Segoe UI");
    }
''', state)
    text = edit(
        text, "text.rs alias tests: -apple-system, BlinkMacSystemFont",
        "on Windows both are unknown: the default serif",
        '''        let then_blink = FontFamilyChain::from_css_value("-apple-system, BlinkMacSystemFont");
        assert_eq!(then_blink.primary, system.primary, "the next family, the system font");
''',
        '''        let then_blink = FontFamilyChain::from_css_value("-apple-system, BlinkMacSystemFont");
        #[cfg(not(windows))]
        assert_eq!(then_blink.primary, system.primary, "the next family, the system font");
        #[cfg(windows)]
        assert_eq!(then_blink.primary, serif.primary, "on Windows both are unknown: the default serif");
''', state)
    # 4. CB7b's stack test: on Windows Segoe UI is both a family the stack names and the system
    #    font, so `no system font ahead of Helvetica` cannot hold there; Chrome for Testing 148 win64
    #    resolves `-apple-system, "Segoe UI", Helvetica` to Segoe UI (70.906 px, as system-ui), which
    #    the test's own later assertion expects. On Windows the walk checks only that -apple-system
    #    was skipped.
    text = edit(
        text, "text.rs CB7b walk: Segoe UI is the system font on Windows",
        "(f == system.primary && !cfg!(windows))",
        '''            !walk.any(|f| f == system.primary || f.eq_ignore_ascii_case("-apple-system")),
''',
        '''            // Windows: Segoe UI is the system font and a family this stack names (Chrome for
            // Testing 148 win64 resolves the stack to it), so only -apple-system is checked there.
            !walk.any(|f| (f == system.primary && !cfg!(windows)) || f.eq_ignore_ascii_case("-apple-system")),
''', state)
    del quote
    return text


def patch_lib(text: str, state: list) -> str:
    # (retired in refresh #22) The two strut-descent test tolerances are upstream: hiwave-macos
    # develop b60da7f5 tests within a pixel on Windows (`let tolerance = if cfg!(windows) { 1.0 }
    # else { 0.5 }`), which is what these edits did.
    del state
    return text


def patch_matrix(text: str, state: list) -> str:
    # The nested layout matrix compares the engine with Chromium on macOS, and its expected-failure
    # list is macOS's. On Windows (other fonts) 21 of the listed cases match, so the check that a
    # listed case still fails does not apply there; a case that newly differs still fails the test.
    return edit(
        text, "nested_layout_matrix_tests.rs: the expected-failure list is macOS's",
        "the expected-failure list is macOS's",
        '''    assert!(
        listed_but_match.is_empty(),
''',
        '''    // Windows: the pins and the expected-failure list are macOS's (other fonts), and some listed
    // cases match here; the check above (no case newly differs) still holds.
    // (apply_windows_patches.py: the expected-failure list is macOS's)
    #[cfg(not(windows))]
    assert!(
        listed_but_match.is_empty(),
''', state)


def patch_flex_ar(text: str, state: list) -> str:
    # flex_aspect_ratio_tests' `inherit-em` row was pinned on Chrome for macOS: its text line (body 10px,
    # the default font) is 13 tall there. Chrome for Testing 148 on Windows (win64) lays it out 11 tall
    # (docs/diagnostics/2026-11-18/flex_inherit_em_cft148_win64.txt), as the engine does here. On
    # Windows the row is compared with the Windows expectation; every other row is unchanged.
    return edit(
        text, "flex_aspect_ratio_tests.rs: inherit-em's Windows expectation",
        "inherit-em's Windows expectation",
        '''    let wrong = mismatches(CHROME_EXTRA, "", extra_page);
''',
        '''    // Windows: inherit-em's Windows expectation, Chrome for Testing 148 win64 (the default font's
    // line is 11 tall there, 13 on macOS): docs/diagnostics/2026-11-18/flex_inherit_em_cft148_win64.txt.
    #[cfg(windows)]
    let table = CHROME_EXTRA.replace(
        "inherit-em c=0,0,200,0 it=0,0,200,0 im=- txt=0,0,800,13",
        "inherit-em c=0,0,200,0 it=0,0,200,0 im=- txt=0,0,800,11",
    );
    #[cfg(not(windows))]
    let table = CHROME_EXTRA.to_string();
    let wrong = mismatches(&table, "", extra_page);
''', state)


PATCH_DIR = REPO / "scripts" / "windows-patches"


def patch_files(check: bool, state: list) -> bool:
    """Apply scripts/windows-patches/*.patch with git. Returns True if any
    patch was (or, under --check, would be) applied."""
    import subprocess
    changed = False
    for patch in sorted(PATCH_DIR.glob("*.patch")):
        name = patch.name
        # Test-only fixes can edit existing tests without adding a function
        # marker. Recognize their exact reverse as an already-applied patch.
        reverse = subprocess.run(
            ["git", "-C", str(REPO), "apply", "-C1", "--ignore-whitespace",
             "--reverse", "--check", str(patch)], capture_output=True, text=True)
        if reverse.returncode == 0:
            state.append((name, "already applied (or landed upstream)"))
            continue
        # The marker is the first test the patch adds: present means applied
        # (by this script, or because upstream landed the change).
        text = patch.read_text(encoding="utf-8")
        marker = None
        for line in text.splitlines():
            if line.startswith("+") and "fn " in line and line.rstrip().endswith("() {"):
                marker = line[1:].strip()
                break
        tree_has_marker = False
        if marker:
            for f in {l[6:] for l in text.splitlines() if l.startswith("+++ b/")}:
                fp = REPO / f
                if fp.exists() and marker in fp.read_text(encoding="utf-8"):
                    tree_has_marker = True
                    break
        if tree_has_marker:
            state.append((name, "already applied (or landed upstream: delete the file if so)"))
            continue
        # Reduced context: sibling patches may touch the same test module.
        # --ignore-whitespace: this repo checks out CRLF (core.autocrlf) and
        # the patches come from an LF tree; git otherwise refuses the context.
        cmd = ["git", "-C", str(REPO), "apply", "-C1", "--ignore-whitespace", "--check", str(patch)]
        rc = subprocess.run(cmd, capture_output=True, text=True)
        if rc.returncode != 0:
            state.append((name, "MISSING: does not apply, upstream moved: " + rc.stderr.strip().splitlines()[-1][:80]))
            continue
        changed = True
        if check:
            state.append((name, "would apply"))
            continue
        subprocess.run(cmd[:-2] + [str(patch)], check=True)
        state.append((name, "applied"))
    return changed


def main() -> int:
    check = "--check" in sys.argv[1:]
    state: list = []
    changed = False
    for path, fn in ((TEXT, patch_text), (LIB, patch_lib), (MATRIX, patch_matrix), (FLEX_AR, patch_flex_ar)):
        text, nl = read(path)
        new = fn(text, state)
        if new != text:
            changed = True
            if not check:
                write(path, new, nl)
    if PATCH_DIR.is_dir():
        changed = patch_files(check, state) or changed
    bad = 0
    for name, what in state:
        print(f"  {what:40} {name}")
        bad += what.startswith("MISSING")
    if check and changed:
        print("patches are NOT all applied")
        return 1
    if bad:
        print(f"{bad} patch(es) could not be applied: upstream moved, look by hand")
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
