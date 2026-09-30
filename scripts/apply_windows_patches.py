"""Apply the Windows-local engine patches to a freshly copied rustkit-layout.

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
3. rustkit-engine/src/lib.rs (TEMPORARY, cross-platform, reported upstream):
   four test modules wrap Engine::new in a module mutex, ENGINE_INIT. Engine::new
   itself takes the GPU test guard, which a thread then holds until it exits.
   A test that builds a second engine (the_layer_pins_selectors_match_the_box
   builds four) holds the guard and waits for the mutex, while the mutex
   holder waits 120 s for the guard and panics: 16 of 220 engine tests fail in
   a parallel run, none in a serial one. The mutex is redundant (the guard
   already serialises creation), so it is removed. Drop this patch when
   upstream removes the mutex; the script then reports it MISSING.
"""
from __future__ import annotations

import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
TEXT = REPO / "crates" / "rustkit-layout" / "src" / "text.rs"
LIB = REPO / "crates" / "rustkit-layout" / "src" / "lib.rs"
ENGINE = REPO / "crates" / "rustkit-engine" / "src" / "lib.rs"
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
        assert_eq!(sans.primary, "SF Pro");
        #[cfg(not(target_os = "macos"))]
        assert_eq!(sans.primary, "Segoe UI");
''',
        '''        #[cfg(target_os = "macos")]
        assert_eq!(sans.primary, "SF Pro");
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


def patch_engine(text: str, state: list) -> str:
    text = edit(
        text, "engine lib.rs test engine() helpers drop the ENGINE_INIT mutex (temporary)",
        "No module mutex here: `Engine::new` takes the GPU test guard",
        '''    fn engine() -> Engine {
        static ENGINE_INIT: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _init_guard = ENGINE_INIT.lock().unwrap_or_else(|e| e.into_inner());
        Engine::new(EngineConfig::default()).expect("engine")
    }
''',
        '''    fn engine() -> Engine {
        // No module mutex here: `Engine::new` takes the GPU test guard, which
        // this thread then holds until it exits. A mutex taken before it
        // inverts the order for a test that builds a second engine: this
        // thread holds the guard and waits for the mutex, the mutex holder
        // waits 120 s for the guard, then panics.
        Engine::new(EngineConfig::default()).expect("engine")
    }
''', state, count=4)
    return text


def main() -> int:
    check = "--check" in sys.argv[1:]
    state: list = []
    changed = False
    for path, fn in ((TEXT, patch_text), (LIB, patch_lib), (ENGINE, patch_engine)):
        text, nl = read(path)
        new = fn(text, state)
        if new != text:
            changed = True
            if not check:
                write(path, new, nl)
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
