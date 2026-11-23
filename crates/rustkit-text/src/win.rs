use crate::{FontMetrics, FontStretch, FontStyle, FontWeight, GlyphMetrics, TextBackendError};
use std::sync::OnceLock;
use windows::core::{PCWSTR, BOOL};
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};

#[derive(Clone)]
pub struct FontCollection {
    collection: IDWriteFontCollection,
}

pub struct FontFamily {
    family: IDWriteFontFamily,
}

pub struct Font {
    font: IDWriteFont,
}

#[derive(Clone)]
pub struct FontFace {
    face: IDWriteFontFace,
}

struct DWriteContext {
    factory: IDWriteFactory,
}

fn ctx() -> Result<&'static DWriteContext, TextBackendError> {
    static CTX: OnceLock<Result<DWriteContext, TextBackendError>> = OnceLock::new();
    let res = CTX.get_or_init(|| init_ctx());
    res.as_ref().map_err(Clone::clone)
}

fn init_ctx() -> Result<DWriteContext, TextBackendError> {
    // Ensure COM is initialized for this process; ignore mode mismatches.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }

    // Create DirectWrite factory (windows crate provides a generic helper)
    let factory: IDWriteFactory = unsafe { DWriteCreateFactory::<IDWriteFactory>(DWRITE_FACTORY_TYPE_SHARED) }
        .map_err(|e| TextBackendError::DirectWrite(format!("{e:?}")))?;

    Ok(DWriteContext { factory })
}

impl FontCollection {
    pub fn system() -> Result<Self, TextBackendError> {
        let ctx = ctx()?;
        let mut collection: Option<IDWriteFontCollection> = None;
        unsafe {
            ctx.factory
                .GetSystemFontCollection(&mut collection, false)
                .map_err(|e| TextBackendError::DirectWrite(format!("{e:?}")))?;
        }
        Ok(Self {
            collection: collection.ok_or_else(|| {
                TextBackendError::DirectWrite("GetSystemFontCollection returned None".into())
            })?,
        })
    }

    /// The family named `name`, or the family Windows substitutes for it (`family_substitute`:
    /// "Times" is Times New Roman, "Helvetica" Arial), as Chrome on Windows resolves it.
    pub fn font_family_by_name(&self, name: &str) -> Result<Option<FontFamily>, TextBackendError> {
        if let Some(family) = self.font_family_by_exact_name(name)? {
            return Ok(Some(family));
        }
        match family_substitute(name) {
            Some(substitute) if !substitute.eq_ignore_ascii_case(name) => self.font_family_by_exact_name(&substitute),
            _ => Ok(None),
        }
    }

    fn font_family_by_exact_name(&self, name: &str) -> Result<Option<FontFamily>, TextBackendError> {
        let name_w = to_wide_null(name);
        let mut index: u32 = 0;
        let mut exists = BOOL(0);
        unsafe {
            self.collection
                .FindFamilyName(PCWSTR(name_w.as_ptr()), &mut index, &mut exists)
                .map_err(|e| TextBackendError::DirectWrite(format!("{e:?}")))?;
        }
        if !exists.as_bool() {
            return Ok(None);
        }
        let family = unsafe { self.collection.GetFontFamily(index) }
            .map_err(|e| TextBackendError::DirectWrite(format!("{e:?}")))?;
        Ok(Some(FontFamily { family }))
    }
}

impl FontFamily {
    pub fn first_matching_font(
        &self,
        weight: FontWeight,
        stretch: FontStretch,
        style: FontStyle,
    ) -> Result<Font, TextBackendError> {
        let dw_weight = DWRITE_FONT_WEIGHT(weight.0 as i32);
        let dw_stretch = DWRITE_FONT_STRETCH(stretch.0 as i32);
        let dw_style = match style {
            FontStyle::Normal => DWRITE_FONT_STYLE_NORMAL,
            FontStyle::Italic => DWRITE_FONT_STYLE_ITALIC,
            FontStyle::Oblique => DWRITE_FONT_STYLE_OBLIQUE,
        };
        let font = unsafe { self.family.GetFirstMatchingFont(dw_weight, dw_stretch, dw_style) }
            .map_err(|e| TextBackendError::DirectWrite(format!("{e:?}")))?;
        Ok(Font { font })
    }
}

impl Font {
    /// The font's PostScript name ("Georgia-Bold"), or an empty string when
    /// the font carries none.
    pub fn postscript_name(&self) -> String {
        unsafe {
            let mut strings: Option<IDWriteLocalizedStrings> = None;
            let mut exists = BOOL(0);
            if self
                .font
                .GetInformationalStrings(
                    DWRITE_INFORMATIONAL_STRING_POSTSCRIPT_NAME,
                    &mut strings,
                    &mut exists,
                )
                .is_err()
                || !exists.as_bool()
            {
                return String::new();
            }
            let Some(strings) = strings else {
                return String::new();
            };
            let Ok(len) = strings.GetStringLength(0) else {
                return String::new();
            };
            let mut buf = vec![0u16; len as usize + 1];
            if strings.GetString(0, &mut buf).is_err() {
                return String::new();
            }
            String::from_utf16_lossy(&buf[..len as usize])
        }
    }

