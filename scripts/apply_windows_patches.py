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
    # 1. sans_serif(): Arial first on Windows.
    text = edit(
        text, "text.rs sans_serif leads with Arial on Windows",
        'let (first, second) = ("Arial", "Segoe UI");',
        '''    /// Create default font chain for sans-serif.
    #[cfg(not(target_os = "macos"))]
    pub fn sans_serif() -> Self {
        Self::new("Segoe UI")
            .with_fallback("Arial")
            .with_fallback("Helvetica")
''',
        '''    /// Create default font chain for sans-serif.
    ///
    /// Windows: Chrome's default `sans-serif` there is Arial, not Segoe UI
    /// (Segoe UI is what `system-ui` resolves to). At 14px Arial's `normal`
    /// line is 16px and Segoe UI's is 19px, so leading with Segoe UI made
    /// every line of a `sans-serif` page 3px too tall and the error
    /// accumulated down the page.
    #[cfg(not(target_os = "macos"))]
    pub fn sans_serif() -> Self {
        #[cfg(windows)]
        let (first, second) = ("Arial", "Segoe UI");
        #[cfg(not(windows))]
        let (first, second) = ("Segoe UI", "Arial");
        Self::new(first)
            .with_fallback(second)
            .with_fallback("Helvetica")
''', state)
    # 2. from_css_value(): skip the macOS-only aliases on Windows.
    text = edit(
        text, "text.rs skips -apple-system / BlinkMacSystemFont on Windows",
        "l != \"-apple-system\" && l != \"blinkmacsystemfont\"",
        "            .collect();\n\n        if families.is_empty() {\n            return Self::sans_serif();\n        }\n",
        '''            .collect();

        // `-apple-system` and `BlinkMacSystemFont` are macOS-only aliases.
        // Chrome on Windows does not recognise them, so they are skipped like
        // any other family that is not installed and the stack falls through
        // (`-apple-system, BlinkMacSystemFont, sans-serif` becomes plain
        // `sans-serif`). Treating them as `system-ui` picked Segoe UI where
        // Chrome picked Arial.
        #[cfg(windows)]
        let families: Vec<&str> = families
            .into_iter()
            .filter(|f| {
                let l = f.to_lowercase();
                l != "-apple-system" && l != "blinkmacsystemfont"
            })
            .collect();

        if families.is_empty() {
            return Self::sans_serif();
        }
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
        text, "text.rs alias tests",
        "fn macos_only_aliases_fall_through_on_windows",
        '''        let apple = FontFamilyChain::from_css_value("-apple-system");
        #[cfg(target_os = "macos")]
        assert_eq!(apple.primary, ".AppleSystemUIFont");
        #[cfg(not(target_os = "macos"))]
        assert_eq!(apple.primary, "Segoe UI");
    }
''',
        '''        let apple = FontFamilyChain::from_css_value("-apple-system");
        #[cfg(target_os = "macos")]
        assert_eq!(apple.primary, ".AppleSystemUIFont");
        // Windows: the alias is skipped, so it falls through to `sans-serif`.
        #[cfg(windows)]
        assert_eq!(apple.primary, "Arial");
        #[cfg(all(not(target_os = "macos"), not(windows)))]
        assert_eq!(apple.primary, "Segoe UI");
    }

    #[cfg(windows)]
    #[test]
    fn macos_only_aliases_fall_through_on_windows() {
        // The css-selectors stack: Chrome on Windows resolves it to Arial.
        let c = FontFamilyChain::from_css_value("-apple-system, BlinkMacSystemFont, sans-serif");
        assert_eq!(c.primary, "Arial");
        // A real family after the aliases still wins.
        let c = FontFamilyChain::from_css_value("-apple-system, 'Segoe UI', sans-serif");
        assert_eq!(c.primary, "Segoe UI");
        // system-ui is not an alias of the two above: it stays Segoe UI.
        let c = FontFamilyChain::from_css_value("system-ui, sans-serif");
        assert_eq!(c.primary, "Segoe UI");
    }
''', state)
    del quote
    return text


def patch_lib(text: str, state: list) -> str:
    text = edit(
        text, "lib.rs strut test 1 tolerance",
        "so the two agree to within a pixel, not half of one.",
        '''        let expected = 40.0 + parent.inline_strut_descent();
        assert!(
            (parent.dimensions.content.height - expected).abs() < 0.5,
''',
        '''        let expected = 40.0 + parent.inline_strut_descent();
        // `inline_strut_descent` is the font's raw fractional descent while
        // the line box rounds ascent and descent to whole pixels (as Chrome
        // does), so the two agree to within a pixel, not half of one. 0.5
        // held for SF and Segoe UI by luck of their descents; Arial's
        // 3.453 rounds up to 4.
        assert!(
            (parent.dimensions.content.height - expected).abs() < 1.0,
''', state)
    text = edit(
        text, "lib.rs strut test 2 tolerance",
        "Within a pixel: the raw fractional descent against a rounded line",
        '''        let expected_y = 124.0 + sd + 10.0 + 2.0;
        assert!(
            (second_row.content.y - expected_y).abs() < 0.5,
''',
        '''        let expected_y = 124.0 + sd + 10.0 + 2.0;
        // Within a pixel: the raw fractional descent against a rounded line
        // box (see test_inline_flex_children_share_a_line).
        assert!(
            (second_row.content.y - expected_y).abs() < 1.0,
''', state)
    return text


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
    for path, fn in ((TEXT, patch_text), (LIB, patch_lib)):
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
