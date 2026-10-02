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
//! Formats: TrueType/OpenType sfnt (`.ttf`/`.otf`/`.ttc`), WOFF and WOFF2.
//! On macOS `CGFontCreateWithDataProvider` decodes all of them, so this
//! workspace carries no decompressor of its own. What it does carry is
//! [`inspect`]: every face's container header and table directory are checked
//! against the bytes actually received, and the decoded size a WOFF/WOFF2
//! header declares is capped, BEFORE the bytes reach the platform decoder.
//! A face that fails is dropped and counted as rejected.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Largest font file accepted, as received.
pub const MAX_FONT_BYTES: usize = 32 * 1024 * 1024;

/// Largest decoded (sfnt) size a WOFF/WOFF2 directory may add up to or its
/// header may declare. The platform decoder allocates from these numbers.
pub const MAX_DECODED_FONT_BYTES: u64 = 128 * 1024 * 1024;

/// The container a font file is in, told from its bytes, never from a URL
/// extension, a `format()` hint or a Content-Type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FontContainer {
    /// A single TrueType/OpenType font.
    Sfnt,
    /// A TrueType collection (`ttcf`).
    Collection,
    Woff,
    Woff2,
}

/// Why [`inspect`] refused a font file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FontRejection {
    /// Shorter than its container's fixed header.
    TooShort,
    /// More than [`MAX_FONT_BYTES`] as received.
    TooLarge,
    /// The first four bytes are no font container this crate knows.
    UnknownContainer,
    /// The header or table directory contradicts the bytes received.
    Malformed(&'static str),
    /// Would decode to more than [`MAX_DECODED_FONT_BYTES`].
    DecodedTooLarge,
}

fn be16(data: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(data.get(at..at + 2)?.try_into().ok()?))
}

fn be32(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(data.get(at..at + 4)?.try_into().ok()?))
}

/// WOFF2 `UIntBase128`: at most five bytes, no leading zero group, no
/// overflow past 32 bits.
fn base128(data: &[u8], pos: &mut usize) -> Option<u32> {
    let mut value: u32 = 0;
    for i in 0..5 {
        let byte = *data.get(*pos)?;
        *pos += 1;
        if i == 0 && byte == 0x80 {
            return None;
        }
        if value & 0xFE00_0000 != 0 {
            return None;
        }
        value = (value << 7) | u32::from(byte & 0x7F);
        if byte & 0x80 == 0 {
            return Some(value);
        }
    }
    None
}

fn padded(len: u32) -> u64 {
    (u64::from(len) + 3) & !3
}

fn inspect_sfnt(data: &[u8]) -> Result<FontContainer, FontRejection> {
    const BAD: FontRejection = FontRejection::Malformed("sfnt table directory");
    let tables = usize::from(be16(data, 4).ok_or(FontRejection::TooShort)?);
    if tables == 0 || data.len() < 12 + 16 * tables {
        return Err(BAD);
    }
    for i in 0..tables {
        let record = 12 + 16 * i;
        let offset = u64::from(be32(data, record + 8).ok_or(BAD)?);
        let length = u64::from(be32(data, record + 12).ok_or(BAD)?);
        if offset + length > data.len() as u64 {
            return Err(FontRejection::Malformed(
                "sfnt table past the end of the file",
            ));
        }
    }
    Ok(FontContainer::Sfnt)
}