    pub fn create_font_face(&self) -> Result<FontFace, TextBackendError> {
        let face = unsafe { self.font.CreateFontFace() }
            .map_err(|e| TextBackendError::DirectWrite(format!("{e:?}")))?;
        Ok(FontFace { face })
    }
}

/// The face the current document registered for `family` (`@font-face`, this thread's
/// `webfonts` set) closest to `weight` and `italic`, or `None` when it registered none. A
/// document's face outranks every system lookup, as on macOS.
pub fn web_font_face(family: &str, weight: u16, italic: bool) -> Option<FontFace> {
    crate::webfonts::lookup(family, weight, italic).map(|face| FontFace { face })
}

impl FontFace {
    /// A face from the raw DirectWrite face (a rasteriser that resolved its own).
    pub fn from_raw(face: IDWriteFontFace) -> Self {
        FontFace { face }
    }

    /// The face's PostScript name ("Ahem"), read from the face itself (a document's web face has
    /// no system `Font` to ask), or an empty string when it carries none.
    pub fn postscript_name(&self) -> String {
        use windows::core::Interface;
        unsafe {
            let Ok(face3) = self.face.cast::<IDWriteFontFace3>() else {
                return String::new();
            };
            let mut strings: Option<IDWriteLocalizedStrings> = None;
            let mut exists = BOOL(0);
            if face3
                .GetInformationalStrings(DWRITE_INFORMATIONAL_STRING_POSTSCRIPT_NAME, &mut strings, &mut exists)
                .is_err()
                || !exists.as_bool()
            {
                return String::new();
            }
            let Some(strings) = strings else {
                return String::new();
            };
            let Ok(len) = strings.GetStringLength(0) else {
                return String::new();
            };
            let mut buf = vec![0u16; len as usize + 1];
            if strings.GetString(0, &mut buf).is_err() {
                return String::new();
            }
            String::from_utf16_lossy(&buf[..len as usize])
        }
    }

    /// The DirectWrite face itself, for a rasterizer that draws with it.
    pub fn raw(&self) -> &IDWriteFontFace {
        &self.face
    }

    /// The face's index inside its font file.
    pub fn index(&self) -> u32 {
        unsafe { self.face.GetIndex() }
    }

    pub fn metrics(&self) -> Result<FontMetrics, TextBackendError> {
        let mut m = DWRITE_FONT_METRICS::default();
        unsafe { self.face.GetMetrics(&mut m) };
        Ok(FontMetrics {
            design_units_per_em: m.designUnitsPerEm,
            ascent: m.ascent,
            descent: m.descent,
            line_gap: m.lineGap,
            underline_position: m.underlinePosition,
            underline_thickness: m.underlineThickness,
            strikethrough_position: m.strikethroughPosition,
            strikethrough_thickness: m.strikethroughThickness,
        })
    }

    pub fn glyph_indices(&self, codepoints: &[u32]) -> Result<Vec<u16>, TextBackendError> {
        let mut out = vec![0u16; codepoints.len()];
        unsafe {
            self.face
                .GetGlyphIndices(codepoints.as_ptr(), codepoints.len() as u32, out.as_mut_ptr())
                .map_err(|e| TextBackendError::DirectWrite(format!("{e:?}")))?;
        }
        Ok(out)
    }

    pub fn design_glyph_metrics(
        &self,
        glyph_indices: &[u16],
        is_sideways: bool,
    ) -> Result<Vec<GlyphMetrics>, TextBackendError> {
        let mut metrics = vec![DWRITE_GLYPH_METRICS::default(); glyph_indices.len()];
        unsafe {
            self.face
                .GetDesignGlyphMetrics(
                    glyph_indices.as_ptr(),
                    glyph_indices.len() as u32,
                    metrics.as_mut_ptr(),
                    is_sideways,
                )
                .map_err(|e| TextBackendError::DirectWrite(format!("{e:?}")))?;
        }
        Ok(metrics
            .into_iter()
            .map(|m| GlyphMetrics {
                advance_width: m.advanceWidth as i32,
            })
            .collect())
    }
}

