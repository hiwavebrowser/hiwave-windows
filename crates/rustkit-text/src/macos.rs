//! macOS text shaping implementation using Core Text
//!
//! This module provides text shaping, font loading, and glyph rendering
//! using Apple's Core Text framework.

use core_foundation::base::TCFType;
use core_graphics::base::CGFloat;
use core_graphics::color_space::CGColorSpace;
use core_graphics::context::CGContext;
use core_graphics::geometry::{CGPoint, CGRect, CGSize};
use core_text::font::{self, CTFont};
use foreign_types_shared::ForeignType;
use thiserror::Error;

use crate::{GlyphFace, Variation};

/// Errors that can occur in text shaping
#[derive(Error, Debug)]
pub enum TextError {
    #[error("Font not found: {0}")]
    FontNotFound(String),

    #[error("Text shaping failed: {0}")]
    ShapingFailed(String),

    #[error("Core Text error: {0}")]
    CoreTextError(String),
}

/// Text shaper using Core Text
pub struct TextShaper {
    font: CTFont,
}

impl TextShaper {
    /// Create a new text shaper with the specified font
    pub fn new(font_name: &str, size: f64) -> Result<Self, TextError> {
        let font = create_font(font_name, size)?;
        Ok(Self { font })
    }

    /// Create a text shaper with the default system font
    pub fn with_system_font(size: f64) -> Self {
        // Try to create system font, fall back to Helvetica
        let font = create_font("SF Pro", size)
            .or_else(|_| create_font(".AppleSystemUIFont", size))
            .or_else(|_| create_font("Helvetica", size))
            .unwrap_or_else(|_| {
                // Ultimate fallback
                font::new_from_name("Helvetica", size)
                    .expect("Failed to create any font")
            });
        Self { font }
    }

    /// Shape text and return glyph information
    pub fn shape(&self, text: &str) -> Result<ShapedText, TextError> {
        // Convert text to UTF-16 for Core Text
        let utf16_chars: Vec<u16> = text.encode_utf16().collect();
        let char_count = utf16_chars.len();
        
        if char_count == 0 {
            return Ok(ShapedText {
                glyphs: vec![],
                positions: vec![],
                advances: vec![],
                font: self.font.clone(),
            });
        }
        
        // Allocate space for glyphs
        let mut glyphs: Vec<core_graphics::font::CGGlyph> = vec![0; char_count];
        
        // Get glyph IDs using Core Text
        unsafe {
            extern "C" {
                fn CTFontGetGlyphsForCharacters(
                    font: core_text::font::CTFontRef,
                    characters: *const u16,
                    glyphs: *mut core_graphics::font::CGGlyph,
                    count: isize,
                ) -> bool;
            }
            
            let success = CTFontGetGlyphsForCharacters(
                self.font.as_concrete_TypeRef(),
                utf16_chars.as_ptr(),
                glyphs.as_mut_ptr(),
                char_count as isize,
            );
            
            // Some glyphs may not be available, but continue silently
            let _ = success;
        }
        
        // Get advances for each glyph
        let mut glyph_advances: Vec<CGSize> = vec![CGSize::new(0.0, 0.0); char_count];
        unsafe {
            extern "C" {
                fn CTFontGetAdvancesForGlyphs(
                    font: core_text::font::CTFontRef,
                    orientation: u32, // kCTFontOrientationDefault = 0
                    glyphs: *const core_graphics::font::CGGlyph,
                    advances: *mut CGSize,
                    count: isize,
                ) -> f64;
            }
            
            let _total_advance = CTFontGetAdvancesForGlyphs(
                self.font.as_concrete_TypeRef(),
                0, // kCTFontOrientationDefault
                glyphs.as_ptr(),
                glyph_advances.as_mut_ptr(),
                char_count as isize,
            );
        }
        
        // Calculate positions from advances
        let mut positions: Vec<(f32, f32)> = Vec::with_capacity(char_count);
        let mut advances: Vec<f32> = Vec::with_capacity(char_count);
        let mut x_pos: f64 = 0.0;
        
        // The run's own face may be an Apple emoji face (`font-family:
        // "Apple Color Emoji"`): its glyphs advance as Chrome places them,
        // as they do when reached by fallback (#1072 R1 nit 4).
        let emoji_advance = apple_emoji_advance(&self.font).map(f64::from);
        for (i, glyph_advance) in glyph_advances.iter().enumerate() {
            // The second unit of a surrogate pair: Core Text puts the glyph
            // in the first slot and .notdef (with its advance) in this one.
            // It draws nothing and takes no space.
            let trailing_surrogate = (0xDC00..=0xDFFF).contains(&utf16_chars[i]);
            let width = match emoji_advance {
                _ if trailing_surrogate => 0.0,
                Some(a) if glyphs[i] != 0 && glyph_advance.width > 0.0 => a,
                _ => glyph_advance.width,
            };
            positions.push((x_pos as f32, 0.0));
            advances.push(width as f32);
            x_pos += width;
            
            // Handle missing glyphs (glyph ID 0)
            if glyphs[i] == 0 && !trailing_surrogate {
                // Use a fallback advance for missing glyphs
                let fallback_advance = self.font.pt_size() * 0.5;
                if advances.last().map(|a| *a == 0.0).unwrap_or(false) {
                    if let Some(last) = advances.last_mut() {
                        *last = fallback_advance as f32;
                    }
                }
            }
        }
        
        // Convert CGGlyph to u16
        let glyphs: Vec<u16> = glyphs.into_iter().map(|g| g as u16).collect();
        
        Ok(ShapedText {
            glyphs,
            positions,
            advances,
            font: self.font.clone(),
        })
    }

    /// Get font metrics
    pub fn get_metrics(&self) -> FontMetrics {
        FontMetrics {
            ascent: blink_ascent(&self.font),
            descent: self.font.descent() as f32,
            leading: self.font.leading() as f32,
            cap_height: self.font.cap_height() as f32,
            x_height: self.font.x_height() as f32,
        }
    }
    
    /// Get the underlying CTFont
    pub fn font(&self) -> &CTFont {
        &self.font
    }
}

/// Shaped text result
pub struct ShapedText {
    pub glyphs: Vec<u16>,
    pub positions: Vec<(f32, f32)>,
    pub advances: Vec<f32>,
    pub font: CTFont,
}

/// Font metrics
#[derive(Debug, Clone, Copy)]
pub struct FontMetrics {
    pub ascent: f32,
    pub descent: f32,
    pub leading: f32,
    pub cap_height: f32,
    pub x_height: f32,
}

/// True for CSS keywords naming the macOS system font. It has no
/// instantiable PostScript name — it resolves ONLY through the UI-font API.
fn is_system_family(lower: &str) -> bool {
    matches!(
        lower,
        "system-ui" | "blinkmacsystemfont" | ".applesystemuifont"
    )
}

/// A face's ascent as Blink reports it on macOS.
///
/// Blink raises the ascent of Times, Helvetica and Courier by 15% of the
/// rounded ascent + descent, to match the vertical metrics of their
/// Microsoft counterparts (`FontMetrics::AscentDescentWithHacks`). Those are
/// Chrome's default, `sans-serif` and `monospace` faces, so a 16px line set
/// in any of them is 18px in Chrome, where the font's own 12 + 4 gives 16.
/// Every other family keeps the font's ascent (Times New Roman, Helvetica
/// Neue and Courier New are other families).
pub fn blink_ascent(font: &CTFont) -> f32 {
    let ascent = font.ascent() as f32;
    if matches!(family_name_or_empty(font).as_str(), "Times" | "Helvetica" | "Courier") {
        let rounded = ascent.round();
        let descent = (font.descent() as f32).round();
        return rounded + ((rounded + descent) * 0.15 + 0.5).floor();
    }
    ascent
}

/// A face's family name, or the empty string when it has none.
///
/// `CTFont::family_name` panics on a face without one, and a downloaded font
/// can lack the name (x.com's does): Core Text returns null for it.
pub fn family_name_or_empty(font: &CTFont) -> String {
    use core_foundation::string::{CFString, CFStringRef};

    extern "C" {
        fn CTFontCopyFamilyName(font: core_text::font::CTFontRef) -> CFStringRef;
    }
    // SAFETY: `font` is a live CTFont. The result follows the create rule
    // and is null when the face has no family name.
    unsafe {
        let name = CTFontCopyFamilyName(font.as_concrete_TypeRef());
        if name.is_null() {
            return String::new();
        }
        CFString::wrap_under_create_rule(name).to_string()
    }
}

/// Map CSS generic families to concrete macOS fonts.
fn map_generic(lower: &str, fam: &str) -> &'static str {
    match lower {
        "sans-serif" => "Helvetica",
        // Chrome's faces on macOS, and the ones layout measures with
        // (rustkit-layout `FontFamilyChain::serif` / `monospace`).
        "serif" => "Times",
        "monospace" => "Courier",
        _ => {
            // Not generic: caller uses the original string.
            let _ = fam;
            ""
        }
    }
}

/// Map a CSS font-weight (100..900) to `kCTFontWeightTrait` (-1.0 ..= 1.0).
///
/// This is Skia's table (`SkTypeface_mac` / `SkFontHost_mac`), which is what
/// Chrome uses to resolve `system-ui` weights on macOS. The anchors are
/// Apple's own constants: Ultralight -0.80, Thin -0.60, Light -0.40,
/// Regular 0.0, Medium 0.23, Semibold 0.30, Bold 0.40, Heavy 0.56, Black 0.62.
/// Intermediate CSS weights interpolate linearly between neighbours.
pub fn ct_weight_trait(css_weight: u16) -> f64 {
    const TABLE: [(u16, f64); 9] = [
        (100, -0.80),
        (200, -0.60),
        (300, -0.40),
        (400, 0.00),
        (500, 0.23),
        (600, 0.30),
        (700, 0.40),
        (800, 0.56),
        (900, 0.62),
    ];
    let w = css_weight.clamp(1, 1000);
    if w <= TABLE[0].0 {
        return TABLE[0].1;
    }
    for pair in TABLE.windows(2) {
        let (w0, t0) = pair[0];
        let (w1, t1) = pair[1];
        if w <= w1 {
            let f = (w - w0) as f64 / (w1 - w0) as f64;
            return t0 + (t1 - t0) * f;
        }
    }
    TABLE[8].1
}

/// The macOS system font at `size`, in the face matching `css_weight`.
///
/// The system font has no instantiable PostScript name and no by-name trait
/// variants, so it resolves only through the UI-font API. That API exposes
/// exactly TWO faces (`kCTFontSystemFontType` = Regular,
/// `kCTFontEmphasizedSystemFontType` = Bold) — which is why the old
/// `weight >= 600 ? bold : regular` gate collapsed all of 100..500 onto
/// `.SFNS-Regular` and 600..900 onto `.SFNS-Bold`. Chrome does not: it sets
/// the variable font's `wght` axis to the CSS weight (`apply_wght_axis`).
/// A `kCTFontWeightTrait` descriptor, the step before that, snapped to the
/// nearest named instance and is now only the fallback for a face with no
/// `wght` axis. CH2.B6c receipt: 'Downloads and Tabs 1' at 13px, weights
/// 300/500/600/800 = 132.84/137.42/139.51/146.56 by trait, Chrome
/// 133.19/137.17/139.78/144.98, wght axis equal to Chrome within 0.05px.
///
/// Receipts (20px, `about`'s `.tagline`, `parity-tests/probe/ct_weight_advance.py`):
/// Light 659.1px vs Regular 670.0px — an 11px error on one line of `font-weight:300`
/// text, enough to wrap a string Chrome fits.
pub fn create_system_font_with_weight(size: f64, css_weight: u16) -> CTFont {
    let base = font::new_ui_font_for_language(font::kCTFontSystemFontType, size, None);
    system_font_weighted(base, size, css_weight)
}

/// The system font's italic at `css_weight`: the italic face first, then the
/// weight on it, so the `wght` variation is not lost to the trait copy
/// (Chrome for Testing 148: italic system-ui is .SFNS-RegularItalic, at 700
/// .SFNS-BoldItalic).
pub fn create_system_italic_with_weight(size: f64, css_weight: u16) -> CTFont {
    use core_text::font_descriptor::kCTFontItalicTrait;
    let upright = font::new_ui_font_for_language(font::kCTFontSystemFontType, size, None);
    let base = upright
        .clone_with_symbolic_traits(kCTFontItalicTrait, kCTFontItalicTrait)
        .unwrap_or(upright);
    system_font_weighted(base, size, css_weight)
}

fn system_font_weighted(base: CTFont, size: f64, css_weight: u16) -> CTFont {
    if css_weight == 400 {
        return base;
    }
    apply_wght_axis(&base, size, css_weight)
        .or_else(|| apply_weight_trait(&base, size, css_weight))
        .unwrap_or_else(|| {
        // Descriptor matching failed (unusual): keep the old two-face split
        // rather than silently returning Regular for bold text.
        let ui_type = if css_weight >= 600 {
            font::kCTFontEmphasizedSystemFontType
        } else {
            font::kCTFontSystemFontType
        };
        font::new_ui_font_for_language(ui_type, size, None)
    })
}

/// The OpenType `wght` axis tag, as Core Text keys a variation dictionary.
const WGHT_AXIS_TAG: i64 = 0x7767_6874;

/// `base` (the variable system font) with its `wght` axis set to the CSS
/// weight itself — what Chrome does. A `kCTFontWeightTrait` descriptor
/// instead snaps to the nearest NAMED instance (Light 300, Medium 510,
/// Semibold 590, Heavy 860 on SF), so 300/500/600/800 measured up to 1.6px
/// off Chrome on a 13px chrome label (CH2.B6c). Returns `None` when the face
/// has no `wght` axis (the caller falls back to the weight trait).
fn apply_wght_axis(base: &CTFont, size: f64, css_weight: u16) -> Option<CTFont> {
    use core_foundation::base::CFType;
    use core_foundation::dictionary::CFDictionary;
    use core_foundation::number::CFNumber;
    use core_foundation::string::CFString;
    use core_foundation::string::CFStringRef;
    use core_text::font_descriptor;

    #[link(name = "CoreText", kind = "framework")]
    extern "C" {
        static kCTFontVariationAxisIdentifierKey: CFStringRef;
        static kCTFontVariationAxisMinimumValueKey: CFStringRef;
        static kCTFontVariationAxisMaximumValueKey: CFStringRef;
    }
    let key = |k: CFStringRef| unsafe { CFString::wrap_under_get_rule(k) };
    let (id_key, min_key, max_key) = unsafe {
        (
            key(kCTFontVariationAxisIdentifierKey),
            key(kCTFontVariationAxisMinimumValueKey),
            key(kCTFontVariationAxisMaximumValueKey),
        )
    };

    // Only a face that has the axis: elsewhere Core Text ignores the
    // attribute and we would return the Regular face for every weight.
    let axes = base.get_variation_axes()?;
    let (min, max) = axes.iter().find_map(|axis| {
        let num = |k: &CFString| axis.find(k).and_then(|v| v.downcast::<CFNumber>());
        if num(&id_key)?.to_i64()? != WGHT_AXIS_TAG {
            return None;
        }
        Some((num(&min_key)?.to_f64()?, num(&max_key)?.to_f64()?))
    })?;
    let wght = (css_weight as f64).clamp(min, max);

    let variation: CFDictionary<CFNumber, CFType> = CFDictionary::from_CFType_pairs(&[(
        CFNumber::from(WGHT_AXIS_TAG),
        CFNumber::from(wght).as_CFType(),
    )]);
    let var_key = unsafe { CFString::wrap_under_get_rule(font_descriptor::kCTFontVariationAttribute) };
    let attrs: CFDictionary<CFString, CFType> =
        CFDictionary::from_CFType_pairs(&[(var_key, variation.as_CFType())]);
    let desc = base
        .copy_descriptor()
        .create_copy_with_attributes(attrs.into_untyped())
        .ok()?;
    Some(font::new_from_descriptor(&desc, size))
}

/// Copy `base`'s descriptor with `kCTFontWeightTrait` overridden, and
/// re-instantiate at `size`. Returns `None` if Core Text can't match.
fn apply_weight_trait(base: &CTFont, size: f64, css_weight: u16) -> Option<CTFont> {
    use core_foundation::base::CFType;
    use core_foundation::dictionary::CFDictionary;
    use core_foundation::number::CFNumber;
    use core_foundation::string::CFString;
    use core_text::font_descriptor;

    let trait_key = unsafe { CFString::wrap_under_get_rule(font_descriptor::kCTFontWeightTrait) };
    let traits_key =
        unsafe { CFString::wrap_under_get_rule(font_descriptor::kCTFontTraitsAttribute) };

    let weight_num = CFNumber::from(ct_weight_trait(css_weight));
    let traits: CFDictionary<CFString, CFType> =
        CFDictionary::from_CFType_pairs(&[(trait_key, weight_num.as_CFType())]);
    let attrs: CFDictionary<CFString, CFType> =
        CFDictionary::from_CFType_pairs(&[(traits_key, traits.as_CFType())]);

    let desc = base
        .copy_descriptor()
        .create_copy_with_attributes(attrs.into_untyped())
        .ok()?;
    Some(font::new_from_descriptor(&desc, size))
}

