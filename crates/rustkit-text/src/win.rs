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