impl FontFace {
    /// Whether the face carries an embedded bitmap strike (its `EBLC` table) at `ppem`.
    pub fn has_bitmap_strike(&self, ppem: u32) -> bool {
        const EBLC: u32 = u32::from_le_bytes(*b"EBLC");
        let (mut data, mut size, mut context, mut exists) =
            (std::ptr::null_mut(), 0u32, std::ptr::null_mut(), BOOL(0));
        // SAFETY: DirectWrite hands back a read-only view of the table, `size` bytes long,
        // valid until ReleaseFontTable; every read below is bounds-checked against it.
        unsafe {
            if self.face.TryGetFontTable(EBLC, &mut data, &mut size, &mut context, &mut exists).is_err() {
                return false;
            }
            if !exists.as_bool() || data.is_null() {
                if !context.is_null() {
                    self.face.ReleaseFontTable(context);
                }
                return false;
            }
            let table = std::slice::from_raw_parts(data as *const u8, size as usize);
            let be32 = |at: usize| table.get(at..at + 4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]));
            // BitmapSize records are 48 bytes from offset 8; ppemY is byte 45 of each.
            let found = be32(4).is_some_and(|count| {
                (0..count as usize).any(|i| table.get(8 + i * 48 + 45).is_some_and(|&y| u32::from(y) == ppem))
            });
            self.face.ReleaseFontTable(context);
            found
        }
    }

    /// `glyph`'s advance in px at `size` the way Chrome measures it on Windows, where it
    /// differs from the design advance: at a size the face has an embedded bitmap strike for,
    /// Skia's DirectWrite scaler measures in GDI-classic mode (whole-pixel advances). Chrome for
    /// Testing 148 win64 gives Cambria Math's U+25BE 10.00 at 12px and 13.00 at 16px (strikes),
    /// 8.36 at 10px (none, the design advance). None when the design advance stands.
    pub fn gdi_classic_advance(&self, glyph: u16, size: f32) -> Option<f32> {
        let ppem = size.round();
        if (size - ppem).abs() > f32::EPSILON || !self.has_bitmap_strike(ppem as u32) {
            return None;
        }
        let upem = self.metrics().ok()?.design_units_per_em as f32;
        let mut m = DWRITE_GLYPH_METRICS::default();
        // SAFETY: one glyph index in, one metrics record out, both locals.
        unsafe { self.face.GetGdiCompatibleGlyphMetrics(size, 1.0, None, false, &glyph, 1, &mut m, false) }.ok()?;
        // Skia rounds a GDI-classic advance to whole pixels.
        Some((m.advanceWidth as f32 * size / upem).round())
    }
}

