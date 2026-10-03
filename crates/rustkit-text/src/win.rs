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

    pub fn font_family_by_name(&self, name: &str) -> Result<Option<FontFamily>, TextBackendError> {
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

impl FontFace {
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