fn inspect_woff(data: &[u8]) -> Result<FontContainer, FontRejection> {
    const HEADER: usize = 44;
    const BAD: FontRejection = FontRejection::Malformed("woff table directory");
    if data.len() < HEADER {
        return Err(FontRejection::TooShort);
    }
    if be32(data, 8) != Some(data.len() as u32) {
        return Err(FontRejection::Malformed(
            "woff length field is not the file size",
        ));
    }
    let tables = usize::from(be16(data, 12).ok_or(BAD)?);
    let directory_end = HEADER + 20 * tables;
    if tables == 0 || data.len() < directory_end {
        return Err(BAD);
    }
    let mut decoded = 12 + 16 * tables as u64;
    for i in 0..tables {
        let entry = HEADER + 20 * i;
        let offset = u64::from(be32(data, entry + 4).ok_or(BAD)?);
        let compressed = be32(data, entry + 8).ok_or(BAD)?;
        let original = be32(data, entry + 12).ok_or(BAD)?;
        if offset < directory_end as u64 || offset + u64::from(compressed) > data.len() as u64 {
            return Err(FontRejection::Malformed("woff table outside the file"));
        }
        if compressed > original {
            return Err(FontRejection::Malformed(
                "woff table larger compressed than decoded",
            ));
        }
        decoded += padded(original);
    }
    let declared = u64::from(be32(data, 16).ok_or(BAD)?);
    if decoded.max(declared) > MAX_DECODED_FONT_BYTES {
        return Err(FontRejection::DecodedTooLarge);
    }
    Ok(FontContainer::Woff)
}

fn inspect_woff2(data: &[u8]) -> Result<FontContainer, FontRejection> {
    const HEADER: usize = 48;
    const BAD: FontRejection = FontRejection::Malformed("woff2 table directory");
    if data.len() < HEADER {
        return Err(FontRejection::TooShort);
    }
    if be32(data, 8) != Some(data.len() as u32) {
        return Err(FontRejection::Malformed(
            "woff2 length field is not the file size",
        ));
    }
    let tables = usize::from(be16(data, 12).ok_or(BAD)?);
    if tables == 0 {
        return Err(BAD);
    }
    let compressed = u64::from(be32(data, 20).ok_or(BAD)?);
    let mut decoded = 12 + 16 * tables as u64;
    let mut pos = HEADER;
    for _ in 0..tables {
        let flags = *data.get(pos).ok_or(BAD)?;
        pos += 1;
        let tag = flags & 0x3F;
        if tag == 0x3F {
            // An arbitrary four-byte tag follows.
            pos += 4;
        }
        let original = base128(data, &mut pos).ok_or(BAD)?;
        // `glyf` (10) and `loca` (11) are transformed unless version 3;
        // every other table is transformed only when the version is not 0.
        let version = flags >> 6;
        let transformed = if tag == 10 || tag == 11 {
            version != 3
        } else {
            version != 0
        };
        if transformed {
            base128(data, &mut pos).ok_or(BAD)?;
        }
        decoded += padded(original);
    }
    if pos as u64 + compressed > data.len() as u64 {
        return Err(FontRejection::Malformed(
            "woff2 compressed stream outside the file",
        ));
    }
    let declared = u64::from(be32(data, 16).ok_or(BAD)?);
    if decoded.max(declared) > MAX_DECODED_FONT_BYTES {
        return Err(FontRejection::DecodedTooLarge);
    }
    Ok(FontContainer::Woff2)
}

/// Identify a font file's container and check its header against the bytes
/// received. This reads headers and table directories only: it decompresses
/// nothing and does not validate table contents.
pub fn inspect(data: &[u8]) -> Result<FontContainer, FontRejection> {
    // The shortest header any container has is the sfnt's 12 bytes.
    if data.len() < 12 {
        return Err(FontRejection::TooShort);
    }
    if data.len() > MAX_FONT_BYTES {
        return Err(FontRejection::TooLarge);
    }
    match &data[..4] {
        b"wOFF" => inspect_woff(data),
        b"wOF2" => inspect_woff2(data),
        b"ttcf" => Ok(FontContainer::Collection),
        [0, 1, 0, 0] | b"OTTO" | b"true" | b"typ1" => inspect_sfnt(data),
        _ => Err(FontRejection::UnknownContainer),
    }
}

/// Identity of a face's bytes: equal for the same file however often it is
/// installed, different for different files. Never 0 (0 means "no web face").
#[cfg(target_os = "macos")]
fn content_id(data: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    data.hash(&mut hasher);
    hasher.finish().max(1)
}

static GENERATION: AtomicU64 = AtomicU64::new(0);

/// Changes whenever the installed face set does (an install of a different
/// set, or a clear). Anything that caches a family-name resolution keys it
/// on this, because the same name can resolve to a different face after.
pub fn generation() -> u64 {
    GENERATION.load(Ordering::Acquire)
}