fn to_wide_null(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// A stable id for a face: the font file it is read from (DirectWrite's
/// reference key for it), the face's index inside that file, and its
/// simulations (synthetic bold/oblique make a different face of the same
/// file). Two faces that draw differently never share an id; 0 is left free
/// to mean "no face".
fn face_id_of(face: &IDWriteFontFace) -> u64 {
    use std::hash::{Hash, Hasher};

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    unsafe {
        face.GetIndex().hash(&mut hasher);
        face.GetSimulations().0.hash(&mut hasher);
        let mut count = 0u32;
        if face.GetFiles(&mut count, None).is_ok() {
            let mut files: Vec<Option<IDWriteFontFile>> = vec![None; count as usize];
            if face.GetFiles(&mut count, Some(files.as_mut_ptr())).is_ok() {
                for file in files.into_iter().flatten() {
                    let mut key: *mut std::ffi::c_void = std::ptr::null_mut();
                    let mut size = 0u32;
                    if file.GetReferenceKey(&mut key as *mut _ as *mut _, &mut size).is_ok()
                        && !key.is_null()
                    {
                        std::slice::from_raw_parts(key as *const u8, size as usize)
                            .hash(&mut hasher);
                    }
                }
            }
        }
    }
    hasher.finish().max(1)
}

mod face_table {
    use super::FontFace;
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};

    /// Bounds what the table keeps alive. Past it the table starts over; a
    /// run whose face is gone is painted through the family-list path until
    /// layout shapes it again.
    const MAX_FACES: usize = 1024;

    pub(super) fn with<R>(f: impl FnOnce(&mut HashMap<u64, FontFace>) -> R) -> R {
        static FACES: OnceLock<Mutex<HashMap<u64, FontFace>>> = OnceLock::new();
        let mut faces = FACES
            .get_or_init(|| Mutex::new(HashMap::new()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if faces.len() >= MAX_FACES {
            faces.clear();
        }
        f(&mut faces)
    }
}

/// Record `face` as a face a run was shaped with and return its id. The
/// Windows counterpart of `macos::intern_face`; the face is size-independent
/// here, so the table is keyed by id alone.
pub fn intern_face(face: &FontFace) -> u64 {
    let id = face_id_of(&face.face);
    face_table::with(|faces| {
        faces.entry(id).or_insert_with(|| face.clone());
    });
    id
}

/// The face `intern_face` recorded under `id`, if it is still held. The
/// Windows counterpart of `macos::face_font`.
pub fn face_by_id(id: u64) -> Option<FontFace> {
    face_table::with(|faces| faces.get(&id).cloned())
}

/// The family Windows substitutes for `name` (HKLM `...\Windows NT\CurrentVersion\FontSubstitutes`:
/// "Times" -> "Times New Roman", "Helvetica" -> "Arial"), which Chrome on Windows also consults for a
/// family the system collection does not have. `None` when Windows names no substitute. The key's
/// few dozen entries are read once, at first use, into a fixed map: what pages ask about adds
/// nothing to it.
pub fn family_substitute(name: &str) -> Option<String> {
    substitutes().get(&name.trim().to_ascii_lowercase()).cloned()
}

fn substitutes() -> &'static std::collections::HashMap<String, String> {
    static TABLE: OnceLock<std::collections::HashMap<String, String>> = OnceLock::new();
    TABLE.get_or_init(read_substitutes)
}

/// Every entry of the FontSubstitutes key: lower-cased name -> family (a `,charset` suffix dropped;
/// an empty value names nothing and is left out).
fn read_substitutes() -> std::collections::HashMap<String, String> {
    use windows::Win32::System::Registry::{
        RegCloseKey, RegEnumValueW, RegOpenKeyExW, HKEY, HKEY_LOCAL_MACHINE, KEY_READ, REG_SZ,
    };
    let mut table = std::collections::HashMap::new();
    let subkey = to_wide_null(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\FontSubstitutes");
    let mut key = HKEY::default();
    if unsafe { RegOpenKeyExW(HKEY_LOCAL_MACHINE, PCWSTR(subkey.as_ptr()), None, KEY_READ, &mut key) }.is_err() {
        return table;
    }
    // A bound on entries read, so a damaged key cannot keep this loop going.
    for index in 0..4096u32 {
        let mut name = [0u16; 256];
        let mut name_len = name.len() as u32;
        let mut data = [0u16; 256];
        let mut data_len = std::mem::size_of_val(&data) as u32;
        let mut kind = 0u32;
        let status = unsafe {
            RegEnumValueW(
                key,
                index,
                Some(windows::core::PWSTR(name.as_mut_ptr())),
                &mut name_len,
                None,
                Some(&mut kind),
                Some(data.as_mut_ptr().cast()),
                Some(&mut data_len),
            )
        };
        if status.is_err() {
            // ERROR_NO_MORE_ITEMS ends the list; a value too long for the buffers is skipped by
            // the same stop, which only drops entries past it (none are that long).
            break;
        }
        if kind != REG_SZ.0 {
            continue;
        }
        let name = String::from_utf16_lossy(&name[..(name_len as usize).min(name.len())]);
        let chars = (data_len as usize / 2).min(data.len());
        let value = String::from_utf16_lossy(&data[..chars]);
        let family = value.trim_end_matches('\0').split(',').next().unwrap_or("").trim().to_string();
        if !family.is_empty() {
            table.insert(name.trim().to_ascii_lowercase(), family);
        }
    }
    unsafe {
        let _ = RegCloseKey(key);
    }
    table
}

/// ICU's Emoji_Presentation property (`UCHAR_EMOJI_PRESENTATION`, 58): the
/// table Chrome's `SymbolsIterator` reads to send a character to the emoji
/// face. Read from the system's ICU (`icu.dll`, Windows 10 1903 and later),
/// as macOS reads libicucore. A character with it is emoji unless U+FE0E
/// follows; one without it is drawn from a text face unless U+FE0F follows.
/// Without `icu.dll` every `is_emoji` character counts, the old behaviour.
pub fn has_emoji_presentation(ch: char) -> bool {
    const UCHAR_EMOJI_PRESENTATION: i32 = 58;
    (ch as u32) >= 0x80 && icu_binary_property(ch, UCHAR_EMOJI_PRESENTATION).unwrap_or_else(|| crate::is_emoji(ch))
}

/// ICU's Emoji property (`UCHAR_EMOJI`, 57): a character U+FE0F can turn into
/// an emoji. Without `icu.dll`, `is_emoji`.
pub fn has_emoji_property(ch: char) -> bool {
    const UCHAR_EMOJI: i32 = 57;
    icu_binary_property(ch, UCHAR_EMOJI).unwrap_or_else(|| crate::is_emoji(ch))
}

/// `u_hasBinaryProperty` from the system's `icu.dll`, looked up once; `None`
/// when the DLL or the export is missing.
fn icu_binary_property(ch: char, which: i32) -> Option<bool> {
    type HasBinaryProperty = unsafe extern "C" fn(i32, i32) -> i8;
    static ICU: OnceLock<Option<HasBinaryProperty>> = OnceLock::new();
    let has = ICU.get_or_init(|| {
        use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_SYSTEM32};
        // SAFETY: loading the system's icu.dll from System32 only (never the
        // application or current directory) and reading one export, whose C
        // signature is `UBool u_hasBinaryProperty(UChar32, UProperty)`.
        unsafe {
            let module = LoadLibraryExW(windows::core::w!("icu.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32).ok()?;
            let proc = GetProcAddress(module, windows::core::s!("u_hasBinaryProperty"))?;
            Some(std::mem::transmute::<*const (), HasBinaryProperty>(proc as *const ()))
        }
    });
    // SAFETY: a pure lookup on a code point; ICU accepts any i32.
    has.map(|f| unsafe { f(ch as i32, which) != 0 })
}

/// The families Chrome's Windows fallback tries for a character no face of the run has, by
/// Unicode block (Blink's `GetFontBasedOnUnicodeBlock`, font_fallback_win.cc): the math,
/// arrow and shape blocks take its math font (Cambria Math first), the symbol, dingbat and
/// pictograph blocks Segoe UI Symbol, emoticons and enclosed alphanumerics its monochrome
/// emoji font. Empty outside those blocks. Chrome for Testing 148 win64 draws U+25BE / U+25B8
/// / U+21BB from Cambria Math (CSS.getPlatformFontsForNode), where DirectWrite's own
/// MapCharacters picks Segoe UI Symbol.
pub fn blink_block_fallback(ch: char) -> &'static [&'static str] {
    const MATH: &[&str] = &["Cambria Math", "Segoe UI Symbol"];
    const SYMBOL: &[&str] = &["Segoe UI Symbol"];
    const MONO_EMOJI: &[&str] = &["Segoe UI Symbol", "Segoe UI Emoji"];
    match ch as u32 {
        // Arrows, Mathematical Operators, Miscellaneous Technical
        0x2190..=0x23FF
        // Geometric Shapes
        | 0x25A0..=0x25FF
        // Misc Mathematical Symbols-A, Supplemental Arrows-A
        | 0x27C0..=0x27FF
        // Supplemental Arrows-B, Misc Mathematical Symbols-B, Supplemental Mathematical Operators
        | 0x2900..=0x2AFF
        // Mathematical Alphanumeric Symbols
        | 0x1D400..=0x1D7FF
        // Arabic Mathematical Alphabetic Symbols
        | 0x1EE00..=0x1EEFF
        // Geometric Shapes Extended
        | 0x1F780..=0x1F7FF => MATH,
        // Miscellaneous Symbols, Dingbats
        0x2600..=0x27BF
        // Miscellaneous Symbols and Arrows
        | 0x2B00..=0x2BFF
        // Gothic
        | 0x10330..=0x1034F
        // Playing Cards
        | 0x1F0A0..=0x1F0FF
        // Misc Symbols and Pictographs
        | 0x1F300..=0x1F5FF
        // Transport and Map Symbols, Alchemical Symbols
        | 0x1F680..=0x1F77F => SYMBOL,
        // Enclosed Alphanumeric Supplement, Emoticons
        0x1F100..=0x1F1FF | 0x1F600..=0x1F64F => MONO_EMOJI,
        _ => &[],
    }
}

/// The text face a character the run's own face (`family`) has no glyph for is drawn from on
/// Windows, with its family name: a text-presentation emoji character Segoe UI Symbol (Blink's
/// monochrome emoji font, as the shaper already measures it); any other character the first
/// installed `blink_block_fallback` family with a glyph for it, then the face DirectWrite's
/// system fallback (IDWriteFontFallback::MapCharacters) maps it to. None when nothing has the
/// glyph. Layout measures the character in this face and paint draws it from the same one, so
/// both ask here. Cached per (character, family).
pub fn text_fallback_face(ch: char, family: &str) -> Option<(FontFace, String)> {
    use std::collections::HashMap;
    use std::sync::Mutex;
    type Key = (char, String);
    static CACHE: OnceLock<Mutex<HashMap<Key, Option<(u64, String)>>>> = OnceLock::new();
    let key = (ch, family.to_ascii_lowercase());
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(hit) = cache.lock().ok().and_then(|c| c.get(&key).cloned()) {
        return hit.and_then(|(id, name)| face_by_id(id).map(|face| (face, name)));
    }
    let found = find_text_fallback(ch, family);
    let stored = found.as_ref().map(|(face, name)| (intern_face(face), name.clone()));
    if let Ok(mut c) = cache.lock() {
        c.insert(key, stored);
    }
    found
}

fn find_text_fallback(ch: char, family: &str) -> Option<(FontFace, String)> {
    let emoji = crate::is_emoji(ch) || has_emoji_property(ch);
    let candidates: &[&str] = if emoji { &["Segoe UI Symbol"] } else { blink_block_fallback(ch) };
    let collection = FontCollection::system().ok()?;
    for name in candidates {
        let Some(fam) = collection.font_family_by_name(name).ok().flatten() else {
            continue;
        };
        let Ok(font) = fam.first_matching_font(FontWeight::from_u32(400), FontStretch::from_u32(5), FontStyle::Normal)
        else {
            continue;
        };
        let Ok(face) = font.create_font_face() else {
            continue;
        };
        if face.glyph_indices(&[ch as u32]).is_ok_and(|ids| ids[0] != 0) {
            return Some((face, name.to_string()));
        }
    }
    if emoji {
        return None; // the emoji path takes it
    }
    map_characters(ch, family, 400, false)
}

/// Text as DirectWrite's text-analysis source: the UTF-16 text, one locale, LTR.
#[windows::core::implement(IDWriteTextAnalysisSource)]
struct Source {
    text: Vec<u16>,
    locale: Vec<u16>,
}

impl IDWriteTextAnalysisSource_Impl for Source_Impl {
    fn GetTextAtPosition(&self, pos: u32, text: *mut *mut u16, len: *mut u32) -> windows::core::Result<()> {
        unsafe {
            if (pos as usize) < self.text.len() {
                *text = self.text.as_ptr().add(pos as usize) as *mut u16;
                *len = (self.text.len() - pos as usize) as u32;
            } else {
                *text = std::ptr::null_mut();
                *len = 0;
            }
        }
        Ok(())
    }
    fn GetTextBeforePosition(&self, pos: u32, text: *mut *mut u16, len: *mut u32) -> windows::core::Result<()> {
        unsafe {
            if pos > 0 && (pos as usize) <= self.text.len() {
                *text = self.text.as_ptr() as *mut u16;
                *len = pos;
            } else {
                *text = std::ptr::null_mut();
                *len = 0;
            }
        }
        Ok(())
    }
    fn GetParagraphReadingDirection(&self) -> DWRITE_READING_DIRECTION {
        DWRITE_READING_DIRECTION_LEFT_TO_RIGHT
    }
    fn GetLocaleName(&self, _pos: u32, len: *mut u32, locale: *mut *mut u16) -> windows::core::Result<()> {
        unsafe {
            *len = self.text.len() as u32;
            *locale = self.locale.as_ptr() as *mut u16;
        }
        Ok(())
    }
    fn GetNumberSubstitution(
        &self,
        _pos: u32,
        len: *mut u32,
        substitution: windows::core::OutRef<'_, IDWriteNumberSubstitution>,
    ) -> windows::core::Result<()> {
        unsafe {
            *len = self.text.len() as u32;
        }
        substitution.write(None)
    }
}


fn map_characters(ch: char, family: &str, weight: u16, italic: bool) -> Option<(FontFace, String)> {
    use windows::core::PCWSTR;

    let mut units = [0u16; 2];
    let text: Vec<u16> = ch.encode_utf16(&mut units).to_vec();
    let length = text.len() as u32;
    let source: IDWriteTextAnalysisSource =
        Source { text, locale: "en-us".encode_utf16().chain(std::iter::once(0)).collect() }.into();
    let family_wide: Vec<u16> = family.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: COM calls on interfaces this function owns; out-pointers are locals.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let factory: IDWriteFactory2 = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).ok()?;
        let fallback = factory.GetSystemFontFallback().ok()?;
        let mut mapped_length = 0u32;
        let mut mapped: Option<IDWriteFont> = None;
        let mut scale = 1.0f32;
        fallback
            .MapCharacters(
                &source,
                0,
                length,
                None,
                PCWSTR(family_wide.as_ptr()),
                DWRITE_FONT_WEIGHT(weight as i32),
                if italic { DWRITE_FONT_STYLE_ITALIC } else { DWRITE_FONT_STYLE_NORMAL },
                DWRITE_FONT_STRETCH_NORMAL,
                &mut mapped_length,
                &mut mapped,
                &mut scale,
            )
            .ok()?;
        let font = mapped?;
        if mapped_length == 0 {
            return None;
        }
        let names = font.GetFontFamily().ok()?.GetFamilyNames().ok()?;
        let name = localized_first(&names)?;
        if name.eq_ignore_ascii_case(family) {
            return None; // the base family draws it itself
        }
        let face = font.CreateFontFace().ok()?;
        Some((FontFace::from_raw(face), name))
    }
}