/// Font names compared without case, spaces, hyphens or underscores (the
/// `named_font` accept test).
fn normalize_name(s: &str) -> String {
    s.chars()
        .filter(|c| !matches!(c, ' ' | '-' | '_'))
        .collect::<String>()
        .to_ascii_lowercase()
}

/// The face of the installed family `family` that CSS font matching
/// (css-fonts-4 §5.2) selects for `weight` / `italic`, or `None` when no
/// family of that name is installed.
///
/// Resolution goes through a Core Text descriptor carrying the FAMILY name
/// plus `kCTFontWeightTrait` (and the italic symbolic trait) — how
/// Chrome/Skia pick a face — because PostScript-name guessing cannot see a
/// family's faces: "Georgia-Bold" exists, but Arial's bold is
/// "Arial-BoldMT", Times New Roman's "TimesNewRomanPS-BoldMT", and the
/// weight ladders of Helvetica Neue / Avenir Next follow no `-Bold` suffix
/// rule at all. Until n50 the LAYOUT resolver also tried the bare family
/// name BEFORE its `-Bold` guesses, so every `font-weight: 700` run on a
/// named family measured with the REGULAR face while paint (which guessed
/// in the right order) drew bold glyphs at regular advances: h1 "The Art
/// of Typography" 443px of overlapping bold ink vs Chrome's 508.
///
/// Core Text matches the NEAREST weight, which differs from CSS at two
/// points, both corrected here: a desired weight ≤ 500 must not land on a
/// bold face when a lighter one exists (Georgia 500: nearest is Bold; CSS
/// walks 500 → 400 first), and a desired weight ≥ 600 takes the next
/// HEAVIER face (Helvetica Neue 600: nearest is Medium; CSS walks upward
/// → Bold, which is what Chrome draws).
///
/// A substitute (Core Text answers Helvetica for a family it lacks) is
/// rejected by family name, as `named_font` does, so a chain walk goes on.
pub fn family_face(family: &str, size: f64, weight: u16, italic: bool) -> Option<CTFont> {
    use core_text::font_descriptor::kCTFontBoldTrait;

    let wanted = normalize_name(family);
    if wanted.is_empty() {
        return None;
    }
    let lookup = |css_weight: u16| -> Option<CTFont> {
        let f = family_descriptor_font(family, size, css_weight, italic);
        (normalize_name(&f.family_name()) == wanted).then_some(f)
    };
    let is_bold = |f: &CTFont| f.symbolic_traits() & kCTFontBoldTrait != 0;

    let face = lookup(weight)?;
    if weight <= 500 && is_bold(&face) {
        return Some(lookup(400).filter(|f| !is_bold(f)).unwrap_or(face));
    }
    if weight >= 600 && !is_bold(&face) {
        if let Some(bold) = lookup(700).filter(is_bold) {
            return Some(bold);
        }
    }
    Some(face)
}

/// `CTFontCreateWithFontDescriptor` for `{ family, weight trait [, italic] }`.
/// Never fails — Core Text substitutes — so the caller checks the family.
fn family_descriptor_font(family: &str, size: f64, css_weight: u16, italic: bool) -> CTFont {
    use core_foundation::base::CFType;
    use core_foundation::dictionary::CFDictionary;
    use core_foundation::number::CFNumber;
    use core_foundation::string::{CFString, CFStringRef};
    use core_text::font_descriptor;

    // SAFETY: the four keys are Core Text's constant CFStrings, valid for
    // the process lifetime; `wrap_under_get_rule` retains them.
    let (weight_key, symbolic_key, family_key, traits_key): (
        CFString,
        CFString,
        CFString,
        CFString,
    ) = unsafe {
        let key = |k: CFStringRef| CFString::wrap_under_get_rule(k);
        (
            key(font_descriptor::kCTFontWeightTrait),
            key(font_descriptor::kCTFontSymbolicTrait),
            key(font_descriptor::kCTFontFamilyNameAttribute),
            key(font_descriptor::kCTFontTraitsAttribute),
        )
    };
    let mut trait_pairs: Vec<(CFString, CFType)> = vec![(
        weight_key,
        CFNumber::from(ct_weight_trait(css_weight)).as_CFType(),
    )];
    if italic {
        trait_pairs.push((
            symbolic_key,
            CFNumber::from(font_descriptor::kCTFontItalicTrait as i32).as_CFType(),
        ));
    }
    let traits: CFDictionary<CFString, CFType> = CFDictionary::from_CFType_pairs(&trait_pairs);
    let attrs: CFDictionary<CFString, CFType> = CFDictionary::from_CFType_pairs(&[
        (family_key, CFString::new(family).as_CFType()),
        (traits_key, traits.as_CFType()),
    ]);
    let desc = font_descriptor::new_from_attributes(&attrs);
    font::new_from_descriptor(&desc, size)
}

/// `CTFontCreateWithName` never fails: an UNINSTALLED name comes back as a
/// substitute face (Helvetica here), so a chain walk that trusts `Ok` stops
/// at its first missing family. The layout crate's monospace chain led with
/// "SF Mono" (not on a stock Mac): `1ch` measured Helvetica's "0" (8.896px
/// at 16px = 0.556em) while the painter, given the bare generic, drew Menlo
/// — WPT overflow-wrap-anywhere-003 laid `PASS` out as `PAS` / `S`. Any
/// page naming a font the machine lacks ahead of its generic fallback hit
/// the same substitute. A face is accepted only when its family or
/// PostScript name is the one asked for (names compared without case,
/// spaces or hyphens; a PostScript prefix match admits "Menlo-Regular" for
/// "Menlo" and "ArialMT" for "Arial").
///
/// `pub` so the LAYOUT side's chain walk (rustkit-layout
/// `create_ct_font_with_traits`) rejects substitutes the same way — until it
/// did, paint walked the chain but measure did not, and `"Missing", Menlo`
/// measured Helvetica while drawing Menlo.
/// Faces tried, in order, for a character the requested face has no glyph
/// for. ONE list for paint (`GlyphRasterizer::rasterize_fallback`) and
/// layout (rustkit-layout `TextShaper::shape`): layout used to shape such a
/// character as glyph 0 of the primary face — a .notdef advance and the
/// primary face's extents — while paint drew it from Apple Color Emoji, so
/// an emoji overlapped the letter after it and its line box came out the
/// primary face's height (16px system-ui: 18px; Chrome, which unites the
/// used fallback face's extents under `line-height: normal`, 26px).
pub const GLYPH_FALLBACK_FAMILIES: &[&str] = &[
    "Apple Color Emoji", // emoji — Chrome/Skia's macOS emoji fallback too
    "Apple Symbols",     // symbols
    "Arial Unicode MS",  // wide Unicode coverage
    "Helvetica Neue",    // general fallback
    "Menlo",             // code/math symbols
];

const EMOJI_FAMILY: &str = "Apple Color Emoji";

/// The faces the colour-glyph path draws from, in its order
/// (`GlyphRasterizer::resolve_color_font`).
const COLOR_GLYPH_FAMILIES: &[&str] = &[EMOJI_FAMILY, "Apple Symbols"];

/// THE face decision, made once per character where the text is shaped and
/// carried to paint as a [`GlyphFace`] (rustkit-layout records it on the
/// display list; the renderer reads it and never decides again). `primary`
/// is the run's face, `variation` the selector after `ch`, and
/// `primary_has_glyph` whether `primary` has a glyph for `ch`.
///
/// Chrome for Testing 148 (CSS.getPlatformFontsForNode, the
/// presentation-matrix probe of 2026-11-08), which this follows:
///
/// * U+FE0F after a character with the Unicode Emoji property goes to the
///   colour faces whether or not the primary has the glyph: `©️` in
///   Helvetica, Menlo or the system font is Apple Color Emoji (18px wide,
///   a 23px line at 14px). After a character without the property (`▾`
///   U+25BE) the selector changes nothing.
/// * U+FE0E asks for a text face: the primary when it has the glyph and is
///   not a colour face, else the cascade's (`☕︎` under Helvetica is Apple
///   Symbols; under the system font `.AppleColorEmojiUI`, the UI emoji
///   face with the system font's extents).
/// * Otherwise the primary when it has the glyph (Apple Symbols keeps its
///   own `☕`); else the colour faces for an Emoji_Presentation character
///   and the cascade face for any other, text-default emoji included (`🛡`
///   under the system font is `.AppleColorEmojiUI`, `⚙` Menlo, `❤` Zapf
///   Dingbats).
///
/// A variation selector or joiner is [`GlyphFace::Ignorable`]: zero
/// advance, nothing drawn, no face of its own (Chrome draws it as a
/// zero-width glyph of the base character's face).
pub fn select_face(
    primary: &CTFont,
    ch: char,
    variation: Variation,
    primary_has_glyph: bool,
) -> GlyphFace {
    if crate::is_default_ignorable(ch) {
        return GlyphFace::Ignorable;
    }
    match variation {
        Variation::Emoji if has_emoji_property(ch) => GlyphFace::Fallback { emoji: true },
        Variation::Text => {
            if primary_has_glyph && !is_color_face(primary) {
                GlyphFace::Primary
            } else {
                GlyphFace::Fallback { emoji: false }
            }
        }
        _ if primary_has_glyph => GlyphFace::Primary,
        _ => GlyphFace::Fallback { emoji: has_emoji_presentation(ch) },
    }
}

/// ICU's Emoji_Presentation property (`UCHAR_EMOJI_PRESENTATION`, 58), from
/// the system's libicucore: the same table Chrome's `SymbolsIterator` reads.
/// A character with it is emoji unless U+FE0E follows.
pub fn has_emoji_presentation(ch: char) -> bool {
    const UCHAR_EMOJI_PRESENTATION: i32 = 58;
    (ch as u32) >= 0x80 && icu_binary_property(ch, UCHAR_EMOJI_PRESENTATION)
}

/// ICU's Emoji property (`UCHAR_EMOJI`, 57): a character U+FE0F can turn
/// into an emoji. The digits, `#`, `*`, `©` and `®` have it.
pub fn has_emoji_property(ch: char) -> bool {
    const UCHAR_EMOJI: i32 = 57;
    icu_binary_property(ch, UCHAR_EMOJI)
}

fn icu_binary_property(ch: char, which: i32) -> bool {
    #[link(name = "icucore")]
    extern "C" {
        fn u_hasBinaryProperty(c: i32, which: i32) -> i8;
    }
    // SAFETY: a pure lookup on a code point; ICU accepts any i32.
    unsafe { u_hasBinaryProperty(ch as i32, which) != 0 }
}

/// Whether `font` draws colour glyphs (Apple Color Emoji and its
/// `.AppleColorEmojiUI` variant): such a face is painted on the colour-glyph
/// path, any other on the coverage-mask path.
pub fn is_color_face(font: &CTFont) -> bool {
    const K_CT_FONT_COLOR_GLYPHS_TRAIT: u32 = 1 << 13;
    font.symbolic_traits() & K_CT_FONT_COLOR_GLYPHS_TRAIT != 0
}

/// The advance Chrome gives a glyph of Apple's emoji faces (Apple Color
/// Emoji, and the system font's `.AppleColorEmojiUI`), or `None` for any
/// other face.
///
/// Core Text's advance for these sbix faces carries the font's `trak`
/// tracking and comes back rounded up: 19 at 14px, 21 at 16px. Chrome for
/// Testing 148 places the next character 18 and 20 along. Measured at 30
/// sizes from 6 to 40px (one `🔍` span each, CB7a), Chrome's advance is
/// exactly the width of a full-size emoji's image box at that size
/// (`CTFontGetBoundingRectsForGlyphs` of U+1F600, 1.25em up to 16px,
/// tapering to 1em at 24px and above) rounded half up; Core Text's own
/// advance is not, nor is the unscaled `hmtx` advance (1em) or a size-1
/// font scaled up. Every glyph of the face takes the same advance, as in
/// Chrome (a flag, whose own image is narrower, is 18 at 14px too).
pub fn apple_emoji_advance(font: &CTFont) -> Option<f32> {
    use std::cell::RefCell;
    use std::collections::HashMap;
    // The symbolic-traits read is the only cost every other face pays.
    if !is_color_face(font) {
        return None;
    }
    // One Core Text query per (face, size): it is on the shaping hot path,
    // once per emoji per `shape` (Prometheus R1 on #1072).
    type Key = (String, u32);
    thread_local! {
        static ADVANCE: RefCell<HashMap<Key, Option<f32>>> = RefCell::new(HashMap::new());
    }
    const MAX_ENTRIES: usize = 1024;
    let name = postscript_name_or_empty(font);
    // A face without a PostScript name is not cached by it (see
    // `shaped_cache_key`).
    if name.is_empty() {
        return apple_emoji_advance_uncached(font);
    }
    let key = (name, (font.pt_size() as f32).to_bits());
    if let Some(hit) = ADVANCE.with(|m| m.borrow().get(&key).copied()) {
        return hit;
    }
    let advance = apple_emoji_advance_uncached(font);
    ADVANCE.with(|m| {
        let mut m = m.borrow_mut();
        if m.len() >= MAX_ENTRIES {
            m.clear();
        }
        m.insert(key, advance);
    });
    advance
}

fn apple_emoji_advance_uncached(font: &CTFont) -> Option<f32> {
    if !font.family_name().contains("Apple Color Emoji") {
        return None;
    }
    let reference = color_glyph_id(font, '\u{1F600}')?;
    extern "C" {
        fn CTFontGetBoundingRectsForGlyphs(
            font: core_text::font::CTFontRef,
            orientation: u32,
            glyphs: *const u16,
            bounding_rects: *mut CGRect,
            count: isize,
        ) -> CGRect;
    }
    let mut bounds = CGRect::new(&CGPoint::new(0.0, 0.0), &CGSize::new(0.0, 0.0));
    // SAFETY: `font` is a live CTFont; one glyph id is read and one rect
    // written.
    unsafe {
        CTFontGetBoundingRectsForGlyphs(font.as_concrete_TypeRef(), 0, &reference, &mut bounds, 1);
    }
    let width = bounds.size.width;
    // f64::round rounds a half away from zero, as Chrome does here
    // (17.5 at 14px is 18, 22.5 at 21px is 23).
    (width > 0.0).then(|| width.round() as f32)
}

/// The characters of the emoji sequence that starts `chars` (one extended
/// grapheme cluster: a ZWJ sequence, a base with a skin-tone modifier, a
/// regional-indicator pair, a keycap, a tag sequence, selectors). 1 for a
/// lone character.
pub fn emoji_cluster_len(chars: &[char]) -> usize {
    use unicode_segmentation::UnicodeSegmentation;
    CLUSTER_SEGMENTATIONS.with(|c| c.set(c.get() + 1));
    // A cluster is short; the longest RGI sequences are ten code points.
    let head: String = chars.iter().take(16).collect();
    head.graphemes(true).next().map_or(1, |g| g.chars().count().max(1))
}