pub(crate) fn bump_generation() {
    GENERATION.fetch_add(1, Ordering::AcqRel);
}

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
        /// `content_id` of the bytes `cgfont` was built from.
        id: u64,
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
    /// was there. Returns how many faces were accepted; a face that fails
    /// `inspect` or that Core Graphics cannot decode is dropped and counted
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
            if super::inspect(&face.data).is_err() {
                continue;
            }
            let provider = CGDataProvider::from_buffer(face.data.clone());
            let Ok(cgfont) = CGFont::from_data_provider(provider) else {
                continue;
            };
            accepted += 1;
            families.entry(key(&face.family)).or_default().push(Face {
                weight: face.weight,
                italic: face.italic,
                id: super::content_id(&face.data),
                cgfont,
            });
        }
        let mut active = slot().write().unwrap();
        active.tag = tag.to_string();
        active.families = families;
        super::bump_generation();
        accepted
    }

    /// Drop the active set. After this no family resolves through the registry.
    pub fn clear() {
        let mut active = slot().write().unwrap();
        if !active.tag.is_empty() || !active.families.is_empty() {
            super::bump_generation();
        }
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

    /// Identity of the registered face a CSS family LIST resolves to for this
    /// style: the first family in the list the document registered, or 0
    /// when it registered none of them. Anything that caches per-family
    /// output (glyph bitmaps) keys on this, because a family name alone says
    /// neither whether the face has arrived yet nor which document's file it
    /// is.
    pub fn face_id(family_list: &str, weight: u16, italic: bool) -> u64 {
        let active = slot().read().unwrap();
        if active.families.is_empty() {
            return 0;
        }
        for family in family_list.split(',') {
            let family = family.trim().trim_matches('"').trim_matches('\'');
            if let Some(faces) = active.families.get(&key(family)) {
                return select(faces, weight, italic).map_or(0, |f| f.id);
            }
        }
        0
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

    pub fn face_id(_family_list: &str, _weight: u16, _italic: bool) -> u64 {
        0
    }

    pub fn lookup_descriptor(_family: &str, _weight: u16, _italic: bool) -> Option<(u16, bool)> {
        None
    }
}

pub use imp::{clear, face_id, install, is_installed, lookup_descriptor};

#[cfg(target_os = "macos")]
pub use imp::lookup;

#[cfg(test)]
mod container_tests {
    use super::*;

    const AHEM: &[u8] = include_bytes!("../tests/fixtures/Ahem.ttf");
    const AHEM_WOFF: &[u8] = include_bytes!("../tests/fixtures/Ahem.woff");
    const AHEM_WOFF2: &[u8] = include_bytes!("../tests/fixtures/Ahem.woff2");

    fn patched(data: &[u8], at: usize, value: u32) -> Vec<u8> {
        let mut out = data.to_vec();
        out[at..at + 4].copy_from_slice(&value.to_be_bytes());
        out
    }

    #[test]
    fn the_container_is_told_from_the_bytes() {
        assert_eq!(inspect(AHEM), Ok(FontContainer::Sfnt));
        assert_eq!(inspect(AHEM_WOFF), Ok(FontContainer::Woff));
        assert_eq!(inspect(AHEM_WOFF2), Ok(FontContainer::Woff2));
        assert_eq!(inspect(&[0u8; 64]), Err(FontRejection::UnknownContainer));
        assert_eq!(
            inspect(b"<!DOCTYPE html><html>"),
            Err(FontRejection::UnknownContainer)
        );
        assert_eq!(inspect(&AHEM[..11]), Err(FontRejection::TooShort));
    }

    #[test]
    fn a_truncated_file_is_rejected_in_every_container() {
        // The last table of each file now ends past the bytes received.
        for file in [AHEM, AHEM_WOFF, AHEM_WOFF2] {
            let cut = &file[..file.len() - 40];
            assert!(
                matches!(inspect(cut), Err(FontRejection::Malformed(_))),
                "{:?} for a file cut by 40 bytes",
                inspect(cut)
            );
        }
        assert_eq!(inspect(&AHEM_WOFF[..20]), Err(FontRejection::TooShort));
        assert_eq!(inspect(&AHEM_WOFF2[..20]), Err(FontRejection::TooShort));
    }

    #[test]
    fn a_header_that_declares_a_huge_decoded_size_is_rejected() {
        // totalSfntSize sits at byte 16 of both headers.
        for file in [AHEM_WOFF, AHEM_WOFF2] {
            assert_eq!(
                inspect(&patched(file, 16, u32::MAX)),
                Err(FontRejection::DecodedTooLarge)
            );
        }
    }

    #[test]
    fn a_woff_table_that_expands_past_the_cap_is_rejected() {
        // First directory entry's origLength (header 44 + tag, offset,
        // compLength = byte 56). The header's own total is left honest.
        assert_eq!(
            inspect(&patched(AHEM_WOFF, 56, u32::MAX)),
            Err(FontRejection::DecodedTooLarge)
        );
    }

    #[test]
    fn a_woff_table_that_points_outside_the_file_is_rejected() {
        // First directory entry's offset (byte 48).
        assert_eq!(
            inspect(&patched(AHEM_WOFF, 48, AHEM_WOFF.len() as u32)),
            Err(FontRejection::Malformed("woff table outside the file"))
        );
        // Offset 0 would alias the header.
        assert_eq!(
            inspect(&patched(AHEM_WOFF, 48, 0)),
            Err(FontRejection::Malformed("woff table outside the file"))
        );
    }

    #[test]
    fn a_woff2_stream_longer_than_the_file_is_rejected() {
        // totalCompressedSize sits at byte 20.
        assert_eq!(
            inspect(&patched(AHEM_WOFF2, 20, AHEM_WOFF2.len() as u32)),
            Err(FontRejection::Malformed(
                "woff2 compressed stream outside the file"
            ))
        );
    }

    #[test]
    fn an_sfnt_table_past_the_end_is_rejected() {
        // First table record's length (12 + tag, checksum, offset = byte 24).
        assert_eq!(
            inspect(&patched(AHEM, 24, u32::MAX)),
            Err(FontRejection::Malformed(
                "sfnt table past the end of the file"
            ))
        );
    }

    #[test]
    fn an_oversized_file_is_rejected_before_its_header_is_read() {
        let mut big = AHEM.to_vec();
        big.resize(MAX_FONT_BYTES + 1, 0);
        assert_eq!(inspect(&big), Err(FontRejection::TooLarge));
    }

    #[test]
    fn base128_refuses_leading_zeros_overflow_and_overlong_numbers() {
        let read = |bytes: &[u8]| base128(bytes, &mut 0);
        assert_eq!(read(&[0x3F]), Some(63));
        assert_eq!(read(&[0x81, 0x00]), Some(128));
        assert_eq!(read(&[0x8F, 0xFF, 0xFF, 0xFF, 0x7F]), Some(u32::MAX));
        assert_eq!(read(&[0x80, 0x01]), None, "leading zero group");
        assert_eq!(read(&[0x9F, 0xFF, 0xFF, 0xFF, 0x7F]), None, "past 32 bits");
        assert_eq!(
            read(&[0x81, 0x81, 0x81, 0x81, 0x81, 0x01]),
            None,
            "six bytes"
        );
        assert_eq!(read(&[0x81]), None, "ends mid-number");
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use core_text::font as ct_font;

    const AHEM: &[u8] = include_bytes!("../tests/fixtures/Ahem.ttf");
    const AHEM_WOFF: &[u8] = include_bytes!("../tests/fixtures/Ahem.woff");
    const AHEM_WOFF2: &[u8] = include_bytes!("../tests/fixtures/Ahem.woff2");

    fn ahem_face(family: &str, weight: u16, italic: bool) -> WebFontFace {
        face_from(family, weight, italic, AHEM)
    }

    fn face_from(family: &str, weight: u16, italic: bool, data: &[u8]) -> WebFontFace {
        WebFontFace {
            family: family.to_string(),
            weight,
            italic,
            data: Arc::new(data.to_vec()),
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
    fn woff_and_woff2_faces_install_and_draw_their_own_glyphs() {
        // Ahem's "X" is an exact em square and no fallback font's is, so a
        // 20x20 block of full ink says the glyph came out of THESE bytes:
        // the compressed tables were decoded, not just the header accepted.
        let _slot = slot_guard();
        for (format, bytes) in [("ttf", AHEM), ("woff", AHEM_WOFF), ("woff2", AHEM_WOFF2)] {
            let family = format!("WebfontsFormat{format}");
            let tag = format!("t7-{format}");
            assert_eq!(
                install(&tag, &[face_from(&family, 400, false, bytes)]),
                1,
                "{format} is accepted"
            );
            let cg = lookup(&family, 400, false).expect("registered family resolves");
            assert_eq!(
                ct_font::new_from_CGFont(&cg, 20.0).family_name(),
                "Ahem",
                "{format}"
            );
            let r = crate::macos::GlyphRasterizer::new(&family, 20.0).expect("rasterizer");
            let (bitmap, w, h, advance, _, _) = r.rasterize_char('X', 0.0).expect("glyph");
            assert_eq!(advance, 20.0, "{format}: Ahem advance is exactly 1em");
            let full = bitmap.iter().filter(|&&v| v == 255).count();
            let partial = bitmap.iter().filter(|&&v| v != 0 && v != 255).count();
            assert_eq!(
                (full, partial),
                (400, 0),
                "{format}: ink in a {w}x{h} bitmap"
            );
        }
    }

    #[test]
    fn a_malformed_compressed_face_never_reaches_the_platform_decoder() {
        let _slot = slot_guard();
        let cut = &AHEM_WOFF2[..AHEM_WOFF2.len() - 40];
        let mut huge = AHEM_WOFF.to_vec();
        huge[16..20].copy_from_slice(&u32::MAX.to_be_bytes());
        let faces = [
            face_from("WebfontsTestCut", 400, false, cut),
            face_from("WebfontsTestHuge", 400, false, &huge),
        ];
        assert_eq!(install("t8", &faces), 0);
        assert!(!is_installed("WebfontsTestCut"));
        assert!(!is_installed("WebfontsTestHuge"));
    }

    #[test]
    fn a_face_id_names_the_file_not_the_family() {
        let _slot = slot_guard();
        clear();
        assert_eq!(
            face_id("WebfontsTestId", 400, false),
            0,
            "nothing registered"
        );

        install("t9-a", &[face_from("WebfontsTestId", 400, false, AHEM)]);
        let ttf = face_id("WebfontsTestId", 400, false);
        assert_ne!(ttf, 0, "a registered family has an identity");
        assert_eq!(
            face_id("Nowhere Sans, 'webfontstestid', serif", 400, false),
            ttf,
            "found through a family list, quoted, in any case"
        );
        assert_eq!(
            face_id("Helvetica, serif", 400, false),
            0,
            "platform families have none"
        );

        // Another document declares the SAME family name with another file.
        install(
            "t9-b",
            &[face_from("WebfontsTestId", 400, false, AHEM_WOFF)],
        );
        let woff = face_id("WebfontsTestId", 400, false);
        assert_ne!(woff, 0);
        assert_ne!(woff, ttf, "same name, different file: a different face");

        // Back to the first document: the same file is the same face again.
        install("t9-c", &[face_from("WebfontsTestId", 400, false, AHEM)]);
        assert_eq!(face_id("WebfontsTestId", 400, false), ttf);

        clear();
        assert_eq!(face_id("WebfontsTestId", 400, false), 0);
    }

    #[test]
    fn a_face_shorter_than_a_table_directory_is_rejected() {
        let _slot = slot_guard();
        let short = |n: usize| WebFontFace {
            family: "WebfontsTestShort".to_string(),
            weight: 400,
            italic: false,
            data: Arc::new(vec![0u8; n]),
        };
        assert_eq!(install("t6", &[short(0), short(11)]), 0);
        assert!(!is_installed("WebfontsTestShort"));
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