/// Collects DirectWrite's script analysis of a text (`AnalyzeScript`).
#[windows::core::implement(IDWriteTextAnalysisSink)]
struct ScriptRuns {
    runs: std::rc::Rc<std::cell::RefCell<Vec<(u32, u32, DWRITE_SCRIPT_ANALYSIS)>>>,
}

impl IDWriteTextAnalysisSink_Impl for ScriptRuns_Impl {
    fn SetScriptAnalysis(&self, pos: u32, len: u32, analysis: *const DWRITE_SCRIPT_ANALYSIS) -> windows::core::Result<()> {
        // SAFETY: DirectWrite passes a valid analysis for the duration of the call.
        let analysis = unsafe { *analysis };
        self.runs.borrow_mut().push((pos, len, analysis));
        Ok(())
    }
    fn SetLineBreakpoints(&self, _: u32, _: u32, _: *const DWRITE_LINE_BREAKPOINT) -> windows::core::Result<()> {
        Ok(())
    }
    fn SetBidiLevel(&self, _: u32, _: u32, _: u8, _: u8) -> windows::core::Result<()> {
        Ok(())
    }
    fn SetNumberSubstitution(
        &self,
        _: u32,
        _: u32,
        _: windows::core::Ref<'_, IDWriteNumberSubstitution>,
    ) -> windows::core::Result<()> {
        Ok(())
    }
}

