//! Web fonts (`@font-face`): the document-scoped face registry.
//!
//! Every font-creation path in this crate and in `rustkit-layout` resolves a
//! family NAME through the platform (Core Text `new_from_name`). A web font is
//! a family name that exists only because the document declared it, so those
//! paths need one extra lookup ahead of the platform: "did the current
//! document register this family?" That lookup lives here.
//!
//! SCOPE, STATED: the registry holds the faces of ONE document at a time —
//! the engine installs a view's partition slice of its (partitioned)
//! `FontLoader` immediately before laying out or painting that view. It is a
//! process-wide slot, not a process-wide cache: two documents never see each
//! other's faces because the engine swaps the slot per view, and the loader
//! it swaps from is keyed by top-level site. Handing every `create_font`
//! call a partition parameter would have been the pure design; it also
//! touches a dozen call sites across four crates for the same guarantee,
//! which this gives by construction.
//!
//! Formats: whatever `CGFontCreateWithDataProvider` accepts — TrueType and
//! OpenType (`.ttf`/`.otf`) sfnt containers. WOFF/WOFF2 are compressed
//! wrappers around the same tables and need a decoder this workspace does not
//! carry yet; they install as nothing and are reported as rejected.

use std::sync::Arc;

/// One face as the engine hands it over: raw bytes plus the descriptors the
/// `@font-face` rule declared for them.
#[derive(Debug, Clone)]
pub struct WebFontFace {
    pub family: String,
    /// CSS weight (100..900).
    pub weight: u16,
    pub italic: bool,
    pub data: Arc<Vec<u8>>,
}

#[cfg(target_os = "macos")]
mod imp {
    use super::WebFontFace;
    use core_graphics::data_provider::CGDataProvider;
    use core_graphics::font::CGFont;
    use std::collections::HashMap;
    use std::sync::{OnceLock, RwLock};

    struct Face {
        weight: u16,
        italic: bool,
        cgfont: CGFont,
    }

    struct Active {
        /// Identifies WHICH face set is installed so a re-install of the
        /// same set is a no-op rather than a re-parse of every font file.
        tag: String,
        families: HashMap<String, Vec<Face>>,
    }

    fn slot() -> &'static RwLock<Active> {
        static SLOT: OnceLock<RwLock<Active>> = OnceLock::new();
        SLOT.get_or_init(|| {
            RwLock::new(Active {
                tag: String::new(),
                families: HashMap::new(),
            })
        })
    }

    fn key(family: &str) -> String {
        family.trim().to_ascii_lowercase()
    }

    /// Install `faces` as the active document font set, replacing whatever
    /// was there. Returns how many faces Core Graphics accepted; a face it
    /// rejected (bad data, unsupported container) is dropped and counted
    /// against that number, never silently kept as a name with no glyphs.
    pub fn install(tag: &str, faces: &[WebFontFace]) -> usize {
        {
            let active = slot().read().unwrap();
            if !tag.is_empty() && active.tag == tag {
                return active.families.values().map(Vec::len).sum();
            }
        }
        let mut families: HashMap<String, Vec<Face>> = HashMap::new();
        let mut accepted = 0usize;
        for face in faces {
            let provider = CGDataProvider::from_buffer(face.data.clone());
            let Ok(cgfont) = CGFont::from_data_provider(provider) else {
                continue;
            };
            accepted += 1;
            families.entry(key(&face.family)).or_default().push(Face {
                weight: face.weight,
                italic: face.italic,
                cgfont,
            });
        }
        let mut active = slot().write().unwrap();
        active.tag = tag.to_string();
        active.families = families;
        accepted
    }

    /// Drop the active set. After this no family resolves through the registry.
    pub fn clear() {
        let mut active = slot().write().unwrap();
        active.tag.clear();
        active.families.clear();
    }

    /// Does the active document declare `family`? Case-insensitive, as CSS
    /// family matching is.
    pub fn is_installed(family: &str) -> bool {
        slot().read().unwrap().families.contains_key(&key(family))
    }

    /// CSS Fonts 4 §5.2 reduced to the two axes the loader records: an exact
    /// italic match beats a mismatched one, then the nearest weight.
    fn select<'a>(faces: &'a [Face], weight: u16, italic: bool) -> Option<&'a Face> {
        faces.iter().min_by_key(|f| {
            let style_penalty: u32 = if f.italic == italic { 0 } else { 10_000 };
            style_penalty + (f.weight as i32 - weight as i32).unsigned_abs()
        })
    }

    /// The registered face of `family` closest to the requested style.
    pub fn lookup(family: &str, weight: u16, italic: bool) -> Option<CGFont> {
        let active = slot().read().unwrap();
        let faces = active.families.get(&key(family))?;
        select(faces, weight, italic).map(|f| f.cgfont.clone())
    }

    /// The `(weight, italic)` descriptors of the face `lookup` would return.
    /// Same selection code, exposed so the rule is testable and debuggable
    /// without comparing font objects.
    pub fn lookup_descriptor(family: &str, weight: u16, italic: bool) -> Option<(u16, bool)> {
        let active = slot().read().unwrap();
        let faces = active.families.get(&key(family))?;
        select(faces, weight, italic).map(|f| (f.weight, f.italic))
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use super::WebFontFace;

    /// No platform registry here yet: web fonts install as nothing, and the
    /// count says so instead of pretending.
    pub fn install(_tag: &str, _faces: &[WebFontFace]) -> usize {
        0
    }

    pub fn clear() {}

    pub fn is_installed(_family: &str) -> bool {
        false
    }

    pub fn lookup_descriptor(_family: &str, _weight: u16, _italic: bool) -> Option<(u16, bool)> {
        None
    }
}