thread_local! {
    static CLUSTER_SEGMENTATIONS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    static CLUSTER_LINES: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// How many grapheme segmentations (`emoji_cluster_len`) this thread made:
/// paint must not segment a run that has no folded emoji sequence.
pub fn emoji_cluster_segmentations() -> u64 {
    CLUSTER_SEGMENTATIONS.with(|c| c.get())
}

/// How many CTLines this thread built to shape emoji sequences
/// (`emoji_cluster_glyph`): one per distinct (face, size, sequence).
pub fn emoji_cluster_lines() -> u64 {
    CLUSTER_LINES.with(|c| c.get())
}

/// The first glyph `face` shapes the emoji sequence `cluster` into, and how
/// many characters of `cluster` it covers; `None` when that glyph covers
/// only the first character (nothing to fold).
///
/// Chrome shapes an emoji sequence as clusters, one advance per glyph the
/// emoji face forms: a ZWJ family, a skin-toned thumbs-up, a flag and a
/// keycap are each ONE glyph, 18 wide at 14px in Chrome for Testing 148
/// (EM1); `👨‍🦲‍🙈`, which the face forms as `👨‍🦲` and `🙈`, is two (36).
/// Layout folds the covered characters into the head's advance, and paint
/// draws this glyph for them.
pub fn emoji_cluster_glyph(face: &CTFont, cluster: &str) -> Option<(u16, usize)> {
    use std::cell::RefCell;
    use std::collections::HashMap;
    if cluster.chars().count() < 2 {
        return None;
    }
    // One CTLine per (face, size, sequence): `shape` reruns on every
    // relayout (Prometheus R1 MED 2 on #1076).
    type Key = (String, u32, String);
    thread_local! {
        static SHAPED: RefCell<HashMap<Key, Option<(u16, usize)>>> = RefCell::new(HashMap::new());
    }
    const MAX_ENTRIES: usize = 4096;
    let Some(key) = shaped_cache_key(&postscript_name_or_empty(face), face.pt_size(), cluster) else {
        return emoji_cluster_glyph_uncached(face, cluster);
    };
    if let Some(hit) = SHAPED.with(|m| m.borrow().get(&key).copied()) {
        return hit;
    }
    let shaped = emoji_cluster_glyph_uncached(face, cluster);
    SHAPED.with(|m| {
        let mut m = m.borrow_mut();
        if m.len() >= MAX_ENTRIES {
            m.clear();
        }
        m.insert(key, shaped);
    });
    shaped
}

/// The shaped-sequence cache key for a face named `ps_name` at `size`, or
/// `None` for a face without a PostScript name (a downloaded face can lack
/// one): two such faces would share an entry, so they are shaped uncached.
fn shaped_cache_key(ps_name: &str, size: f64, cluster: &str) -> Option<(String, u32, String)> {
    (!ps_name.is_empty()).then(|| (ps_name.to_string(), (size as f32).to_bits(), cluster.to_string()))
}

fn emoji_cluster_glyph_uncached(face: &CTFont, cluster: &str) -> Option<(u16, usize)> {
    use core_foundation::attributed_string::CFMutableAttributedString;
    use core_foundation::base::CFRange;
    use core_foundation::string::CFString;
    use core_text::line::CTLine;
    use core_text::string_attributes::kCTFontAttributeName;

    if cluster.chars().count() < 2 {
        return None;
    }
    let mut astr = CFMutableAttributedString::new();
    astr.replace_str(&CFString::new(cluster), CFRange::init(0, 0));
    let range = CFRange::init(0, astr.char_len());
    // SAFETY: `face` is a live CTFont; the attribute is set over the
    // string's own range.
    unsafe {
        astr.set_attribute(range, kCTFontAttributeName, face);
    }
    CLUSTER_LINES.with(|c| c.set(c.get() + 1));
    let line = CTLine::new_with_attributed_string(astr.as_concrete_TypeRef());
    // (glyph, UTF-16 index of its first character), inked glyphs only:
    // selectors and joiners shape to none or to deleted (0xFFFF) ones.
    let mut inked: Vec<(u16, isize)> = Vec::new();
    for run in line.glyph_runs().iter() {
        // A run in another face: the emoji face does not draw this.
        let glyphs = run.glyphs();
        let indices = run.string_indices();
        for (g, i) in glyphs.iter().zip(indices.iter()) {
            if *g != 0 && *g != 0xFFFF {
                inked.push((*g, *i));
            }
        }
    }
    let (glyph, start) = *inked.first()?;
    if start != 0 {
        return None;
    }
    let end_utf16 = inked.get(1).map_or(astr.char_len(), |(_, i)| *i) as usize;
    // UTF-16 offset to a character count.
    let mut units = 0usize;
    let mut covered = 0usize;
    for ch in cluster.chars() {
        if units >= end_utf16 {
            break;
        }
        units += ch.len_utf16();
        covered += 1;
    }
    (covered > 1).then_some((glyph, covered))
}

/// The horizontal advance of `glyph` in `font`, in CSS px at the font's
/// size: what layout measures a character at and what paint owes for it.
pub fn glyph_advance(font: &CTFont, glyph: u16) -> f32 {
    if let Some(advance) = apple_emoji_advance(font) {
        return advance;
    }
    extern "C" {
        fn CTFontGetAdvancesForGlyphs(
            font: core_text::font::CTFontRef,
            orientation: u32,
            glyphs: *const u16,
            advances: *mut CGSize,
            count: isize,
        ) -> f64;
    }
    let mut advance = CGSize::new(0.0, 0.0);
    // SAFETY: `font` is a live CTFont; one glyph id is read and one
    // advance written.
    unsafe {
        CTFontGetAdvancesForGlyphs(font.as_concrete_TypeRef(), 0, &glyph, &mut advance, 1);
    }
    advance.width as f32
}

/// The face a character is drawn from when `primary` has no glyph for it,
/// and its glyph id there. ONE function for layout (which takes the advance
/// and the face's extents) and paint (which draws the glyph), so both use
/// the same face. The presentation is the character's own
/// (`has_emoji_presentation`); a caller that knows the face decision
/// (`select_face`) uses `fallback_face_for_presentation`.
pub fn fallback_face_for(primary: &CTFont, ch: char) -> Option<(CTFont, u16)> {
    fallback_face_for_presentation(primary, ch, has_emoji_presentation(ch))
}

/// `fallback_face_for` with the presentation decided by the caller (the
/// `emoji` of a [`GlyphFace::Fallback`]).
///
/// Chrome asks the system for the fallback of the font in use, and so does
/// this: `CTFontCreateForString` walks Core Text's cascade list for
/// `primary`. A fixed list tried in order gave `⌘`, `←` and `→` in Courier
/// Apple Symbols' advance (22.3 to 22.6px in a 16px `<kbd>`), where the
/// cascade, and Chrome, give Menlo's (21.23px).
///
/// An emoji-presented character (Emoji_Presentation, or U+FE0F after it)
/// takes Apple Color Emoji first, as Blink's emoji fallback priority does:
/// `⏰` and `⌨️` lines are the emoji face's height (an `about` heading is
/// 29px). Every other character takes the cascade first, text-default emoji
/// included, which is what Chrome for Testing 148 does: under the system
/// font `🛡` is `.AppleColorEmojiUI` (the system font's extents, so a 17px
/// line at 14px, not 23) and `⚙` and `⌨` are Menlo; under Helvetica `❤` is
/// Zapf Dingbats. Taking Apple Color Emoji for every character above
/// Latin-1 it has made the chrome sidebar's shield and settings rows 6px
/// taller than Chrome's. ASCII and Latin-1 never go to the emoji face
/// unasked (it has the digits, `#`, `*`, `©` and `®`). The fixed list is
/// the last resort. `None` means no face has the character (Core Text
/// answers with its LastResort face, which is not a glyph anyone meant).
pub fn fallback_face_for_presentation(
    primary: &CTFont,
    ch: char,
    emoji: bool,
) -> Option<(CTFont, u16)> {
    use std::cell::RefCell;
    use std::collections::HashMap;

    type Key = (String, u32, char, bool);
    thread_local! {
        static FOUND: RefCell<HashMap<Key, Option<(CTFont, u16)>>> = RefCell::new(HashMap::new());
    }
    const MAX_ENTRIES: usize = 4096;

    let key = (postscript_name_or_empty(primary), (primary.pt_size() as f32).to_bits(), ch, emoji);
    if let Some(hit) = FOUND.with(|m| m.borrow().get(&key).cloned()) {
        return hit;
    }
    let found = fallback_face_uncached(primary, ch, emoji);
    FOUND.with(|m| {
        let mut m = m.borrow_mut();
        if m.len() >= MAX_ENTRIES {
            m.clear();
        }
        m.insert(key, found.clone());
    });
    found
}

fn fallback_face_uncached(primary: &CTFont, ch: char, emoji: bool) -> Option<(CTFont, u16)> {
    let size = primary.pt_size();
    let named = |families: &[&str]| -> Option<(CTFont, u16)> {
        families.iter().find_map(|name| {
            let font = font::new_from_name(name, size).ok()?;
            let glyph = color_glyph_id(&font, ch)?;
            Some((font, glyph))
        })
    };

    if emoji {
        if let Some(hit) = named(COLOR_GLYPH_FAMILIES) {
            return Some(hit);
        }
    }
    // A text face asked for an Emoji_Presentation character (U+FE0E after
    // it): the cascade is asked with the selector, as Chrome asks it, so
    // Core Text answers a text face where one has the character (`☕︎`
    // under Helvetica is Apple Symbols, not Apple Color Emoji).
    let text_request = !emoji && has_emoji_presentation(ch);
    if let Some(font) = cascade_font(primary, ch, text_request) {
        if let Some(glyph) = color_glyph_id(&font, ch) {
            return Some((font, glyph));
        }
    }
    named(GLYPH_FALLBACK_FAMILIES)
}

/// Core Text's cascade face for `ch` under `primary`, at `primary`'s size;
/// with `text_request`, for `ch` followed by U+FE0E. `None` when the answer
/// is the LastResort face.
fn cascade_font(primary: &CTFont, ch: char, text_request: bool) -> Option<CTFont> {
    use core_foundation::base::CFRange;
    use core_foundation::string::{CFString, CFStringRef};
    use core_text::font::CTFontRef;

    extern "C" {
        fn CTFontCreateForString(
            current_font: CTFontRef,
            string: CFStringRef,
            range: CFRange,
        ) -> CTFontRef;
    }

    let mut text = ch.to_string();
    if text_request {
        text.push('\u{FE0E}');
    }
    let string = CFString::new(&text);
    let range = CFRange::init(0, text.encode_utf16().count() as isize);
    // SAFETY: `primary` and `string` are live, and the range is the whole
    // string in UTF-16 units. The result follows the create rule.
    let font = unsafe {
        let raw = CTFontCreateForString(
            primary.as_concrete_TypeRef(),
            string.as_concrete_TypeRef(),
            range,
        );
        if raw.is_null() {
            return None;
        }
        CTFont::wrap_under_create_rule(raw)
    };
    (postscript_name_or_empty(&font) != "LastResort").then_some(font)
}

/// A face's PostScript name, or the empty string when it has none (see
/// `family_name_or_empty`).
fn postscript_name_or_empty(font: &CTFont) -> String {
    use core_foundation::string::{CFString, CFStringRef};

    extern "C" {
        fn CTFontCopyPostScriptName(font: core_text::font::CTFontRef) -> CFStringRef;
    }
    // SAFETY: `font` is a live CTFont. The result follows the create rule
    // and is null when the face has no PostScript name.
    unsafe {
        let name = CTFontCopyPostScriptName(font.as_concrete_TypeRef());
        if name.is_null() {
            return String::new();
        }
        CFString::wrap_under_create_rule(name).to_string()
    }
}

pub fn named_font(name: &str, size: f64) -> Option<CTFont> {
    let font = font::new_from_name(name, size).ok()?;
    let want = normalize_name(name);
    if want.is_empty() {
        return None;
    }
    let family = normalize_name(&font.family_name());
    let postscript = normalize_name(&font.postscript_name());
    if family == want || postscript == want || postscript.starts_with(&want) {
        Some(font)
    } else {
        None
    }
}

/// Create a CTFont with the specified family and size.
///
/// `family` may be a raw CSS font-family LIST ("system-ui, -apple-system,
/// sans-serif"): entries are tried in order with generic keywords mapped to
/// real fonts. Before 2026-07-10 the whole list was passed verbatim to
/// new_from_name, which failed on any multi-family value — the renderer
/// painted Helvetica for every styled page regardless of the author's fonts.
pub fn create_font(family: &str, size: f64) -> Result<CTFont, TextError> {
    count_css_list_resolution();
    for fam in family.split(',') {
        let fam = fam.trim().trim_matches('"').trim_matches('\'');
        if fam.is_empty() {
            continue;
        }
        // A face the document itself registered (@font-face) outranks every
        // platform lookup: the family name may exist nowhere else.
        if let Some(cg) = crate::webfonts::lookup(fam, 400, false) {
            return Ok(font::new_from_CGFont(&cg, size));
        }
        let lower = fam.to_ascii_lowercase();
        // Not a family Chrome knows (CB7b): skipped, as an uninstalled font is.
        if lower == "-apple-system" {
            continue;
        }
        if is_system_family(&lower) {
            return Ok(font::new_ui_font_for_language(
                font::kCTFontSystemFontType,
                size,
                None,
            ));
        }
        let mapped = map_generic(&lower, fam);
        let name = if mapped.is_empty() { fam } else { mapped };
        if let Some(f) = named_font(name, size) {
            return Ok(f);
        }
    }
    // Nothing in the list exists here: Chrome's default (standard) font,
    // Times, as layout's chain takes (`FontFamilyChain::serif`); Core Text's
    // own substitute only if that is missing too.
    if let Some(f) = named_font(map_generic("serif", "serif"), size) {
        return Ok(f);
    }
    font::new_from_name(family, size).map_err(|_| TextError::FontNotFound(family.to_string()))
}

/// Create a font with specific weight and style traits
fn create_font_with_traits(
    family: &str,
    size: f64,
    weight: u16,
    italic: bool,
) -> Result<CTFont, TextError> {
    count_css_list_resolution();
    for fam in family.split(',') {
        let fam = fam.trim().trim_matches('"').trim_matches('\'');
        if fam.is_empty() {
            continue;
        }
        // Document-registered face first (see create_font); the registry
        // picks the nearest declared weight/style itself.
        if let Some(cg) = crate::webfonts::lookup(fam, weight, italic) {
            return Ok(font::new_from_CGFont(&cg, size));
        }
        let lower = fam.to_ascii_lowercase();
        // Not a family Chrome knows (CB7b): skipped, as an uninstalled font is.
        if lower == "-apple-system" {
            continue;
        }

        // System font: resolve the real weighted face via kCTFontWeightTrait,
        // as Chrome/Skia do. "-Bold" name variants never exist for it, and the
        // UI-font API alone offers only Regular/Bold — so the previous
        // `weight >= 600` gate shaped 300 as Regular and 600 as Bold.
        if is_system_family(&lower) {
            // An italic system family is the system font's italic (before,
            // it fell through to the name guesses and on to the default
            // serif: Times-Italic where layout measured SF).
            return Ok(if italic {
                create_system_italic_with_weight(size, weight)
            } else {
                create_system_font_with_weight(size, weight)
            });
        }

        let mapped = map_generic(&lower, fam);
        let base = if mapped.is_empty() { fam } else { mapped };

        // The family's own face for this weight/style (CSS matching over
        // the installed faces); the name guesses below are the fallback
        // for PostScript-name inputs ("HelveticaNeue-Light").
        if let Some(f) = family_face(base, size, weight, italic) {
            return Ok(f);
        }

        let mut variants: Vec<String> = Vec::new();
        if weight >= 700 && italic {
            variants.push(format!("{}-BoldItalic", base));
        }
        if weight >= 700 {
            variants.push(format!("{}-Bold", base));
            variants.push(format!("{}Bold", base));
        }
        if italic {
            variants.push(format!("{}-Italic", base));
            variants.push(format!("{}-Oblique", base));
        }
        variants.push(base.to_string());

        for v in &variants {
            if let Some(f) = named_font(v, size) {
                return Ok(f);
            }
        }
    }

    // Nothing in the list exists here: Chrome's default (standard) font in
    // the requested weight and style, as layout's chain measures it (Times
    // Bold for `font: bold 14px -apple-system`; Chrome for Testing 148:
    // Times-Bold, Times-Italic, Times-BoldItalic). `create_font` alone would
    // paint Times-Roman for all three (Prometheus R1 on #1074).
    if let Some(f) = family_face(map_generic("serif", "serif"), size, weight, italic) {
        return Ok(f);
    }
    // Fall back to base font resolution over the same list
    create_font(family, size)
}

thread_local! {
    /// How many times this thread resolved a CSS family list to a face
    /// (`create_font`, `create_font_with_traits`). See `css_list_resolutions`.
    static CSS_LIST_RESOLUTIONS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Number of CSS family-list resolutions this thread has made. Painting a
/// shaped run must not move it: the run names its face, so a list lookup on
/// that path means paint chose a face again (and may choose another one).
pub fn css_list_resolutions() -> u64 {
    CSS_LIST_RESOLUTIONS.with(std::cell::Cell::get)
}

fn count_css_list_resolution() {
    CSS_LIST_RESOLUTIONS.with(|n| n.set(n.get() + 1));
}

/// The faces layout has shaped with, by id, so paint can draw a shaped run
/// with the SAME font object instead of resolving the CSS family list again.
///
/// An id names one face: its PostScript name, the `@font-face` file it came
/// from (0 for a platform font), and the weight and style it was asked for.
/// The last two are in the id because the system font's weights are
/// instances of one variable font and need not differ by name. Fonts are
/// kept per id AND size, as layout created them: the system font picks its
/// optical size from the size it is created at.
mod face_table {
    use super::CTFont;
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};

    /// Bounds what the table keeps alive (a web face holds its file's
    /// bytes). Past it the table starts over; a run whose face is gone is
    /// painted through the family-list path until layout shapes it again.
    const MAX_FONTS: usize = 1024;

    #[derive(Default)]
    pub(super) struct Faces {
        /// `(face id, size bits)` to the font layout shaped with.
        pub(super) fonts: HashMap<(u64, u32), CTFont>,
    }

    pub(super) fn with<R>(f: impl FnOnce(&mut Faces) -> R) -> R {
        static FACES: OnceLock<Mutex<Faces>> = OnceLock::new();
        let mut faces = FACES
            .get_or_init(|| Mutex::new(Faces::default()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        f(&mut faces)
    }

    impl Faces {
        pub(super) fn keep(&mut self, key: (u64, u32), font: &CTFont) {
            if self.fonts.contains_key(&key) {
                return;
            }
            if self.fonts.len() >= MAX_FONTS {
                self.fonts.clear();
                // Shaped runs that layout memoised name faces that are no
                // longer held. A new generation makes it shape them again,
                // which records their faces again.
                crate::webfonts::bump_generation();
            }
            self.fonts.insert(key, font.clone());
        }
    }
}

/// Record `font` as a face a run was shaped with and return its id and
/// PostScript name. `web_face` is `webfonts::face_id` for the family that
/// resolved (0 for a platform font); `weight` and `italic` are the style
/// the face was resolved for.
pub fn intern_face(font: &CTFont, size: f32, web_face: u64, weight: u16, italic: bool) -> (u64, String) {
    use std::hash::{Hash, Hasher};

    let postscript_name = font.postscript_name();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (postscript_name.as_str(), web_face, weight, italic).hash(&mut hasher);
    // 0 is left free to mean "no face".
    let id = hasher.finish().max(1);
    face_table::with(|faces| faces.keep((id, size.to_bits()), font));
    (id, postscript_name)
}

/// The font `intern_face` recorded for `face` at `size`, if it is still held.
pub fn face_font(face: u64, size: f32) -> Option<CTFont> {
    face_table::with(|faces| faces.fonts.get(&(face, size.to_bits())).cloned())
}

/// Rasterize glyphs to bitmaps using Core Text/Core Graphics
pub struct GlyphRasterizer {
    font: CTFont,
    font_size: f32,
    font_weight: u16,
    font_italic: bool,
    /// Draw coverage masks the way Chrome's Skia does on macOS for text
    /// with normal hinting: CoreGraphics font smoothing ON (its outlines
    /// are dilated), black on white in an RGB context, linearised with
    /// `(v * v + 128) / 255` and reduced to luminance
    /// (`SkScalerContext_Mac::generateImage`, `onFilterRec`, Chromium 148).
    /// Off: the undilated outline coverage Skia draws for unhinted text
    /// (`-webkit-font-smoothing: antialiased`, `text-rendering:
    /// geometricPrecision`). See [`GlyphRasterizer::with_dilation`].
    dilate: bool,
    /// The horizontal subpixel phase (0, 0.25, 0.5 or 0.75) glyphs are drawn
    /// at by `rasterize_with_face` (TX3; see [`GlyphRasterizer::with_subpixel`]).
    subpixel_x: f32,
}

/// What CoreGraphics font smoothing does on this machine, probed once as
/// Skia's `SkCTFontGetSmoothBehavior` does (Chromium 148): one glyph drawn
/// white on black into two RGB contexts, smoothing off and on. Smoothed
/// pixels with unequal channels are `Subpixel`; any pixel that differs from
/// the unsmoothed draw is `Some`; otherwise `None` (smoothing turned off in
/// System Settings, or `AppleFontSmoothing` 0). Skia probes with a bundled
/// spider glyph; any outline glyph answers the same question.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SmoothBehavior {
    None,
    Some,
    Subpixel,
}

/// [`SmoothBehavior`] of this machine, probed on first use.
pub fn smooth_behavior() -> SmoothBehavior {
    smooth_probe().0
}

/// The probe's result and how it was reached: `"probed"`, or why it fell
/// back to `None` (the probe font or its glyph did not load). The renderer
/// logs this once, so a silent `None` (every glyph undilated) is visible.
pub fn smooth_probe() -> (SmoothBehavior, &'static str) {
    static PROBE: std::sync::OnceLock<(SmoothBehavior, &'static str)> = std::sync::OnceLock::new();
    *PROBE.get_or_init(probe_smooth_behavior)
}

fn probe_smooth_behavior() -> (SmoothBehavior, &'static str) {
    use std::os::raw::c_void;
    extern "C" {
        fn CTFontDrawGlyphs(
            font: core_text::font::CTFontRef,
            glyphs: *const u16,
            positions: *const CGPoint,
            count: usize,
            context: *mut c_void,
        );
        fn CTFontGetGlyphsForCharacters(
            font: core_text::font::CTFontRef,
            characters: *const u16,
            glyphs: *mut u16,
            count: isize,
        ) -> bool;
    }
    let Ok(font) = font::new_from_name("Helvetica", 16.0) else {
        return (SmoothBehavior::None, "Helvetica did not load: treated as no smoothing");
    };
    let font_ref = font.as_concrete_TypeRef();
    let mut glyph = [0u16; 1];
    let ch = ['W' as u16];
    // SAFETY: one character in, one glyph out; the font outlives the call.
    if !unsafe { CTFontGetGlyphsForCharacters(font_ref, ch.as_ptr(), glyph.as_mut_ptr(), 1) } {
        return (SmoothBehavior::None, "Helvetica has no 'W': treated as no smoothing");
    }
    let draw = |smooth: bool| -> Vec<u8> {
        let space = CGColorSpace::create_device_rgb();
        let info = core_graphics::base::kCGImageAlphaNoneSkipFirst
            | core_graphics::base::kCGBitmapByteOrder32Little;
        let mut ctx = CGContext::create_bitmap_context(None, 16, 16, 8, 16 * 4, &space, info);
        ctx.set_rgb_fill_color(0.0, 0.0, 0.0, 1.0);
        ctx.fill_rect(CGRect::new(&CGPoint::new(0.0, 0.0), &CGSize::new(16.0, 16.0)));
        ctx.set_allows_font_smoothing(true);
        ctx.set_should_smooth_fonts(smooth);
        ctx.set_should_antialias(true);
        ctx.set_gray_fill_color(1.0, 1.0);
        let at = CGPoint::new(0.0, 3.0);
        // SAFETY: a live context and font, one glyph.
        unsafe { CTFontDrawGlyphs(font_ref, glyph.as_ptr(), &at, 1, ctx.as_ptr() as *mut c_void) };
        ctx.data()[..16 * 16 * 4].to_vec()
    };
    let (plain, smoothed) = (draw(false), draw(true));
    (smooth_behavior_of(&plain, &smoothed), "probed")
}

/// The decision of [`probe_smooth_behavior`] from its two BGRX bitmaps.
pub fn smooth_behavior_of(plain: &[u8], smoothed: &[u8]) -> SmoothBehavior {
    let mut behavior = SmoothBehavior::None;
    for (p, s) in plain.chunks_exact(4).zip(smoothed.chunks_exact(4)) {
        if s[0] != s[1] || s[1] != s[2] {
            return SmoothBehavior::Subpixel;
        }
        if p[..3] != s[..3] {
            behavior = SmoothBehavior::Some;
        }
    }
    behavior
}

/// The coverage mask (`width * height` bytes, top row first) of `glyph` of
/// `font_ref` drawn at `origin` (CG coordinates, bottom-left origin).
///
/// Undilated: white on black in a device-gray context with font smoothing
/// off, so coverage comes from the outline alone (9a9f1872; Skia's path for
/// unhinted A8 glyphs).
///
/// Dilated: Skia's path for hinted A8 glyphs on a Mac where smoothing has an
/// effect (Chromium 148 `SkScalerContext_Mac::generateImage`): black on white
/// in a device-RGB context with smoothing on, each channel linearised with
/// `(v * v + 128) / 255` (CoreGraphics' smoothed output carries a 2.0
/// coverage gamma), then `255 - SkComputeLuminance` of the result.
unsafe fn draw_coverage_mask(
    font_ref: core_text::font::CTFontRef,
    glyph: u16,
    origin: CGPoint,
    width: u32,
    height: u32,
    dilate: bool,
    subpixel: bool,
) -> Vec<u8> {
    use std::os::raw::c_void;
    extern "C" {
        fn CTFontDrawGlyphs(
            font: core_text::font::CTFontRef,
            glyphs: *const u16,
            positions: *const CGPoint,
            count: usize,
            context: *mut c_void,
        );
        // Subpixel controls: CoreGraphics grid-fits glyph origins by
        // default, which rounds away a fractional origin; quantization must
        // be off for it to survive.
        fn CGContextSetAllowsFontSubpixelPositioning(c: *mut c_void, allows: bool);
        fn CGContextSetShouldSubpixelPositionFonts(c: *mut c_void, should: bool);
        fn CGContextSetAllowsFontSubpixelQuantization(c: *mut c_void, allows: bool);
        fn CGContextSetShouldSubpixelQuantizeFonts(c: *mut c_void, should: bool);
    }
    let (w, h) = (width as usize, height as usize);
    let glyphs = [glyph];
    if !dilate {
        let color_space = CGColorSpace::create_device_gray();
        let mut context = CGContext::create_bitmap_context(None, w, h, 8, w, &color_space, 0);
        let ctx = context.as_ptr() as *mut c_void;
        if subpixel {
            CGContextSetAllowsFontSubpixelPositioning(ctx, true);
            CGContextSetShouldSubpixelPositionFonts(ctx, true);
            CGContextSetAllowsFontSubpixelQuantization(ctx, false);
            CGContextSetShouldSubpixelQuantizeFonts(ctx, false);
        }
        context.set_allows_antialiasing(true);
        context.set_should_antialias(true);
        // Font smoothing DILATES the outline (~0.3px per side, ~0.6px on
        // top; n33 on Ahem): off here, the outline alone.
        context.set_allows_font_smoothing(false);
        context.set_should_smooth_fonts(false);
        context.set_gray_fill_color(0.0, 1.0);
        context.fill_rect(CGRect::new(&CGPoint::new(0.0, 0.0), &CGSize::new(w as CGFloat, h as CGFloat)));
        context.set_gray_fill_color(1.0, 1.0);
        CTFontDrawGlyphs(font_ref, glyphs.as_ptr(), &origin, 1, ctx);
        return context.data()[..w * h].to_vec();
    }
    let color_space = CGColorSpace::create_device_rgb();
    // kCGImageAlphaNoneSkipFirst | kCGBitmapByteOrder32Little: BGRX bytes.
    let info = core_graphics::base::kCGImageAlphaNoneSkipFirst
        | core_graphics::base::kCGBitmapByteOrder32Little;
    let mut context = CGContext::create_bitmap_context(None, w, h, 8, w * 4, &color_space, info);
    let ctx = context.as_ptr() as *mut c_void;
    if subpixel {
        CGContextSetAllowsFontSubpixelPositioning(ctx, true);
        CGContextSetShouldSubpixelPositionFonts(ctx, true);
        CGContextSetAllowsFontSubpixelQuantization(ctx, false);
        CGContextSetShouldSubpixelQuantizeFonts(ctx, false);
    }
    context.set_allows_antialiasing(true);
    context.set_should_antialias(true);
    context.set_allows_font_smoothing(true);
    context.set_should_smooth_fonts(true);
    context.set_rgb_fill_color(1.0, 1.0, 1.0, 1.0);
    context.fill_rect(CGRect::new(&CGPoint::new(0.0, 0.0), &CGSize::new(w as CGFloat, h as CGFloat)));
    context.set_rgb_fill_color(0.0, 0.0, 0.0, 1.0);
    CTFontDrawGlyphs(font_ref, glyphs.as_ptr(), &origin, 1, ctx);
    let data = context.data();
    let linear = |v: u8| -> u32 { ((v as u32) * (v as u32) + 128) / 255 };
    let mut mask = Vec::with_capacity(w * h);
    for px in data[..w * h * 4].chunks_exact(4) {
        let (b, g, r) = (px[0], px[1], px[2]);
        // rgb_to_a8: 0xFF - each linearised channel, then
        // SkComputeLuminance (54 R + 183 G + 19 B) >> 8.
        let (r, g, b) = (255 - linear(r), 255 - linear(g), 255 - linear(b));
        mask.push(((r * 54 + g * 183 + b * 19) >> 8) as u8);
    }
    mask
}

/// A rasterized glyph: `(bitmap, width, height, advance, bearing_x,
/// bearing_y)` (BITMAP-EDGE and INTEGER-BASELINE contracts, see
/// `GlyphRasterizer::rasterize_char`). The bitmap is a coverage mask
/// (`width * height` bytes) or premultiplied RGBA (`* 4`), as the
/// [`GlyphRaster`] it comes in says.
pub type Raster = (Vec<u8>, u32, u32, f32, f32, f32);

/// A glyph on the path its face takes: a coverage mask the renderer tints,
/// or a colour face's own artwork.
pub enum GlyphRaster {
    Mask(Raster),
    Color(Raster),
}

impl GlyphRaster {
    /// The advance the glyph carries: layout's advance for the character
    /// (`select_face` decided one face for both).
    pub fn advance(&self) -> f32 {
        match self {
            GlyphRaster::Mask(r) | GlyphRaster::Color(r) => r.3,
        }
    }
}

impl GlyphRasterizer {
    /// The same rasterizer drawing dilated (smoothed) masks when `on`
    /// (see the `dilate` field).
    pub fn with_dilation(mut self, on: bool) -> Self {
        self.dilate = on;
        self
    }

    /// The same rasterizer drawing at horizontal subpixel phase `x`
    /// (`rasterize_with_face`, and the fallback face it may take): the
    /// quarter-pixel phase Skia rounds a glyph origin to (TX3).
    pub fn with_subpixel(mut self, x: f32) -> Self {
        self.subpixel_x = x;
        self
    }

    /// Create a new glyph rasterizer for a font
    pub fn new(family: &str, size: f64) -> Result<Self, TextError> {
        let font = create_font(family, size)?;
        Ok(Self { 
            font,
            font_size: size as f32,
            font_weight: 400,
            font_italic: false,
            dilate: false,
            subpixel_x: 0.0,
        })
    }
    
    /// Create with default system font
    pub fn with_size(size: f32) -> Self {
        let font = create_font("Helvetica", size as f64)
            .or_else(|_| create_font("Arial", size as f64))
            .unwrap_or_else(|_| font::new_from_name("Helvetica", size as f64).unwrap());
        Self { 
            font,
            font_size: size,
            font_weight: 400,
            font_italic: false,
            dilate: false,
            subpixel_x: 0.0,
        }
    }
    
    /// Create with specific weight and style
    pub fn with_style(family: &str, size: f32, weight: u16, italic: bool) -> Self {
        let font = create_font_with_traits(family, size as f64, weight, italic)
            .or_else(|_| create_font_with_traits("Helvetica", size as f64, weight, italic))
            .unwrap_or_else(|_| font::new_from_name("Helvetica", size as f64).unwrap());
        Self {
            font,
            font_size: size,
            font_weight: weight,
            font_italic: italic,
            dilate: false,
            subpixel_x: 0.0,
        }
    }
    
    /// A rasterizer for a face layout already chose (`face_font`), drawn at
    /// `size`. No family lookup happens: when `size` is the size the face
    /// was created at, the font is used as it is.
    pub fn for_face(font: CTFont, size: f32) -> Self {
        let font = if (font.pt_size() as f32 - size).abs() > f32::EPSILON {
            font.clone_with_font_size(size as f64)
        } else {
            font
        };
        Self {
            font,
            font_size: size,
            font_weight: 400,
            font_italic: false,
            dilate: false,
            subpixel_x: 0.0,
        }
    }

    /// Get font weight
    pub fn weight(&self) -> u16 {
        self.font_weight
    }
    
    /// Get whether font is italic
    pub fn is_italic(&self) -> bool {
        self.font_italic
    }
    
    /// Rasterize a character to an alpha bitmap using Core Graphics
    ///
    /// Returns (bitmap, width, height, advance, bearing_x, bearing_y).
    /// BITMAP-EDGE CONTRACT: `bearing_x`/`bearing_y` position the returned
    /// bitmap's top-left corner relative to (pen, baseline) — the bitmap's
    /// 2px AA padding is already folded in, so a caller places the bitmap at
    /// `(pen + bearing_x, baseline - bearing_y)` with no further adjustment.
    /// Rasterize `ch` with its ink shifted right by `subpixel_x` of a pixel.
    ///
    /// FRACTION OWNERSHIP (load-bearing — see PR body): the horizontal
    /// fraction of the destination position is consumed HERE, by drawing the
    /// glyph at a shifted origin inside the bitmap. The returned `bearing_x`
    /// is deliberately the UNSHIFTED nominal bearing, so the caller must
    /// place this bitmap at `floor(dest_x) + bearing_x`. Adding the fraction
    /// again at placement double-applies it.
    ///
    /// `subpixel_x` is clamped to [0, 1). Passing 0.0 keeps the bitmap the
    /// same WIDTH as before, but NOT byte-identical to the pre-subpixel tree:
    /// this function also enables CGContext subpixel positioning, which
    /// changes grid-fitting at every phase including 0.
    pub fn rasterize_char(
        &self,
        ch: char,
        subpixel_x: f32,
    ) -> Option<(Vec<u8>, u32, u32, f32, f32, f32)> {
        let subpixel_x = if subpixel_x.is_finite() {
            subpixel_x.clamp(0.0, 1.0 - f32::EPSILON)
        } else {
            0.0
        };
        // Get glyph for character
        let chars: [u16; 1] = [ch as u16];
        let mut glyphs: [u16; 1] = [0];
        
        unsafe {
            use core_text::font::CTFontRef;
            
            // Get the raw CTFont reference
            let font_ref = self.font.as_concrete_TypeRef();
            
            // Get glyph ID for the character
            extern "C" {
                fn CTFontGetGlyphsForCharacters(
                    font: CTFontRef,
                    characters: *const u16,
                    glyphs: *mut u16,
                    count: isize,
                ) -> bool;
            }

            let success = CTFontGetGlyphsForCharacters(
                font_ref,
                chars.as_ptr(),
                glyphs.as_mut_ptr(),
                1,
            );
            
            if !success || glyphs[0] == 0 {
                // Fallback for characters without glyphs
                return self.rasterize_fallback(ch);
            }

            self.rasterize_glyph_id(glyphs[0], subpixel_x)
        }
    }

    /// Rasterize glyph `glyph` of THIS face: no character lookup, and no
    /// fallback face. A shaped run names its glyphs by id, so paint draws
    /// exactly what layout measured. `rasterize_char` ends here once it has
    /// found the character's glyph, so both entries return the same bitmap
    /// for the same glyph. Same return tuple and contracts as
    /// `rasterize_char`.
    pub fn rasterize_glyph_id(
        &self,
        glyph: u16,
        subpixel_x: f32,
    ) -> Option<(Vec<u8>, u32, u32, f32, f32, f32)> {
        let subpixel_x = if subpixel_x.is_finite() {
            subpixel_x.clamp(0.0, 1.0 - f32::EPSILON)
        } else {
            0.0
        };
        let glyphs: [u16; 1] = [glyph];

        unsafe {
            use core_text::font::CTFontRef;

            let font_ref = self.font.as_concrete_TypeRef();

            extern "C" {
                fn CTFontGetAdvancesForGlyphs(
                    font: CTFontRef,
                    orientation: u32,
                    glyphs: *const u16,
                    advances: *mut CGSize,
                    count: isize,
                ) -> f64;
                
                fn CTFontGetBoundingRectsForGlyphs(
                    font: CTFontRef,
                    orientation: u32,
                    glyphs: *const u16,
                    bounding_rects: *mut CGRect,
                    count: isize,
                ) -> CGRect;
            }
            
            // Get glyph advance
            let mut advance_size = CGSize::new(0.0, 0.0);
            CTFontGetAdvancesForGlyphs(
                font_ref,
                0, // kCTFontOrientationHorizontal
                glyphs.as_ptr(),
                &mut advance_size,
                1,
            );
            
            // Get glyph bounding rect
            let mut bounds = CGRect::new(&CGPoint::new(0.0, 0.0), &CGSize::new(0.0, 0.0));
            CTFontGetBoundingRectsForGlyphs(
                font_ref,
                0,
                glyphs.as_ptr(),
                &mut bounds,
                1,
            );
            
            // Calculate bitmap dimensions with padding
            let padding = 2.0;
            // One extra column ONLY when the ink is shifted, so a phase-0
            // rasterization stays byte-identical to the pre-subpixel result.
            // TX3: the pen sits on a whole column of the bitmap and the
            // outline at its own fractional offset plus the phase, as Skia
            // draws it; the bearing is then a whole pixel, so the bitmap is
            // placed on the device grid and sampled texel for pixel. (The
            // outline used to start on a whole column, with the fraction in
            // the bearing: the quad was sampled between texels.)
            let left = bounds.origin.x.floor();
            let width = ((bounds.origin.x + bounds.size.width + subpixel_x as f64).ceil() - left
                + padding * 2.0)
                .max(4.0) as u32;
            let (draw_y, height, bearing_y) = baseline_seat(&bounds, padding);

            // Calculate position to draw glyph
            // Origin is at bottom-left, glyph origin needs adjustment
            let x = padding - left + subpixel_x as f64;
            // INTEGER-BASELINE CONTRACT: `draw_y` is a whole CG row (see
            // baseline_seat) so the outline is rasterized at vertical phase 0.
            let y = draw_y;
            let bitmap = draw_coverage_mask(font_ref, glyphs[0], CGPoint::new(x, y), width, height, self.dilate, true);

            let advance = advance_size.width as f32;
            // BITMAP-EDGE CONTRACT: the returned bearings position the padded
            // bitmap's top-left corner relative to (pen, baseline) — the ink
            // sits `padding` inside the bitmap, so the padding must be folded
            // in HERE. Returning the outline's bounds while shipping a padded
            // bitmap seated every glyph on every page (+2,+2)px (n30: 'a' ink
            // at x=11 for pen x=8, poking past lba001's 1ch cover).
            // `bearing_y` comes from baseline_seat: the exact integer row
            // count from the bitmap top to the baseline row it was drawn on
            // (INTEGER-BASELINE CONTRACT).
            let bearing_x = (left - padding) as f32;

            Some((bitmap, width, height, advance, bearing_x, bearing_y))
        }
    }

    /// Fallback rasterization for characters without glyphs
    fn rasterize_fallback(&self, ch: char) -> Option<(Vec<u8>, u32, u32, f32, f32, f32)> {
        // The face layout shaped this character with (`fallback_face_for`).
        if let Some((fallback_font, glyph)) = fallback_face_for(&self.font, ch) {
            if let Some(result) = self.rasterize_glyph_with_font(&fallback_font, glyph) {
                return Some(result);
            }
        }

        // No fallback found - return transparent placeholder
        let (width, height) = estimate_glyph_size(ch, self.font_size);
        let width = width.max(4);
        let height = height.max(4);
        let bitmap = vec![0u8; (width * height) as usize];
        
        let advance = self.font_size * width_factor(ch);
        let bearing_y = self.font_size * 0.8;
        
        Some((bitmap, width, height, advance, 0.0, bearing_y))
    }
    
    /// Rasterize glyph `glyph` of a specific font (for fallback)
    fn rasterize_glyph_with_font(&self, font: &CTFont, glyph: u16) -> Option<(Vec<u8>, u32, u32, f32, f32, f32)> {
        let glyphs: [u16; 1] = [glyph];

        unsafe {
            use core_text::font::CTFontRef;

            extern "C" {
                fn CTFontGetAdvancesForGlyphs(
                    font: CTFontRef,
                    orientation: u32,
                    glyphs: *const u16,
                    advances: *mut CGSize,
                    count: isize,
                ) -> f64;
                
                fn CTFontGetBoundingRectsForGlyphs(
                    font: CTFontRef,
                    orientation: u32,
                    glyphs: *const u16,
                    bounding_rects: *mut CGRect,
                    count: isize,
                ) -> CGRect;
            }
            
            let font_ref = font.as_concrete_TypeRef();

            let mut advance_size = CGSize::new(0.0, 0.0);
            CTFontGetAdvancesForGlyphs(
                font_ref,
                0,
                glyphs.as_ptr(),
                &mut advance_size,
                1,
            );
            
            let mut bounds = CGRect::new(&CGPoint::new(0.0, 0.0), &CGSize::new(0.0, 0.0));
            CTFontGetBoundingRectsForGlyphs(
                font_ref,
                0,
                glyphs.as_ptr(),
                &mut bounds,
                1,
            );
            
            let padding = 2.0;
            // TX3, as rasterize_glyph_id: whole-pixel bearing, outline at its
            // own fraction plus the rasterizer's phase.
            let left = bounds.origin.x.floor();
            let sx = self.subpixel_x as f64;
            let width = ((bounds.origin.x + bounds.size.width + sx).ceil() - left + padding * 2.0)
                .max(4.0) as u32;
            let (draw_y, height, bearing_y) = baseline_seat(&bounds, padding);

            let draw_x = padding - left + sx;
            // The fallback face is drawn as the primary one is (the same
            // dilation), so it does not paint lighter or heavier.
            let bitmap = draw_coverage_mask(font_ref, glyph, CGPoint::new(draw_x, draw_y), width, height, self.dilate, sx > 0.0);

            // The face's advance as layout takes it (`glyph_advance`): an
            // Apple emoji face's is Chrome's, not Core Text's tracked one
            // (#1072 R1 nit 3).
            let advance = apple_emoji_advance(font).unwrap_or(advance_size.width as f32);
            // BITMAP-EDGE CONTRACT (see rasterize_char): bearings place the
            // padded bitmap, not the outline. bearing_y is baseline_seat's
            // integer row count (INTEGER-BASELINE CONTRACT).
            let bearing_x = (left - padding) as f32;

            Some((bitmap, width, height, advance, bearing_x, bearing_y))
        }
    }

    /// Rasterize a color glyph (emoji) to a premultiplied **RGBA** bitmap.
    ///
    /// The grayscale path (`rasterize_char`) draws into a device-gray context
    /// and the renderer tints the coverage mask with the text color — correct
    /// for outline fonts, but for a color-bitmap font (Apple Color Emoji, sbix)
    /// it collapses the artwork into a flat tinted blob (image-gallery's emoji
    /// rendered as solid squares). CoreText's `CTFontDrawGlyphs` renders the
    /// real color artwork when the destination is a device-RGB context, so we
    /// draw into an RGBA context and hand the renderer straight color pixels.
    ///
    /// Returns `(rgba, width, height, advance, bearing_x, bearing_y)` where
    /// `rgba` is `width*height*4` premultiplied bytes, or `None` if the char
    /// has no glyph in any color-capable fallback font.
    pub fn rasterize_char_color(&self, ch: char) -> Option<Raster> {
        let font = self.resolve_color_font(ch)?;
        self.rasterize_char_color_with_font(&font, ch)
    }

    /// THE face decision (`select_face`) for `ch` in this rasterizer's own
    /// font, for a caller that has no record from layout (the legacy text
    /// commands without advances). `variation` is the selector after `ch`.
    pub fn select_face(&self, ch: char, variation: Variation) -> GlyphFace {
        select_face(&self.font, ch, variation, color_glyph_id(&self.font, ch).is_some())
    }

    /// The glyph for `ch` from the face `face` names (layout's recorded
    /// decision, or `select_face`'s for a legacy caller), on the path that
    /// face takes: a colour face's artwork (`GlyphRaster::Color`) or a
    /// coverage mask (`GlyphRaster::Mask`). Nothing here decides a face: a
    /// `Fallback` resolves through `fallback_face_for_presentation`, the
    /// same lookup layout measured with, so the advance in the result is
    /// layout's advance. `None` for an `Ignorable` (nothing is drawn) and
    /// for a character no face has.
    pub fn rasterize_with_face(&self, ch: char, face: GlyphFace) -> Option<GlyphRaster> {
        match face {
            GlyphFace::Ignorable => None,
            GlyphFace::Primary => {
                if is_color_face(&self.font) {
                    self.rasterize_char_color_with_font(&self.font, ch).map(GlyphRaster::Color)
                } else {
                    // The common path, as it always was (rasterize_char
                    // falls back by itself should this font lack the
                    // glyph: a face the record was not made for).
                    self.rasterize_char(ch, self.subpixel_x).map(GlyphRaster::Mask)
                }
            }
            GlyphFace::Fallback { emoji } => {
                let (font, glyph) = fallback_face_for_presentation(&self.font, ch, emoji)?;
                if is_color_face(&font) {
                    self.rasterize_char_color_with_font(&font, ch).map(GlyphRaster::Color)
                } else {
                    self.rasterize_glyph_with_font(&font, glyph).map(GlyphRaster::Mask)
                }
            }
        }
    }

    /// Find a font containing a glyph for `ch`, preferring color-emoji fonts.
    fn resolve_color_font(&self, ch: char) -> Option<CTFont> {
        let has_glyph = |font: &CTFont| -> bool { color_glyph_id(font, ch).is_some() };
        // Emoji chars won't resolve in the instance (text) font, so go straight
        // to the color fonts; keep the instance font first for symbol glyphs
        // that a text font may cover.
        for name in ["Apple Color Emoji", "Apple Symbols"] {
            if let Ok(f) = font::new_from_name(name, self.font_size as f64) {
                if has_glyph(&f) {
                    return Some(f);
                }
            }
        }
        if has_glyph(&self.font) {
            return Some(self.font.clone());
        }
        None
    }

    fn rasterize_char_color_with_font(
        &self,
        font: &CTFont,
        ch: char,
    ) -> Option<(Vec<u8>, u32, u32, f32, f32, f32)> {
        // Astral chars (all pictographic emoji live above U+FFFF) need a UTF-16
        // surrogate pair — `ch as u16` truncates them, which is why emoji glyph
        // lookup silently failed. color_glyph_id encodes UTF-16 correctly.
        let glyph = color_glyph_id(font, ch)?;
        self.rasterize_glyph_color_with_font(font, glyph)
    }

    /// The emoji sequence `cluster` (EM1) as the ONE glyph its emoji face
    /// forms it with ([`emoji_cluster_glyph`]), from the face layout measured
    /// its head character in. `None` when the face does not form it; the
    /// caller then draws the head alone, as layout measured it.
    pub fn rasterize_emoji_cluster(&self, cluster: &str, face: GlyphFace) -> Option<GlyphRaster> {
        let head = cluster.chars().next()?;
        let font = match face {
            GlyphFace::Fallback { emoji: true } => fallback_face_for_presentation(&self.font, head, true)?.0,
            // The run's own face draws its sequences only when it is an
            // Apple emoji face; any other primary draws the head alone.
            GlyphFace::Primary if apple_emoji_advance(&self.font).is_some() => self.font.clone(),
            _ => return None,
        };
        let (glyph, _) = emoji_cluster_glyph(&font, cluster)?;
        self.rasterize_glyph_color_with_font(&font, glyph).map(GlyphRaster::Color)
    }

    fn rasterize_glyph_color_with_font(
        &self,
        font: &CTFont,
        glyph: u16,
    ) -> Option<(Vec<u8>, u32, u32, f32, f32, f32)> {
        let glyphs: [u16; 1] = [glyph];

        unsafe {
            use core_text::font::CTFontRef;
            use std::os::raw::c_void;

            extern "C" {
                fn CTFontGetAdvancesForGlyphs(
                    font: CTFontRef,
                    orientation: u32,
                    glyphs: *const u16,
                    advances: *mut CGSize,
                    count: isize,
                ) -> f64;
                fn CTFontGetBoundingRectsForGlyphs(
                    font: CTFontRef,
                    orientation: u32,
                    glyphs: *const u16,
                    bounding_rects: *mut CGRect,
                    count: isize,
                ) -> CGRect;
                fn CTFontDrawGlyphs(
                    font: CTFontRef,
                    glyphs: *const u16,
                    positions: *const CGPoint,
                    count: usize,
                    context: *mut c_void,
                );
            }

            let font_ref = font.as_concrete_TypeRef();

            let mut advance_size = CGSize::new(0.0, 0.0);
            CTFontGetAdvancesForGlyphs(font_ref, 0, glyphs.as_ptr(), &mut advance_size, 1);

            let mut bounds = CGRect::new(&CGPoint::new(0.0, 0.0), &CGSize::new(0.0, 0.0));
            CTFontGetBoundingRectsForGlyphs(font_ref, 0, glyphs.as_ptr(), &mut bounds, 1);

            let padding = 2.0;
            let width = (bounds.size.width.ceil() + padding * 2.0).max(4.0) as u32;
            let (draw_y, height, bearing_y) = baseline_seat(&bounds, padding);

            // Device-RGB, premultiplied-last (RGBA). CTFontDrawGlyphs renders
            // the color-bitmap artwork here instead of a coverage mask.
            let color_space = CGColorSpace::create_device_rgb();
            const KCG_IMAGE_ALPHA_PREMULTIPLIED_LAST: u32 = 1;
            let mut context = CGContext::create_bitmap_context(
                None,
                width as usize,
                height as usize,
                8,
                width as usize * 4,
                &color_space,
                KCG_IMAGE_ALPHA_PREMULTIPLIED_LAST,
            );

            context.set_allows_antialiasing(true);
            context.set_should_antialias(true);

            let draw_x = padding - bounds.origin.x;
            let position = CGPoint::new(draw_x, draw_y);
            CTFontDrawGlyphs(font_ref, glyphs.as_ptr(), &position, 1, context.as_ptr() as *mut c_void);

            let data = context.data();
            let rgba: Vec<u8> = std::slice::from_raw_parts(
                data.as_ptr() as *const u8,
                (width * height * 4) as usize,
            )
            .to_vec();

            // Layout's advance for the same face (`glyph_advance`), so the
            // glyph is centred on the width layout gave it.
            let advance = apple_emoji_advance(font).unwrap_or(advance_size.width as f32);
            // BITMAP-EDGE CONTRACT (see rasterize_char): bearings place the
            // padded bitmap, not the outline. bearing_y is baseline_seat's
            // integer row count (INTEGER-BASELINE CONTRACT).
            let bearing_x = (bounds.origin.x - padding) as f32;

            Some((rgba, width, height, advance, bearing_x, bearing_y))
        }
    }

    /// Get glyph ID for a character
    pub fn get_glyph(&self, ch: char) -> u16 {
        ch as u16
    }
    
    /// Rasterize a glyph by character (we use char code as ID)
    pub fn rasterize(&self, glyph: u16) -> Option<(Vec<u8>, u32, u32, f32, f32, f32)> {
        if let Some(ch) = char::from_u32(glyph as u32) {
            self.rasterize_char(ch, 0.0)
        } else {
            None
        }
    }
}

/// Vertical seat of a padded glyph bitmap — the INTEGER-BASELINE CONTRACT.
///
/// Returns `(draw_y, height, bearing_y)`: the CG y at which to draw the glyph
/// origin, the bitmap height, and the exact number of rows from the bitmap's
/// TOP edge down to the baseline row.
///
/// WHY (the intra-word "wave", 2026-08-26): a CoreGraphics bitmap context has
/// its origin at the BOTTOM-left, so the baseline is anchored from the
/// bitmap's bottom — but `bearing_y` is consumed from the TOP. The previous
/// seat drew the baseline at the fractional CG row `padding - origin.y` in a
/// bitmap `ceil(h) + 2*padding` tall and reported `bearing_y = origin.y + h +
/// padding`, i.e. where the ink top is. The bitmap TOP is `ceil(h) - h`
/// higher than that. So every glyph seated `ceil(h) - h` px LOW — a 0..1px
/// error keyed to each glyph's OWN ink height. 'l', 'o' and 'g' on one line
/// each landed on a different fraction and were then bilinearly resampled
/// at that fraction: that is the wave. Capitals share a height, which is why
/// a line of initials looks straight and the wave grows with a word's
/// letter variety.
///
/// FIX: put the baseline on a WHOLE CG row (so CoreText rasterizes the
/// outline at vertical phase 0 — what Skia does for horizontal text), size
/// the bitmap from that row, and report the bearing as the exact integer row
/// count from the top. A caller that snaps its baseline to a device row then
/// paints every glyph on the line pixel-aligned, with no vertical resampling.
fn baseline_seat(bounds: &CGRect, padding: f64) -> (f64, u32, f32) {
    let draw_y = (padding - bounds.origin.y).ceil().max(0.0);
    let ink_top = draw_y + bounds.origin.y + bounds.size.height;
    let height = (ink_top + padding).ceil().max(4.0) as u32;
    let bearing_y = (height as f64 - draw_y) as f32;
    (draw_y, height, bearing_y)
}

/// Estimate glyph size based on character and font size
fn estimate_glyph_size(ch: char, font_size: f32) -> (u32, u32) {
    let height = (font_size * 1.2).ceil() as u32;
    let width = (font_size * width_factor(ch)).ceil() as u32;
    (width.max(1), height.max(1))
}

/// Get approximate width factor for a character
fn width_factor(ch: char) -> f32 {
    match ch {
        ' ' => 0.3,
        'i' | 'l' | '!' | '|' | '\'' | '.' | ',' | ':' | ';' => 0.3,
        'f' | 'j' | 't' | 'r' => 0.4,
        'm' | 'w' | 'M' | 'W' | '@' | '%' => 0.9,
        _ if ch.is_ascii_uppercase() => 0.7,
        _ if ch.is_ascii() => 0.55,
        _ => 0.9, // CJK and other wide characters
    }
}

/// Resolve the glyph id for `ch` in `font`, encoding it as UTF-16 so astral
/// codepoints (emoji, all above U+FFFF) work. `CTFontGetGlyphsForCharacters`
/// maps a surrogate pair to the real glyph in the high-surrogate slot and 0 in
/// the low slot, so we read `glyphs[0]`. Returns `None` if the font has no
/// glyph for the char.
fn color_glyph_id(font: &CTFont, ch: char) -> Option<u16> {
    let mut buf = [0u16; 2];
    let units = ch.encode_utf16(&mut buf).len();
    let mut glyphs = [0u16; 2];
    unsafe {
        use core_text::font::CTFontRef;
        extern "C" {
            fn CTFontGetGlyphsForCharacters(
                font: CTFontRef,
                characters: *const u16,
                glyphs: *mut u16,
                count: isize,
            ) -> bool;
        }
        let ok = CTFontGetGlyphsForCharacters(
            font.as_concrete_TypeRef(),
            buf.as_ptr(),
            glyphs.as_mut_ptr(),
            units as isize,
        );
        if ok && glyphs[0] != 0 {
            Some(glyphs[0])
        } else {
            None
        }
    }
}

pub use crate::emoji::is_emoji;

/// Get available system fonts
pub fn get_available_fonts() -> Vec<String> {
    // Return a common list of fonts available on macOS
    vec![
        "Helvetica".to_string(),
        "Helvetica Neue".to_string(),
        "Arial".to_string(),
        "Times New Roman".to_string(),
        "Courier New".to_string(),
        "Georgia".to_string(),
        "Verdana".to_string(),
        "SF Pro".to_string(),
        "SF Mono".to_string(),
        "Menlo".to_string(),
        "Monaco".to_string(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_create_font() {
        let font = create_font("Helvetica", 16.0);
        assert!(font.is_ok(), "Should create Helvetica font");
    }

    /// Core Text hands back a substitute for a name it does not have, so
    /// the chain must keep walking past it. T-RED before `named_font`: the
    /// first, nonexistent family "won" as Helvetica and Menlo was never
    /// reached (WPT overflow-wrap-anywhere-003's `4ch` measured a
    /// proportional "0").
    #[test]
    fn test_chain_walks_past_an_uninstalled_family() {
        let font = create_font("No Such Face n34, Menlo, monospace", 16.0).expect("font");
        assert_eq!(font.family_name(), "Menlo");
        let weighted = create_font_with_traits("No Such Face n34, Menlo", 16.0, 700, false)
            .expect("font");
        assert_eq!(weighted.family_name(), "Menlo");
        // A bare generic still maps straight to its platform face, and those
        // are Chrome's on macOS: Courier and Times, not Menlo and Times New
        // Roman.
        assert_eq!(create_font("monospace", 16.0).expect("font").family_name(), "Courier");
        assert_eq!(create_font("serif", 16.0).expect("font").family_name(), "Times");
        assert_eq!(create_font("sans-serif", 16.0).expect("font").family_name(), "Helvetica");
    }

    /// A downloaded font may have no family name (x.com's does not), and
    /// `CTFont::family_name` panics on it. The ascent of such a face is the
    /// font's own. The fixture is Ahem with its `name` table hidden from the
    /// table directory.
    #[test]
    fn a_face_without_a_family_name_keeps_its_own_ascent() {
        use core_graphics::data_provider::CGDataProvider;
        use core_graphics::font::CGFont;

        let mut bytes = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/Ahem.ttf"
        ))
        .expect("fixture");
        let tables = u16::from_be_bytes([bytes[4], bytes[5]]) as usize;
        let record = (0..tables)
            .map(|i| 12 + 16 * i)
            .find(|&at| &bytes[at..at + 4] == b"name")
            .expect("Ahem has a name table");
        bytes[record..record + 4].copy_from_slice(b"namf");
        let provider = CGDataProvider::from_buffer(std::sync::Arc::new(bytes));
        let cg = CGFont::from_data_provider(provider).expect("decodes");
        let font = font::new_from_CGFont(&cg, 16.0);

        assert_eq!(family_name_or_empty(&font), "");
        assert_eq!(blink_ascent(&font), font.ascent() as f32);
        // The three adjusted families, and one that is not.
        let line = |name: &str| {
            let f = named_font(name, 16.0).expect("installed");
            blink_ascent(&f).round() + (f.descent() as f32).round()
        };
        assert_eq!(line("Times"), 18.0);
        assert_eq!(line("Helvetica"), 18.0);
        assert_eq!(line("Courier"), 18.0);
        let neue = named_font("Helvetica Neue", 16.0).expect("installed");
        assert_eq!(blink_ascent(&neue), neue.ascent() as f32);
    }

    #[test]
    fn test_is_emoji_classification() {
        assert!(is_emoji('🏔'), "mountain is emoji");
        assert!(is_emoji('☕'), "coffee is emoji");
        assert!(is_emoji('✨'), "sparkles is emoji");
        assert!(is_emoji('🎯'), "target is emoji");
        assert!(!is_emoji('A'), "letter is not emoji");
        assert!(!is_emoji('7'), "digit is not emoji");
        assert!(!is_emoji(' '), "space is not emoji");
    }

    #[test]
    fn test_rasterize_color_emoji_is_actually_colored() {
        // The grayscale path collapses a color-bitmap emoji into a flat tinted
        // mask; the color path must yield real RGBA artwork. Assert the emoji
        // rasterizes to a premultiplied RGBA buffer with MORE THAN ONE distinct
        // opaque color (a flat mask would have a single hue).
        let r = GlyphRasterizer::with_style("Helvetica", 48.0, 400, false);
        let out = r.rasterize_char_color('🏔');
        assert!(out.is_some(), "emoji should rasterize via color path");
        let (rgba, w, h, _adv, _bx, _by) = out.unwrap();
        assert_eq!(rgba.len(), (w * h * 4) as usize, "RGBA buffer sized w*h*4");
        let mut hues = std::collections::HashSet::new();
        for px in rgba.chunks_exact(4) {
            if px[3] > 32 {
                // Quantize to shrug off AA noise; count distinct opaque colors.
                hues.insert((px[0] / 32, px[1] / 32, px[2] / 32));
            }
        }
        assert!(
            hues.len() > 1,
            "color emoji must have multiple distinct colors, got {} (flat mask?)",
            hues.len()
        );
    }

    #[test]
    fn test_create_font_css_family_list() {
        // Regression (2026-07-10): a raw CSS family list was passed verbatim
        // to new_from_name, failing on any multi-family value — the renderer
        // painted Helvetica for every styled page.
        let font = create_font("system-ui, -apple-system, sans-serif", 16.0);
        assert!(font.is_ok(), "CSS family list should resolve");
        let font = create_font("\"NoSuchFont-XYZ\", Arial", 16.0);
        assert!(font.is_ok(), "fallback within the list should resolve");
    }

    /// css-fonts-4 §5.2 over the installed faces of a named family, via the
    /// descriptor path Chrome uses — not PostScript-name guessing. T-RED
    /// before n50 on the layout side: `named_font("Georgia")` answered for
    /// weight 700, so bold headings measured with the regular face.
    #[test]
    fn family_face_picks_the_css_weight_not_the_nearest() {
        let bold =
            |f: &CTFont| f.symbolic_traits() & core_text::font_descriptor::kCTFontBoldTrait != 0;
        let georgia_700 = family_face("Georgia", 44.0, 700, false).expect("Georgia");
        assert_eq!(georgia_700.postscript_name(), "Georgia-Bold");
        assert!(bold(&georgia_700));
        // 500 walks DOWN to 400 before it walks up: nearest-weight says Bold.
        let georgia_500 = family_face("Georgia", 44.0, 500, false).expect("Georgia");
        assert_eq!(georgia_500.postscript_name(), "Georgia");
        // 600 walks UP: Georgia has no semibold, so Bold — as Chrome draws h2.
        assert_eq!(
            family_face("Georgia", 44.0, 600, false)
                .unwrap()
                .postscript_name(),
            "Georgia-Bold"
        );
        // Helvetica Neue 600: nearest is Medium, CSS (and Chrome) take Bold.
        let hn_600 = family_face("Helvetica Neue", 20.0, 600, false).expect("Helvetica Neue");
        assert!(bold(&hn_600), "got {}", hn_600.postscript_name());
        // Name guessing could never find these: the bold face has an "MT"
        // suffix, not a "-Bold" one.
        assert_eq!(
            family_face("Arial", 20.0, 700, false)
                .unwrap()
                .postscript_name(),
            "Arial-BoldMT"
        );
        assert_eq!(
            family_face("Times New Roman", 20.0, 700, true)
                .unwrap()
                .postscript_name(),
            "TimesNewRomanPS-BoldItalicMT"
        );
        assert_eq!(
            family_face("Georgia", 20.0, 400, true)
                .unwrap()
                .postscript_name(),
            "Georgia-Italic"
        );
        // A family the machine lacks is a miss, not Helvetica.
        assert!(family_face("No Such Family n50", 20.0, 700, false).is_none());
        // Paint's resolver goes through it for a CSS list.
        let painted = create_font_with_traits("Georgia, serif", 44.0, 700, false).unwrap();
        assert_eq!(painted.postscript_name(), "Georgia-Bold");
    }

    #[test]
    fn test_system_font_bold_face() {
        // The system font has no "-Bold" PostScript variant; weight >= 600
        // must route through the UI-font API and return the emphasized face
        // (the old path silently rasterized bold headings with the regular
        // face, ~6% narrower than Chrome).
        let regular = create_font_with_traits("system-ui", 32.0, 400, false).unwrap();
        let bold = create_font_with_traits("system-ui", 32.0, 700, false).unwrap();
        let (rn, bn) = (regular.postscript_name(), bold.postscript_name());
        assert_ne!(rn, bn, "bold system face must differ from regular");
        // The face is the variable font at wght 700 (CH2.B6c), named
        // `.SFNS-Regular_…_wght2BC0000`, not `.SFNS-Bold`: read its weight.
        use core_text::font_descriptor::TraitAccessors;
        let wt = bold.all_traits().normalized_weight();
        assert!(wt >= 0.3, "a bold face expected, got {bn} (weight trait {wt})");
    }
    
    #[test]
    fn test_system_font_resolves_every_css_weight_to_its_own_face() {
        // The UI-font API exposes only Regular and Bold, so the old
        // `weight >= 600` gate collapsed 100..500 onto .SFNS-Regular and
        // 600..900 onto .SFNS-Bold. Chrome/Skia apply kCTFontWeightTrait.
        // Drives the real resolution path (create_font_with_traits), not a
        // hand-simulated table.
        let faces: Vec<(u16, String)> = [100u16, 200, 300, 400, 500, 600, 700, 800, 900]
            .iter()
            .map(|&w| {
                let f = create_font_with_traits("system-ui", 32.0, w, false).unwrap();
                (w, f.postscript_name())
            })
            .collect();

        // Every step must land on a distinct face — no two CSS weights share one.
        for pair in faces.windows(2) {
            assert_ne!(
                pair[0].1, pair[1].1,
                "weights {} and {} collapsed onto the same face {}",
                pair[0].0, pair[1].0, pair[0].1
            );
        }

        // The specific regression: 300 must be lighter than 400, and 600 must
        // NOT be the same face as 700 (the two ends of the old binary gate).
        let name = |w: u16| faces.iter().find(|(x, _)| *x == w).unwrap().1.clone();
        // The faces are wght-axis instances (CH2.B6c), so their names carry
        // the axis value, not "Light": read the weight trait.
        use core_text::font_descriptor::TraitAccessors;
        let trait_of =
            |w: u16| create_font_with_traits("system-ui", 32.0, w, false).unwrap().all_traits().normalized_weight();
        assert!(
            trait_of(300) < trait_of(400),
            "font-weight:300 must resolve lighter than 400, got {}",
            name(300)
        );
        assert_ne!(
            name(600),
            name(700),
            "semibold and bold must not share a face"
        );
    }

    #[test]
    fn system_font_weights_between_the_named_faces_measure_like_chrome() {
        // CH2.B6c. A kCTFontWeightTrait descriptor snaps the system font to
        // its nearest NAMED instance (Light, Medium, Semibold, Heavy); Chrome
        // sets the variable font's `wght` axis to the CSS weight. Widths are
        // Chromium 143's for this run at 13px (Reduce, 11-05); develop gave
        // 132.84 / 137.42 / 139.51 / 146.56.
        // Measured as layout measures a run: kerned (a CTLine), ligatures off.
        use core_foundation::attributed_string::CFMutableAttributedString;
        use core_foundation::base::CFRange;
        use core_foundation::number::CFNumber;
        use core_foundation::string::CFString;
        use core_text::line::CTLine;
        use core_text::string_attributes::{kCTFontAttributeName, kCTLigatureAttributeName};

        const RUN: &str = "Downloads and Tabs 1";
        for (weight, chrome) in [(300u16, 133.19f64), (500, 137.17), (600, 139.78), (800, 144.98)] {
            let font = create_system_font_with_weight(13.0, weight);
            let mut astr = CFMutableAttributedString::new();
            astr.replace_str(&CFString::new(RUN), CFRange::init(0, 0));
            let range = CFRange::init(0, astr.char_len());
            unsafe {
                astr.set_attribute(range, kCTFontAttributeName, &font);
                astr.set_attribute(range, kCTLigatureAttributeName, &CFNumber::from(0i32));
            }
            let width =
                CTLine::new_with_attributed_string(astr.as_concrete_TypeRef()).get_typographic_bounds().width;
            assert!(
                (width - chrome).abs() <= 0.05,
                "font-weight:{weight} at 13px: {width:.2} px, Chrome {chrome}"
            );
        }
    }

    #[test]
    fn test_ct_weight_trait_table_matches_skia_anchors() {
        // Apple's kCTFontWeight* constants, which is what Skia's table encodes.
        assert_eq!(ct_weight_trait(100), -0.80);
        assert_eq!(ct_weight_trait(300), -0.40);
        assert_eq!(ct_weight_trait(400), 0.00);
        assert_eq!(ct_weight_trait(700), 0.40);
        assert_eq!(ct_weight_trait(900), 0.62);
        // Off-table CSS weights interpolate and stay monotonic.
        let t350 = ct_weight_trait(350);
        assert!(t350 > -0.40 && t350 < 0.0, "350 between Light and Regular");
        assert_eq!(ct_weight_trait(1000), 0.62, "clamped at the top");
    }

    #[test]
    fn test_text_shaper() {
        let shaper = TextShaper::with_system_font(16.0);
        let result = shaper.shape("Hello");
        assert!(result.is_ok());
        let shaped = result.unwrap();
        assert_eq!(shaped.glyphs.len(), 5);
    }
    
    #[test]
    fn test_font_metrics() {
        let shaper = TextShaper::with_system_font(16.0);
        let metrics = shaper.get_metrics();
        assert!(metrics.ascent > 0.0);
    }
    
    #[test]
    fn test_glyph_rasterizer() {
        let rasterizer = GlyphRasterizer::with_size(16.0);
        
        let result = rasterizer.rasterize_char('A', 0.0);
        assert!(result.is_some(), "Should rasterize character");
        
        let (bitmap, width, height, advance, _, _) = result.unwrap();
        assert!(width > 0);
        assert!(height > 0);
        assert!(advance > 0.0);
        assert!(!bitmap.is_empty());
        
        // Check that the bitmap has non-zero values (not all transparent)
        let has_content = bitmap.iter().any(|&b| b > 0);
        assert!(has_content, "Bitmap should have visible content");
    }
    
    #[test]
    fn test_bearing_places_padded_bitmap_ink_at_metrics() {
        // BITMAP-EDGE CONTRACT: a caller places the returned bitmap at
        // (pen + bearing_x, baseline - bearing_y). The ink inside must then
        // land where the font's metrics say — LSB right of the pen, bottom on
        // the baseline. The old code returned OUTLINE bounds for a PADDED
        // bitmap, seating every glyph (+2,+2)px: 'H' ink began ~2.8px right
        // of the pen (true Menlo LSB ~0.8) and ended ~2px below the baseline.
        let rasterizer = GlyphRasterizer::with_style("Menlo", 16.0, 400, false);
        let (bitmap, width, height, advance, bearing_x, bearing_y) =
            rasterizer.rasterize_char('H', 0.0).expect("rasterizes");

        // Ink extents in the bitmap (threshold cuts the AA fringe).
        let mut first_col = None;
        let mut last_row = None;
        for y in 0..height {
            for x in 0..width {
                if bitmap[(y * width + x) as usize] >= 64 {
                    first_col = Some(first_col.map_or(x, |c: u32| c.min(x)));
                    last_row = Some(last_row.map_or(y, |r: u32| r.max(y)));
                }
            }
        }
        let first_col = first_col.expect("H has ink") as f32;
        let last_row = last_row.expect("H has ink") as f32;

        // Placed ink left edge = pen + bearing_x + first_col; Menlo 'H' has a
        // small positive LSB, so this must sit in [-1, 2] px of the pen.
        let ink_left = bearing_x + first_col;
        assert!(
            (-1.0..=2.0).contains(&ink_left),
            "ink left edge {ink_left} px from pen — glyph is mis-seated \
             horizontally (padding not folded into bearing_x?)"
        );

        // 'H' sits ON the baseline: bitmap row `bearing_y` below the bitmap
        // top IS the baseline, so the last ink row must be just above it.
        let baseline_residual = bearing_y - (last_row + 1.0);
        assert!(
            baseline_residual.abs() <= 1.0,
            "ink bottom is {baseline_residual} px from the baseline — glyph \
             is mis-seated vertically (padding not folded into bearing_y?)"
        );

        assert!(advance > 0.0);
    }

    /// Last row (from the top) holding ink at or above `threshold`.
    fn last_ink_row(bitmap: &[u8], width: u32, height: u32, threshold: u8) -> Option<u32> {
        (0..height)
            .rev()
            .find(|&y| (0..width).any(|x| bitmap[(y * width + x) as usize] >= threshold))
    }

    #[test]
    fn integer_baseline_contract_seats_every_flat_glyph_on_the_same_row() {
        // THE WAVE DISCRIMINATOR. Flat-bottomed glyphs of one font at one
        // size all sit ON the baseline, so `bearing_y - (last_ink_row + 1)`
        // must be the SAME number — zero — for every one of them, and
        // `bearing_y` must be a whole row. The old seat reported
        // `origin.y + h + padding` for a bitmap whose top was `ceil(h) - h`
        // higher: 'x' (short) and 'l' (tall) got different fractional
        // bearings and painted on different fractional rows. That per-glyph
        // 0..1px spread IS the intra-word wave; this test fails on it.
        let r = GlyphRasterizer::with_style("Helvetica", 16.0, 400, false);
        let mut residuals = Vec::new();
        for ch in ['H', 'x', 'l', 'n', 'm', 'E', 'z'] {
            let (bitmap, w, h, _adv, _bx, by) = r.rasterize_char(ch, 0.0).expect("rasterizes");
            assert_eq!(by.fract(), 0.0, "{ch:?}: bearing_y {by} is not a whole row");
            // Full-coverage rows only: the AA fringe below a flat bottom is
            // the vertical-phase leak this contract removes, so a fringe row
            // at >=64 would itself be the bug.
            let last = last_ink_row(&bitmap, w, h, 128).expect("has ink") as f32;
            residuals.push((ch, by - (last + 1.0)));
        }
        for (ch, res) in &residuals {
            assert_eq!(
                *res, 0.0,
                "{ch:?}: ink bottom is {res} rows off the baseline row (all: {residuals:?})"
            );
        }
    }

    #[test]
    fn integer_baseline_contract_holds_for_descenders_and_fallback_paths() {
        // Descenders hang BELOW the baseline; the contract still says the
        // bitmap's baseline row is exactly `bearing_y` from the top, so the
        // ink top of 'g' must sit above it by its outline height, whole rows.
        let r = GlyphRasterizer::with_style("Helvetica", 16.0, 400, false);
        // Primary path: descenders and a glyph that floats above the baseline.
        // Fallback path (rasterize_char_with_font): a CJK ideograph Helvetica
        // has no glyph for, so rasterize_char routes through rasterize_fallback.
        for ch in ['g', 'p', 'y', '_', '\u{00B0}', '\u{6F22}'] {
            let (bitmap, w, h, _adv, _bx, by) = r.rasterize_char(ch, 0.0).expect("rasterizes");
            assert!(bitmap.iter().any(|&v| v > 0), "{ch:?}: no ink — fallback font missing?");
            assert_eq!(by.fract(), 0.0, "{ch:?}: bearing_y {by} is not a whole row");
            assert!(by <= h as f32, "{ch:?}: baseline row {by} is below the bitmap ({h})");
            assert_eq!(bitmap.len(), (w * h) as usize);
        }
        // Color path (emoji) shares the seat.
        let (_rgba, _w, h, _adv, _bx, by) =
            r.rasterize_char_color('\u{1F3D4}').expect("color emoji rasterizes");
        assert_eq!(by.fract(), 0.0, "color path bearing_y {by} is not a whole row");
        assert!(by <= h as f32, "color path baseline row {by} is below the bitmap ({h})");
    }

    /// Courier has no `⌘`, `←` or `→`. Chrome draws them from the face the
    /// system cascade gives for Courier, which is Menlo; the fixed list gave
    /// Apple Symbols, whose advances are wider (an `about` `<kbd>` was 22.3
    /// to 22.6px against Chrome's 21.23px).
    #[test]
    fn a_symbol_the_face_lacks_comes_from_the_systems_cascade_face() {
        let courier = named_font("Courier", 16.0).expect("Courier is installed");
        for ch in ['\u{2318}', '\u{2190}', '\u{2192}'] {
            assert!(color_glyph_id(&courier, ch).is_none(), "{ch:?}: Courier has it");
            let (face, glyph) = fallback_face_for(&courier, ch).expect("a fallback face");
            assert_eq!(family_name_or_empty(&face), "Menlo", "{ch:?}");
            assert_eq!(Some(glyph), color_glyph_id(&face, ch), "{ch:?}");
            assert_eq!(face.pt_size(), 16.0, "{ch:?}: the fallback keeps the size");
        }
    }

    /// Paint resolves `-apple-system` as Chrome does: an unknown family,
    /// skipped (CB7b; Chrome for Testing 148, see the layout chain's pin).
    #[test]
    fn create_font_skips_apple_system_like_chrome() {
        let ps = |family: &str| create_font(family, 14.0).expect("a font").postscript_name();
        assert_eq!(ps("-apple-system"), ps("serif"), "alone: the default serif");
        assert_eq!(ps("-apple-system, BlinkMacSystemFont"), ps("system-ui"));
        assert_eq!(ps("BlinkMacSystemFont"), ps("system-ui"));
        assert_eq!(ps("-apple-system, \"Segoe UI\", Helvetica"), "Helvetica");
    }

    /// A bold or italic `-apple-system` (or any all-unknown list) paints in
    /// the weighted default face, as layout measures it. Chrome for Testing
    /// 148 (CSS.getPlatformFontsForNode, 14px): 700 is Times-Bold, italic
    /// Times-Italic, both Times-BoldItalic (Prometheus R1 MED on #1074).
    #[test]
    fn an_all_unknown_list_paints_the_weighted_default_face() {
        let ps = |weight: u16, italic: bool| {
            create_font_with_traits("-apple-system", 14.0, weight, italic)
                .expect("a font")
                .postscript_name()
        };
        assert_eq!(ps(700, false), "Times-Bold");
        assert_eq!(ps(400, true), "Times-Italic");
        assert_eq!(ps(700, true), "Times-BoldItalic");
        assert_eq!(ps(400, false), "Times-Roman");
    }

    /// An italic system family paints the system font's italic, not the
    /// default serif (Prometheus delta R1 MED on #1074). Chrome for Testing
    /// 148 (CSS.getPlatformFontsForNode, 14px): italic system-ui and italic
    /// BlinkMacSystemFont are .SFNS-RegularItalic, italic 700 is
    /// .SFNS-BoldItalic.
    #[test]
    fn an_italic_system_family_paints_the_system_italic() {
        use core_text::font_descriptor;
        for family in ["system-ui", "BlinkMacSystemFont", "-apple-system, system-ui"] {
            let regular = create_font_with_traits(family, 14.0, 400, true).expect("a font");
            assert!(
                !regular.postscript_name().starts_with("Times"),
                "{family} italic is the system font, got {}",
                regular.postscript_name()
            );
            assert_eq!(regular.family_name(), create_font("system-ui", 14.0).unwrap().family_name(), "{family}");
            assert!(regular.symbolic_traits() & font_descriptor::kCTFontItalicTrait != 0, "{family} italic trait");
            let bold = create_font_with_traits(family, 14.0, 700, true).expect("a font");
            assert!(bold.symbolic_traits() & font_descriptor::kCTFontItalicTrait != 0, "{family} 700 italic trait");
            // Bold by its width (a variable face need not set the bold
            // trait): Chrome 72.73 for italic 400 and 78.48 for italic 700.
            let width = |f: &CTFont| {
                use core_foundation::attributed_string::CFMutableAttributedString;
                use core_foundation::base::CFRange;
                use core_foundation::string::CFString;
                use core_text::line::CTLine;
                use core_text::string_attributes::kCTFontAttributeName;
                let mut astr = CFMutableAttributedString::new();
                astr.replace_str(&CFString::new("Hello world"), CFRange::init(0, 0));
                let range = CFRange::init(0, astr.char_len());
                unsafe { astr.set_attribute(range, kCTFontAttributeName, f) };
                CTLine::new_with_attributed_string(astr.as_concrete_TypeRef()).get_typographic_bounds().width
            };
            assert!((width(&regular) - 72.73).abs() < 0.3, "{family} italic 400 width {}", width(&regular));
            assert!((width(&bold) - 78.48).abs() < 0.3, "{family} italic 700 width {}", width(&bold));
        }
    }

    /// An Apple emoji glyph advances as far as Chrome for Testing 148 places
    /// the next character (span widths of one `🔍` at each size, Helvetica
    /// primary, CB7a). Core Text's tracked advance is a pixel wider at most
    /// sizes (19 for Chrome's 18 at 14px), which put every emoji icon on the
    /// chrome board 1 to 2px wide.
    #[test]
    fn an_apple_emoji_advances_as_chrome_places_it() {
        let chrome: &[(f64, f32)] = &[
            (6.0, 8.0), (8.0, 10.0), (9.0, 11.0), (10.0, 13.0), (10.5, 13.0), (11.0, 14.0),
            (12.0, 15.0), (13.0, 16.0), (13.5, 17.0), (14.0, 18.0), (14.5, 18.0), (15.0, 19.0),
            (16.0, 20.0), (17.0, 21.0), (18.0, 21.0), (19.0, 22.0), (20.0, 22.0), (21.0, 23.0),
            (22.0, 23.0), (23.0, 24.0), (24.0, 24.0), (26.0, 26.0), (30.0, 30.0), (40.0, 40.0),
        ];
        for &(size, want) in chrome {
            let helvetica = font::new_from_name("Helvetica", size).expect("Helvetica");
            for ch in ['\u{1F50D}', '\u{1F6E1}', '\u{1F4C8}'] {
                let (face, glyph) = fallback_face_for_presentation(&helvetica, ch, true).expect("a face");
                let got = glyph_advance(&face, glyph);
                assert_eq!(got, want, "{ch:?} at {size}px in {}: Chrome advances {want}", face.postscript_name());
            }
        }
        // The system font's emoji face (`.AppleColorEmojiUI`) too: Chrome 18 / 20.
        for (size, want) in [(14.0, 18.0), (16.0, 20.0)] {
            let face = font::new_ui_font_for_language(font::kCTFontSystemFontType, size, None);
            let (emoji, glyph) = fallback_face_for_presentation(&face, '\u{1F6E1}', true).expect("a face");
            assert_eq!(glyph_advance(&emoji, glyph), want, "UI emoji at {size}px");
        }
    }

    /// Every path that reports an Apple emoji glyph's advance reports
    /// Chrome's (#1072 R1 nits 3 and 4): the legacy mask fallback a caller
    /// without layout's record reaches, and the shaper's own advances when
    /// the run's PRIMARY face is Apple Color Emoji. Chrome for Testing 148:
    /// `🔍` in `font: 14px "Apple Color Emoji"` is 18; two at 16px are 40.
    #[test]
    fn every_path_reports_the_apple_emoji_advance() {
        let helvetica = GlyphRasterizer::with_style("Helvetica", 14.0, 400, false);
        let (_, _, _, legacy, ..) = helvetica.rasterize_char('\u{1F50D}', 0.0).expect("rasterizes");
        assert_eq!(legacy, 18.0, "legacy fallback advance under Helvetica at 14px");
        let shaper = TextShaper::new("Apple Color Emoji", 16.0).expect("the emoji face");
        let shaped = shaper.shape("\u{1F50D}\u{1F50D}").expect("shapes");
        let total: f32 = shaped.advances.iter().sum();
        assert_eq!(total, 40.0, "two emoji in the emoji face at 16px");
    }

    /// An emoji sequence is shaped once per (face, size, sequence), not per
    /// call: `shape` reruns on every relayout (Prometheus R1 MED 2 on #1076).
    #[test]
    fn an_emoji_sequence_is_shaped_once_per_face_and_size() {
        let face = font::new_from_name("Apple Color Emoji", 14.0).expect("the emoji face");
        let family = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}";
        let first = emoji_cluster_glyph(&face, family);
        assert!(first.is_some(), "the family is one glyph");
        let before = emoji_cluster_lines();
        for _ in 0..5 {
            assert_eq!(emoji_cluster_glyph(&face, family), first);
        }
        assert_eq!(emoji_cluster_lines(), before, "no CTLine for a sequence already shaped");
    }

    /// A face with no PostScript name (a downloaded face can lack one) is not
    /// cached by name: two such faces would share an entry and one would
    /// draw the other's glyph (Prometheus delta R1 LOW on #1076).
    #[test]
    fn a_nameless_face_is_not_cached_by_name() {
        assert!(shaped_cache_key("", 14.0, "\u{1F44D}\u{1F3FD}").is_none());
        assert!(shaped_cache_key("AppleColorEmoji", 14.0, "\u{1F44D}\u{1F3FD}").is_some());
    }

    /// Paint draws the fallback glyph from that same face: the advance the
    /// rasteriser reports is Menlo's, not Apple Symbols'.
    #[test]
    fn the_rasteriser_draws_a_missing_symbol_from_the_cascade_face() {
        let menlo = GlyphRasterizer::with_style("Menlo", 16.0, 400, false);
        let courier = GlyphRasterizer::with_style("Courier", 16.0, 400, false);
        for ch in ['\u{2318}', '\u{2190}', '\u{2192}'] {
            let (_, _, _, want, ..) = menlo.rasterize_char(ch, 0.0).expect("Menlo has it");
            let (bitmap, _, _, got, ..) = courier.rasterize_char(ch, 0.0).expect("rasterizes");
            assert!(bitmap.iter().any(|&v| v > 0), "{ch:?}: no ink");
            assert_eq!(got, want, "{ch:?}: advance is not Menlo's");
        }
    }

    /// The colour-glyph path's faces still come first for its characters, a
    /// character outside the BMP is looked up as itself, and a character no
    /// face has is `None`, not Core Text's LastResort box.
    #[test]
    fn fallback_keeps_emoji_faces_and_refuses_the_last_resort_face() {
        let helvetica = named_font("Helvetica", 16.0).expect("Helvetica is installed");
        // Emoji_Presentation characters (U+23F0 is outside `is_emoji`'s
        // ranges) take Apple Color Emoji, and Chrome's line for them is that
        // face's height.
        for ch in ['\u{2615}', '\u{1F3AF}', '\u{23F0}'] {
            let (face, glyph) = fallback_face_for(&helvetica, ch).expect("an emoji face");
            assert_eq!(family_name_or_empty(&face), "Apple Color Emoji", "{ch:?}");
            assert_ne!(glyph, 0);
        }
        // Apple Color Emoji has the digits too (keycap bases). A face
        // without digits takes them from a text face, not from it.
        let dingbats = named_font("Zapf Dingbats", 16.0).expect("Zapf Dingbats is installed");
        assert!(color_glyph_id(&dingbats, '2').is_none(), "Zapf Dingbats has a 2");
        let (face, _) = fallback_face_for(&dingbats, '2').expect("a text face");
        assert_ne!(family_name_or_empty(&face), "Apple Color Emoji");
        // U+0378 is unassigned: only LastResort answers for it.
        assert!(fallback_face_for(&helvetica, '\u{0378}').is_none());
        // Asked twice, the answer is the same (the second is the memo's).
        let first = fallback_face_for(&helvetica, '\u{6F22}').map(|(f, g)| (f.postscript_name(), g));
        let again = fallback_face_for(&helvetica, '\u{6F22}').map(|(f, g)| (f.postscript_name(), g));
        assert!(first.is_some(), "a CJK face");
        assert_eq!(first, again);
    }

    /// Chrome for Testing 148 (CSS.getPlatformFontsForNode, impl-board3
    /// probe 2026-11-08): a character WITHOUT Emoji_Presentation (an emoji
    /// that is text by default) takes the system's cascade face for the
    /// primary face, like any other missing glyph. Only Emoji_Presentation
    /// characters (or a following U+FE0F) take Apple Color Emoji first. The
    /// chrome sidebar's `🛡` (U+1F6E1) under the system font is
    /// `.AppleColorEmojiUI`, whose extents are the system font's (its line is
    /// 17px at 14px, not the emoji face's 23), and `⚙` (U+2699) is Menlo
    /// (8.44px wide, not 18).
    #[test]
    fn a_text_default_emoji_takes_the_cascade_face_like_chrome() {
        let system = create_font("system-ui", 14.0).expect("system font");
        let helvetica = named_font("Helvetica", 14.0).expect("Helvetica is installed");
        let ps = |primary: &CTFont, ch: char| {
            fallback_face_for(primary, ch).map(|(f, _)| f.postscript_name()).unwrap_or_default()
        };
        assert_eq!(ps(&system, '\u{1F6E1}'), ".AppleColorEmojiUI", "shield under system-ui");
        assert_eq!(ps(&system, '\u{2699}'), "Menlo-Regular", "gear under system-ui");
        assert_eq!(ps(&system, '\u{2328}'), "Menlo-Regular", "keyboard under system-ui");
        assert_eq!(ps(&system, '\u{1F6AB}'), "AppleColorEmoji", "no-entry is Emoji_Presentation");
        assert_eq!(ps(&system, '\u{1F4C8}'), "AppleColorEmoji", "chart is Emoji_Presentation");
        assert_eq!(ps(&helvetica, '\u{1F6E1}'), "AppleColorEmoji", "shield under Helvetica");
        assert_eq!(ps(&helvetica, '\u{2764}'), "ZapfDingbatsITC", "heart under Helvetica");
        assert_eq!(ps(&helvetica, '\u{2328}'), "Menlo-Regular", "keyboard under Helvetica");
    }

    /// Chrome for Testing 148 (CSS.getPlatformFontsForNode, 14px, the
    /// presentation-matrix probe of 2026-11-08) for a symbol × {plain, U+FE0E,
    /// U+FE0F} under a primary that has or lacks its glyph. The face is ONE
    /// decision for layout and paint, and these are its outcomes:
    ///
    /// * U+FE0F after a character with the Emoji property goes to Apple Color
    ///   Emoji whether or not the primary has the glyph (`©️` in Helvetica,
    ///   Menlo or the system font is the emoji face, 18px wide, a 23px line);
    ///   after a character without it (`▾` U+25BE) it changes nothing.
    /// * U+FE0E asks for a text face: the primary when it has the glyph,
    ///   else the cascade's.
    /// * Plain: the primary when it has the glyph; else Apple Color Emoji
    ///   for an Emoji_Presentation character and the cascade face for any
    ///   other (`🛡` under the system font is `.AppleColorEmojiUI`, `⚙` and
    ///   `⌨` Menlo, `☺` Helvetica, `❤` Zapf Dingbats, `▾` under Helvetica
    ///   Lucida Grande).
    ///
    /// For every cell the advance layout measures and the advance paint's
    /// glyph carries are the same number: a line never wraps on one face
    /// and paints with another.
    #[test]
    fn layout_and_paint_take_one_face_per_presentation_like_chrome() {
        const ACE: &str = "AppleColorEmoji";
        // "" is the primary's own PostScript name.
        const P: &str = "";
        let system = create_font("system-ui", 14.0).expect("system font");
        let helvetica = named_font("Helvetica", 14.0).expect("Helvetica is installed");
        let menlo = named_font("Menlo", 14.0).expect("Menlo is installed");
        type Row = (char, [&'static str; 3]);
        let matrix: [(&CTFont, &str, [Row; 7]); 3] = [
            (
                &system,
                "system",
                [
                    ('\u{A9}', [P, P, ACE]),
                    ('\u{2699}', ["Menlo-Regular", "Menlo-Regular", ACE]),
                    ('\u{1F6E1}', [".AppleColorEmojiUI", ".AppleColorEmojiUI", ACE]),
                    ('\u{25BE}', [P, P, P]),
                    ('\u{2328}', ["Menlo-Regular", "Menlo-Regular", ACE]),
                    ('\u{263A}', ["Helvetica", "Helvetica", ACE]),
                    ('\u{2764}', ["ZapfDingbatsITC", "ZapfDingbatsITC", ACE]),
                ],
            ),
            (
                &helvetica,
                "Helvetica",
                [
                    ('\u{A9}', [P, P, ACE]),
                    ('\u{2699}', ["Menlo-Regular", "Menlo-Regular", ACE]),
                    ('\u{1F6E1}', [ACE, ACE, ACE]),
                    ('\u{25BE}', ["LucidaGrande", "LucidaGrande", "LucidaGrande"]),
                    ('\u{2328}', ["Menlo-Regular", "Menlo-Regular", ACE]),
                    ('\u{263A}', [P, P, ACE]),
                    ('\u{2764}', ["ZapfDingbatsITC", "ZapfDingbatsITC", ACE]),
                ],
            ),
            (
                &menlo,
                "Menlo",
                [
                    ('\u{A9}', [P, P, ACE]),
                    ('\u{2699}', [P, P, ACE]),
                    ('\u{1F6E1}', [ACE, ACE, ACE]),
                    ('\u{25BE}', [P, P, P]),
                    ('\u{2328}', [P, P, ACE]),
                    ('\u{263A}', [P, P, ACE]),
                    ('\u{2764}', [P, P, ACE]),
                ],
            ),
        ];
        let variants: [(Option<char>, &str); 3] =
            [(None, "plain"), (Some('\u{FE0E}'), "VS15"), (Some('\u{FE0F}'), "VS16")];
        let mut wrong = Vec::new();
        for (primary, primary_name, rows) in &matrix {
            let primary_ps = primary.postscript_name();
            let paint = GlyphRasterizer::for_face((*primary).clone(), 14.0);
            for (ch, want_faces) in rows {
                for ((next, variant), want) in variants.iter().zip(want_faces) {
                    let want = if want.is_empty() { primary_ps.as_str() } else { want };
                    // ONE decision, as layout makes it; paint's own view
                    // of the same font agrees with it.
                    let variation = Variation::after(*next);
                    let has = color_glyph_id(primary, *ch).is_some();
                    let decided = select_face(primary, *ch, variation, has);
                    assert_eq!(paint.select_face(*ch, variation), decided, "{primary_name} {ch:?} {variant}");
                    // Layout's side: the decided face's advance.
                    let (face, glyph) = match decided {
                        GlyphFace::Primary => ((*primary).clone(), color_glyph_id(primary, *ch).expect("has")),
                        GlyphFace::Fallback { emoji } => {
                            fallback_face_for_presentation(primary, *ch, emoji).expect("a face")
                        }
                        GlyphFace::Ignorable => unreachable!("{ch:?} is not a selector"),
                    };
                    let layout_advance = glyph_advance(&face, glyph);
                    // Paint's side: the recorded decision, nothing re-decided.
                    let raster = paint.rasterize_with_face(*ch, decided).expect("paints");
                    let paint_advance = raster.advance();
                    let colour = matches!(raster, GlyphRaster::Color(_));
                    let got = face.postscript_name();
                    if got != want
                        || (layout_advance - paint_advance).abs() > 0.01
                        || colour != is_color_face(&face)
                    {
                        wrong.push(format!(
                            "{primary_name} {ch:?} {variant}: {decided:?} face {got} (Chrome {want}), layout {layout_advance} vs paint {paint_advance}, colour path {colour}"
                        ));
                    }
                }
            }
        }
        assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    }

    #[test]
    fn test_whitespace_transparent() {
        let rasterizer = GlyphRasterizer::with_size(16.0);
        
        let result = rasterizer.rasterize_char(' ', 0.0);
        assert!(result.is_some());
        
        let (bitmap, _, _, _, _, _) = result.unwrap();
        // Whitespace should be transparent (all zeros)
        let all_transparent = bitmap.iter().all(|&b| b == 0);
        assert!(all_transparent, "Whitespace should be transparent");
    }
    
    #[test]
    fn test_width_factors() {
        // Narrow characters should have smaller width factor
        assert!(width_factor('i') < width_factor('m'));
        assert!(width_factor('.') < width_factor('W'));
    }
    /// Horizontal centre of mass of the ink, in pixel columns.
    ///
    /// This is the measurement that actually isolates the subpixel shift.
    /// Comparing raw bitmaps does NOT: a shifted rasterization also gains a
    /// pad column, so `assert_ne!(b0, b5)` passes on the width change alone
    /// and stays green even if the shift is deleted (verified by mutation).
    /// Centre of mass ignores the extra empty column and moves only when the
    /// ink moves.
    #[cfg(test)]
    fn ink_centre_x(bitmap: &[u8], width: u32, height: u32) -> f64 {
        let mut weighted = 0.0f64;
        let mut total = 0.0f64;
        for y in 0..height as usize {
            for x in 0..width as usize {
                let v = bitmap[y * width as usize + x] as f64;
                weighted += v * x as f64;
                total += v;
            }
        }
        assert!(total > 0.0, "glyph rasterized with no ink at all");
        weighted / total
    }

    /// Mutation-check both directions: a shifted phase must MOVE THE INK,
    /// and phase 0 must leave it exactly as it was.
    #[test]
    fn subpixel_phase_shifts_the_ink() {
        let r = GlyphRasterizer::with_size(16.0);
        let (b0, w0, h0, adv0, bx0, by0) =
            r.rasterize_char('n', 0.0).expect("phase 0 rasterizes");
        let (b5, w5, h5, adv5, bx5, by5) =
            r.rasterize_char('n', 0.5).expect("phase .5 rasterizes");

        // POSITIVE: the ink itself moved right by the requested fraction.
        // Deleting the offset in the draw position makes this go red; the
        // earlier byte-comparison form did not (it rode the width change).
        let c0 = ink_centre_x(&b0, w0, h0);
        let c5 = ink_centre_x(&b5, w5, h5);
        let shift = c5 - c0;
        assert!(
            (shift - 0.5).abs() < 0.12,
            "ink centre moved {shift:.3}px, expected ~0.5px (phase .5)"
        );

        assert_eq!(w5, w0 + 1, "shifted glyph gets exactly one pad column");
        assert_eq!(h5, h0, "vertical extent must not change for an x shift");

        // The metrics contract is phase-independent: advance and bearings
        // describe the glyph, not where inside the bitmap we drew it.
        assert_eq!(adv0, adv5, "advance must not depend on subpixel phase");
        assert_eq!(bx0, bx5, "bearing_x must stay the UNSHIFTED nominal bearing");
        assert_eq!(by0, by5, "bearing_y must not depend on an x shift");
    }

    /// The shift must be proportional, not merely present — a quarter phase
    /// moves a quarter pixel. Catches an offset that is applied but wrong.
    #[test]
    fn subpixel_shift_is_proportional_to_phase() {
        let r = GlyphRasterizer::with_size(16.0);
        let (b0, w0, h0, ..) = r.rasterize_char('n', 0.0).unwrap();
        let base = ink_centre_x(&b0, w0, h0);
        for (phase, expected) in [(0.25f32, 0.25f64), (0.5, 0.5), (0.75, 0.75)] {
            let (b, w, h, ..) = r.rasterize_char('n', phase).unwrap();
            let shift = ink_centre_x(&b, w, h) - base;
            assert!(
                (shift - expected).abs() < 0.12,
                "phase {phase} shifted ink {shift:.3}px, expected ~{expected}"
            );
        }
    }

    #[test]
    fn phase_zero_is_deterministic() {
        // NOT a bit-identity claim against the pre-subpixel tree: enabling
        // CGContext subpixel POSITIONING changes grid-fitting for every
        // phase including 0, so phase-0 ink moved slightly (measured: centre
        // 4.52 -> 4.82 at 16px). Micro parity moved 5.2% -> 5.1%, every case
        // improving, so the change is benign — but it is a change, and the
        // sequencing pin's "bit-identical" wording does not survive it.
        let r = GlyphRasterizer::with_size(16.0);
        let a = r.rasterize_char('n', 0.0).expect("rasterizes");
        let b = r.rasterize_char('n', 0.0).expect("rasterizes");
        assert_eq!(a.0, b.0, "phase 0 must be deterministic");
        assert_eq!(a.1, b.1);
    }

    #[test]
    fn subpixel_phase_is_clamped_not_wrapped() {
        let r = GlyphRasterizer::with_size(16.0);
        // Out-of-range and non-finite inputs must not panic or widen twice.
        for bad in [-1.0f32, 1.0, 2.5, f32::NAN, f32::INFINITY] {
            let got = r.rasterize_char('n', bad);
            assert!(got.is_some(), "rasterize must survive subpixel_x={bad}");
        }
        // Negative and NaN both fall back to phase 0 — same width as phase 0.
        let (_, w0, ..) = r.rasterize_char('n', 0.0).unwrap();
        let (_, wneg, ..) = r.rasterize_char('n', -1.0).unwrap();
        let (_, wnan, ..) = r.rasterize_char('n', f32::NAN).unwrap();
        assert_eq!(wneg, w0, "negative phase must clamp to 0, not widen");
        assert_eq!(wnan, w0, "NaN phase must clamp to 0, not widen");
    }

}