impl FontFace {
    /// Each character's advance in `text` at `size` px, positioned by
    /// DirectWrite's shaper with pair kerning (the face's GPOS `kern`, as
    /// HarfBuzz applies it for Chrome) and no ligatures: one entry per `char`.
    /// `None` when the shaper does not give one glyph per character, so the
    /// caller keeps the nominal advances.
    pub fn kerned_advances(&self, text: &str, size: f32) -> Option<Vec<f32>> {
        use windows::core::PCWSTR;
        let utf16: Vec<u16> = text.encode_utf16().collect();
        let len = utf16.len() as u32;
        if len == 0 {
            return Some(Vec::new());
        }
        let locale: Vec<u16> = "en-us".encode_utf16().chain(std::iter::once(0)).collect();
        let source: IDWriteTextAnalysisSource = Source { text: utf16.clone(), locale: locale.clone() }.into();
        let script_runs = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let sink: IDWriteTextAnalysisSink = ScriptRuns { runs: script_runs.clone() }.into();
        let mut features = [
            DWRITE_FONT_FEATURE { nameTag: DWRITE_FONT_FEATURE_TAG_KERNING, parameter: 1 },
            DWRITE_FONT_FEATURE { nameTag: DWRITE_FONT_FEATURE_TAG_STANDARD_LIGATURES, parameter: 0 },
            DWRITE_FONT_FEATURE { nameTag: DWRITE_FONT_FEATURE_TAG_CONTEXTUAL_LIGATURES, parameter: 0 },
        ];
        let typographic =
            DWRITE_TYPOGRAPHIC_FEATURES { features: features.as_mut_ptr(), featureCount: features.len() as u32 };
        let feature_list: [*const DWRITE_TYPOGRAPHIC_FEATURES; 1] = [&typographic];
        // Each code unit's advance (a surrogate pair's second unit gets 0).
        let mut unit_advances = vec![0f32; utf16.len()];
        // SAFETY: COM calls on interfaces owned here; every buffer outlives the call and is sized
        // as DirectWrite documents (glyph buffers: 3n/2 + 16).
        unsafe {
            let analyzer = ctx().ok()?.factory.CreateTextAnalyzer().ok()?;
            analyzer.AnalyzeScript(&source, 0, len, &sink).ok()?;
            let runs = script_runs.borrow().clone();
            for (pos, run_len, script) in runs {
                let s = &utf16[pos as usize..(pos + run_len) as usize];
                let max = run_len * 3 / 2 + 16;
                let mut cluster_map = vec![0u16; s.len()];
                let mut text_props = vec![DWRITE_SHAPING_TEXT_PROPERTIES::default(); s.len()];
                let mut glyphs = vec![0u16; max as usize];
                let mut glyph_props = vec![DWRITE_SHAPING_GLYPH_PROPERTIES::default(); max as usize];
                let mut count = 0u32;
                let feature_len = [run_len];
                analyzer
                    .GetGlyphs(
                        PCWSTR(s.as_ptr()),
                        run_len,
                        &self.face,
                        false,
                        false,
                        &script,
                        PCWSTR(locale.as_ptr()),
                        None,
                        Some(feature_list.as_ptr()),
                        Some(feature_len.as_ptr()),
                        1,
                        max,
                        cluster_map.as_mut_ptr(),
                        text_props.as_mut_ptr(),
                        glyphs.as_mut_ptr(),
                        glyph_props.as_mut_ptr(),
                        &mut count,
                    )
                    .ok()?;
                let mut advances = vec![0f32; count as usize];
                let mut offsets = vec![DWRITE_GLYPH_OFFSET::default(); count as usize];
                analyzer
                    .GetGlyphPlacements(
                        PCWSTR(s.as_ptr()),
                        cluster_map.as_ptr(),
                        text_props.as_mut_ptr(),
                        run_len,
                        glyphs.as_ptr(),
                        glyph_props.as_ptr(),
                        count,
                        &self.face,
                        size,
                        false,
                        false,
                        &script,
                        PCWSTR(locale.as_ptr()),
                        Some(feature_list.as_ptr()),
                        Some(feature_len.as_ptr()),
                        1,
                        advances.as_mut_ptr(),
                        offsets.as_mut_ptr(),
                    )
                    .ok()?;
                // One glyph per character, in order: the n-th character's
                // first unit maps to glyph n.
                let mut n = 0usize;
                let mut u = 0usize;
                while u < s.len() {
                    if cluster_map[u] as usize != n {
                        return None;
                    }
                    unit_advances[pos as usize + u] = advances[n];
                    u += if (0xD800..0xDC00).contains(&s[u]) { 2 } else { 1 };
                    n += 1;
                }
                if n != count as usize {
                    return None;
                }
            }
        }
        let mut out = Vec::with_capacity(text.chars().count());
        let mut u = 0usize;
        for c in text.chars() {
            out.push(unit_advances[u]);
            u += c.len_utf16();
        }
        Some(out)
    }
}