pub use imp::{clear, install, is_installed, lookup_descriptor};

#[cfg(target_os = "macos")]
pub use imp::lookup;

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use core_text::font as ct_font;

    const AHEM: &[u8] = include_bytes!("../tests/fixtures/Ahem.ttf");

    fn ahem_face(family: &str, weight: u16, italic: bool) -> WebFontFace {
        WebFontFace {
            family: family.to_string(),
            weight,
            italic,
            data: Arc::new(AHEM.to_vec()),
        }
    }

    // The slot is process-wide and cargo runs tests on parallel threads.
    // "Re-install before every positive check" was not enough: another
    // test's install() can land BETWEEN this test's install() and its
    // lookup(), replacing the set, and the positive assertion fails — seen
    // as `an_installed_face_resolves_by_family_case_insensitively` going red
    // about one full-suite run in eight on #163/#164 while passing alone and
    // under --test-threads=1. Every test that touches the slot holds this
    // guard for its whole body, so the slot is single-writer per test.
    use std::sync::{Mutex, MutexGuard};

    fn slot_guard() -> MutexGuard<'static, ()> {
        static LOCK: Mutex<()> = Mutex::new(());
        // A panicking test must not poison the rest of the suite.
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn an_installed_face_resolves_by_family_case_insensitively() {
        let _slot = slot_guard();
        let faces = [ahem_face("WebfontsTestAhem", 400, false)];
        let n = install("t1", &faces);
        assert_eq!(n, 1, "Core Graphics accepts the TrueType Ahem");
        let ct = {
            install("t1", &faces);
            let cg = lookup("WEBFONTSTESTAHEM", 400, false).expect("registered family resolves");
            ct_font::new_from_CGFont(&cg, 25.0)
        };
        // Not a fallback: the CTFont built from those bytes reports Ahem's
        // own family name.
        assert_eq!(ct.family_name(), "Ahem");
    }

    #[test]
    fn a_family_nobody_declared_does_not_resolve() {
        let _slot = slot_guard();
        install("t2", &[ahem_face("WebfontsTestOther", 400, false)]);
        assert!(lookup("WebfontsTestNeverDeclared", 400, false).is_none());
        assert!(!is_installed("Helvetica"), "system fonts are not web fonts");
    }

    #[test]
    fn garbage_bytes_are_rejected_not_registered() {
        let _slot = slot_guard();
        let junk = WebFontFace {
            family: "WebfontsTestJunk".to_string(),
            weight: 400,
            italic: false,
            data: Arc::new(vec![0u8; 64]),
        };
        let n = install("t3", &[junk]);
        assert_eq!(n, 0);
        assert!(
            !is_installed("WebfontsTestJunk"),
            "a name with no glyphs behind it must not shadow the platform lookup"
        );
    }

    #[test]
    fn an_ahem_square_rasterizes_with_no_partial_coverage_fringe() {
        // Ahem's glyphs are exact em squares. Rasterized at an integer size
        // on an integer origin, every bitmap pixel must be fully inside the
        // square (255) or fully outside (0); any intermediate value is the
        // rasterizer adding ink the outline does not have. n33 measured that
        // ink on the WPT board: a ~30% fringe one column either side of every
        // Ahem square and ~60% on the row above, which is exactly the
        // difference between a reftest PASS and FAIL for every overlap case.
        let _slot = slot_guard();
        let faces = [ahem_face("WebfontsRasterProbe", 400, false)];
        install("t5", &faces);
        let r = crate::macos::GlyphRasterizer::new("WebfontsRasterProbe", 20.0)
            .expect("registered family rasterizes");
        let (bitmap, w, h, advance, bx, by) = r.rasterize_char('X', 0.0).expect("glyph");
        assert_eq!(advance, 20.0, "Ahem advance is exactly 1em");
        let mut partial = Vec::new();
        let mut ink_cols = std::collections::BTreeSet::new();
        let mut ink_rows = std::collections::BTreeSet::new();
        for row in 0..h as usize {
            for col in 0..w as usize {
                let v = bitmap[row * w as usize + col];
                if v != 0 && v != 255 {
                    partial.push((col, row, v));
                }
                if v != 0 {
                    ink_cols.insert(col);
                    ink_rows.insert(row);
                }
            }
        }
        assert_eq!(
            ink_cols.len(),
            20,
            "ink spans {} columns, expected exactly 20 (bitmap {w}x{h}, bearing {bx},{by}); \
             partial pixels: {:?}",
            ink_cols.len(),
            &partial[..partial.len().min(12)]
        );
        assert_eq!(ink_rows.len(), 20, "ink spans {} rows, expected exactly 20", ink_rows.len());
        assert!(
            partial.is_empty(),
            "{} partially-covered pixels in an integer-aligned em square, e.g. {:?} — \
             the rasterizer is dilating the outline",
            partial.len(),
            &partial[..partial.len().min(12)]
        );
    }

    #[test]
    fn the_nearest_style_wins_and_italic_outranks_weight() {
        let _slot = slot_guard();
        let faces = [
            ahem_face("WebfontsTestStyled", 400, false),
            ahem_face("WebfontsTestStyled", 700, true),
        ];
        let pick = |w, i| {
            install("t4", &faces);
            lookup_descriptor("WebfontsTestStyled", w, i).expect("family installed")
        };
        assert_eq!(pick(400, false), (400, false));
        assert_eq!(pick(900, false), (400, false), "upright 900 prefers the upright face");
        assert_eq!(pick(400, true), (700, true), "italic 400 prefers the italic face");
    }
}