/// The first name in a localized-strings list.
unsafe fn localized_first(names: &IDWriteLocalizedStrings) -> Option<String> {
    unsafe {
        let len = names.GetStringLength(0).ok()?;
        let mut buf = vec![0u16; len as usize + 1];
        names.GetString(0, &mut buf).ok()?;
        Some(String::from_utf16_lossy(&buf[..len as usize]))
    }
}

#[cfg(test)]
mod system_fallback_tests {
    use super::*;

    /// Chrome for Testing 148 win64 draws these from Cambria Math under Segoe UI
    /// (CSS.getPlatformFontsForNode), by Blink's block table: DirectWrite's own
    /// MapCharacters would pick Segoe UI Symbol. U+2699 (a text-presentation emoji
    /// character) keeps Segoe UI Symbol, as Chrome draws it.
    #[test]
    fn the_chrome_pages_math_symbols_fall_back_to_cambria_math() {
        for ch in ['\u{25BE}', '\u{25B8}', '\u{21BB}'] {
            let (face, name) = text_fallback_face(ch, "Segoe UI").expect("a fallback face");
            assert_eq!(name, "Cambria Math", "{ch:?}");
            assert_ne!(face.glyph_indices(&[ch as u32]).unwrap()[0], 0, "{ch:?}");
        }
        assert_eq!(text_fallback_face('\u{2699}', "Segoe UI").map(|f| f.1).as_deref(), Some("Segoe UI Symbol"));
        // U+23F1 (Miscellaneous Technical, Emoji property): Blink's text-presentation
        // emoji font, not its math font.
        assert_eq!(text_fallback_face('\u{23F1}', "Segoe UI").map(|f| f.1).as_deref(), Some("Segoe UI Symbol"));
        assert_eq!(blink_block_fallback('A'), &[] as &[&str]);
    }

    /// Cambria Math carries bitmap strikes at 12 and 16 ppem but not 10, so Chrome
    /// measures U+25BE whole-pixel there (10.00, 13.00) and by design at 10px.
    #[test]
    fn a_bitmap_strike_size_measures_whole_pixel_advances() {
        let (face, _) = text_fallback_face('\u{25BE}', "Segoe UI").expect("Cambria Math");
        let glyph = face.glyph_indices(&[0x25BE]).unwrap()[0];
        assert!(face.has_bitmap_strike(12) && face.has_bitmap_strike(16) && !face.has_bitmap_strike(10));
        assert_eq!(face.gdi_classic_advance(glyph, 12.0), Some(10.0));
        assert_eq!(face.gdi_classic_advance(glyph, 16.0), Some(13.0));
        assert_eq!(face.gdi_classic_advance(glyph, 10.0), None);
    }

    /// Outside Blink's blocks, DirectWrite's system fallback decides: U+0E01 (Thai,
    /// which Segoe UI lacks) maps to a Thai face, not to nothing.
    #[test]
    fn a_character_outside_the_block_table_takes_directwrites_fallback() {
        let (face, name) = text_fallback_face('\u{0E01}', "Segoe UI").expect("a fallback face");
        assert_ne!(face.glyph_indices(&[0x0E01]).unwrap()[0], 0, "{name}");
    }
}

#[cfg(test)]
mod emoji_presentation_tests {
    use super::*;

    /// Chrome for Testing 148 win64 draws the first list from Segoe UI Symbol
    /// (no Emoji_Presentation) and the second from Segoe UI Emoji
    /// (Emoji_Presentation): the chrome board's icons, CSS.getPlatformFontsForNode.
    #[test]
    fn the_system_icu_tells_text_from_emoji_presentation() {
        for ch in ['\u{1F6E1}', '\u{2699}', '\u{2B07}', 'A', '\u{00A9}'] {
            assert!(!has_emoji_presentation(ch), "{ch:?}");
        }
        for ch in ['\u{1F50D}', '\u{1F6AB}', '\u{1F4C8}', '\u{1F551}', '\u{1F504}'] {
            assert!(has_emoji_presentation(ch), "{ch:?}");
        }
        // U+FE0F can make these emoji; a letter it cannot.
        for ch in ['\u{2699}', '\u{00A9}', '\u{1F6E1}'] {
            assert!(has_emoji_property(ch), "{ch:?}");
        }
        assert!(!has_emoji_property('A'));
    }
}

#[cfg(test)]
mod family_substitute_tests {
    use super::*;

    fn postscript_of(name: &str) -> Option<String> {
        let collection = FontCollection::system().expect("the system collection");
        let family = collection.font_family_by_name(name).expect("lookup")?;
        let font = family
            .first_matching_font(FontWeight(400), FontStretch(5), FontStyle::Normal)
            .expect("a font");
        Some(font.postscript_name())
    }

    /// Windows names the classic families by substitution (FontSubstitutes): "Times" is Times New
    /// Roman and "Helvetica" Arial, as Chrome on Windows resolves them. DirectWrite's collection
    /// alone knows neither name, so text fell back to Segoe UI.
    #[test]
    fn the_classic_family_names_resolve_by_windows_substitution() {
        assert_eq!(postscript_of("Times").as_deref(), Some("TimesNewRomanPSMT"));
        assert_eq!(postscript_of("helvetica").as_deref(), Some("ArialMT"));
        assert_eq!(postscript_of("Times New Roman").as_deref(), Some("TimesNewRomanPSMT"));
        assert_eq!(postscript_of("NoSuchFamilyAnywhere"), None);
    }

    /// The table is the registry key's entries, read once: names pages ask about add nothing.
    #[test]
    fn asking_about_many_names_does_not_grow_the_substitute_table() {
        let before = substitutes().len();
        assert!(before > 0 && before < 4096, "{before} entries");
        for i in 0..1000 {
            assert_eq!(family_substitute(&format!("NoSuchFamily{i}")), None);
        }
        assert_eq!(family_substitute("TIMES").as_deref(), Some("Times New Roman"));
        assert_eq!(substitutes().len(), before);
    }
}
