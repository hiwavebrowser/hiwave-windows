//! # RustKit Layout
//!
//! Layout engine for the RustKit browser engine.
//! Implements block and inline layout algorithms.
//!
//! ## Design Goals
//!
//! 1. **Block layout**: Stack boxes vertically with margin collapse
//! 2. **Inline layout**: Flow text and inline elements horizontally with wrapping
//! 3. **Text shaping**: Use DirectWrite for accurate text measurement
//! 4. **Display list**: Generate paint commands with correct z-order
//! 5. **Positioned elements**: Support relative, absolute, fixed, sticky
//! 6. **Float layout**: Basic float behavior and clearance
//! 7. **Stacking contexts**: Z-index based paint ordering
//! 8. **Text rendering**: Font fallback, decorations, line height

pub mod flex;
pub mod forms;
pub mod grid;
pub mod images;
pub mod intrinsic_cache;
pub mod margin_collapse;
pub mod multicol;
pub mod scroll;
pub mod text;

pub use flex::{layout_flex_container, Axis, FlexItem, FlexLine};
pub use forms::{
    calculate_caret_position, calculate_selection_rects, render_button, render_checkbox,
    render_input, render_radio, CaretInfo, InputLayout, InputState, SelectionInfo,
};
pub use grid::{layout_grid_container, GridItem, GridLayout, GridTrack};
pub use images::{
    calculate_intrinsic_size, calculate_placeholder_size, render_background_image,
    render_broken_image, render_image, ImageLayoutInfo,
};
pub use intrinsic_cache::IntrinsicSizingMode;
pub use margin_collapse::{
    collapse_margins, establishes_bfc, is_margin_collapsible_through,
    should_collapse_with_first_child, should_collapse_with_last_child, CollapsibleMargin,
};
/// The document-scoped web-font registry (`@font-face` faces the engine
/// installs per view). Re-exported so the engine reaches it through the
/// crate that owns the loader rather than depending on rustkit-text directly.
pub use rustkit_text::webfonts;
pub use scroll::{
    calculate_scroll_into_view, handle_wheel_event, is_scroll_container, render_scrollbars,
    ScrollAlignment, ScrollMomentum, ScrollState, Scrollbar, ScrollbarOrientation, StickyOffsets,
    StickyState, WheelDeltaMode,
};
pub use text::{
    apply_text_transform, collapse_whitespace, FontCache, FontCacheKey, FontDisplay, FontFaceRule,
    FontFamilyChain, FontLoader, LineHeight, PositionedGlyph, ShapedRun, TextDecoration, TextError,
    TextMetrics, TextShaper, TopLevelSite,
};

use rustkit_css::{BoxSizing, Color, ComputedStyle, Length, TextAlign};
use std::cmp::Ordering;

/// The `word-break` the line breaker should run with.
///
/// css-text-3 §5.3: `line-break: anywhere` puts a soft wrap opportunity
/// around every typographic character unit, "disregarding any prohibition
/// against line breaks" — including `word-break: keep-all` on the same
/// element. The breaker models exactly that set of opportunities for
/// `break-all` (every grapheme boundary is a REAL opportunity, used in normal
/// line filling), so `anywhere` maps onto it here rather than onto
/// `overflow-wrap: anywhere`, whose emergency breaks only fire for a word
/// that overflows a line on its own.
pub fn effective_word_break(style: &ComputedStyle) -> rustkit_css::WordBreak {
    if style.line_break == rustkit_css::LineBreak::Anywhere {
        rustkit_css::WordBreak::BreakAll
    } else {
        style.word_break
    }
}
use thiserror::Error;

/// Errors that can occur in layout.
#[derive(Error, Debug)]
pub enum LayoutError {
    #[error("Layout failed: {0}")]
    LayoutFailed(String),

    #[error("Text shaping error: {0}")]
    TextShapingError(String),
}

thread_local! {
    /// Memo for [`normal_line_height`]: shaping a probe glyph per box per layout
    /// pass showed up as real time, and the answer only depends on the font.
    static NORMAL_LINE_HEIGHT_CACHE: std::cell::RefCell<
        std::collections::HashMap<(String, u32, u16, u8), f32>,
    > = std::cell::RefCell::new(std::collections::HashMap::new());
}

/// The font's own `line-height: normal`, in px, matching Blink's model:
/// `round(ascent) + round(descent) + line_gap`.
///
/// Two things matter here, and only the pair of them gets Chrome's answer:
///
/// 1. `normal` is derived from the FONT (ascent + descent + line-gap), not from
///    a flat ratio. RustKit hardcoded `font_size * 1.2`, drifting up to ~1.2px
///    per line at 16px system-ui and compounding down the page.
/// 2. Blink rounds ascent and descent to whole pixels INDEPENDENTLY before
///    summing (SimpleFontData). So `normal` is always an integer, and line
///    boxes land on whole pixels. Summing the raw floats instead is *closer on
///    average* yet scores WORSE on the pixel meter: fractional line boxes push
///    every baseline onto a sub-pixel offset and antialiasing diverges
///    everywhere. Measured: 24/26 -> 23/26 with raw floats. Rounding is not a
///    cosmetic detail, it is the mechanism.
///
/// Verified against Chrome 148 on 20 font/size pairs drawn from the committed
/// baselines (`crates/rustkit-layout/tests/normal_line_height_probe.rs` and
/// `scripts/probe_normal_lineheight.py`): exact on 19/20.
///
/// Falls back to the flat ratio only when shaping yields no usable metrics
/// (missing font) -- same behaviour as before this existed.
pub fn normal_line_height(style: &ComputedStyle, font_size: f32) -> f32 {
    let key = (
        style.font_family.clone(),
        font_size.to_bits(),
        style.font_weight.0 as u16,
        style.font_style as u8,
    );
    if let Some(px) = NORMAL_LINE_HEIGHT_CACHE.with(|c| c.borrow().get(&key).copied()) {
        return px;
    }

    let m = measure_text_advanced(
        "x",
        &style.font_family,
        font_size,
        style.font_weight,
        style.font_style,
    );
    let px = if m.ascent > 0.0 {
        used_font_line_height(&m)
    } else {
        font_size * rustkit_css::NORMAL_LINE_HEIGHT_FALLBACK_RATIO
    };

    NORMAL_LINE_HEIGHT_CACHE.with(|c| c.borrow_mut().insert(key, px));
    px
}

/// Blink's `normal` line height for one face's extents:
/// `round(ascent) + round(descent) + round(line_gap)` (SimpleFontData rounds
/// all three; Arial at 16px is 14.48 + 3.39 + 0.52 = 18 in Chrome, and
/// 17.52 when the gap rides unrounded).
fn used_font_line_height(m: &TextMetrics) -> f32 {
    m.ascent.round() + m.descent.round() + m.leading.round()
}

/// The CONTENT height a non-`auto` `aspect-ratio` implies for a box whose
/// block size is `auto`, given its resolved content width. `None` when the box
/// has no usable ratio or no inline size to derive from.
///
/// css-sizing-4 §4: the ratio applies to the box named by `box-sizing`, not
/// always to the content box. Measured against Chrome 141, a 400px-wide box
/// with `padding: 20px` and `aspect-ratio: 2 / 1`:
///
/// | box-sizing | Chrome border box | via content box |
/// |---|---|---|
/// | `border-box`  | **200** (= 400/2)          | 220 — wrong by the padding |
/// | `content-box` | **220** (= 360/2 + 40)     | 220 — same |
///
/// So a content-box derivation is right only under `content-box`, and every
/// corpus page opens with `* { box-sizing: border-box }`.
pub(crate) fn aspect_ratio_content_height(
    style: &ComputedStyle,
    content_width: f32,
    padding_border_w: f32,
    padding_border_h: f32,
) -> Option<f32> {
    let ratio = style.aspect_ratio?;
    if !(ratio > 0.0) || !ratio.is_finite() || !(content_width > 0.0) {
        return None;
    }
    Some(if style.box_sizing == BoxSizing::BorderBox {
        (((content_width + padding_border_w) / ratio) - padding_border_h).max(0.0)
    } else {
        content_width / ratio
    })
}

/// Intrinsic BORDER-box size of a form control: the bare-control calibration,
/// or the control's content line composed with author padding/border. Block
/// flow (`layout_form_control`) and flex items (`flex::get_intrinsic_*`) both
/// size from here — the flex path carried its own older blobs (button = label
/// + 24 wide, 1.5em + 12 tall, author padding ignored), so flex-positioning's
/// `.btn { padding: 8px 16px }` row built 55.9x33 for Chrome's 63.9x34 and
/// everything below it sat a pixel high.
pub(crate) fn form_control_intrinsic_size(
    style: &ComputedStyle,
    control: &FormControlType,
) -> (f32, f32) {
    let font_size = match style.font_size {
        Length::Px(px) => px,
        _ => 16.0,
    };

    // Author padding + border COMPOSE with the control's content height
    // (DIG-1, css-selectors heatmap 2026-07-11): the old blob formula
    // font_size*1.5+8 pretended to be the whole border-box, so
    // input{padding:8px; border:2px} measured 29px where Chrome builds
    // 35 (content line 15, plus 16 padding, plus 4 border) — and
    // every section below slid up by the deficit. When the author sets
    // vertical padding/border, the content line composes with them; the
    // blob stays for bare controls (UA-default look, form-controls case
    // depends on it).
    //
    // n51: the unit closure resolved px and em only, so `padding: 1rem
    // 1.5rem` — the idiom on every styled search box — read as NO author
    // padding and the control fell to the bare 19px blob (new_tab's
    // `#searchInput`: 19 for Chrome's 52, and everything below it 33px
    // high). Resolve every absolute unit the way the rest of layout does.
    let author_pb_v = {
        let px = |l: &Length| match l {
            Length::Percent(_) | Length::Auto => 0.0,
            other => other.to_px(font_size, 16.0, 0.0),
        };
        px(&style.padding_top)
            + px(&style.padding_bottom)
            + px(&style.border_top_width)
            + px(&style.border_bottom_width)
    };
    // The content line is the control font's `normal` line (n58): Arial is
    // 15 at the UA 13.333px and 16 at 14px in Chrome; the old `font_size + 1`
    // read 14.33 / 15, so every styled control was 0.7–1px short.
    let single_line_box = |blob: f32| {
        if author_pb_v > 0.0 {
            normal_line_height(style, font_size) + author_pb_v
        } else {
            blob
        }
    };

    // Bare-control border-box sizes are calibrated to Chrome CfT-148 at
    // the UA control font (13.333px system-ui, PR #42): single-line
    // input/button/select build a 19px border-box (UA border included),
    // checkbox/radio 13x13, textarea 15px per row + 2px border, range
    // 16x129, color 27x50 (form-controls t8 dig, 2026-07-17: the old
    // blobs fs*1.5+8 / fs*1.5+12 / fs*1.2 measured 28/32/16 — +9/+13/+3
    // on every bare control, cascading +63px of drift down the page).
    // Scaled by font_size so author-sized controls keep proportion; at
    // the UA font the scale is 1.0 and the match to Chrome is exact.
    let ua_scale = font_size / (40.0 / 3.0);

    // Calculate intrinsic dimensions based on control type
    match control {
        FormControlType::TextInput { input_type, .. } => match input_type.as_str() {
            "range" => (129.0 * ua_scale, 16.0 * ua_scale),
            "color" => (50.0 * ua_scale, 27.0 * ua_scale),
            // Default text input: size=20 at the UA control font builds
            // a 149px border-box in Chrome CfT-148 (n53 form-controls
            // y-table: 149 on every bare text/email/password/number
            // input; the old 12em blob said 160).
            _ => (149.0 * ua_scale, single_line_box(19.0 * ua_scale)),
        },
        FormControlType::TextArea { rows, cols, .. } => {
            // Textarea: cols × the monospace advance (0.6em) plus 18px
            // of border + vertical scrollbar gutter — Chrome builds 178
            // for cols=20 and 338 for cols=40 (n53).
            let rows = (*rows).max(2) as f32;
            let cols = (*cols).max(20) as f32;
            (
                font_size * 0.6 * cols + 18.0 * ua_scale,
                (15.0 * rows + 2.0) * ua_scale,
            )
        }
        FormControlType::Button { label, .. } => {
            // Button: measured label width plus padding (was a
            // chars-times-0.6em guess that oversized real labels ~40%).
            // Height composes author padding/border like TextInput/Select
            // (DIG-2): the css-selectors buttons (padding 8px 16px) build
            // (fs+1)+16 = 31 in Chrome; the blob said 33. Width also
            // composes when the author sets horizontal padding.
            let label_width = measure_text_advanced(
                label,
                &style.font_family,
                font_size,
                style.font_weight,
                style.font_style,
            )
            .width;
            let px = |l: &Length| match l {
                Length::Percent(_) | Length::Auto => 0.0,
                other => other.to_px(font_size, 16.0, 0.0),
            };
            let author_pb_h = px(&style.padding_left)
                + px(&style.padding_right)
                + px(&style.border_left_width)
                + px(&style.border_right_width);
            let width = if author_pb_h > 0.0 {
                label_width + author_pb_h
            } else {
                label_width + 24.0
            };
            (width, single_line_box(19.0 * ua_scale))
        }
        FormControlType::Checkbox { .. } | FormControlType::Radio { .. } => {
            // Fixed size for checkboxes and radios
            (13.0 * ua_scale, 13.0 * ua_scale)
        }
        FormControlType::Select { size, options, .. } => {
            // Chrome sizes a select to its WIDEST option (n53
            // form-controls: listbox 39 = "Item 4" 37 + 2px border;
            // dropdown 137 = "A longer option text" + 24px of border and
            // arrow well, 60 = "Select" + 24). The old 10em blob built
            // 133 for both.
            let widest = options
                .iter()
                .map(|o| {
                    measure_text_advanced(
                        o,
                        &style.font_family,
                        font_size,
                        style.font_weight,
                        style.font_style,
                    )
                    .width
                })
                .fold(0.0_f32, f32::max);
            if *size > 1 {
                // Inline listbox: 16px per visible row + 2px border.
                (
                    widest + 2.0 * ua_scale,
                    (16.0 * *size as f32 + 2.0) * ua_scale,
                )
            } else {
                // Dropdown: widest option plus the arrow well.
                (widest + 24.0 * ua_scale, single_line_box(19.0 * ua_scale))
            }
        }
    }
}

/// Resolve a box's `line-height` to px, consulting font metrics for `normal`.
pub fn resolve_line_height(style: &ComputedStyle, font_size: f32) -> f32 {
    match style.line_height {
        // Only `normal` needs the font; skip shaping entirely otherwise.
        rustkit_css::LineHeight::Normal => normal_line_height(style, font_size),
        other => other.to_px(font_size),
    }
}

/// Height of a closing line box (CSS2 §10.8.1): the members sit on one
/// baseline, so the box spans the largest extent above it plus the largest
/// below it, and the container's strut floors both. `tallest` / `bottom_edge`
/// are the block-children loops' older accounting — the tallest member, and
/// bottom-edge members plus the strut descent — kept as floors for the
/// members the align pass leaves at the line top.
fn line_advance(tallest: f32, bottom_edge: f32, extents: (f32, f32), strut: (f32, f32)) -> f32 {
    tallest
        .max(bottom_edge)
        .max(extents.0.max(strut.0) + extents.1.max(strut.1))
}

/// Line height of ONE shaped text run: the box's `line-height`, except that
/// under `normal` the run's own extents win when they are taller. A run's
/// metrics are the union of every face it used (`TextShaper::shape` folds
/// in the fallback faces), and Blink sizes a `normal` line box from the
/// used faces, not the primary alone (NGInlineBoxState::AccumulateUsedFonts):
/// "☕ coffee" at 16px system-ui is a 26px line in Chrome (emoji face 20 + 6),
/// not the primary face's 18. An explicit `line-height` ignores the used
/// faces, as in Chrome (the 24px control on the repro stays 24).
///
/// Layout (`layout_text*`) and paint (`render_text`) MUST both go through
/// here — the half-leading that seats the baseline is derived from it.
pub fn run_line_height(style: &ComputedStyle, font_size: f32, metrics: &TextMetrics) -> f32 {
    let base = resolve_line_height(style, font_size);
    if !matches!(style.line_height, rustkit_css::LineHeight::Normal) || metrics.ascent <= 0.0 {
        return base;
    }
    base.max(used_font_line_height(metrics))
}

/// Distance from a line's top to the baseline of a text run, as Blink seats it:
/// ascent and descent are whole pixels (SkScalarRoundToScalar) and the leading
/// above the text is FLOORED (`CalculateLeadingSpace`), the remainder going
/// below. A 16px run with ascent 15.47 / descent 3.38 on a 27.2px line sits at
/// floor((27.2 - 18) / 2) + 15 = 19, not 4.18 + 15.47 = 19.65: seated on the
/// fractional sum, every line whose top lands below .35 painted one row low.
/// The leading is SIGNED (see `half_leading`): a `line-height: 1` heading with
/// a taller content area seats its baseline above where a zero floor put it.
pub(crate) fn blink_baseline_offset(line_height: f32, ascent: f32, descent: f32) -> f32 {
    let (ascent, descent) = (ascent.round(), descent.round());
    half_leading(line_height, ascent, descent).floor() + ascent
}

/// Half-leading of a line: half of `line-height` minus the content area
/// (ascent + descent), SIGNED. CSS2 §10.8.1 puts no floor on it — when the
/// line-height is smaller than the content area the leading is negative and
/// the glyphs overflow the line box equally above and below, which is what
/// every `line-height: 1` heading and `line-height: 0.9` display line on a
/// real page relies on (a 40px system-ui heading has a 47px content area).
/// Until n57 six sites clamped this at zero, so such a line seated its
/// baseline a whole |half-leading| low (4px at 40px, 2px on a 14px emoji in
/// a 20px line — the chrome strip's url icon); Chrome floors it (Blink
/// `FontHeight::AddLeading`) but never clamps it.
pub fn half_leading(line_height: f32, ascent: f32, descent: f32) -> f32 {
    (line_height - (ascent + descent)) / 2.0
}

/// The metrics a run's baseline is SEATED on. Under `line-height: normal`
/// the run's united metrics (primary face + every fallback face it used,
/// see `TextShaper::shape`): Blink unites the used fonts into the line box
/// and the baseline sits at their max ascent ("☕ coffee" at 16px: 20 above).
/// Under an EXPLICIT line-height Blink skips that accumulation
/// (`NGInlineBoxState::AccumulateUsedFonts` runs only for `normal`) and
/// seats the run on the PRIMARY face alone: "🔒 secure" at 16px in a 20px
/// line has its baseline at top + 1 + 15 = 16 in Chrome, not top + 20.
/// Seating it on the emoji face put every icon-plus-label line 2–4px low.
///
/// ASCII text never reaches a fallback face, so its united metrics ARE the
/// primary's and no second probe is shaped.
fn seat_metrics(style: &ComputedStyle, font_size: f32, text: &str, united: &TextMetrics) -> (f32, f32) {
    if matches!(style.line_height, rustkit_css::LineHeight::Normal) || text.is_ascii() {
        return (united.ascent, united.descent);
    }
    let primary = measure_text_advanced(
        "x",
        &style.font_family,
        font_size,
        style.font_weight,
        style.font_style,
    );
    if primary.ascent > 0.0 {
        (primary.ascent, primary.descent)
    } else {
        (united.ascent, united.descent)
    }
}

/// Convert a specified size on a replaced element to a CONTENT size.
///
/// css-sizing-3 §3.1: under `box-sizing: border-box` a specified `width`,
/// `height`, `max-width` or `max-height` names the BORDER box, so the
/// element's own border and padding come out of it; under `content-box` (the
/// initial value) it names the content box already. `None` — an `auto` size —
/// stays `None`: the intrinsic size is a content size in both modes, and the
/// decoration adds OUTSIDE it. That asymmetry is the whole defect this
/// function exists for; a border-box image at its natural size is 102px wide
/// where a border-box image at `width: 102px` has a 100px content box.
///
/// A free function rather than a method so the arithmetic is testable without
/// a `LayoutBox`, and so that `layout_image` calling it is the only wiring a
/// mutation has to break.
pub fn replaced_content_size(
    specified: Option<f32>,
    decoration: f32,
    border_box_sizing: bool,
) -> Option<f32> {
    specified.map(|size| {
        if border_box_sizing {
            (size - decoration).max(0.0)
        } else {
            size
        }
    })
}

/// Cross a known CONTENT size to the other axis through a preferred aspect
/// ratio, honouring the box the ratio spans.
///
/// css-sizing-4 §4: `aspect-ratio` applies to the box named by `box-sizing`,
/// so under `border-box` the ratio spans the element's own decoration and the
/// derived axis has to give its decoration back. MEASURED against Chrome 148
/// on a 1px-bordered 100x100 image with `aspect-ratio: 16/9`:
///
/// ```text
/// box-sizing: border-box ; width: 160px  ->  border box 160.0000 x 90.0000
/// box-sizing: content-box; width: 160px  ->  border box 162.0000 x 92.0000
/// ```
///
/// If the ratio spanned the content box in both modes the border-box case
/// would build 90.875 tall, which is what makes this branch load-bearing
/// rather than cosmetic.
pub fn ratio_cross_content_size(
    known_content: f32,
    known_decoration: f32,
    derived_decoration: f32,
    ratio: f32,
    known_is_width: bool,
    border_box_sizing: bool,
) -> f32 {
    let known_ratio_box = if border_box_sizing {
        known_content + known_decoration
    } else {
        known_content
    };
    let derived_ratio_box = if known_is_width {
        known_ratio_box / ratio
    } else {
        known_ratio_box * ratio
    };
    if border_box_sizing {
        (derived_ratio_box - derived_decoration).max(0.0)
    } else {
        derived_ratio_box.max(0.0)
    }
}

/// Resolve a replaced element's specified sizes against a preferred aspect
/// ratio, returning CONTENT sizes.
///
/// css-sizing-4 §4/§5.1: a specified `aspect-ratio` REPLACES the element's
/// natural ratio. Until 2026-08-23 `style.aspect_ratio` was consulted only on
/// the block-height path, so it never reached a replaced element at all and
/// images-intrinsic test11 (`width: 160px; aspect-ratio: 16/9`) built 160x160
/// where Chrome builds 160x90.
///
/// The four cases, each MEASURED against Chrome 148 rather than derived
/// (100x100 natural image, `border: 1px solid red`, `aspect-ratio: 16/9`):
///
/// ```text
/// width and height both set   border-box   160x200   ratio ignored
/// width set, height auto      border-box   160x90    height from the ratio
/// height set, width auto      border-box   160x90    width from the ratio
/// both auto                   border-box   102x57.375
///                             content-box  102x58.25 natural width, then ratio
/// ```
///
/// A free function for the same reason as `replaced_content_size`: the case
/// analysis is the part that can be wrong, and `layout_image` calling it is
/// the only wiring a mutation has to break.
pub fn preferred_ratio_sizes(
    explicit_width: Option<f32>,
    explicit_height: Option<f32>,
    natural_width: Option<f32>,
    ratio: Option<f32>,
    horizontal_decoration: f32,
    vertical_decoration: f32,
    border_box_sizing: bool,
) -> (Option<f32>, Option<f32>) {
    let ratio = match ratio {
        Some(r) if r > 0.0 && r.is_finite() => r,
        _ => return (explicit_width, explicit_height),
    };
    match (explicit_width, explicit_height) {
        // Both specified: the ratio does not get a vote.
        (Some(_), Some(_)) => (explicit_width, explicit_height),
        (Some(w), None) => (
            Some(w),
            Some(ratio_cross_content_size(
                w,
                horizontal_decoration,
                vertical_decoration,
                ratio,
                true,
                border_box_sizing,
            )),
        ),
        (None, Some(h)) => (
            Some(ratio_cross_content_size(
                h,
                vertical_decoration,
                horizontal_decoration,
                ratio,
                false,
                border_box_sizing,
            )),
            Some(h),
        ),
        // Both auto: the natural width is the used width and the ratio names
        // the height. The natural HEIGHT is discarded — that is what "replaces
        // the natural ratio" means.
        (None, None) => match natural_width {
            Some(nw) if nw > 0.0 => (
                Some(nw),
                Some(ratio_cross_content_size(
                    nw,
                    horizontal_decoration,
                    vertical_decoration,
                    ratio,
                    true,
                    border_box_sizing,
                )),
            ),
            _ => (None, None),
        },
    }
}

/// CSS position property values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Position {
    #[default]
    Static,
    Relative,
    Absolute,
    Fixed,
    Sticky,
}

/// CSS float property values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Float {
    #[default]
    None,
    Left,
    Right,
}

/// CSS clear property values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Clear {
    #[default]
    None,
    Left,
    Right,
    Both,
}

/// Offset values for positioned elements.
#[derive(Debug, Clone, Copy, Default)]
pub struct PositionOffsets {
    pub top: Option<f32>,
    pub right: Option<f32>,
    pub bottom: Option<f32>,
    pub left: Option<f32>,
}

/// Float exclusion area.
#[derive(Debug, Clone, Copy)]
pub struct FloatExclusion {
    pub rect: Rect,
    pub float_type: Float,
}

/// Float context for tracking float exclusions.
#[derive(Debug, Clone, Default)]
pub struct FloatContext {
    pub left_floats: Vec<FloatExclusion>,
    pub right_floats: Vec<FloatExclusion>,
}

impl FloatContext {
    /// Create a new empty float context.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a left float.
    pub fn add_left(&mut self, rect: Rect) {
        self.left_floats.push(FloatExclusion {
            rect,
            float_type: Float::Left,
        });
    }

    /// Add a right float.
    pub fn add_right(&mut self, rect: Rect) {
        self.right_floats.push(FloatExclusion {
            rect,
            float_type: Float::Right,
        });
    }

    /// Get available width at a given y position.
    pub fn available_width(&self, y: f32, container_width: f32) -> (f32, f32) {
        let mut left_edge: f32 = 0.0;
        let mut right_edge: f32 = container_width;

        for float in &self.left_floats {
            if y >= float.rect.y && y < float.rect.bottom() {
                left_edge = left_edge.max(float.rect.right());
            }
        }

        for float in &self.right_floats {
            if y >= float.rect.y && y < float.rect.bottom() {
                right_edge = right_edge.min(float.rect.x);
            }
        }

        (left_edge, right_edge)
    }

    /// Clear floats up to a given y position.
    pub fn clear(&mut self, clear: Clear) -> f32 {
        let mut clear_y: f32 = 0.0;

        match clear {
            Clear::Left => {
                for float in &self.left_floats {
                    clear_y = clear_y.max(float.rect.bottom());
                }
            }
            Clear::Right => {
                for float in &self.right_floats {
                    clear_y = clear_y.max(float.rect.bottom());
                }
            }
            Clear::Both => {
                for float in &self.left_floats {
                    clear_y = clear_y.max(float.rect.bottom());
                }
                for float in &self.right_floats {
                    clear_y = clear_y.max(float.rect.bottom());
                }
            }
            Clear::None => {}
        }

        clear_y
    }

    /// Get the clear position for all floats (bottom of lowest float).
    pub fn clear_all(&self) -> f32 {
        let mut clear_y: f32 = 0.0;
        for float in &self.left_floats {
            clear_y = clear_y.max(float.rect.bottom());
        }
        for float in &self.right_floats {
            clear_y = clear_y.max(float.rect.bottom());
        }
        clear_y
    }

    /// Find the best y position for a new float of the given size.
    ///
    /// This finds the highest position where the float can fit without
    /// overlapping existing floats.
    pub fn find_float_position(
        &self,
        float_type: Float,
        width: f32,
        height: f32,
        start_y: f32,
        container_width: f32,
    ) -> (f32, f32) {
        match float_type {
            Float::Left => self.find_left_float_position(width, height, start_y, container_width),
            Float::Right => self.find_right_float_position(width, height, start_y, container_width),
            Float::None => (0.0, start_y),
        }
    }

    /// Find position for a left float.
    fn find_left_float_position(
        &self,
        width: f32,
        height: f32,
        start_y: f32,
        container_width: f32,
    ) -> (f32, f32) {
        let mut y = start_y;
        let mut iterations = 0;
        const MAX_ITERATIONS: usize = 1000; // Prevent infinite loops

        loop {
            if iterations >= MAX_ITERATIONS {
                break;
            }
            iterations += 1;

            let (left_edge, right_edge) = self.available_width(y, container_width);
            let available = right_edge - left_edge;

            if available >= width {
                // Check if this position works for the entire height of the float
                let mut fits = true;
                let mut check_y = y;
                while check_y < y + height {
                    let (l, r) = self.available_width(check_y, container_width);
                    if r - l < width {
                        fits = false;
                        // Move y past this obstruction
                        y = self.next_clear_y_after(check_y);
                        break;
                    }
                    check_y += 1.0; // Check in 1px increments
                }

                if fits {
                    return (left_edge, y);
                }
            } else {
                // Not enough space, move down
                y = self.next_clear_y_after(y);
            }

            // If we've moved past all floats, we're done
            if y >= self.clear_all() {
                let (left_edge, _) = self.available_width(y, container_width);
                return (left_edge, y);
            }
        }

        // Fallback: place at clear position
        let (left_edge, _) = self.available_width(self.clear_all(), container_width);
        (left_edge, self.clear_all())
    }

    /// Find position for a right float.
    fn find_right_float_position(
        &self,
        width: f32,
        height: f32,
        start_y: f32,
        container_width: f32,
    ) -> (f32, f32) {
        let mut y = start_y;
        let mut iterations = 0;
        const MAX_ITERATIONS: usize = 1000;

        loop {
            if iterations >= MAX_ITERATIONS {
                break;
            }
            iterations += 1;

            let (left_edge, right_edge) = self.available_width(y, container_width);
            let available = right_edge - left_edge;

            if available >= width {
                let mut fits = true;
                let mut check_y = y;
                while check_y < y + height {
                    let (l, r) = self.available_width(check_y, container_width);
                    if r - l < width {
                        fits = false;
                        y = self.next_clear_y_after(check_y);
                        break;
                    }
                    check_y += 1.0;
                }

                if fits {
                    return (right_edge - width, y);
                }
            } else {
                y = self.next_clear_y_after(y);
            }

            if y >= self.clear_all() {
                let (_, right_edge) = self.available_width(y, container_width);
                return (right_edge - width, y);
            }
        }

        let (_, right_edge) = self.available_width(self.clear_all(), container_width);
        (right_edge - width, self.clear_all())
    }

    /// Find the next y position where a float ends.
    fn next_clear_y_after(&self, y: f32) -> f32 {
        let mut next_y = f32::MAX;

        for float in &self.left_floats {
            if float.rect.bottom() > y {
                next_y = next_y.min(float.rect.bottom());
            }
        }

        for float in &self.right_floats {
            if float.rect.bottom() > y {
                next_y = next_y.min(float.rect.bottom());
            }
        }

        if next_y == f32::MAX {
            y
        } else {
            next_y
        }
    }

    /// Get the available rectangle at a given y position for content.
    ///
    /// Returns (x, width) of the available space.
    pub fn available_rect(&self, y: f32, height: f32, container_width: f32) -> (f32, f32) {
        let mut min_left: f32 = 0.0;
        let mut max_right: f32 = container_width;

        // Check the entire height range
        let mut check_y = y;
        while check_y < y + height {
            let (left, right) = self.available_width(check_y, container_width);
            min_left = min_left.max(left);
            max_right = max_right.min(right);
            check_y += 1.0;
        }

        (min_left, (max_right - min_left).max(0.0))
    }

    /// Check if there are any active floats at the given y position.
    pub fn has_floats_at(&self, y: f32) -> bool {
        for float in &self.left_floats {
            if y >= float.rect.y && y < float.rect.bottom() {
                return true;
            }
        }
        for float in &self.right_floats {
            if y >= float.rect.y && y < float.rect.bottom() {
                return true;
            }
        }
        false
    }

    /// Check if a float of the given size fits at the specified position.
    pub fn float_fits(
        &self,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        container_width: f32,
    ) -> bool {
        // Check bounds
        if x < 0.0 || x + width > container_width {
            return false;
        }

        // Check against existing floats
        let float_rect = Rect {
            x,
            y,
            width,
            height,
        };

        for float in &self.left_floats {
            if rects_overlap(&float_rect, &float.rect) {
                return false;
            }
        }

        for float in &self.right_floats {
            if rects_overlap(&float_rect, &float.rect) {
                return false;
            }
        }

        true
    }

    /// Get all floats that overlap with the given y range.
    pub fn floats_in_range(&self, y_start: f32, y_end: f32) -> Vec<&FloatExclusion> {
        let mut result = Vec::new();

        for float in &self.left_floats {
            if float.rect.y < y_end && float.rect.bottom() > y_start {
                result.push(float);
            }
        }

        for float in &self.right_floats {
            if float.rect.y < y_end && float.rect.bottom() > y_start {
                result.push(float);
            }
        }

        result
    }

    /// Remove floats that are completely above the given y position.
    /// This can be used to clean up floats that are no longer relevant.
    pub fn remove_floats_above(&mut self, y: f32) {
        self.left_floats.retain(|f| f.rect.bottom() > y);
        self.right_floats.retain(|f| f.rect.bottom() > y);
    }

    /// Check if the context has any floats.
    pub fn is_empty(&self) -> bool {
        self.left_floats.is_empty() && self.right_floats.is_empty()
    }

    /// Get the total number of floats.
    pub fn float_count(&self) -> usize {
        self.left_floats.len() + self.right_floats.len()
    }
}

/// Check if two rectangles overlap.
fn rects_overlap(a: &Rect, b: &Rect) -> bool {
    a.x < b.right() && a.right() > b.x && a.y < b.bottom() && a.bottom() > b.y
}

/// CSS 2.1 §10.3.7: with `width: auto`, does an out-of-flow box shrink to fit?
///
/// **Precondition: the caller has already established `width` computes to
/// `auto`.** The width test is the caller's `match` arm and is deliberately
/// not restated here, so this predicate has no branch its call site makes
/// unreachable.
///
/// The rule reads as six sub-cases in the spec, but on the auto-width side it
/// collapses to one question: are BOTH `left` and `right` specified? If they
/// are, the equation is solved for width and the box stretches between them
/// (`inset: 0` overlays). In every other combination — both auto, or exactly
/// one of them given — width is shrink-to-fit and the remaining offset is
/// solved afterwards.
///
/// In-flow block boxes are untouched: `width: auto` there fills the
/// containing block (§10.3.3), which is a different rule with the same
/// keyword.
pub(crate) fn auto_width_shrinks_to_fit(position: Position, offsets: &PositionOffsets) -> bool {
    matches!(position, Position::Absolute | Position::Fixed)
        && !(offsets.left.is_some() && offsets.right.is_some())
}

/// CSS 2.1 §10.3.5 shrink-to-fit: `min(max(preferred minimum, available),
/// preferred)`, in CONTENT-box px.
///
/// `available` is the space left for content after the box's own margins,
/// borders and padding — i.e. exactly what the `width: auto` fill path would
/// have used. The intrinsic estimators answer border-box widths, so the same
/// padding+border they added is subtracted back off; taking it from
/// `horizontal_padding_border` rather than from the caller's separately
/// resolved values keeps the two halves of that subtraction from drifting
/// apart on a relative unit.
pub(crate) fn shrink_to_fit_content_width(layout_box: &LayoutBox, available: f32) -> f32 {
    let padding_border = crate::grid::horizontal_padding_border(&layout_box.style);
    // No `.max(0.0)` on the floor, deliberately: `available` is non-negative
    // at every call site, so `.max(available)` already dominates a negative
    // preferred minimum and a clamp here could never change an answer. It was
    // written, measured to survive its own mutation probe, and removed —
    // a guard nothing can hold is decoration.
    let preferred_minimum = crate::grid::own_min_content_width(layout_box) - padding_border;
    // The clamp on the CEILING is load-bearing and is not the same case:
    // `own_max_content_width` answers 0 without adding padding+border back for
    // a `display: none` box, and `.min(preferred)` would then hand back that
    // negative number as the used width.
    let preferred = (crate::grid::own_max_content_width(layout_box) - padding_border).max(0.0);
    preferred_minimum.max(available).min(preferred)
}

/// Margin collapse context.
#[derive(Debug, Clone, Default)]
pub struct MarginCollapseContext {
    /// Pending positive margin.
    pub positive_margin: f32,
    /// Pending negative margin.
    pub negative_margin: f32,
    /// The boxes laid out against this context are formatting roots (flex /
    /// grid items, the root element): their own margins never collapse
    /// through them with their children's (CSS 2.1 §8.3.1, css-flexbox-1
    /// §4). The container that owns the context sets this.
    pub children_are_formatting_roots: bool,
    /// The owner already adjoined its first in-flow block child's top-margin
    /// chain into ITS OWN context (parent/first-child through-collapse). The
    /// first in-flow block child takes this flag and contributes no top
    /// margin of its own — that margin sits above the parent's border box.
    pub first_child_top_adjoined: bool,
    /// The owner's bottom edge is open (no border/padding-bottom, auto
    /// height, no BFC): the last in-flow child's bottom margin stays pending
    /// in the context on return so the owner can adjoin it to its own bottom
    /// margin, instead of being materialized into the content height.
    pub last_child_collapses_through: bool,
}

impl MarginCollapseContext {
    /// Create a new margin collapse context.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adjoin another context's pending margins into this one (positive and
    /// negative parts kept separate, as §8.3.1's max-positive + min-negative
    /// rule requires).
    pub fn absorb(&mut self, other: &MarginCollapseContext) {
        self.add_margin(other.positive_margin);
        self.add_margin(other.negative_margin);
    }

    /// Add a margin to the collapse context.
    pub fn add_margin(&mut self, margin: f32) {
        if margin >= 0.0 {
            self.positive_margin = self.positive_margin.max(margin);
        } else {
            self.negative_margin = self.negative_margin.min(margin);
        }
    }

    /// Resolve the collapsed margin.
    pub fn resolve(&self) -> f32 {
        self.positive_margin + self.negative_margin
    }

    /// Reset the context.
    pub fn reset(&mut self) {
        self.positive_margin = 0.0;
        self.negative_margin = 0.0;
    }
}

/// A 2D rectangle.
#[derive(Debug, Clone, Copy, Default)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Rect {
    pub fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub fn zero() -> Self {
        Self::default()
    }

    pub fn right(&self) -> f32 {
        self.x + self.width
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.height
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }
}

/// Edge sizes (margin, padding, border).
#[derive(Debug, Clone, Copy, Default)]
pub struct EdgeSizes {
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
}

impl EdgeSizes {
    pub fn horizontal(&self) -> f32 {
        self.left + self.right
    }

    pub fn vertical(&self) -> f32 {
        self.top + self.bottom
    }
}

/// Box dimensions including content, padding, border, and margin.
#[derive(Debug, Clone, Default)]
pub struct Dimensions {
    /// Content area.
    pub content: Rect,
    /// Padding.
    pub padding: EdgeSizes,
    /// Border.
    pub border: EdgeSizes,
    /// Margin.
    pub margin: EdgeSizes,
}

impl Dimensions {
    /// Get the padding box (content + padding).
    pub fn padding_box(&self) -> Rect {
        Rect {
            x: self.content.x - self.padding.left,
            y: self.content.y - self.padding.top,
            width: self.content.width + self.padding.horizontal(),
            height: self.content.height + self.padding.vertical(),
        }
    }

    /// Get the border box (content + padding + border).
    pub fn border_box(&self) -> Rect {
        let pb = self.padding_box();
        Rect {
            x: pb.x - self.border.left,
            y: pb.y - self.border.top,
            width: pb.width + self.border.horizontal(),
            height: pb.height + self.border.vertical(),
        }
    }

    /// Get the margin box (content + padding + border + margin).
    pub fn margin_box(&self) -> Rect {
        let bb = self.border_box();
        Rect {
            x: bb.x - self.margin.left,
            y: bb.y - self.margin.top,
            width: bb.width + self.margin.horizontal(),
            height: bb.height + self.margin.vertical(),
        }
    }
}

/// Type of layout box.
#[derive(Debug, Clone)]
pub enum BoxType {
    /// Block-level box.
    Block,
    /// Inline-level box.
    Inline,
    /// Anonymous block (for grouping inline content).
    AnonymousBlock,
    /// Text run.
    Text(String),
    /// Replaced element (image).
    /// Contains: (url, natural_width, natural_height)
    Image {
        url: String,
        natural_width: f32,
        natural_height: f32,
    },
    /// Form control (input, button, textarea, select).
    FormControl(FormControlType),
    /// A forced line break (`<br>`, CSS 2.1 §9.4.2): closes the current
    /// line box in its parent's inline flow, occupies no space, paints
    /// nothing. Before this variant existed `<br>` was an empty inline that
    /// the tree builder filtered out, so "a<br>b" rendered on one line on
    /// every page.
    LineBreak,
}

/// Type of form control for layout/rendering.
#[derive(Debug, Clone, PartialEq)]
pub enum FormControlType {
    /// Text input field.
    TextInput {
        value: String,
        placeholder: String,
        input_type: String, // "text", "password", "email", etc.
    },
    /// Multi-line text area.
    TextArea {
        value: String,
        placeholder: String,
        rows: u32,
        cols: u32,
    },
    /// Button element.
    Button {
        label: String,
        button_type: String, // "submit", "button", "reset"
    },
    /// Checkbox input.
    Checkbox { checked: bool },
    /// Radio button input.
    Radio { checked: bool, name: String },
    /// Select dropdown (placeholder for future).
    Select {
        options: Vec<String>,
        selected_index: Option<usize>,
        /// The `size` attribute (visible rows); 0 when unspecified.
        /// size > 1 (or `multiple`) renders as an inline listbox, not a
        /// dropdown — Chrome CfT-148 builds 16px per visible row + 2px.
        size: u32,
    },
}

/// Stacking context for z-index ordering.
#[derive(Debug, Clone, Default)]
pub struct StackingContext {
    /// Z-index value (0 for auto).
    pub z_index: i32,
    /// Whether this creates a new stacking context.
    pub creates_context: bool,
    /// Positioned children in this stacking context.
    pub positioned_children: Vec<usize>,
}

/// One line of a wrapped text box.
///
/// Populated by `layout_text` when a `BoxType::Text` box wraps onto multiple
/// lines; `None`/absent means the box is a single run (the pre-wrap layout
/// model) and renders exactly as before.
#[derive(Debug, Clone)]
pub struct TextLine {
    /// The line's text content (whitespace at the break point removed).
    pub text: String,
    /// Measured width of this line in px.
    pub width: f32,
    /// X offset from the box's content origin (per-line text-align).
    pub x_offset: f32,
    /// `text-align: justify` (css-text-3 §7.3): extra advance added to each
    /// word separator on this line so the line fills the container. 0 on
    /// every line that is not justified — the last line of a run (the block's
    /// last line, or a line that continues into a sibling) keeps its natural
    /// spacing, as does a line with no justification opportunity.
    pub justify_space: f32,
}

impl TextLine {
    /// Justification opportunities on this line: the word separators
    /// css-text-3 §7.3 lists that this engine can expand (U+0020 and
    /// U+00A0 — both shape as one glyph with its own advance).
    pub fn justification_opportunities(text: &str) -> usize {
        text.chars().filter(|c| Self::is_word_separator(*c)).count()
    }

    /// Whether `c` is a word separator that justification expands.
    pub fn is_word_separator(c: char) -> bool {
        matches!(c, ' ' | '\u{a0}')
    }

    /// The width a justified line's INK spans before expansion: the
    /// trimmed text shaped by the same advance path paint uses
    /// (`shape_line_advances`, letter/word-spacing applied). `TextLine::width`
    /// is not that number — the wrap keeps the collapsible space at the
    /// break point in both `text` and `width` (Georgia 17.6px: 4.4px), and
    /// slack measured against it left every justified line 4px short of the
    /// container. `None` when the shaper cannot give per-char advances
    /// (ligature clusters): paint cannot expand such a line either.
    pub fn natural_ink_width(
        text: &str,
        style: &rustkit_css::ComputedStyle,
        font_size: f32,
    ) -> Option<f32> {
        let trimmed = text.trim_end();
        if trimmed.is_empty() {
            return None;
        }
        shape_line_advances(trimmed, style, font_size).map(|adv| adv.iter().sum())
    }
}

/// Identity of the DOM element a layout box was generated from.
///
/// This exists so the geometry oracle can JOIN RustKit's exported layout tree
/// against Chrome's `layout-rects.json`, which is keyed by selector. Positional
/// matching is not an option: the two trees do not agree box-for-box (Chrome
/// skips zero-size and non-rendered elements; RustKit synthesizes anonymous
/// boxes), so pairing by index produces a confident WRONG answer rather than a
/// missing one.
///
/// `LayoutBox::identity` is `Option` on purpose and the `Option` is load-bearing:
/// anonymous boxes and text boxes have NO originating element, so they carry
/// `None` and must be EXCLUDED from comparison. Giving them a synthesized
/// identity would silently pair them with real Chrome elements and report
/// geometry failures that do not exist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElementIdentity {
    /// Document-order index of the originating element, assigned during layout
    /// tree construction. Stable within a single build; not stable across
    /// builds of different documents.
    pub element_id: usize,
    /// Lowercased tag name, e.g. `div`.
    pub tag: String,
    /// Chrome-compatible selector, the join key against `layout-rects.json`.
    /// Produced by mirroring `getSelector()` from the baseline capture script.
    pub selector: String,
}

/// A layout box in the layout tree.
#[derive(Debug)]
pub struct LayoutBox {
    /// Box type.
    pub box_type: BoxType,
    /// Computed dimensions.
    pub dimensions: Dimensions,
    /// Computed style.
    pub style: ComputedStyle,
    /// Child boxes.
    pub children: Vec<LayoutBox>,
    /// CSS position property.
    pub position: Position,
    /// Position offsets (top, right, bottom, left).
    pub offsets: PositionOffsets,
    /// Float property.
    pub float: Float,
    /// Clear property.
    pub clear: Clear,
    /// Z-index for stacking.
    pub z_index: i32,
    /// Whether this box creates a stacking context.
    pub stacking_context: Option<StackingContext>,
    /// Reference to containing block (for positioned elements).
    #[allow(dead_code)]
    pub containing_block_index: Option<usize>,
    /// Viewport dimensions for resolving vh/vw units.
    pub viewport: (f32, f32),
    /// Sticky positioning state (for position: sticky elements).
    pub sticky_state: Option<StickyState>,
    /// Optional element ID for intrinsic sizing cache.
    /// When set, enables caching of min-content/max-content calculations.
    pub element_id: Option<usize>,
    /// Identity of the originating DOM element, or `None` for anonymous and
    /// text boxes (which have no element and are excluded from oracle joins).
    /// Kept in lockstep with `element_id`; see `ElementIdentity`.
    pub identity: Option<Box<ElementIdentity>>,
    /// Resolved `href` when this box came from an `<a href>`. Hit testing
    /// reports the nearest such ancestor so a click on a link's text (or on
    /// an image inside it) navigates, without the hit path needing DOM
    /// access.
    pub link_href: Option<String>,
    /// Caret offset when this box is the FOCUSED text control, else `None`.
    /// Set by the engine at layout-build time from live edit state; the
    /// painter uses it to draw the caret and the focus ring. This was
    /// previously impossible ("focus tracking requires DOM node ID in
    /// LayoutBox") and is unblocked by `node_id` below.
    pub focused_caret: Option<usize>,
    /// Raw `NodeId` of the originating DOM node, when there is one.
    ///
    /// Carried as a plain `usize` so `rustkit-layout` keeps no dependency on
    /// `rustkit-dom`; the engine converts back at the boundary. This is the
    /// long-standing "requires node_id tracking in LayoutBox" TODO that
    /// blocked click-to-focus and keyboard dispatch — without it a hit test
    /// can locate a box but not the element it came from.
    pub node_id: Option<usize>,
    /// Wrapped lines for text boxes (`None` = single-run text, no wrap).
    pub text_lines: Option<Vec<TextLine>>,
    /// FLOW offset of visual line 0 when this text box was laid out
    /// MID-LINE by `layout_text_in_flow` (phase-5 split): line 0 starts at
    /// the inline cursor, not the container origin. `None` = block-path
    /// text (lone run or pure wrap). IFC Slice B2 uses this to tell "align
    /// every visual line" apart from "this line record owns only the LAST
    /// visual line" — a mid-line first fragment must never be re-aligned
    /// as if it owned its whole line.
    pub text_flow_first_offset: Option<f32>,
}

impl LayoutBox {
    /// Create a new layout box.
    pub fn new(box_type: BoxType, style: ComputedStyle) -> Self {
        Self {
            box_type,
            dimensions: Dimensions::default(),
            style,
            children: Vec::new(),
            position: Position::Static,
            offsets: PositionOffsets::default(),
            float: Float::None,
            clear: Clear::None,
            z_index: 0,
            stacking_context: None,
            containing_block_index: None,
            viewport: (0.0, 0.0),
            sticky_state: None,
            element_id: None,
            identity: None,
            link_href: None,
            focused_caret: None,
            node_id: None,
            text_lines: None,
            text_flow_first_offset: None,
        }
    }

    /// Create a new layout box with positioning.
    pub fn with_position(box_type: BoxType, style: ComputedStyle, position: Position) -> Self {
        let mut layout_box = Self::new(box_type, style);
        layout_box.position = position;

        // Create stacking context if positioned with z-index
        if position != Position::Static {
            layout_box.stacking_context = Some(StackingContext::default());
        }

        layout_box
    }

    /// Create a new layout box with float.
    pub fn with_float(box_type: BoxType, style: ComputedStyle, float: Float) -> Self {
        let mut layout_box = Self::new(box_type, style);
        layout_box.float = float;
        layout_box
    }

    /// Set z-index and create stacking context if needed.
    pub fn set_z_index(&mut self, z_index: i32) {
        self.z_index = z_index;
        if self.position != Position::Static {
            let mut ctx = self.stacking_context.take().unwrap_or_default();
            ctx.z_index = z_index;
            ctx.creates_context = true;
            self.stacking_context = Some(ctx);
        }
    }

    /// Set the element ID for intrinsic sizing cache support.
    ///
    /// When set, enables caching of expensive min-content/max-content
    /// calculations for this element across layout passes.
    pub fn set_element_id(&mut self, id: usize) {
        self.element_id = Some(id);
    }

    /// Get the element ID if set.
    pub fn element_id(&self) -> Option<usize> {
        self.element_id
    }

    /// Attach the originating element's identity.
    ///
    /// Sets `element_id` and `identity` together so the two can never disagree:
    /// a box with an identity always has the matching id, and a box without one
    /// has neither. Callers must NOT call this for anonymous or text boxes.
    pub fn set_identity(&mut self, identity: ElementIdentity) {
        self.element_id = Some(identity.element_id);
        self.identity = Some(Box::new(identity));
    }

    /// Get the originating element's identity, if this box came from an element.
    pub fn identity(&self) -> Option<&ElementIdentity> {
        self.identity.as_deref()
    }

    /// Set position offsets.
    pub fn set_offsets(
        &mut self,
        top: Option<f32>,
        right: Option<f32>,
        bottom: Option<f32>,
        left: Option<f32>,
    ) {
        self.offsets = PositionOffsets {
            top,
            right,
            bottom,
            left,
        };
    }

    /// Update sticky positions based on scroll state.
    ///
    /// This should be called before building the display list when scroll has changed.
    /// It recursively updates all sticky elements in the tree based on the current
    /// scroll position relative to their containing blocks.
    pub fn update_sticky_positions(&mut self, scroll_x: f32, scroll_y: f32, container_rect: Rect) {
        // Update this element's sticky state if it's sticky
        if let Some(ref mut sticky_state) = self.sticky_state {
            sticky_state.update(scroll_y, container_rect);

            // Apply the sticky adjustment to dimensions if stuck
            if sticky_state.is_stuck {
                if let Some(stuck_rect) = sticky_state.stuck_rect {
                    // Adjust content position to the stuck position
                    // We need to account for margin/border/padding
                    let border_box = self.dimensions.border_box();
                    let dy = stuck_rect.y - border_box.y;
                    self.dimensions.content.y += dy;

                    // Handle horizontal sticky if applicable
                    if sticky_state.offsets.left.is_some() || sticky_state.offsets.right.is_some() {
                        let dx = stuck_rect.x - border_box.x;
                        self.dimensions.content.x += dx;
                    }
                }
            }
        }

        // Determine the container rect for children
        // For scroll containers, use the content rect; otherwise pass through
        let child_container = if is_scroll_container(self.style.overflow_x, self.style.overflow_y) {
            self.dimensions.content
        } else {
            container_rect
        };

        // Recursively update children
        for child in &mut self.children {
            child.update_sticky_positions(scroll_x, scroll_y, child_container);
        }
    }

    /// Reset sticky positions to their original normal flow positions.
    ///
    /// Call this before relayout or when scroll position is reset.
    pub fn reset_sticky_positions(&mut self) {
        if let Some(ref sticky_state) = self.sticky_state {
            // Restore original position
            let original = sticky_state.original_rect;
            let border_box = self.dimensions.border_box();

            // Calculate offset from current to original
            let dx = original.x - border_box.x;
            let dy = original.y - border_box.y;

            self.dimensions.content.x += dx;
            self.dimensions.content.y += dy;
        }

        // Reset sticky state
        if let Some(ref mut sticky_state) = self.sticky_state {
            sticky_state.is_stuck = false;
            sticky_state.stuck_rect = None;
        }

        // Recursively reset children
        for child in &mut self.children {
            child.reset_sticky_positions();
        }
    }

    /// Perform layout within the given containing block.
    pub fn layout(&mut self, containing_block: &Dimensions) {
        self.layout_with_definite_height(containing_block, containing_block.content.height);
    }

    /// Perform layout with an explicit definite height for percentage resolution.
    /// This is used by grid layout when re-laying out children - the containing_block
    /// is used for positioning, while definite_height is used for percentage height resolution.
    pub fn layout_with_definite_height(
        &mut self,
        containing_block: &Dimensions,
        definite_height: f32,
    ) {
        self.layout_with_percent_base(
            containing_block,
            (definite_height > 0.0).then_some(definite_height),
        );
    }

    /// `layout_with_definite_height`, keeping a DEFINITE ZERO base distinct
    /// from "no definite base" — a bare `f32` collapses the two onto 0 and
    /// sends the zero case to the viewport fallback.
    pub(crate) fn layout_with_percent_base(
        &mut self,
        containing_block: &Dimensions,
        definite_height: Option<f32>,
    ) {
        match &self.box_type {
            BoxType::Block | BoxType::AnonymousBlock => {
                // Check for flex or grid container
                if self.style.display.is_flex() {
                    self.layout_block_with_definite_height(containing_block, definite_height);
                    // Flex layout is applied to children. No containing
                    // block goes over: on this path `containing_block` is
                    // often the STATIC-POSITION STAND-IN (its height is the
                    // parent's flow cursor), and treating that as a real
                    // containing block sizes an `inset: 0` overlay under an
                    // auto-height parent to the cursor. `reanchor_absolute`
                    // is the one place that always holds the real box.
                    flex::layout_flex_container(self, &self.dimensions.clone());
                } else if self.style.display.is_grid() {
                    self.layout_block_with_definite_height(containing_block, definite_height);
                    // Grid layout is applied to children
                    grid::layout_grid_container(
                        self,
                        self.dimensions.content.width,
                        self.dimensions.content.height,
                    );
                } else {
                    self.layout_block_with_definite_height(containing_block, definite_height);
                }
            }
            BoxType::Inline => {
                // Inline boxes: position at containing block's current content area
                self.layout_inline(containing_block);
            }
            BoxType::Text(text) => {
                // Text boxes: calculate dimensions based on text content
                self.layout_text(text.clone(), containing_block);
            }
            BoxType::Image {
                natural_width,
                natural_height,
                ..
            } => {
                // Replaced element: use intrinsic dimensions or explicit sizing
                self.layout_image(*natural_width, *natural_height, containing_block);
            }
            BoxType::FormControl(ref control) => {
                // Form controls are replaced elements with intrinsic sizing
                self.layout_form_control(control.clone(), containing_block);
            }
            BoxType::LineBreak => {
                // Zero-size marker; the parent's inline flow closes the line.
                self.dimensions.content = Rect::new(
                    containing_block.content.x,
                    containing_block.content.y + containing_block.content.height,
                    0.0,
                    0.0,
                );
            }
        }

        // Apply positioning offsets after normal layout
        self.apply_position_offsets(containing_block);
        // This box is final: anchor its abspos children to its padding box.
        self.reanchor_absolute_children();
    }

    /// Layout an inline box.
    fn layout_inline(&mut self, containing_block: &Dimensions) {
        // Calculate margins, padding, and borders for the inline box
        let d = &mut self.dimensions;
        let container_width = containing_block.content.width;

        d.margin.left = self.style.margin_left.to_px(16.0, 16.0, container_width);
        d.margin.right = self.style.margin_right.to_px(16.0, 16.0, container_width);
        // Vertical margins don't apply to inline elements
        d.margin.top = 0.0;
        d.margin.bottom = 0.0;

        d.padding.left = self.style.padding_left.to_px(16.0, 16.0, container_width);
        d.padding.right = self.style.padding_right.to_px(16.0, 16.0, container_width);
        d.padding.top = self.style.padding_top.to_px(16.0, 16.0, container_width);
        d.padding.bottom = self.style.padding_bottom.to_px(16.0, 16.0, container_width);

        d.border.left = self
            .style
            .border_left_width
            .to_px(16.0, 16.0, container_width);
        d.border.right = self
            .style
            .border_right_width
            .to_px(16.0, 16.0, container_width);
        d.border.top = self
            .style
            .border_top_width
            .to_px(16.0, 16.0, container_width);
        d.border.bottom = self
            .style
            .border_bottom_width
            .to_px(16.0, 16.0, container_width);

        // Position at containing block's content area
        d.content.x = containing_block.content.x + d.margin.left + d.border.left + d.padding.left;
        d.content.y = containing_block.content.y + containing_block.content.height;

        // Check for explicit CSS width first
        let explicit_width = match self.style.width {
            Length::Px(px) if px > 0.0 => Some(px),
            Length::Percent(pct) if pct > 0.0 => Some(pct / 100.0 * container_width),
            Length::Em(em) if em > 0.0 => {
                let font_size = match self.style.font_size {
                    Length::Px(px) => px,
                    _ => 16.0,
                };
                Some(em * font_size)
            }
            _ => None,
        };

        // Check for explicit CSS height
        let explicit_height = match self.style.height {
            Length::Px(px) if px > 0.0 => Some(px),
            Length::Percent(pct) if pct > 0.0 && containing_block.content.height > 0.0 => {
                Some(pct / 100.0 * containing_block.content.height)
            }
            Length::Em(em) if em > 0.0 => {
                let font_size = match self.style.font_size {
                    Length::Px(px) => px,
                    _ => 16.0,
                };
                Some(em * font_size)
            }
            _ => None,
        };

        // Layout inline children sequentially
        // Use the containing block's width for child layout, not our own (which might be 0)
        let available_width = containing_block.content.width;
        let mut cursor_x = 0.0;
        let mut max_height = 0.0f32;

        for child in &mut self.children {
            let mut cb = self.dimensions.clone();
            cb.content.x = self.dimensions.content.x + cursor_x;
            cb.content.width = available_width; // Pass parent's available width
            cb.content.height = 0.0;

            child.layout(&cb);

            cursor_x += child.dimensions.margin_box().width;
            max_height = max_height.max(child.dimensions.margin_box().height);
        }

        // Set content dimensions:
        // 1. Use explicit CSS width if specified
        // 2. Otherwise use computed width from children
        // 3. Ensure minimum width for padding/border contribution
        let computed_width = if let Some(w) = explicit_width {
            w
        } else if cursor_x > 0.0 {
            cursor_x
        } else {
            // Inline box with no children and no explicit width:
            // Use horizontal padding + border as minimum (inline-block behavior)
            let horizontal_box =
                self.dimensions.padding.horizontal() + self.dimensions.border.horizontal();
            horizontal_box
        };
        self.dimensions.content.width = computed_width;

        // Height: explicit height, else the CONTENT AREA (font ascent +
        // descent, §10.6.1) — not line-height. The line box still advances
        // by line-height in the parent loops; only the reported rect
        // changes. See inline_content_area.
        let min_height = self.dimensions.padding.vertical() + self.dimensions.border.vertical();
        let (content_area_height, _) = self.inline_content_area();
        let line_height = content_area_height;

        // Height calculation for inline boxes:
        // Inline boxes should always have at least line-height to maintain proper vertical rhythm.
        // This is critical for flex containers where inline items need proper sizing.
        // A non-replaced inline's rect height is its CONTENT AREA even when
        // children are taller (a 50px <img> inside an <a> overflows the
        // anchor's box in Chrome; the anchor stays font-tall). Children
        // still drive WIDTH via the cursor above. `line_height` here is the
        // content-area height — see the assignment at the declaration.
        let _ = max_height; // width path consumed it; height deliberately does not
        let computed_height = if let Some(h) = explicit_height {
            h
        } else {
            line_height.max(min_height)
        };
        self.dimensions.content.height = computed_height;
    }

    /// Single-line measured width of a text box (no wrapping), used by the
    /// block child loop to decide whether a text run fits on the current
    /// line box. Uses the same measurement as layout_text's single-line path.
    fn text_single_line_width(&self) -> f32 {
        let BoxType::Text(ref text) = self.box_type else {
            return 0.0;
        };
        let font_size = match self.style.font_size {
            Length::Px(px) => px,
            _ => 16.0,
        };
        let letter_spacing = match self.style.letter_spacing {
            Length::Px(px) => px,
            Length::Em(em) => em * font_size,
            Length::Rem(rem) => rem * 16.0,
            _ => 0.0,
        };
        let word_spacing = match self.style.word_spacing {
            Length::Px(px) => px,
            Length::Em(em) => em * font_size,
            Length::Rem(rem) => rem * 16.0,
            _ => 0.0,
        };
        measure_text_with_spacing(
            text,
            &self.style.font_family,
            font_size,
            self.style.font_weight,
            self.style.font_style,
            letter_spacing,
            word_spacing,
        )
        .width
    }

    /// Layout a text box.
    fn layout_text(&mut self, text: String, containing_block: &Dimensions) {
        self.layout_text_with_zero_wrap(text, containing_block, false);
    }

    /// Block-path text layout. `container_is_definite_zero`: the containing
    /// block's used width is an AUTHOR `width: 0` (css-text-3 §5.1 — a
    /// resolved width, text wraps at every opportunity it has), as opposed
    /// to the bare `container_width == 0` this path otherwise reads as "no
    /// resolved width yet" (intrinsic sizing pass — never wrap against it).
    /// Only the container knows which zero it is, so the inline-flow loops
    /// pass it down (WPT break-boundary-2-chars-001: `abc` in a zero-wide
    /// `word-break: break-all` inline-block is `a` / `b` / `c`).
    fn layout_text_with_zero_wrap(
        &mut self,
        text: String,
        containing_block: &Dimensions,
        container_is_definite_zero: bool,
    ) {
        // Get font size
        let font_size = match self.style.font_size {
            Length::Px(px) => px,
            _ => 16.0,
        };

        // Get letter-spacing and word-spacing in pixels
        // CSS "normal" keyword (Auto/Zero) maps to 0.0 via the wildcard
        let letter_spacing = match self.style.letter_spacing {
            Length::Px(px) => px,
            Length::Em(em) => em * font_size,
            Length::Rem(rem) => rem * 16.0, // Root font size assumed 16px
            _ => 0.0,
        };
        let word_spacing = match self.style.word_spacing {
            Length::Px(px) => px,
            Length::Em(em) => em * font_size,
            Length::Rem(rem) => rem * 16.0,
            _ => 0.0,
        };

        // Use proper text measurement for width with spacing
        let metrics = measure_text_with_spacing(
            &text,
            &self.style.font_family,
            font_size,
            self.style.font_weight,
            self.style.font_style,
            letter_spacing,
            word_spacing,
        );
        let text_width = metrics.width;

        let container_width = containing_block.content.width;
        self.text_lines = None;
        // Block-path text never starts mid-line; a box re-laid out here
        // (e.g. after a line wrap re-layout) must shed any stale phase-5
        // flow state from an earlier pass.
        self.text_flow_first_offset = None;

        // Wrap overflowing text into line boxes (CSS2 §9.4.2, css-text-3 §5).
        // container_width == 0 means the containing block has no resolved width
        // yet (intrinsic sizing pass) — never wrap against an unresolved width
        // — UNLESS the container said its zero is an author `width: 0`.
        let can_wrap = !matches!(
            self.style.white_space,
            rustkit_css::WhiteSpace::Nowrap | rustkit_css::WhiteSpace::Pre
        );
        let width_is_resolved = container_width > 0.0 || container_is_definite_zero;
        let overflows = can_wrap && width_is_resolved && text_width > container_width;
        // css-text-3 §4.1.1 / §5.1: under the pre family a preserved segment
        // break is a FORCED line break, whatever the width. The wrapper has
        // always split at mandatory breaks (`break_into_lines`), but nothing
        // reached it: the `white-space: pre` gate above never called it, and
        // the overflow test skipped it for any pre-wrap / pre-line run that
        // fit its container — so `<pre>` laid every source line on ONE line
        // box (article-typography's six-line code block: 65px vs 195).
        let has_segment_breaks = matches!(
            self.style.white_space,
            rustkit_css::WhiteSpace::Pre
                | rustkit_css::WhiteSpace::PreWrap
                | rustkit_css::WhiteSpace::PreLine
                | rustkit_css::WhiteSpace::BreakSpaces
        ) && text.contains(|c| c == '\n' || c == '\r');
        if overflows || has_segment_breaks {
            // Soft wrapping off (`pre`) or no resolved width: each segment
            // is one line however wide it is — only the forced breaks split.
            let wrap_width = if can_wrap && width_is_resolved {
                container_width
            } else {
                f32::INFINITY
            };
            let shaper = TextShaper::new();
            let chain = FontFamilyChain::from_css_value(&self.style.font_family);
            if let Ok(lines) = shaper.wrap_text_white_space(
                &text,
                &chain,
                self.style.font_weight,
                self.style.font_style,
                self.style.font_stretch,
                font_size,
                wrap_width,
                effective_word_break(&self.style),
                self.style.overflow_wrap,
                self.style.white_space,
            ) {
                // A single-segment run with a trailing newline ("abc\n") is
                // still line-box text: the break character must not reach
                // the single-line path's measurement.
                if lines.len() > 1 || has_segment_breaks {
                    // Known phase-1 gap: wrap_text shapes without letter/word-
                    // spacing, so spaced text may break slightly late. Ledgered.
                    //
                    // IFC Slice A: alignment is a property of the LINE, owned
                    // by the parent's apply_text_align_offset — leaves never
                    // self-align (x_offset stays 0 here; the parent pass sets
                    // per-line offsets). Two alignment sources meant mixed
                    // runs double-shifted or half-shifted depending on which
                    // path a child happened to take.
                    let text_lines: Vec<TextLine> = lines
                        .iter()
                        .map(|l| TextLine {
                            text: l.text(),
                            width: l.width,
                            x_offset: 0.0,
                            justify_space: 0.0,
                        })
                        .collect();
                    let max_line_width = text_lines.iter().map(|l| l.width).fold(0.0f32, f32::max);
                    let line_count = text_lines.len();
                    self.text_lines = Some(text_lines);
                    self.dimensions.content.x = containing_block.content.x;
                    self.dimensions.content.y =
                        containing_block.content.y + containing_block.content.height;
                    // Same clamp as the single-line path: an unresolved
                    // (zero) container width must not zero the box.
                    self.dimensions.content.width = if container_width > 0.0 {
                        max_line_width.min(container_width)
                    } else {
                        max_line_width
                    };
                    self.dimensions.content.height =
                        line_count as f32 * run_line_height(&self.style, font_size, &metrics);
                    return;
                }
            }
        }

        // Single-line path (fits, nowrap/pre, or wrapping unavailable).
        // IFC Slice A: no leaf self-align — the flow cursor owns x here and
        // the PARENT's apply_text_align_offset shifts recorded lines as a
        // unit. (The old per-leaf offset centered each run against the full
        // block width independently, which is why mixed runs mis-centered.)
        self.dimensions.content.x = containing_block.content.x;
        self.dimensions.content.y = containing_block.content.y + containing_block.content.height;
        // Use text width, clamping to containing block only if it has a meaningful width
        // This prevents text from collapsing to 0 width in intrinsic sizing scenarios
        self.dimensions.content.width = if container_width > 0.0 {
            text_width.min(container_width)
        } else {
            text_width // Don't clamp if containing block has no width yet
        };
        self.dimensions.content.height = run_line_height(&self.style, font_size, &metrics);
    }

    /// A non-atomic inline child whose text wrapped onto several line boxes:
    /// `(line count, end x of the last line relative to the container's
    /// content left, that text's line height)`. `None` when every
    /// descendant sits on one line. The last wrapped text descendant wins
    /// (it is the one the flow continues after).
    fn inline_wrapped_tail(inline: &LayoutBox, container_left: f32) -> Option<(usize, f32, f32)> {
        fn walk(b: &LayoutBox, left: f32, out: &mut Option<(usize, f32, f32)>) {
            if let (BoxType::Text(_), Some(lines)) = (&b.box_type, b.text_lines.as_ref()) {
                if let Some(last) = lines.last().filter(|_| lines.len() > 1) {
                    let end = b.dimensions.content.x - left + last.x_offset + last.width;
                    *out = Some((lines.len(), end.max(0.0), b.get_line_height()));
                }
            }
            for c in &b.children {
                walk(c, left, out);
            }
        }
        let mut out = None;
        walk(inline, container_left, &mut out);
        out
    }

    /// Whether a text child that does NOT fit the remaining line space
    /// should split across line boxes (phase-5 IFC flow) rather than drop
    /// to its own block row. IFC Slice B2 opened the split to Center/Right:
    /// closed lines get per-visual-line alignment (`align_split_close`),
    /// so "a run spanning several line boxes cannot be shifted as one
    /// child" no longer forces the block path.
    ///
    /// n49: the split applies from the line START too. The old `cursor_x >
    /// 0` gate sent a paragraph's FIRST long run down the block path, which
    /// closes every line it makes — so `<p>long text… <span>x</span> more`
    /// put the span on a fresh line under the run instead of on the run's
    /// last line (article-typography's `.highlight`; every paragraph that
    /// opens with a long run and carries a link/em/strong later — most of
    /// them). The flow path at cursor 0 is the block path with the last
    /// line left open.
    fn text_splits_inline(child: &LayoutBox, _cursor_x: f32) -> bool {
        matches!(child.box_type, BoxType::Text(_))
            && !matches!(
                child.style.white_space,
                rustkit_css::WhiteSpace::Nowrap | rustkit_css::WhiteSpace::Pre
            )
    }

    /// Lay out a text box that STARTS MID-LINE in an inline formatting
    /// context: the first line fills the remaining width of the current
    /// line box (container width minus `first_line_offset`), subsequent
    /// lines wrap at the container's full width (CSS2 §9.4.2).
    ///
    /// Positions the box at the top of the current line box (`line_top_y`);
    /// line 0 carries `x_offset = first_line_offset`. Returns
    /// `(line_count, last_line_width)` so the block child loop can continue
    /// the last line after this run. Only called for `BoxType::Text`
    /// children whose single-line width exceeds the remaining line space.
    fn layout_text_in_flow(
        &mut self,
        containing_block: &Dimensions,
        line_top_y: f32,
        first_line_offset: f32,
    ) -> (usize, f32) {
        let BoxType::Text(ref text) = self.box_type else {
            return (0, 0.0);
        };
        let text = text.clone();
        let container_width = containing_block.content.width;
        let first_line_width = (container_width - first_line_offset).max(0.0);
        let font_size = match self.style.font_size {
            Length::Px(px) => px,
            _ => 16.0,
        };

        let shaper = TextShaper::new();
        let chain = FontFamilyChain::from_css_value(&self.style.font_family);
        // A run that starts at an inline cursor past the line start says so
        // explicitly: with a zero-wide container the first and full budgets
        // are both 0 and the shaper's `first < max` proxy cannot tell — it
        // glued the first grapheme onto the open line. A run at the line
        // START (n49: the flow path now takes those too) wraps as the block
        // path did — an unbreakable first word overflows its line instead
        // of leaving an empty line box above itself.
        let wrapped = if first_line_offset > 0.0 {
            shaper.wrap_text_mid_line_white_space(
                &text,
                &chain,
                self.style.font_weight,
                self.style.font_style,
                self.style.font_stretch,
                font_size,
                first_line_width,
                container_width,
                effective_word_break(&self.style),
                self.style.overflow_wrap,
                self.style.white_space,
            )
        } else {
            shaper.wrap_text_white_space(
                &text,
                &chain,
                self.style.font_weight,
                self.style.font_style,
                self.style.font_stretch,
                font_size,
                container_width,
                effective_word_break(&self.style),
                self.style.overflow_wrap,
                self.style.white_space,
            )
        };
        let lines = match wrapped {
            Ok(lines) if !lines.is_empty() => lines,
            _ => {
                // Shaping failed — fall back to the block path's layout.
                self.layout_text(text, containing_block);
                return (1, self.dimensions.content.width);
            }
        };

        // The run's extents across all its lines (fallback faces included),
        // so the line height matches what `render_text` derives from
        // measuring the same text.
        let mut run_metrics = TextMetrics::default();
        for line in &lines {
            run_metrics.ascent = run_metrics.ascent.max(line.ascent());
            run_metrics.descent = run_metrics.descent.max(line.descent());
            for run in &line.runs {
                run_metrics.leading = run_metrics.leading.max(run.metrics.leading);
            }
        }
        let line_height = run_line_height(&self.style, font_size, &run_metrics);

        let text_lines: Vec<TextLine> = lines
            .iter()
            .enumerate()
            .map(|(i, l)| TextLine {
                text: l.text(),
                width: l.width,
                x_offset: if i == 0 { first_line_offset } else { 0.0 },
                justify_space: 0.0,
            })
            .collect();
        let line_count = text_lines.len();
        let last_width = text_lines.last().map(|l| l.width).unwrap_or(0.0);
        self.text_lines = Some(text_lines);
        self.text_flow_first_offset = Some(first_line_offset);
        self.dimensions.content.x = containing_block.content.x;
        self.dimensions.content.y = line_top_y;
        self.dimensions.content.width = container_width;
        self.dimensions.content.height = line_count as f32 * line_height;
        (line_count, last_width)
    }

    /// Layout a replaced element (image).
    fn layout_image(
        &mut self,
        natural_width: f32,
        natural_height: f32,
        containing_block: &Dimensions,
    ) {
        // A replaced element carries its own box decoration. Until 2026-08-22
        // this function left margin/border/padding at zero, so `border_box()`
        // WAS the content box and an image at its natural size never gained
        // its border: images-intrinsic test1 (100x100 natural,
        // `border: 1px solid red`) measured 100 where Chrome builds 102.
        // The eleven sized tests on that page hid it — under the corpus's
        // `box-sizing: border-box` a specified size IS the border box, so a
        // renderer that ignores the border and one that subtracts it agree
        // on every box except the `auto` one.
        let cb_width = containing_block.content.width;
        {
            let d = &mut self.dimensions;
            d.margin.left = self.style.margin_left.to_px(16.0, 16.0, cb_width);
            d.margin.right = self.style.margin_right.to_px(16.0, 16.0, cb_width);
            d.margin.top = self.style.margin_top.to_px(16.0, 16.0, cb_width);
            d.margin.bottom = self.style.margin_bottom.to_px(16.0, 16.0, cb_width);
            d.border.left = self.style.border_left_width.to_px(16.0, 16.0, cb_width);
            d.border.right = self.style.border_right_width.to_px(16.0, 16.0, cb_width);
            d.border.top = self.style.border_top_width.to_px(16.0, 16.0, cb_width);
            d.border.bottom = self.style.border_bottom_width.to_px(16.0, 16.0, cb_width);
            d.padding.left = self.style.padding_left.to_px(16.0, 16.0, cb_width);
            d.padding.right = self.style.padding_right.to_px(16.0, 16.0, cb_width);
            d.padding.top = self.style.padding_top.to_px(16.0, 16.0, cb_width);
            d.padding.bottom = self.style.padding_bottom.to_px(16.0, 16.0, cb_width);
        }
        let horizontal_decoration = self.dimensions.border.left
            + self.dimensions.border.right
            + self.dimensions.padding.left
            + self.dimensions.padding.right;
        let vertical_decoration = self.dimensions.border.top
            + self.dimensions.border.bottom
            + self.dimensions.padding.top
            + self.dimensions.padding.bottom;
        let border_box_sizing = self.style.box_sizing == BoxSizing::BorderBox;

        // Calculate explicit dimensions from style. Every specified size below
        // (width/height and the maxima) is converted to a CONTENT size, since
        // that is what the intrinsic-sizing rules and `dimensions.content` are
        // expressed in; `auto` stays absent and takes the natural size.
        let explicit_width = replaced_content_size(
            match self.style.width {
                Length::Px(px) => Some(px),
                Length::Percent(pct) => Some(pct / 100.0 * containing_block.content.width),
                _ => None,
            },
            horizontal_decoration,
            border_box_sizing,
        );

        let explicit_height = replaced_content_size(
            match self.style.height {
                Length::Px(px) => Some(px),
                Length::Percent(pct) => Some(pct / 100.0 * containing_block.content.height),
                _ => None,
            },
            vertical_decoration,
            border_box_sizing,
        );

        // A specified `aspect-ratio` replaces the natural ratio before the
        // intrinsic-sizing rules run, so a missing axis is derived from the
        // ratio rather than from the image's own proportions.
        let (explicit_width, explicit_height) = preferred_ratio_sizes(
            explicit_width,
            explicit_height,
            if natural_width > 0.0 {
                Some(natural_width)
            } else {
                None
            },
            self.style.aspect_ratio,
            horizontal_decoration,
            vertical_decoration,
            border_box_sizing,
        );

        // Determine final dimensions using intrinsic size calculation
        let (mut width, mut height) = crate::images::calculate_intrinsic_size(
            if natural_width > 0.0 {
                Some(natural_width)
            } else {
                None
            },
            if natural_height > 0.0 {
                Some(natural_height)
            } else {
                None
            },
            explicit_width,
            explicit_height,
            containing_block.content.width,
        );

        // CSS 2.1 §10.4: max-width/max-height constrain replaced elements while
        // preserving the aspect ratio (max-width applied first, then max-height,
        // matching the constraint-violation table for the common cases).
        let max_width = replaced_content_size(
            match self.style.max_width {
                Length::Px(px) => Some(px),
                Length::Percent(pct) => Some(pct / 100.0 * containing_block.content.width),
                _ => None,
            },
            horizontal_decoration,
            border_box_sizing,
        );
        let max_height = replaced_content_size(
            match self.style.max_height {
                Length::Px(px) => Some(px),
                Length::Percent(pct) => Some(pct / 100.0 * containing_block.content.height),
                _ => None,
            },
            vertical_decoration,
            border_box_sizing,
        );
        if let Some(mw) = max_width {
            if width > mw && width > 0.0 {
                height = if height > 0.0 {
                    mw * height / width
                } else {
                    height
                };
                width = mw;
            }
        }
        if let Some(mh) = max_height {
            if height > mh && height > 0.0 {
                width = if width > 0.0 {
                    mh * width / height
                } else {
                    width
                };
                height = mh;
            }
        }

        // Position within containing block. The CONTENT box sits inside this
        // element's own margin/border/padding, so the BORDER box starts at the
        // containing block's content edge — the same offsets
        // `calculate_block_position` applies to a block box.
        self.dimensions.content.x = containing_block.content.x
            + self.dimensions.margin.left
            + self.dimensions.border.left
            + self.dimensions.padding.left;
        self.dimensions.content.y = containing_block.content.y
            + containing_block.content.height
            + self.dimensions.margin.top
            + self.dimensions.border.top
            + self.dimensions.padding.top;
        self.dimensions.content.width = width;
        self.dimensions.content.height = height;
    }

    /// Layout a form control (input, button, textarea, etc.)
    fn layout_form_control(&mut self, control: FormControlType, containing_block: &Dimensions) {
        let font_size = match self.style.font_size {
            Length::Px(px) => px,
            _ => 16.0,
        };

        let (intrinsic_width, intrinsic_height) = form_control_intrinsic_size(&self.style, &control);

        // Override with explicit CSS dimensions if specified, but always fall back to intrinsic
        // if the explicit value resolves to zero (e.g., percent of zero-height container)
        let width = match self.style.width {
            Length::Px(px) if px > 0.0 => px,
            Length::Percent(pct) => {
                let resolved = pct / 100.0 * containing_block.content.width;
                if resolved > 0.0 {
                    resolved
                } else {
                    intrinsic_width
                }
            }
            Length::Em(em) if em > 0.0 => em * font_size,
            _ => intrinsic_width,
        };

        let height = match self.style.height {
            Length::Px(px) if px > 0.0 => px,
            Length::Percent(pct) => {
                let resolved = pct / 100.0 * containing_block.content.height;
                // CRITICAL: Fall back to intrinsic height if percent resolves to 0
                // This fixes form controls in flex containers before flex layout runs
                if resolved > 0.0 {
                    resolved
                } else {
                    intrinsic_height
                }
            }
            Length::Em(em) if em > 0.0 => em * font_size,
            _ => intrinsic_height,
        };

        // Author margins are part of the control's margin box: the line box
        // advances by them and its height counts them (css-selectors §4/§6:
        // `input{margin:4px 0}` / `button{margin:4px}` rows built 37.5/33.5
        // where Chrome builds 43/39 — the margins were never resolved, so
        // margin_box() was the bare rect). The blob/composed height above is
        // the BORDER box, so padding/border stay folded into content here.
        let cw = containing_block.content.width;
        self.dimensions.margin.top = self.length_to_px(&self.style.margin_top, cw);
        self.dimensions.margin.bottom = self.length_to_px(&self.style.margin_bottom, cw);
        self.dimensions.margin.left = self.length_to_px(&self.style.margin_left, cw);
        self.dimensions.margin.right = self.length_to_px(&self.style.margin_right, cw);

        // Position within containing block (the inline path re-places the
        // box from its margin edge; the block path lands here).
        self.dimensions.content.x = containing_block.content.x + self.dimensions.margin.left;
        self.dimensions.content.y = containing_block.content.y
            + containing_block.content.height
            + self.dimensions.margin.top;
        self.dimensions.content.width = width;
        self.dimensions.content.height = height;
    }

    /// Space the line-box strut leaves under an inline-level replaced element
    /// sitting on the baseline: font descent plus half-leading (CSS 2.1 §10.8).
    /// Approximation until a real line-box model exists — without it, a block
    /// containing a lone inline <img> is measurably shorter than Chrome's
    /// (the fixture-visible symptom: every .container ~6px short, compounding
    /// down the page). Computed from the CONTAINER's font/line-height, since
    /// the strut belongs to the line box, not the image.
    /// True when this inline-level box's baseline is its bottom margin edge
    /// (CSS2 §10.8.1): replaced elements (images), and atomic inlines with no
    /// in-flow line boxes. The line box then extends below the box by the
    /// strut's descent + half-leading; boxes with in-flow content carry their
    /// own below-baseline part inside their margin height instead.
    fn baseline_is_bottom_edge(&self) -> bool {
        if matches!(self.box_type, BoxType::Image { .. }) {
            return true;
        }
        // BARE form controls carry a SYNTHETIC baseline — the inner text
        // line's baseline (HTML UA behavior; Chrome CfT-148) — not their
        // bottom margin edge. Their below-baseline part lives INSIDE the
        // border box (form_control_baseline_hang), so the line must not
        // extend the strut descent under them: bottom-edge treatment made
        // every control line strut_descent too tall (input rows 25 vs Chrome
        // 24, a fixed 50px button line 56 vs Chrome 50), compounding per row.
        // Checkbox / radio have no inner text line: their baseline is the
        // bottom margin edge (Blink), so the strut hangs below them and a
        // sibling label's text drops to that baseline — css-selectors §4:
        // 20px checkbox + "Checkbox" label is a 23px line with the label's
        // top at +6 in Chrome.
        // Every other control (bare OR author-padded) uses the hang model.
        // Author-padded controls used to keep the bottom-edge model because
        // it "measured closer" (line 39 vs 30.3 for the pad-8 buttons) —
        // that calibration was made while the controls' MARGINS were never
        // resolved (n43); with the 4px margins in the margin box, the hang
        // model builds Chrome's 39 and the bottom-edge model overshoots to
        // 41.5 (margin box + strut descent).
        // A textarea is a scroll container (Chrome's UA `overflow: auto`),
        // so like any inline-block whose overflow is not visible its
        // baseline is the bottom margin edge (CSS2 §10.8.1) — n53
        // form-controls: a bare 32px textarea alone on a line builds a 38px
        // line in Chrome (the strut's descent hangs below it); the hang
        // model built 32 and slid every section below by 6.
        if let BoxType::FormControl(control) = &self.box_type {
            return matches!(
                control,
                FormControlType::Checkbox { .. }
                    | FormControlType::Radio { .. }
                    | FormControlType::TextArea { .. }
            );
        }
        if !self.style.display.is_atomic_inline() {
            return false;
        }
        // An inline-block with in-flow line boxes sits on its LAST line's
        // baseline (inline_block_baseline_y); one with none, or with an
        // overflow other than visible, on its bottom margin edge. The
        // overflow clause is CSS2 §10.8.1's and applies to inline-block
        // ONLY: an inline-flex/grid scroll container keeps its content
        // baseline (Blink ShouldIgnoreOverflowPropertyForInlineBlockBaseline
        // — about's `a.sponsor-btn { display: inline-flex; overflow: hidden }`
        // hung a strut descent under itself and shifted the page 3.4px).
        if self.children.is_empty() {
            return true;
        }
        let clipped = self.style.overflow_x != rustkit_css::Overflow::Visible
            || self.style.overflow_y != rustkit_css::Overflow::Visible;
        (clipped && self.style.display == rustkit_css::Display::InlineBlock)
            || self.inline_block_baseline_y().is_none()
    }

    /// CSS2 §10.8.1: the baseline of an inline-block with in-flow content is
    /// the baseline of its last line box — the last in-flow text run's last
    /// line (a wrapped `<label>` hangs its single-line siblings off its
    /// SECOND line), or, through nested blocks, the last descendant that
    /// carries a baseline (text, control, image). None when no in-flow
    /// descendant does (bottom-edge fallback). Absolute y in the box's
    /// current geometry — read BEFORE the box is shifted.
    fn inline_block_baseline_y(&self) -> Option<f32> {
        for c in self.children.iter().rev() {
            if matches!(c.position, Position::Absolute | Position::Fixed)
                || c.float != Float::None
                || c.style.display == rustkit_css::Display::None
            {
                continue;
            }
            match &c.box_type {
                BoxType::Text(_) => {
                    let fs = match c.style.font_size {
                        Length::Px(px) => px,
                        _ => 16.0,
                    };
                    let m = measure_text_advanced(
                        "x",
                        &c.style.font_family,
                        fs,
                        c.style.font_weight,
                        c.style.font_style,
                    );
                    let (ascent, descent) = if m.ascent > 0.0 {
                        (m.ascent, m.descent)
                    } else {
                        (fs * 0.8, fs * 0.2)
                    };
                    let line_h = resolve_line_height(&c.style, fs);
                    let half_leading = half_leading(line_h, ascent, descent);
                    // Last line's baseline: the run's bottom minus the
                    // below-baseline part of one line (paint seats each
                    // line at content.y + i * line-height).
                    let bottom = c.dimensions.content.y + c.dimensions.content.height;
                    return Some(bottom - (half_leading + descent));
                }
                BoxType::FormControl(_) => {
                    let mb = c.dimensions.margin_box();
                    let hang = if c.baseline_is_bottom_edge() {
                        0.0
                    } else {
                        c.form_control_baseline_hang()
                    };
                    return Some(mb.y + mb.height - hang);
                }
                BoxType::Image { .. } => {
                    let mb = c.dimensions.margin_box();
                    return Some(mb.y + mb.height);
                }
                BoxType::LineBreak => continue,
                _ => {
                    if c.style.display.is_atomic_inline() && c.baseline_is_bottom_edge() {
                        let mb = c.dimensions.margin_box();
                        return Some(mb.y + mb.height);
                    }
                    if let Some(b) = c.inline_block_baseline_y() {
                        return Some(b);
                    }
                }
            }
        }
        None
    }

    /// Distance from a form control's synthetic baseline (its inner text
    /// line's baseline) to its bottom margin edge: font descent +
    /// half-leading + author bottom padding/border. Chrome hangs exactly
    /// this much of a control below the line baseline (bare 19px input:
    /// ~4-5px), where the bottom-edge model hung the whole box + strut.
    fn form_control_baseline_hang(&self) -> f32 {
        let font_size = match self.style.font_size {
            Length::Px(px) => px,
            _ => 16.0,
        };
        let line_height = self.style.line_height.to_px(font_size);
        let metrics = measure_text_advanced(
            "x",
            &self.style.font_family,
            font_size,
            self.style.font_weight,
            self.style.font_style,
        );
        let (ascent, descent) = if metrics.ascent > 0.0 {
            (metrics.ascent, metrics.descent)
        } else {
            (font_size * 0.8, font_size * 0.2)
        };
        let half_leading = half_leading(line_height, ascent, descent);
        // A single-line control taller than its text line (author `height`)
        // centres the line in its content box, so half the spare height
        // hangs below the baseline too. n54 form-controls §4: a `height:
        // 50px` button read as 44.6 above + 5.4 below; under the 6px strut
        // descent that is a 50.6px line for Chrome's 50.
        let centres_its_line = match &self.box_type {
            BoxType::FormControl(FormControlType::Button { .. })
            | BoxType::FormControl(FormControlType::TextInput { .. }) => true,
            BoxType::FormControl(FormControlType::Select { size, .. }) => *size <= 1,
            _ => false,
        };
        let spare_below = if centres_its_line && !matches!(self.style.height, Length::Auto) {
            ((self.dimensions.content.height - line_height) / 2.0).max(0.0)
        } else {
            0.0
        };
        descent
            + half_leading
            + spare_below
            + self.dimensions.padding.bottom
            + self.dimensions.border.bottom
    }

    /// Content area of a NON-REPLACED inline box and the half-leading that
    /// centers it in its own line box.
    ///
    /// CSS2 §10.6.1: an inline's height is its content area — font ascent +
    /// descent — NOT its line-height; §10.8.1 distributes the leading half
    /// above and half below. Chrome's element rects agree (an <a> at
    /// font-size 16 / line-height 1.5 reads ~18px tall at a ~3.8px offset,
    /// never 24). Reporting the line box as the element rect put every
    /// inline element's border box a half-leading high and a leading tall —
    /// 24.9% of all Gate A failure rows on the honest local board.
    fn inline_content_area(&self) -> (f32, f32) {
        let font_size = match self.style.font_size {
            Length::Px(px) => px,
            _ => 16.0,
        };
        let line_height = resolve_line_height(&self.style, font_size);
        let metrics = measure_text_advanced(
            "x",
            &self.style.font_family,
            font_size,
            self.style.font_weight,
            self.style.font_style,
        );
        let (ascent, descent) = if metrics.ascent > 0.0 {
            (metrics.ascent, metrics.descent)
        } else {
            (font_size * 0.8, font_size * 0.2)
        };
        let content = ascent + descent;
        let half_leading = half_leading(line_height, ascent, descent);
        (content, half_leading)
    }

    /// Seat a non-replaced inline's rect on its content area, a half-leading
    /// below the line top. Its direct TEXT children stay where they are: a
    /// text box is the line-height-tall slot at the line top, and paint seats
    /// the glyphs inside that slot from the run's own leading
    /// (`blink_baseline_offset`). Translating them too applied the leading
    /// twice — every `<span>`/`<a>`/`<strong>` on a line with leading painted
    /// its text below its neighbours (article-typography `.meta`, 14.4px on a
    /// 23.04px line: baseline 184 for Chrome's 181).
    ///
    /// Nested inlines (`<a><em>text</em></a>`) have no shift of their own — the
    /// outermost one carries their rects — so the rule recurses: inline rects
    /// move, text leaves at any inline depth stay, anything else (an atomic
    /// inline) moves whole. Stopping at direct children left `span > strong`
    /// text a full half-leading low (7px on a 16px/32px line).
    fn shift_inline_content_area(&mut self, half_leading: f32) {
        self.dimensions.content.y += half_leading;
        for sub in &mut self.children {
            match sub.box_type {
                BoxType::Text(_) => {}
                BoxType::Inline => sub.shift_inline_content_area(half_leading),
                _ => crate::flex::translate_subtree(sub, 0.0, half_leading),
            }
        }
    }

    /// Above/below-baseline extents of a line of text in `s`, half-leading
    /// included — the strut when `s` is the container's style (CSS2 §10.8.1).
    fn text_baseline_extents(s: &ComputedStyle) -> (f32, f32) {
        let fs = match s.font_size {
            Length::Px(px) => px,
            _ => 16.0,
        };
        let m = measure_text_advanced("x", &s.font_family, fs, s.font_weight, s.font_style);
        let line_h = resolve_line_height(s, fs);
        let half_leading = half_leading(line_h, m.ascent, m.descent);
        (half_leading + m.ascent, half_leading + m.descent)
    }

    /// The same split for sizing a line box: the two parts sum to the
    /// line-height EXACTLY. Where a face's ascent + descent exceeds its
    /// `normal` line-height by a rounding sliver, the clamped half-leading
    /// above would make every text line that sliver taller than the run's
    /// own line boxes (4 lines read 73.69 for 73.60).
    ///
    /// The part above the baseline is a WHOLE pixel, as in Blink: a face's
    /// ascent and descent are rounded (SimpleFontData) and the half-leading
    /// added above is floored (FontHeight::AddLeading); the fraction of the
    /// line-height stays below. With the raw metrics a 36px wrapped label
    /// over a 16px/24px strut summed to 37.58 for Chrome's 37 (form-controls
    /// §5), and every such row pushed the page down by the sliver.
    fn text_line_box_extents(s: &ComputedStyle) -> (f32, f32) {
        let fs = match s.font_size {
            Length::Px(px) => px,
            _ => 16.0,
        };
        let m = measure_text_advanced("x", &s.font_family, fs, s.font_weight, s.font_style);
        let (ascent, descent) = if m.ascent > 0.0 {
            (m.ascent.round(), m.descent.round())
        } else {
            ((fs * 0.8).round(), (fs * 0.2).round())
        };
        let line_h = resolve_line_height(s, fs);
        let above = ascent + ((line_h - (ascent + descent)) / 2.0).floor();
        (above, (line_h - above).max(0.0))
    }

    /// This line member's extents above and below the line's baseline, for
    /// the line box's height (CSS2 §10.8.1: the line box spans the highest
    /// box top to the lowest box bottom once every member sits on the
    /// baseline, the strut included). Mirrors apply_vertical_align's
    /// placement arms. None for members the align pass leaves at the line
    /// top: `vertical-align: top|bottom` boxes,
    /// which hang from the line edge and swallow the strut instead of
    /// stacking on its descent; `middle` keeps the loop's own accounting
    /// (it centres on the container's x-height, not on this box's font).
    /// Reads the member's current geometry.
    fn line_member_baseline_extents(&self) -> Option<(f32, f32)> {
        if matches!(
            self.style.vertical_align,
            rustkit_css::VerticalAlign::Top
                | rustkit_css::VerticalAlign::Bottom
                | rustkit_css::VerticalAlign::Middle
        ) {
            return None;
        }
        // A non-atomic inline sits on the baseline as a line of text in its
        // own font (its rect is only the content area; the line sees the
        // line-height split).
        if matches!(self.box_type, BoxType::Text(_) | BoxType::Inline) {
            return Some(Self::text_line_box_extents(&self.style));
        }
        let h = self.dimensions.margin_box().height;
        let above = if matches!(self.box_type, BoxType::FormControl(_))
            && !self.baseline_is_bottom_edge()
        {
            (h - self.form_control_baseline_hang()).max(0.0)
        } else if self.baseline_is_bottom_edge() {
            h
        } else if self.style.display.is_atomic_inline() {
            let d = &self.dimensions;
            let top = d.content.y - d.margin.top - d.border.top - d.padding.top;
            // Whole pixels, for the same reason as text_line_box_extents: the
            // inner baseline is a stack of line boxes plus a text ascent
            // whose fraction belongs BELOW it (a wrapped 2 x 18px label reads
            // 31.54 here; Blink's is 18 + 13 = 31).
            (self.inline_block_baseline_y()? - top + 0.01).floor()
        } else {
            return None;
        };
        Some((above, (h - above).max(0.0)))
    }

    fn inline_strut_descent(&self) -> f32 {
        let font_size = match self.style.font_size {
            Length::Px(px) => px,
            _ => 16.0,
        };
        let line_height = resolve_line_height(&self.style, font_size);
        let metrics = measure_text_advanced(
            "x",
            &self.style.font_family,
            font_size,
            self.style.font_weight,
            self.style.font_style,
        );
        let (ascent, descent) = if metrics.ascent > 0.0 {
            (metrics.ascent, metrics.descent)
        } else {
            (font_size * 0.8, font_size * 0.2)
        };
        let half_leading = half_leading(line_height, ascent, descent);
        descent + half_leading
    }

    /// Get line height for text layout.
    fn get_line_height(&self) -> f32 {
        let font_size = match self.style.font_size {
            Length::Px(px) => px,
            _ => 16.0,
        };
        resolve_line_height(&self.style, font_size)
    }

    /// Perform layout with margin collapse context.
    pub fn layout_with_collapse(
        &mut self,
        containing_block: &Dimensions,
        margin_context: &mut MarginCollapseContext,
        float_context: &mut FloatContext,
    ) {
        self.layout_with_collapse_in(containing_block, margin_context, float_context, None);
    }

    /// `layout_with_collapse`, plus the containing block's DEFINITE content
    /// height for percentage resolution (see
    /// `definite_content_height_for_children`). `None` keeps the historical
    /// behaviour: percentages read `containing_block.content.height`, which on
    /// the flow path is the parent's cursor.
    pub(crate) fn layout_with_collapse_in(
        &mut self,
        containing_block: &Dimensions,
        margin_context: &mut MarginCollapseContext,
        float_context: &mut FloatContext,
        percent_height_base: Option<f32>,
    ) {
        // Handle clear property
        if self.clear != Clear::None {
            let clear_y = float_context.clear(self.clear);
            if clear_y > 0.0 {
                margin_context.reset();
            }
        }

        match &self.box_type {
            BoxType::Block | BoxType::AnonymousBlock => {
                self.layout_block_with_collapse(
                    containing_block,
                    margin_context,
                    float_context,
                    percent_height_base,
                );
            }
            BoxType::Inline => {
                self.layout_inline(containing_block);
            }
            BoxType::Text(text) => {
                self.layout_text(text.clone(), containing_block);
            }
            BoxType::Image {
                natural_width,
                natural_height,
                ..
            } => {
                self.layout_image(*natural_width, *natural_height, containing_block);
            }
            BoxType::FormControl(ref control) => {
                self.layout_form_control(control.clone(), containing_block);
            }
            BoxType::LineBreak => {
                self.dimensions.content = Rect::new(
                    containing_block.content.x,
                    containing_block.content.y + containing_block.content.height,
                    0.0,
                    0.0,
                );
            }
        }

        // Handle float
        if self.float != Float::None {
            self.layout_float(containing_block, float_context);
        }

        // Apply positioning offsets after normal layout
        self.apply_position_offsets(containing_block);
        // This box is final: anchor its abspos children to its padding box.
        self.reanchor_absolute_children();
    }

    /// Layout a block-level box with an explicit definite height for percentage resolution.
    fn layout_block_with_definite_height(
        &mut self,
        containing_block: &Dimensions,
        definite_height: Option<f32>,
    ) {
        tracing::trace!(
            containing_width = containing_block.content.width,
            definite_height = definite_height.unwrap_or(f32::NAN),
            "layout_block_with_definite_height called"
        );

        // Calculate width first (depends on containing block)
        self.calculate_block_width(containing_block);

        tracing::trace!(
            calculated_width = self.dimensions.content.width,
            "After calculate_block_width"
        );

        // Position the box
        self.calculate_block_position(containing_block);

        // Layout children. Percentage-height children resolve against THIS
        // box's height when it is definite (CSS 2.1 §10.5); the containing
        // block they are handed carries the flow cursor instead.
        let definite_for_children =
            self.definite_content_height_for_children(definite_height.unwrap_or(0.0));
        self.layout_block_children(definite_for_children);

        // Height depends on children - use definite_height for percentage resolution
        self.calculate_block_height(definite_height);
    }

    /// Layout a block-level box with margin collapse.
    fn layout_block_with_collapse(
        &mut self,
        containing_block: &Dimensions,
        margin_context: &mut MarginCollapseContext,
        float_context: &mut FloatContext,
        percent_height_base: Option<f32>,
    ) {
        // Calculate width first (depends on containing block)
        self.calculate_block_width(containing_block);

        // Calculate margin/padding/border
        self.calculate_block_vertical_box_model(containing_block);

        // CSS 2.1 §8.3.1 parent/first-child through-collapse. A box whose top
        // edge is open (no border-top, no padding-top, not a formatting root)
        // adjoins its first in-flow block child's top margin — recursively,
        // down the chain of open first children — with its own top margin
        // and the previous sibling's bottom margin, ABOVE its border box.
        // Before this, `.title{margin-bottom:10px}` followed by a plain
        // wrapper whose first child had `margin-top:4px` laid the wrapper's
        // child at 10 + 4 where Chrome puts it at max(10, 4): every
        // unpadded wrapper on css-selectors carried a +4 (and +8 for the
        // wrapper-in-wrapper), and every later section rode on it.
        let in_flow = !matches!(self.position, Position::Absolute | Position::Fixed)
            && self.float == Float::None;
        let is_formatting_root = margin_context.children_are_formatting_roots
            || establishes_bfc(&self.style, self.float);
        let top_chain = if is_formatting_root {
            Vec::new()
        } else {
            self.first_child_top_margin_chain()
        };
        // The parent already adjoined this box's whole chain (its own top
        // margin included) into the parent's context: contribute nothing.
        let top_adjoined_by_parent =
            in_flow && std::mem::take(&mut margin_context.first_child_top_adjoined);
        if !top_adjoined_by_parent {
            margin_context.add_margin(self.dimensions.margin.top);
            for m in &top_chain {
                margin_context.add_margin(*m);
            }
        }
        let collapsed_margin = margin_context.resolve();

        // Position the box with collapsed margin
        self.dimensions.content.x = containing_block.content.x
            + self.dimensions.margin.left
            + self.dimensions.border.left
            + self.dimensions.padding.left;

        self.dimensions.content.y = containing_block.content.y
            + containing_block.content.height
            + collapsed_margin
            + self.dimensions.border.top
            + self.dimensions.padding.top;

        // Children start with a fresh margin context: this box consumed the
        // pending margin when it positioned itself above, so passing the
        // parent context down would re-apply an already-materialized margin
        // to the first child (double count). The through-collapse edges ride
        // on the flags instead: the first in-flow block child skips its top
        // margin when it was adjoined above, and the last child's bottom
        // margin stays pending when this box's bottom edge is open.
        // Knowable now, before any child flows: this box's own height when it
        // does not depend on its children. Children resolve percentage heights
        // against it (CSS 2.1 §10.5); `None` leaves them on the old path.
        let definite_for_children =
            self.definite_content_height_for_children(percent_height_base
                .unwrap_or(containing_block.content.height));

        let mut child_margin_context = MarginCollapseContext::new();
        child_margin_context.children_are_formatting_roots =
            self.style.display.is_flex() || self.style.display.is_grid();
        child_margin_context.first_child_top_adjoined = !top_chain.is_empty();
        child_margin_context.last_child_collapses_through = !is_formatting_root
            && should_collapse_with_last_child(
                &self.style,
                self.float,
                self.dimensions.border.bottom,
                self.dimensions.padding.bottom,
            );

        // Check for flex or grid container - these have special child layout
        if self.style.display.is_flex() {
            // For flex containers, layout children normally first to get their intrinsic sizes
            self.layout_block_children_with_collapse(
                &mut child_margin_context,
                float_context,
                definite_for_children,
            );
            // Then apply flex layout algorithm. See the sibling call in
            // `layout_with_definite_height` for why no containing block goes
            // over from here.
            flex::layout_flex_container(self, &self.dimensions.clone());
        } else if self.style.display.is_grid() {
            // For grid containers, layout children normally first
            self.layout_block_children_with_collapse(
                &mut child_margin_context,
                float_context,
                definite_for_children,
            );
            // Then apply grid layout algorithm
            grid::layout_grid_container(
                self,
                self.dimensions.content.width,
                self.dimensions.content.height,
            );
        } else if let Some(cols) =
            multicol::column_geometry(&self.style, self.dimensions.content.width)
        {
            // Multi-column: the children flow at the column width, then
            // balance over the columns (see multicol).
            let full_width = self.dimensions.content.width;
            self.dimensions.content.width = cols.width;
            self.layout_block_children_with_collapse(
                &mut child_margin_context,
                float_context,
                definite_for_children,
            );
            self.dimensions.content.width = full_width;
            multicol::balance_columns(self, &cols);
        } else {
            // Normal block layout
            self.layout_block_children_with_collapse(
                &mut child_margin_context,
                float_context,
                definite_for_children,
            );
        }

        // Height depends on children
        self.calculate_block_height(percent_height_base.or_else(|| {
            (containing_block.content.height > 0.0).then_some(containing_block.content.height)
        }));

        // Reset margin context for next sibling, add bottom margin — and the
        // last child's bottom margin that collapsed through our open bottom
        // edge (it was left pending by layout_block_children_with_collapse).
        margin_context.reset();
        margin_context.add_margin(self.dimensions.margin.bottom);
        if child_margin_context.last_child_collapses_through {
            margin_context.absorb(&child_margin_context);
        }
    }

    /// CSS 2.1 §8.3.1: the top margins that collapse THROUGH this box's top
    /// edge — its first in-flow block child's top margin, then that child's
    /// first in-flow block child's, ... — as long as each edge on the way is
    /// open (no border-top / padding-top, not a formatting root) and no line
    /// box or float intervenes. Empty when this box's own top edge is closed.
    /// Percentage margins resolve against this box's content width (the
    /// deeper widths are not laid out yet — ledgered approximation).
    fn first_child_top_margin_chain(&self) -> Vec<f32> {
        let width = self.dimensions.content.width;
        let mut chain = Vec::new();
        let mut node = self;
        loop {
            let border_top = node.length_to_px(&node.style.border_top_width, width);
            let padding_top = node.length_to_px(&node.style.padding_top, width);
            if !should_collapse_with_first_child(&node.style, node.float, border_top, padding_top) {
                break;
            }
            // First in-flow child: out-of-flow boxes are skipped; anything
            // inline-level opens a line box and ends the chain.
            let first = node.children.iter().find(|c| {
                !matches!(c.position, Position::Absolute | Position::Fixed)
                    && c.float == Float::None
            });
            let Some(child) = first else { break };
            if !matches!(child.box_type, BoxType::Block | BoxType::AnonymousBlock)
                || child.style.display.is_atomic_inline()
            {
                break;
            }
            chain.push(child.length_to_px(&child.style.margin_top, width));
            // A child that is itself a formatting root still adjoins its own
            // margin, but nothing collapses through IT.
            if establishes_bfc(&child.style, child.float) {
                break;
            }
            node = child;
        }
        chain
    }

    /// Calculate vertical box model values (margin, border, padding).
    fn calculate_block_vertical_box_model(&mut self, containing_block: &Dimensions) {
        let style = &self.style;

        self.dimensions.margin.top =
            self.length_to_px(&style.margin_top, containing_block.content.width);
        self.dimensions.margin.bottom =
            self.length_to_px(&style.margin_bottom, containing_block.content.width);
        self.dimensions.border.top =
            self.length_to_px(&style.border_top_width, containing_block.content.width);
        self.dimensions.border.bottom =
            self.length_to_px(&style.border_bottom_width, containing_block.content.width);
        self.dimensions.padding.top =
            self.length_to_px(&style.padding_top, containing_block.content.width);
        self.dimensions.padding.bottom =
            self.length_to_px(&style.padding_bottom, containing_block.content.width);
    }

    /// Layout a floated box.
    fn layout_float(&mut self, containing_block: &Dimensions, float_context: &mut FloatContext) {
        // Calculate dimensions
        self.calculate_block_width(containing_block);
        self.calculate_block_vertical_box_model(containing_block);

        // Find position based on float type
        let (left_edge, right_edge) = float_context.available_width(
            containing_block.content.y + containing_block.content.height,
            containing_block.content.width,
        );

        let box_width = self.dimensions.margin_box().width;

        match self.float {
            Float::Left => {
                self.dimensions.content.x = containing_block.content.x
                    + left_edge
                    + self.dimensions.margin.left
                    + self.dimensions.border.left
                    + self.dimensions.padding.left;

                float_context.add_left(self.dimensions.margin_box());
            }
            Float::Right => {
                self.dimensions.content.x = containing_block.content.x + right_edge - box_width
                    + self.dimensions.margin.left
                    + self.dimensions.border.left
                    + self.dimensions.padding.left;

                float_context.add_right(self.dimensions.margin_box());
            }
            Float::None => {}
        }

        self.dimensions.content.y = containing_block.content.y
            + containing_block.content.height
            + self.dimensions.margin.top
            + self.dimensions.border.top
            + self.dimensions.padding.top;

        // Layout children
        let definite_for_children =
            self.definite_content_height_for_children(containing_block.content.height);
        self.layout_block_children(definite_for_children);
        self.calculate_block_height(
            (containing_block.content.height > 0.0).then_some(containing_block.content.height),
        );
    }

    /// Offsets with percent values resolved against the containing block.
    /// Build-time transfer can only pre-resolve absolute lengths; percents
    /// (e.g. `left: -100%` off-canvas shimmer overlays) need the containing
    /// block, so they resolve here at apply time from the computed style.
    pub(crate) fn resolved_offsets(&self, containing_block: &Dimensions) -> PositionOffsets {
        let resolve = |pre: Option<f32>, st: &Option<Length>, basis: f32| {
            pre.or(match st {
                Some(Length::Percent(p)) => Some(p / 100.0 * basis),
                _ => None,
            })
        };
        PositionOffsets {
            top: resolve(
                self.offsets.top,
                &self.style.top,
                containing_block.content.height,
            ),
            bottom: resolve(
                self.offsets.bottom,
                &self.style.bottom,
                containing_block.content.height,
            ),
            left: resolve(
                self.offsets.left,
                &self.style.left,
                containing_block.content.width,
            ),
            right: resolve(
                self.offsets.right,
                &self.style.right,
                containing_block.content.width,
            ),
        }
    }

    /// Apply position offsets for positioned elements.
    fn apply_position_offsets(&mut self, containing_block: &Dimensions) {
        // Children are laid out at the pre-offset (flow) origin, then this
        // runs. A positioned box that MOVES must carry its already-placed
        // subtree with it: content coordinates are absolute, so shifting only
        // the box origin strands every descendant (text, nested boxes) at the
        // flow position. That is why `position:absolute; bottom:0` overlay
        // captions (image-gallery cards) laid out below the card and were
        // clipped by overflow:hidden, and why a relative offset moved a box
        // but left its text behind. Capture the origin, apply offsets, then
        // translate the subtree by the delta. Static/sticky produce no origin
        // change (sticky only records a threshold), so this is a no-op there.
        let (origin_x, origin_y) = (self.dimensions.content.x, self.dimensions.content.y);
        match self.position {
            Position::Static => {
                // No offsets applied
            }
            Position::Relative => {
                // Offset from normal flow position
                if let Some(top) = self.offsets.top {
                    self.dimensions.content.y += top;
                } else if let Some(bottom) = self.offsets.bottom {
                    self.dimensions.content.y -= bottom;
                }

                if let Some(left) = self.offsets.left {
                    self.dimensions.content.x += left;
                } else if let Some(right) = self.offsets.right {
                    self.dimensions.content.x -= right;
                }
            }
            Position::Absolute => {
                // Shares the full CSS2 §10.3.7/§10.6.4 implementation with
                // Fixed — including the both-offsets-set + auto-size stretch
                // (`inset: 0` filling the containing block). The old inline
                // arm here handled single offsets only, so every
                // `position:absolute; inset:0` overlay (settings' toggle
                // sliders) kept its intrinsic size instead of filling.
                self.apply_position_offsets_absolute(containing_block);
            }
            Position::Fixed => {
                // CSS2 §10.1: a fixed element's containing block is the
                // VIEWPORT, not the block that laid it out. Resolving
                // bottom:0 against the root's flow height painted
                // bottom-anchored elements mid-page (viewport-probe: y=280
                // in a 600px viewport whose content ended at 320). Falls
                // back to the passed block only when no viewport has been
                // set (bare unit trees).
                if self.viewport.0 > 0.0 && self.viewport.1 > 0.0 {
                    let viewport_cb = Dimensions {
                        content: Rect::new(0.0, 0.0, self.viewport.0, self.viewport.1),
                        ..Default::default()
                    };
                    self.apply_position_offsets_absolute(&viewport_cb);
                } else {
                    self.apply_position_offsets_absolute(containing_block);
                }
            }
            Position::Sticky => {
                // Sticky positioning: element stays in normal flow but can "stick"
                // when scrolled past its threshold.
                //
                // The offsets (top, left, etc.) define the sticky threshold, not
                // an initial offset like relative positioning.
                //
                // Store the original position and sticky offsets in StickyState.
                // The actual sticky adjustment happens during rendering based on scroll.
                let original_rect = self.dimensions.border_box();
                let sticky_offsets = StickyOffsets {
                    top: self.offsets.top,
                    right: self.offsets.right,
                    bottom: self.offsets.bottom,
                    left: self.offsets.left,
                };
                self.sticky_state = Some(StickyState::new(original_rect, sticky_offsets));
                // Position stays at normal flow - no offset applied during layout
            }
        }

        // Carry the already-laid-out subtree with a moved box origin (see the
        // note at the top of this fn). Only fires when the origin actually
        // changed, so static/sticky and unmoved relatives cost nothing.
        let (dx, dy) = (
            self.dimensions.content.x - origin_x,
            self.dimensions.content.y - origin_y,
        );
        if dx != 0.0 || dy != 0.0 {
            for child in &mut self.children {
                crate::flex::translate_subtree(child, dx, dy);
            }
        }
    }

    /// This box's content height when it is definite BEFORE its children
    /// lay out (an absolute `height`), else `None`. Percentages are left
    /// out: they need the grandparent's definite height, which this box
    /// does not hold. Mirrors the absolute arms of calculate_block_height.
    fn definite_content_height(&self) -> Option<f32> {
        let padding_border = self.dimensions.padding.top
            + self.dimensions.padding.bottom
            + self.dimensions.border.top
            + self.dimensions.border.bottom;
        let specified = match self.style.height {
            Length::Px(h) => h,
            Length::Em(em) => {
                em * match self.style.font_size {
                    Length::Px(px) => px,
                    _ => 16.0,
                }
            }
            Length::Rem(rem) => rem * 16.0,
            Length::Vh(vh) if self.viewport.1 > 0.0 => vh / 100.0 * self.viewport.1,
            _ => return None,
        };
        Some(if self.style.box_sizing == BoxSizing::BorderBox {
            (specified - padding_border).max(0.0)
        } else {
            specified
        })
    }

    /// This box's inner (content-box) height when `height: auto` is made
    /// DEFINITE by opposite insets on an out-of-flow box, else `None`.
    ///
    /// CSS2 §10.6.4: for an absolutely positioned box with `height: auto`
    /// and neither `top` nor `bottom` auto, the used height is fixed by the
    /// constraint equation — it is as definite as a specified `height`, and
    /// it is the number every `inset: 0` overlay is built on. Nothing in the
    /// style carries it: `style.height` reads `Auto`, so every consumer that
    /// asks *style* whether the main size is definite answers "no" and falls
    /// back to sizing by content.
    ///
    /// It lives here, beside `definite_content_height`, because the flex
    /// container path and `apply_position_offsets_absolute` must agree on
    /// the number to the last bit — two copies of this subtraction that
    /// drift apart are the defect class night 8 recorded.
    pub(crate) fn inset_definite_content_height(
        &self,
        containing_block: &Dimensions,
    ) -> Option<f32> {
        if !matches!(self.position, Position::Absolute | Position::Fixed) {
            return None;
        }
        if !matches!(self.style.height, Length::Auto) {
            return None;
        }
        let offsets = self.resolved_offsets(containing_block);
        let (top, bottom) = (offsets.top?, offsets.bottom?);
        Some(
            (containing_block.content.height
                - top
                - bottom
                - self.dimensions.margin.top
                - self.dimensions.margin.bottom
                - self.dimensions.border.top
                - self.dimensions.border.bottom
                - self.dimensions.padding.top
                - self.dimensions.padding.bottom)
                .max(0.0),
        )
    }

    /// Re-resolve an absolutely positioned box's offsets against its REAL
    /// containing block, carrying the already-laid-out subtree with it. The
    /// first pass positioned it against a stand-in whose height was the
    /// parent's flow cursor (static position); only `bottom`-anchored and
    /// `inset`-stretched boxes move here. `position: fixed` is untouched —
    /// its containing block is the viewport, not this parent.
    pub(crate) fn reanchor_absolute(&mut self, containing_block: &Dimensions) {
        if self.position != Position::Absolute {
            return;
        }
        let (origin_x, origin_y) = (self.dimensions.content.x, self.dimensions.content.y);
        let (origin_w, origin_h) = (
            self.dimensions.content.width,
            self.dimensions.content.height,
        );
        // CSS 2.1 §10.5: a percentage height resolves against the CONTAINING
        // BLOCK — for an out-of-flow box, the padding box this re-anchor is
        // handed. During flow layout the stand-in's `content.height` was the
        // parent's flow cursor (the static-position trick documented at both
        // `layout_block_children` call sites), so a percentage resolved
        // against "content laid out so far". chrome_rustkit's `.sidebar`
        // (`top: 84px; height: calc(100% - 84px)` in a 100px strip) read that
        // cursor as 84 and came out ZERO tall against Chrome's 16.
        //
        // Only the lengths that actually depend on the base are recomputed:
        // a `Px` height is already right, and an `auto` one is its content.
        if matches!(self.style.height, Length::Percent(_) | Length::Calc(_)) {
            self.calculate_block_height(Some(containing_block.content.height));
        }
        self.apply_position_offsets_absolute(containing_block);
        let (dx, dy) = (
            self.dimensions.content.x - origin_x,
            self.dimensions.content.y - origin_y,
        );
        if dx != 0.0 || dy != 0.0 {
            for child in &mut self.children {
                crate::flex::translate_subtree(child, dx, dy);
            }
        }
        // On the block path this is where an inset-stretched box's used height
        // FIRST becomes known: the child was laid out against a stand-in whose
        // height is the parent's flow cursor (the static-position trick), so
        // the containing block's real height was not available until now.
        //
        // A translate carries the subtree but does not re-justify it. A flex
        // container whose main size just changed has to redistribute its free
        // space, or `justify-content` keeps the answer it computed from the
        // cursor — `center` in a stand-in the size of its own content means no
        // free space at all, and every item stays packed against the start
        // edge. Passing the containing block as well keeps step 11d's
        // re-derivation on the same number.
        if self.style.display.is_flex()
            && self.inset_definite_content_height(containing_block).is_some()
        {
            crate::flex::layout_flex_container_in(
                self,
                &self.dimensions.clone(),
                Some(containing_block),
            );
        }

        // An `inset`-stretched box changes SIZE here, after its own children
        // were anchored to its pre-stretch box: a `bottom: 2px` knob inside
        // an `inset: 0` slider sat 22px above the slider (settings' toggles,
        // n46). Its abspos children are re-anchored to the new padding box.
        if self.dimensions.content.width != origin_w || self.dimensions.content.height != origin_h
        {
            self.reanchor_absolute_children();
        }
    }

    /// CSS 2.1 §10.1: the containing block of an absolutely positioned
    /// descendant is the PADDING box of its positioned ancestor. Expressed as
    /// a `Dimensions` whose content rect is that padding box, because
    /// `apply_position_offsets_absolute` reads `containing_block.content`.
    fn abspos_containing_block(&self) -> Dimensions {
        Dimensions {
            content: self.dimensions.padding_box(),
            ..Default::default()
        }
    }

    /// Re-anchor every absolutely positioned child against this box's FINAL
    /// padding box. Runs once this box's own size and position are settled —
    /// after its children laid out, its height resolved and its own offsets
    /// (including an `inset: 0` stretch) applied.
    ///
    /// During child layout an abspos child resolves `bottom:` against a
    /// stand-in whose height is the parent's flow cursor (its static
    /// position). The old re-anchor ran inside that loop and only when the
    /// parent's `height` was an absolute length, against the CONTENT box: a
    /// `bottom: 2px` knob inside an `inset: 0` slider (settings' toggles),
    /// inside any auto-height positioned box, or inside a padded one landed
    /// against the parent's top / short of the padding edge. Idempotent:
    /// offsets are recomputed absolutely, so a box the parent's own move
    /// already carried simply lands in the same place.
    pub(crate) fn reanchor_absolute_children(&mut self) {
        let cb = self.abspos_containing_block();
        for child in &mut self.children {
            child.reanchor_absolute(&cb);
        }
    }

    /// Apply absolute positioning offsets.
    fn apply_position_offsets_absolute(&mut self, containing_block: &Dimensions) {
        let offsets = self.resolved_offsets(containing_block);
        let has_left = offsets.left.is_some();
        let has_right = offsets.right.is_some();
        let has_top = offsets.top.is_some();
        let has_bottom = offsets.bottom.is_some();

        // Handle horizontal positioning
        if has_left && has_right {
            // When both left and right are set with width: auto, stretch to fill
            let left = offsets.left.unwrap();
            let right = offsets.right.unwrap();

            // Calculate stretched width if width is auto
            if matches!(self.style.width, Length::Auto) {
                let available_width = containing_block.content.width
                    - left
                    - right
                    - self.dimensions.margin.left
                    - self.dimensions.margin.right
                    - self.dimensions.border.left
                    - self.dimensions.border.right
                    - self.dimensions.padding.left
                    - self.dimensions.padding.right;
                self.dimensions.content.width = available_width.max(0.0);
            }

            // Position from left
            self.dimensions.content.x = containing_block.content.x
                + left
                + self.dimensions.margin.left
                + self.dimensions.border.left
                + self.dimensions.padding.left;
        } else if let Some(left) = offsets.left {
            self.dimensions.content.x = containing_block.content.x
                + left
                + self.dimensions.margin.left
                + self.dimensions.border.left
                + self.dimensions.padding.left;
        } else if let Some(right) = offsets.right {
            self.dimensions.content.x = containing_block.content.right()
                - right
                - self.dimensions.margin.right
                - self.dimensions.border.right
                - self.dimensions.padding.right
                - self.dimensions.content.width;
        }

        // Handle vertical positioning
        if has_top && has_bottom {
            // When both top and bottom are set with height: auto, stretch to fill
            let top = offsets.top.unwrap();

            // Calculate stretched height if height is auto. The subtraction
            // lives in `inset_definite_content_height` so the flex container
            // path resolves the identical number.
            if let Some(available_height) = self.inset_definite_content_height(containing_block) {
                self.dimensions.content.height = available_height;
            }

            // Position from top
            self.dimensions.content.y = containing_block.content.y
                + top
                + self.dimensions.margin.top
                + self.dimensions.border.top
                + self.dimensions.padding.top;
        } else if let Some(top) = offsets.top {
            self.dimensions.content.y = containing_block.content.y
                + top
                + self.dimensions.margin.top
                + self.dimensions.border.top
                + self.dimensions.padding.top;
        } else if let Some(bottom) = offsets.bottom {
            self.dimensions.content.y = containing_block.content.bottom()
                - bottom
                - self.dimensions.margin.bottom
                - self.dimensions.border.bottom
                - self.dimensions.padding.bottom
                - self.dimensions.content.height;
        }
    }

    /// Calculate block width.
    fn calculate_block_width(&mut self, containing_block: &Dimensions) {
        let style = &self.style;

        // Get values from style
        let margin_left = self.length_to_px(&style.margin_left, containing_block.content.width);
        let margin_right = self.length_to_px(&style.margin_right, containing_block.content.width);
        let border_left =
            self.length_to_px(&style.border_left_width, containing_block.content.width);
        let border_right =
            self.length_to_px(&style.border_right_width, containing_block.content.width);
        let padding_left = self.length_to_px(&style.padding_left, containing_block.content.width);
        let padding_right = self.length_to_px(&style.padding_right, containing_block.content.width);

        let total_margin_border_padding =
            margin_left + margin_right + border_left + border_right + padding_left + padding_right;

        // Calculate content width
        let content_width = match style.width {
            Length::Auto => {
                let available =
                    (containing_block.content.width - total_margin_border_padding).max(0.0);
                if style.display.is_atomic_inline() {
                    // CSS2 §10.3.9: an atomic inline (inline-block/-flex/
                    // -grid) with width:auto shrinks to fit —
                    // min(max(preferred_min, available), preferred) — it
                    // never fills the containing block. The estimators
                    // return border-box widths, so strip this box's own
                    // padding+border to compare in content space.
                    let pb = border_left + border_right + padding_left + padding_right;
                    let preferred = (crate::grid::estimate_max_content_width(self) - pb).max(0.0);
                    let preferred_min =
                        (crate::grid::estimate_min_content_width(self) - pb).max(0.0);
                    preferred_min.max(available.min(preferred))
                } else if auto_width_shrinks_to_fit(
                    self.position,
                    &self.resolved_offsets(containing_block),
                ) {
                    // Out of flow: CSS 2.1 §10.3.7 makes `width: auto`
                    // shrink-to-fit rather than fill. `.footer { position:
                    // fixed; bottom: 1rem }` on new_tab is the corpus's
                    // clearest case: Chrome sizes it to its text at 137.59px,
                    // RustKit stretched it across the whole 1280px viewport.
                    //
                    // Presence of left/right is read through `resolved_offsets`
                    // — the same accessor `apply_position_offsets_absolute`
                    // uses to decide the very same stretch-vs-solve question —
                    // so the two cannot disagree about whether an offset was
                    // specified.
                    //
                    // Known limit, stated rather than hidden: for `position:
                    // fixed` the containing block is the viewport, and this
                    // function is handed the flow parent. That only matters
                    // when it clamps, i.e. when max-content exceeds `available`.
                    shrink_to_fit_content_width(self, available)
                } else {
                    // Fill available space (CSS 2.1 §10.3.3)
                    available
                }
            }
            _ => {
                let specified_width =
                    self.length_to_px(&style.width, containing_block.content.width);
                // With box-sizing: border-box, the specified width includes padding and border
                if style.box_sizing == BoxSizing::BorderBox {
                    (specified_width - padding_left - padding_right - border_left - border_right)
                        .max(0.0)
                } else {
                    specified_width
                }
            }
        };

        // Apply min-width constraint (also respects box-sizing)
        let min_width_raw = self.length_to_px(&style.min_width, containing_block.content.width);
        let min_width = if style.box_sizing == BoxSizing::BorderBox && min_width_raw > 0.0 {
            (min_width_raw - padding_left - padding_right - border_left - border_right).max(0.0)
        } else {
            min_width_raw
        };
        let content_width = content_width.max(min_width);

        // Apply max-width constraint (also respects box-sizing)
        let max_width = match style.max_width {
            Length::Auto | Length::Zero => f32::INFINITY,
            _ => {
                let max_width_raw =
                    self.length_to_px(&style.max_width, containing_block.content.width);
                if style.box_sizing == BoxSizing::BorderBox {
                    (max_width_raw - padding_left - padding_right - border_left - border_right)
                        .max(0.0)
                } else {
                    max_width_raw
                }
            }
        };
        let content_width = content_width.min(max_width);

        // CSS 2.1 §10.3.3: auto horizontal margins absorb the space left
        // over once the used width is known. With width:auto the content
        // has already consumed all available space (free space is zero),
        // so this only moves boxes whose width came from a specified
        // width or a min/max-width clamp — `margin: 0 auto` centers them.
        // Auto margins resolved to 0.0 above, so free space is computed
        // against the non-auto sides only.
        // Floats are excluded: their auto margins are used as 0 (§10.3.5).
        let margin_left_auto =
            matches!(style.margin_left, Length::Auto) && self.float == Float::None;
        let margin_right_auto =
            matches!(style.margin_right, Length::Auto) && self.float == Float::None;
        let (margin_left, margin_right) = if margin_left_auto || margin_right_auto {
            let free_space = containing_block.content.width
                - content_width
                - border_left
                - border_right
                - padding_left
                - padding_right
                - margin_left
                - margin_right;
            if free_space > 0.0 {
                if margin_left_auto && margin_right_auto {
                    (
                        margin_left + free_space / 2.0,
                        margin_right + free_space / 2.0,
                    )
                } else if margin_left_auto {
                    (margin_left + free_space, margin_right)
                } else {
                    (margin_left, margin_right + free_space)
                }
            } else {
                (margin_left, margin_right)
            }
        } else {
            (margin_left, margin_right)
        };

        self.dimensions.content.width = content_width;
        self.dimensions.margin.left = margin_left;
        self.dimensions.margin.right = margin_right;
        self.dimensions.border.left = border_left;
        self.dimensions.border.right = border_right;
        self.dimensions.padding.left = padding_left;
        self.dimensions.padding.right = padding_right;
    }

    /// Calculate block position.
    fn calculate_block_position(&mut self, containing_block: &Dimensions) {
        let style = &self.style;

        self.dimensions.margin.top =
            self.length_to_px(&style.margin_top, containing_block.content.width);
        self.dimensions.margin.bottom =
            self.length_to_px(&style.margin_bottom, containing_block.content.width);
        self.dimensions.border.top =
            self.length_to_px(&style.border_top_width, containing_block.content.width);
        self.dimensions.border.bottom =
            self.length_to_px(&style.border_bottom_width, containing_block.content.width);
        self.dimensions.padding.top =
            self.length_to_px(&style.padding_top, containing_block.content.width);
        self.dimensions.padding.bottom =
            self.length_to_px(&style.padding_bottom, containing_block.content.width);

        // Position below the containing block's content
        self.dimensions.content.x = containing_block.content.x
            + self.dimensions.margin.left
            + self.dimensions.border.left
            + self.dimensions.padding.left;

        self.dimensions.content.y = containing_block.content.y
            + containing_block.content.height
            + self.dimensions.margin.top
            + self.dimensions.border.top
            + self.dimensions.padding.top;

        tracing::trace!(
            "calculate_block_position: cb.y={}, cb.height={}, margin_top={}, result_y={}",
            containing_block.content.y,
            containing_block.content.height,
            self.dimensions.margin.top,
            self.dimensions.content.y
        );
    }

    /// Layout block children.
    fn layout_block_children(&mut self, definite_height: Option<f32>) {
        let mut cursor_y = 0.0;
        let mut cursor_x = 0.0;
        let mut line_height = 0.0_f32;
        // Extent of the current line BELOW the baseline contributed by boxes
        // whose bottom edge IS their baseline (empty atomic inlines, images):
        // the strut's descent extends the line box under them.
        let mut line_below_baseline = 0.0_f32;
        // Above/below-baseline extents of the current line's baseline-anchored
        // members; the strut floors both when the line closes (line_advance).
        let mut line_extents = (0.0_f32, 0.0_f32);
        let container_width = self.dimensions.content.width;
        let text_align = self.style.text_align;
        let strut_descent = self.inline_strut_descent();
        let strut = Self::text_line_box_extents(&self.style);
        // A `<br>` on an otherwise empty line still produces a line box of
        // the container's line-height (CSS 2.1 §9.4.2 / §10.8).
        let empty_line_height = self.get_line_height();
        // white-space: nowrap|pre suppress soft-wrapping of inline-level
        // children — the line box grows past container_width and overflow
        // handles any scroll. Captured before the &mut children borrow.
        let container_allows_wrap = !matches!(
            self.style.white_space,
            rustkit_css::WhiteSpace::Nowrap | rustkit_css::WhiteSpace::Pre
        );
        // css-text-3 §5.1: an author `width: 0` is a RESOLVED used width, so
        // text in it wraps at every opportunity it has (WPT
        // break-boundary-2-chars-001: `a` / `b` / `c` down a zero-wide
        // break-all inline-block). The block path reads a bare 0 as "no
        // resolved width yet" (intrinsic pass); only the container can say
        // which zero this is. Absolute lengths only — a percentage of an
        // unresolved parent also lands on 0 and must keep the guard.
        let container_is_definite_zero = container_width <= 0.0
            && (matches!(self.style.width, Length::Zero)
                || matches!(self.style.width, Length::Px(px) if px <= 0.0));

        // Track lines for text-align adjustment after layout: (start_index, end_index, line_width)
        let mut lines: Vec<(usize, usize, f32)> = Vec::new();
        let mut line_start_index: Option<usize> = None;
        let mut line_width = 0.0_f32;
        // IFC Slice B2: phase-5 mid-line splits whose CLOSED lines (line 0
        // + middles) need alignment after the loop: (line_start, text_index).
        let mut split_records: Vec<(usize, usize)> = Vec::new();

        for (i, child) in self.children.iter_mut().enumerate() {
            // Skip absolutely/fixed positioned children for flow layout.
            // The stand-in's height is the flow cursor (static position);
            // `bottom`/`inset` are re-resolved against the real padding box
            // by reanchor_absolute_children once this box is final.
            if child.position == Position::Absolute || child.position == Position::Fixed {
                let mut cb = self.dimensions.clone();
                cb.content.height = cursor_y;
                child.layout(&cb);
                continue;
            }

            // `<br>`: forced line break — close the current line box. With
            // nothing on the line yet, the break still advances by one
            // empty line box.
            if matches!(child.box_type, BoxType::LineBreak) {
                if let Some(start) = line_start_index {
                    lines.push((start, i, line_width));
                }
                let advance = if cursor_x > 0.0 || line_height > 0.0 {
                    line_advance(line_height, line_below_baseline, line_extents, strut)
                } else {
                    empty_line_height
                };
                child.dimensions.content = Rect::new(
                    self.dimensions.content.x + cursor_x,
                    self.dimensions.content.y + cursor_y,
                    0.0,
                    0.0,
                );
                cursor_y += advance;
                cursor_x = 0.0;
                line_height = 0.0;
                line_below_baseline = 0.0;
                line_extents = (0.0, 0.0);
                line_start_index = None;
                line_width = 0.0;
                continue;
            }

            // Check if child is an atomic inline (inline-block/-flex/-grid)
            let is_inline_block = child.style.display.is_atomic_inline();
            // CSS2 §9.4.2: ALL inline-level boxes share line boxes, not just
            // atomic inlines — non-atomic inline boxes (spans/links), inline
            // form controls and inline images flow on the same line. A text
            // run joins the current line only when it fits the remaining
            // space as a single line; longer text keeps the block path and
            // wraps there (full IFC text splitting is a later phase).
            let flows_inline = is_inline_block
                || (child.style.display == rustkit_css::Display::Inline
                    && matches!(
                        child.box_type,
                        BoxType::Inline | BoxType::FormControl(_) | BoxType::Image { .. }
                    ))
                // IFC Slice B (symmetric join): a fitting text run joins the
                // line from ANY cursor position, including 0. The old
                // cursor_x > 0 gate sent the FIRST text sibling down the
                // block path, so `Some <b>bold</b> text` stacked as
                // "Some" / "bold text" — half line-model, half leaf-model
                // (session-3 falsification). Non-fitting text keeps the
                // block path (wrap or phase-5 split as before).
                // Under nowrap/pre, text never soft-wraps, so it joins the
                // line from any cursor position — even past container_width
                // (e.g. whitespace between inline-blocks in an overflow-x
                // scroller). Otherwise it flows inline only when it fits.
                || (matches!(child.box_type, BoxType::Text(_))
                    && (!container_allows_wrap
                        || child.text_single_line_width() <= container_width - cursor_x));

            if flows_inline {
                // Layout inline-level child to get its dimensions first
                let mut cb = self.dimensions.clone();
                cb.content.x = self.dimensions.content.x + cursor_x;
                cb.content.y = self.dimensions.content.y + cursor_y;
                // Children self-position at cb.y + cb.height; the cursor is
                // already baked into cb.y, so the height term must be zero.
                cb.content.height = 0.0;
                child.layout_with_percent_base(
                    &cb,
                    definite_height.or((cb.content.height > 0.0).then_some(cb.content.height)),
                );

                let child_width = child.dimensions.margin_box().width;
                let child_height = child.dimensions.margin_box().height;

                // Check if child fits on current line (nowrap/pre never soft-wrap)
                if container_allows_wrap
                    && cursor_x > 0.0
                    && cursor_x + child_width > container_width
                {
                    // Record completed line for text-align
                    if let Some(start) = line_start_index {
                        lines.push((start, i, line_width));
                    }

                    // Wrap to next line
                    cursor_y += line_advance(line_height, line_below_baseline, line_extents, strut);
                    cursor_x = 0.0;
                    line_height = 0.0;
                    line_below_baseline = 0.0;
                    line_extents = (0.0, 0.0);
                    line_start_index = Some(i);
                    line_width = 0.0;

                    // Re-layout at new position
                    cb.content.x = self.dimensions.content.x;
                    cb.content.y = self.dimensions.content.y + cursor_y;
                    child.layout_with_percent_base(
                    &cb,
                    definite_height.or((cb.content.height > 0.0).then_some(cb.content.height)),
                );
                }

                // Track line start
                if line_start_index.is_none() {
                    line_start_index = Some(i);
                }

                // Position the child. The cursor addresses the MARGIN box;
                // the content rect sits margin+border+padding inside it
                // (dropping border/padding here shifted every decorated
                // inline-block up-left by border+padding).
                child.dimensions.content.x = self.dimensions.content.x
                    + cursor_x
                    + child.dimensions.margin.left
                    + child.dimensions.border.left
                    + child.dimensions.padding.left;
                child.dimensions.content.y = self.dimensions.content.y
                    + cursor_y
                    + child.dimensions.margin.top
                    + child.dimensions.border.top
                    + child.dimensions.padding.top;

                // NON-REPLACED inline (§10.6.1/§10.8.1): the rect is the
                // content area, but the LINE still advances by the child's
                // line-height, and the content box sits a half-leading below
                // the line top. Replaced/atomic inlines keep border-box line
                // sizing. Without the decoupling, shrinking the rect would
                // collapse every line to font height — the advance and the
                // report are different quantities.
                if matches!(child.box_type, BoxType::Inline) {
                    let (_, half_leading) = child.inline_content_area();
                    if half_leading > 0.0 {
                        child.shift_inline_content_area(half_leading);
                    }
                    line_height = line_height.max(child.get_line_height());
                    // The inline's text wrapped onto several line boxes: the
                    // flow must advance past ALL of them, exactly as the
                    // phase-5 split below does for a bare text run. Before
                    // n50 the line advanced by ONE line-height and the next
                    // block sibling was laid over lines 2..n (`<p><span>long
                    // text</span></p>` was one line tall; `<pre><code>` six
                    // lines painted over the following paragraph).
                    if let Some((n_lines, last_end, lh)) =
                        Self::inline_wrapped_tail(child, self.dimensions.content.x)
                    {
                        if let Some(start) = line_start_index {
                            lines.push((start, i + 1, line_width + child_width));
                        }
                        cursor_y += line_advance(line_height, line_below_baseline, line_extents, strut).max(lh)
                            + (n_lines as f32 - 2.0).max(0.0) * lh;
                        cursor_x = last_end;
                        line_width = last_end;
                        line_height = lh;
                        line_below_baseline = 0.0;
                        line_extents = (0.0, 0.0);
                        line_start_index = Some(i);
                        continue;
                    }
                } else {
                    line_height = line_height.max(child_height);
                }
                // Advance cursor
                cursor_x += child_width;
                line_width += child_width;
                // vertical-align: top|bottom boxes do not anchor to the
                // baseline at all (CSS2 §10.8): a top-aligned box hangs from
                // the line-box top, so a tall one SWALLOWS the strut instead
                // of stacking above its descent. Extending the strut under
                // them made every nowrap card row strut_descent too tall —
                // sticky-scroll's .horizontal-scroll read 156.8 where Chrome
                // reads 150 (+6.8 = descent + half-leading at line-height
                // 1.6), and the error cascaded into every later sibling's y.
                // Every baseline-anchored member raises the line's ascent or
                // deepens its descent; the line box is their sum, not the
                // tallest member (n54: `<label>Text:</label><input>` with no
                // whitespace between built a 19px line for Chrome's 24).
                let member_extents = match child.float {
                    Float::None => child.line_member_baseline_extents(),
                    _ => None,
                };
                if let Some((above, below)) = member_extents {
                    line_extents = (line_extents.0.max(above), line_extents.1.max(below));
                }
                let anchored_to_baseline = !matches!(
                    child.style.vertical_align,
                    rustkit_css::VerticalAlign::Top | rustkit_css::VerticalAlign::Bottom
                );
                if anchored_to_baseline && child.baseline_is_bottom_edge() {
                    // Box bottom sits ON the baseline; the strut extends the
                    // line box below it (CSS2 §10.8.1).
                    line_below_baseline = line_below_baseline.max(child_height + strut_descent);
                }

                if child.float != Float::None {
                    // Floated elements don't affect cursor
                    cursor_x -= child_width;
                    line_width -= child_width;
                }
            } else if Self::text_splits_inline(child, cursor_x) {
                // Phase 5 (IFC text splitting): a text run that does NOT fit
                // the remaining space fills it and wraps onward at full
                // width instead of dropping to its own block row.
                let cb = self.dimensions.clone();
                let line_top = self.dimensions.content.y + cursor_y;
                let (n_lines, last_w) = child.layout_text_in_flow(&cb, line_top, cursor_x);
                let lh = child.get_line_height();
                if n_lines <= 1 {
                    // Degenerate (shaping fallback): continue the line.
                    cursor_x += last_w;
                    line_width += last_w;
                    line_height = line_height.max(lh);
                } else {
                    // Line 0 closes the current line box; middle lines are
                    // full; the LAST line stays open for following content.
                    // B2: the closed lines are aligned by align_split_close
                    // after the loop (the open last line is aligned by the
                    // recorded-lines pass when it eventually closes).
                    split_records.push((line_start_index.unwrap_or(i), i));
                    cursor_y += line_advance(line_height, line_below_baseline, line_extents, strut).max(lh)
                        + (n_lines as f32 - 2.0).max(0.0) * lh;
                    cursor_x = last_w;
                    line_width = last_w;
                    line_height = lh;
                    line_below_baseline = 0.0;
                    line_extents = (0.0, 0.0);
                    line_start_index = Some(i);
                }
            } else {
                // Regular block layout
                // First, finish any inline-block line
                if cursor_x > 0.0 {
                    if let Some(start) = line_start_index {
                        lines.push((start, i, line_width));
                    }
                    cursor_y += line_advance(line_height, line_below_baseline, line_extents, strut);
                    cursor_x = 0.0;
                    line_height = 0.0;
                    line_below_baseline = 0.0;
                    line_extents = (0.0, 0.0);
                    line_start_index = None;
                    line_width = 0.0;
                }

                let mut cb = self.dimensions.clone();
                cb.content.height = cursor_y;
                match (container_is_definite_zero, &child.box_type) {
                    // Author `width: 0`: block-path text wraps against it
                    // (see layout_text_with_zero_wrap).
                    (true, BoxType::Text(t)) => {
                        let t = t.clone();
                        child.layout_text_with_zero_wrap(t, &cb, true);
                    }
                    _ => child.layout_with_percent_base(
                        &cb,
                        definite_height.or((cb.content.height > 0.0).then_some(cb.content.height)),
                    ),
                }

                // An inline-level box (e.g. a styled <span>/<a>) laid out on
                // its own is centered/right-aligned as a single-item line so
                // its box decoration follows text-align.
                if matches!(child.box_type, BoxType::Inline) {
                    lines.push((i, i + 1, child.dimensions.margin_box().width));
                }
                // IFC Slice A: a text run laid on the block path (a lone or
                // first text child — the inline gate requires cursor_x > 0)
                // is a single-item line. Leaves no longer self-align, so
                // without this record centered headings would go left.
                if matches!(child.box_type, BoxType::Text(_)) {
                    lines.push((i, i + 1, child.dimensions.content.width));
                }

                if child.float == Float::None {
                    cursor_y += child.dimensions.margin_box().height;
                    // An inline image on the baseline leaves the strut's
                    // descent + half-leading below it (see inline_strut_descent).
                    if matches!(child.box_type, BoxType::Image { .. })
                        && child.style.display == rustkit_css::Display::Inline
                    {
                        cursor_y += strut_descent;
                    }
                }
            }
        }

        // Record any remaining inline-block line
        if cursor_x > 0.0 {
            if let Some(start) = line_start_index {
                lines.push((start, self.children.len(), line_width));
            }
            cursor_y += line_advance(line_height, line_below_baseline, line_extents, strut);
        }

        // IFC Slice B2: align the CLOSED lines of each mid-line split
        // (line 0 = prior siblings + first fragment as one unit; middles
        // per-line) before the recorded-lines pass touches last fragments.
        for (start, ti) in split_records {
            Self::align_split_close(&mut self.children, start, ti, container_width, text_align);
        }

        // Apply text-align to all recorded lines
        let valign_font = self.style.clone();
        for (start, end, width) in lines {
            Self::apply_text_align_offset(
                &mut self.children[start..end],
                width,
                container_width,
                text_align,
            );
            // Slice C: vertical alignment about the line baseline.
            Self::apply_vertical_align(&mut self.children[start..end], &valign_font);
        }

        self.dimensions.content.height = cursor_y;
    }

    /// IFC Slice C (CSS2 §10.8 subset): align line members VERTICALLY about
    /// the line's alphabetic baseline. Layout owns Y (same contract as
    /// Slice A owns X): text boxes place so their baseline (content top +
    /// half-leading + ascent, matching the paint emission math exactly)
    /// sits on the line baseline; bottom-edge boxes (images, empty atomic
    /// inlines) put their margin-bottom edge on it (`vertical-align:
    /// baseline`) or center on baseline − x-height/2 (`middle`). Other
    /// vertical-align values intentionally fall through to baseline
    /// (Slice C subset). Text-only lines of one font shift by ZERO by
    /// construction — the pass only moves members when a taller neighbor
    /// raises the line's ascent (a 32px img next to 16px text: Chrome puts
    /// the text baseline at the img bottom; we top-aligned both and the
    /// text floated 17px high).
    fn apply_vertical_align(children: &mut [LayoutBox], container_font: &ComputedStyle) {
        let members: Vec<usize> = (0..children.len())
            .filter(|&i| {
                let c = &children[i];
                !matches!(c.position, Position::Absolute | Position::Fixed)
                    && c.style.display != rustkit_css::Display::None
            })
            .collect();
        if members.is_empty() {
            return;
        }

        let font_px = |s: &ComputedStyle| match s.font_size {
            Length::Px(px) => px,
            _ => 16.0,
        };
        // Above/below-baseline extents for a text-carrying style, matching
        // the paint emission: glyph top = content_y + half_leading,
        // baseline = glyph top + ascent.
        //
        // Whole-pixel, as paint seats a run (`blink_baseline_offset`) and as
        // the line box is sized: on the fractional split a member dropped to
        // a neighbour's baseline landed between rows and painted one off.
        let text_extents = Self::text_line_box_extents;

        // A member's top on THIS line. A mid-line split run reaches the
        // recorded line only through its LAST visual line (the open one its
        // followers share); its box top is the top of its FIRST line, and
        // reading that as the line top hoisted every follower up to the
        // run's first line (n49: `<p>long run… <span>x</span>` put the span
        // on line 1 of a 4-line run).
        let member_top = |c: &LayoutBox| -> f32 {
            let d = &c.dimensions;
            let box_top = d.content.y - d.margin.top - d.border.top - d.padding.top;
            // A non-atomic inline's rect is its content area, a half-leading
            // below the line slot its text sits in; the slot is the member.
            // One whose text wrapped reaches this line through its last line
            // box, like a split run.
            if matches!(c.box_type, BoxType::Inline) {
                let slot_top = box_top - c.inline_content_area().1;
                return match Self::inline_wrapped_tail(c, 0.0) {
                    Some((n, _, lh)) => slot_top + (n as f32 - 1.0) * lh,
                    None => slot_top,
                };
            }
            match (&c.text_flow_first_offset, &c.text_lines) {
                (Some(_), Some(tls)) if tls.len() > 1 => {
                    box_top + (tls.len() as f32 - 1.0) * c.get_line_height()
                }
                _ => box_top,
            }
        };
        // An inline-block's inner baseline in whole pixels, exactly as
        // line_member_baseline_extents sizes the line from it.
        let atomic_above = |above: f32| (above + 0.01).floor();
        // A non-atomic inline the pass can seat: baseline-aligned, all on one
        // line. `top|bottom|middle` inlines and wrapped ones stay put
        // (ledgered) but still count toward the line top.
        let seats_as_text = |c: &LayoutBox| -> bool {
            matches!(c.box_type, BoxType::Inline)
                && !matches!(
                    c.style.vertical_align,
                    rustkit_css::VerticalAlign::Top
                        | rustkit_css::VerticalAlign::Bottom
                        | rustkit_css::VerticalAlign::Middle
                )
                && Self::inline_wrapped_tail(c, 0.0).is_none()
        };
        let line_top = members
            .iter()
            .map(|&i| member_top(&children[i]))
            .fold(f32::INFINITY, f32::min);

        // Pass 1: the line's ascent = max above-baseline extent, floored by
        // the container strut.
        let (strut_above, _strut_below) = text_extents(container_font);
        let mut ascent = strut_above;
        let fs = font_px(container_font);
        // x-height ~= 0.5em (Slice C v1; the metrics type doesn't expose
        // x-height — refine when middle-alignment precision matters).
        let x_height = fs * 0.5;
        for &i in &members {
            let c = &children[i];
            let above = if matches!(c.box_type, BoxType::Text(_)) || seats_as_text(c) {
                text_extents(&c.style).0
            } else if matches!(c.box_type, BoxType::FormControl(_)) && !c.baseline_is_bottom_edge()
            {
                // Bare control, synthetic baseline: the part above it is the
                // box minus the inner-text hang (form_control_baseline_hang).
                (c.dimensions.margin_box().height - c.form_control_baseline_hang()).max(0.0)
            } else if c.baseline_is_bottom_edge() {
                let h = c.dimensions.margin_box().height;
                match c.style.vertical_align {
                    rustkit_css::VerticalAlign::Middle => h / 2.0 + x_height / 2.0,
                    _ => h, // baseline (Slice C subset: others fall through)
                }
            } else if c.style.display.is_atomic_inline() {
                // Inline-block with in-flow content: its last line box's
                // baseline (CSS2 §10.8.1). n53 form-controls: every
                // `label { display: inline-block }` beside an input sat at
                // the line top, 5px above Chrome; a wrapped label hung its
                // row off its second line.
                match c.inline_block_baseline_y() {
                    Some(b) => atomic_above(b - member_top(c)),
                    None => continue,
                }
            } else {
                continue;
            };
            ascent = ascent.max(above);
        }
        let baseline_y = line_top + ascent;

        // Pass 2: shift members to the baseline. Zero-shift when a member's
        // own extent already defines the ascent.
        for &i in &members {
            let c = &mut children[i];
            // A small-font `<span>` beside body text sat at the line top —
            // its baseline a few px above its neighbours' (n54 settings: no
            // on-row text at all). Its slot drops like a text run's; the
            // rect and the text inside move together.
            let target_top = if matches!(c.box_type, BoxType::Text(_)) || seats_as_text(c) {
                baseline_y - text_extents(&c.style).0
            } else if matches!(c.box_type, BoxType::FormControl(_)) && !c.baseline_is_bottom_edge()
            {
                baseline_y
                    - (c.dimensions.margin_box().height - c.form_control_baseline_hang()).max(0.0)
            } else if c.baseline_is_bottom_edge() {
                let mb = c.dimensions.margin_box();
                match c.style.vertical_align {
                    rustkit_css::VerticalAlign::Middle => {
                        baseline_y - x_height / 2.0 - mb.height / 2.0
                    }
                    _ => baseline_y - mb.height,
                }
            } else if c.style.display.is_atomic_inline() {
                match c.inline_block_baseline_y() {
                    Some(b) => baseline_y - atomic_above(b - member_top(c)),
                    None => continue,
                }
            } else {
                continue;
            };
            let current_top = member_top(c);
            let dy = target_top - current_top;
            let is_multi_line_split = c.text_flow_first_offset.is_some()
                && c.text_lines.as_ref().is_some_and(|t| t.len() > 1);
            // A split run's last line cannot move on its own (its lines
            // are content.y + i * line-height); a taller follower on that
            // line leaves the run where it is. Ledgered, not chased.
            if dy.abs() > 0.01 && !is_multi_line_split {
                crate::flex::translate_subtree(c, 0.0, dy);
            }
        }
    }

    /// Apply text-align offset to inline children on a line.
    ///
    /// IFC Slice A: this is the SOLE owner of horizontal alignment. Leaves
    /// never self-align (layout_text places at the flow origin with zero
    /// per-line offsets), so every recorded line shifts as a unit here —
    /// mixed runs keep their relative positions.
    fn apply_text_align_offset(
        children: &mut [LayoutBox],
        line_width: f32,
        container_width: f32,
        text_align: TextAlign,
    ) {
        let offset = match text_align {
            TextAlign::Left => 0.0,
            TextAlign::Right => (container_width - line_width).max(0.0),
            TextAlign::Center => ((container_width - line_width) / 2.0).max(0.0),
            TextAlign::Justify => 0.0, // no line shift: gaps are distributed below
        };

        // css-text-3 §7.3: under `justify`, every line that ends in a soft
        // wrap spreads its slack across its word separators. A wrapped run's
        // last line is never justified — it is either the block's last line
        // or it continues into the next inline sibling (a mixed line, which
        // stays start-aligned: distributing across sibling runs is not
        // implemented). Line 0 of a mid-line split starts after prior
        // siblings for the same reason. Preserved-newline runs (pre-wrap /
        // pre-line) are left alone: their breaks may be forced.
        if matches!(text_align, TextAlign::Justify) {
            for child in children.iter_mut() {
                if !matches!(
                    child.style.white_space,
                    rustkit_css::WhiteSpace::Normal | rustkit_css::WhiteSpace::Nowrap
                ) {
                    continue;
                }
                // Line 0 of a split that starts past the line start is a
                // mixed line (prior siblings own part of it); a split that
                // starts AT the line start owns line 0 outright.
                let first_justifiable =
                    usize::from(child.text_flow_first_offset.is_some_and(|o| o > 0.0));
                let font_size = match child.style.font_size {
                    Length::Px(px) => px,
                    _ => 16.0,
                };
                let style = child.style.clone();
                let Some(text_lines) = child.text_lines.as_mut() else {
                    continue;
                };
                let n = text_lines.len();
                for tl in text_lines
                    .iter_mut()
                    .take(n.saturating_sub(1))
                    .skip(first_justifiable)
                {
                    let opportunities = TextLine::justification_opportunities(tl.text.trim_end());
                    let natural = TextLine::natural_ink_width(&tl.text, &style, font_size);
                    tl.justify_space = match natural {
                        Some(w) if opportunities > 0 && container_width - tl.x_offset - w > 0.0 => {
                            (container_width - tl.x_offset - w) / opportunities as f32
                        }
                        _ => 0.0,
                    };
                }
            }
            return;
        }

        for child in children {
            // A wrapped text run aligns each VISUAL line independently
            // against the container. Only Right/Center write offsets: under
            // Left/Justify any existing x_offset is a FLOW offset from the
            // phase-5 mid-line split (first line starts at the old cursor),
            // which alignment must not clobber.
            if matches!(text_align, TextAlign::Right | TextAlign::Center) {
                if let Some(text_lines) = child.text_lines.as_mut() {
                    if child.text_flow_first_offset.is_some() {
                        // IFC Slice B2: a mid-line split box reaches a
                        // recorded line only via its LAST visual line (the
                        // open line the split left behind; followers in
                        // this record shift by the same line offset).
                        // Earlier lines were closed by align_split_close —
                        // never re-stomp them. `+=` keeps the FLOW offset
                        // when the first and last line coincide.
                        if let Some(last) = text_lines.last_mut() {
                            last.x_offset += offset;
                        }
                        continue;
                    }
                    for tl in text_lines.iter_mut() {
                        tl.x_offset = match text_align {
                            TextAlign::Right => (container_width - tl.width).max(0.0),
                            TextAlign::Center => ((container_width - tl.width) / 2.0).max(0.0),
                            _ => 0.0,
                        };
                    }
                    continue;
                }
            }

            if offset > 0.0
                && (child.style.display.is_atomic_inline()
                    || matches!(
                        child.box_type,
                        BoxType::Inline
                            | BoxType::Text(_)
                            | BoxType::FormControl(_)
                            | BoxType::Image { .. }
                    ))
            {
                // Shift the SUBTREE, not just the box origin: with leaf
                // self-align gone, a span's inner text only moves if the
                // line shift carries it (the old box-only shift depended on
                // the text re-centering itself — the dual-source hack Slice
                // A removes).
                crate::flex::translate_subtree(child, offset, 0.0);
            }
        }
    }

    /// IFC Slice B2: align the CLOSED visual lines of a phase-5 mid-line
    /// text split under Center/Right.
    ///
    /// Line 0 mixes prior inline siblings with the text's FIRST visual
    /// line, so everything on it shifts by ONE offset computed from the
    /// assembled line's width — the first fragment ends up FLOW ⊕ ALIGN
    /// (`x_offset = first_line_offset + O₀`), never re-aligned as if it
    /// owned the whole line. Middle lines belong wholly to the run and
    /// align independently. The LAST line is left untouched: it stays open
    /// for following siblings and is aligned by the recorded-lines pass
    /// (`apply_text_align_offset`) when it closes.
    ///
    /// Under Left/Justify this is a no-op — FLOW offsets are already final.
    fn align_split_close(
        children: &mut [LayoutBox],
        line_start: usize,
        text_index: usize,
        container_width: f32,
        text_align: TextAlign,
    ) {
        if !matches!(text_align, TextAlign::Right | TextAlign::Center) {
            return;
        }
        let Some(text_lines) = children[text_index].text_lines.as_ref() else {
            return;
        };
        let n = text_lines.len();
        if n < 2 {
            return;
        }
        let flow0 = text_lines[0].x_offset;
        let line0_width = flow0 + text_lines[0].width;
        let align = |w: f32| match text_align {
            TextAlign::Right => (container_width - w).max(0.0),
            TextAlign::Center => ((container_width - w) / 2.0).max(0.0),
            _ => 0.0,
        };
        let o0 = align(line0_width);
        if o0 > 0.0 {
            let (prior, rest) = children.split_at_mut(text_index);
            for sib in &mut prior[line_start..] {
                // A prior sibling that is itself a mid-line split shares
                // this line box only through ITS last visual line —
                // shifting the whole box would drag its earlier lines
                // along.
                if sib.text_flow_first_offset.is_some() {
                    if let Some(tls) = sib.text_lines.as_mut() {
                        if let Some(last) = tls.last_mut() {
                            last.x_offset += o0;
                        }
                        continue;
                    }
                }
                crate::flex::translate_subtree(sib, o0, 0.0);
            }
            if let Some(tls) = rest[0].text_lines.as_mut() {
                tls[0].x_offset = flow0 + o0;
            }
        }
        // Middle lines (0 and last excluded): pure per-line alignment.
        if let Some(tls) = children[text_index].text_lines.as_mut() {
            for tl in &mut tls[1..n - 1] {
                tl.x_offset = align(tl.width);
            }
        }
    }

    /// Layout block children with margin collapse.
    fn layout_block_children_with_collapse(
        &mut self,
        margin_context: &mut MarginCollapseContext,
        float_context: &mut FloatContext,
        definite_height: Option<f32>,
    ) {
        let mut cursor_y = 0.0;
        let mut cursor_x = 0.0;
        let mut line_height = 0.0_f32;
        // See layout_block_children: below-baseline extent of the current line.
        let mut line_below_baseline = 0.0_f32;
        // Above/below-baseline extents of the current line's baseline-anchored
        // members; the strut floors both when the line closes (line_advance).
        let mut line_extents = (0.0_f32, 0.0_f32);
        let container_width = self.dimensions.content.width;
        let text_align = self.style.text_align;
        let strut_descent = self.inline_strut_descent();
        let strut = Self::text_line_box_extents(&self.style);
        // See layout_block_children: a `<br>` on an empty line is a line box.
        let empty_line_height = self.get_line_height();
        // white-space: nowrap|pre suppress soft-wrapping (see layout_block_children).
        let container_allows_wrap = !matches!(
            self.style.white_space,
            rustkit_css::WhiteSpace::Nowrap | rustkit_css::WhiteSpace::Pre
        );
        // css-text-3 §5.1: an author `width: 0` is a RESOLVED used width, so
        // text in it wraps at every opportunity it has (WPT
        // break-boundary-2-chars-001: `a` / `b` / `c` down a zero-wide
        // break-all inline-block). The block path reads a bare 0 as "no
        // resolved width yet" (intrinsic pass); only the container can say
        // which zero this is. Absolute lengths only — a percentage of an
        // unresolved parent also lands on 0 and must keep the guard.
        let container_is_definite_zero = container_width <= 0.0
            && (matches!(self.style.width, Length::Zero)
                || matches!(self.style.width, Length::Px(px) if px <= 0.0));

        // Track lines for text-align adjustment after layout: (start_index, end_index, line_width)
        let mut lines: Vec<(usize, usize, f32)> = Vec::new();
        let mut line_start_index: Option<usize> = None;
        let mut line_width = 0.0_f32;
        // IFC Slice B2: phase-5 mid-line splits whose CLOSED lines (line 0
        // + middles) need alignment after the loop: (line_start, text_index).
        let mut split_records: Vec<(usize, usize)> = Vec::new();

        // `cb.content.height = cursor_y` below is the STATIC POSITION trick
        // (calculate_block_position stacks a box at cb.y + cb.height), not
        // the containing block's height — so `bottom: 0` / `inset: 0` on an
        // abspos child resolves against "content laid out so far". A
        // `::after { inset: 0 }` cover on a `height: 100px` div holding two
        // 27px lines came out 54px tall (WPT overflow-wrap-anywhere-001).
        // Once this box is final, reanchor_absolute_children re-resolves the
        // child against the real padding box (CSS 2.1 §10.1).

        for (i, child) in self.children.iter_mut().enumerate() {
            // Skip absolutely/fixed positioned children for flow layout
            if child.position == Position::Absolute || child.position == Position::Fixed {
                let mut cb = self.dimensions.clone();
                cb.content.height = cursor_y;
                // Out-of-flow boxes do not participate in margin collapsing
                // (CSS 2.1 §8.3.1). The static position must still land where
                // the box would have been in flow — including the pending
                // sibling margin — so the child gets a CLONE of the context to
                // resolve against; the live context stays intact for the next
                // in-flow sibling (an abspos box between two blocks must not
                // swallow the margin between them).
                let mut oof_margin_context = margin_context.clone();
                child.layout_with_collapse(&cb, &mut oof_margin_context, float_context);
                continue;
            }

            // Check if child is an atomic inline (inline-block/-flex/-grid)
            let is_inline_block = child.style.display.is_atomic_inline();

            // Line boxes interrupt margin adjacency (CSS 2.1 §8.3.1): any
            // inline-level child (inline-block, inline, text, replaced) must
            // first materialize the pending block margin into the cursor —
            // otherwise the preceding block's bottom margin is silently
            // dropped — and must not collapse margins across the line box.
            let is_inline_level = is_inline_block
                || matches!(
                    child.box_type,
                    BoxType::Inline
                        | BoxType::Text(_)
                        | BoxType::Image { .. }
                        | BoxType::FormControl(_)
                        | BoxType::LineBreak
                );
            if is_inline_level {
                cursor_y += margin_context.resolve();
                margin_context.reset();
            }

            // `<br>`: forced line break (see layout_block_children).
            if matches!(child.box_type, BoxType::LineBreak) {
                if let Some(start) = line_start_index {
                    lines.push((start, i, line_width));
                }
                let advance = if cursor_x > 0.0 || line_height > 0.0 {
                    line_advance(line_height, line_below_baseline, line_extents, strut)
                } else {
                    empty_line_height
                };
                child.dimensions.content = Rect::new(
                    self.dimensions.content.x + cursor_x,
                    self.dimensions.content.y + cursor_y,
                    0.0,
                    0.0,
                );
                cursor_y += advance;
                cursor_x = 0.0;
                line_height = 0.0;
                line_below_baseline = 0.0;
                line_extents = (0.0, 0.0);
                line_start_index = None;
                line_width = 0.0;
                continue;
            }

            // CSS2 §9.4.2: ALL inline-level boxes share line boxes — see
            // layout_block_children for the full rationale.
            let flows_inline = is_inline_block
                || (child.style.display == rustkit_css::Display::Inline
                    && matches!(
                        child.box_type,
                        BoxType::Inline | BoxType::FormControl(_) | BoxType::Image { .. }
                    ))
                // IFC Slice B (symmetric join): a fitting text run joins the
                // line from ANY cursor position, including 0. The old
                // cursor_x > 0 gate sent the FIRST text sibling down the
                // block path, so `Some <b>bold</b> text` stacked as
                // "Some" / "bold text" — half line-model, half leaf-model
                // (session-3 falsification). Non-fitting text keeps the
                // block path (wrap or phase-5 split as before).
                // Under nowrap/pre, text never soft-wraps, so it joins the
                // line from any cursor position — even past container_width
                // (e.g. whitespace between inline-blocks in an overflow-x
                // scroller). Otherwise it flows inline only when it fits.
                || (matches!(child.box_type, BoxType::Text(_))
                    && (!container_allows_wrap
                        || child.text_single_line_width() <= container_width - cursor_x));

            if flows_inline {
                // Inline-level content never collapses margins with siblings
                // (and an inline-block establishes its own BFC), so lay it
                // out against a throwaway context instead of leaking margins
                // into the parent's.
                let mut ib_margin_context = MarginCollapseContext::new();
                let margin_context = &mut ib_margin_context;
                // Layout to get dimensions first
                let mut cb = self.dimensions.clone();
                cb.content.x = self.dimensions.content.x + cursor_x;
                cb.content.y = self.dimensions.content.y + cursor_y;
                // Children self-position at cb.y + cb.height; the cursor is
                // already baked into cb.y, so the height term must be zero.
                cb.content.height = 0.0;
                child.layout_with_collapse_in(&cb, margin_context, float_context, definite_height);

                let child_width = child.dimensions.margin_box().width;
                let child_height = child.dimensions.margin_box().height;

                // Check if child fits on current line (nowrap/pre never soft-wrap)
                if container_allows_wrap
                    && cursor_x > 0.0
                    && cursor_x + child_width > container_width
                {
                    // Record completed line for text-align
                    if let Some(start) = line_start_index {
                        lines.push((start, i, line_width));
                    }

                    // Wrap to next line
                    cursor_y += line_advance(line_height, line_below_baseline, line_extents, strut);
                    cursor_x = 0.0;
                    line_height = 0.0;
                    line_below_baseline = 0.0;
                    line_extents = (0.0, 0.0);
                    line_start_index = Some(i);
                    line_width = 0.0;

                    // Re-layout at new position
                    cb.content.x = self.dimensions.content.x;
                    cb.content.y = self.dimensions.content.y + cursor_y;
                    child.layout_with_collapse_in(
                        &cb,
                        margin_context,
                        float_context,
                        definite_height,
                    );
                }

                // Track line start
                if line_start_index.is_none() {
                    line_start_index = Some(i);
                }

                // Position the child. The cursor addresses the MARGIN box;
                // the content rect sits margin+border+padding inside it
                // (dropping border/padding here shifted every decorated
                // inline-block up-left by border+padding).
                child.dimensions.content.x = self.dimensions.content.x
                    + cursor_x
                    + child.dimensions.margin.left
                    + child.dimensions.border.left
                    + child.dimensions.padding.left;
                child.dimensions.content.y = self.dimensions.content.y
                    + cursor_y
                    + child.dimensions.margin.top
                    + child.dimensions.border.top
                    + child.dimensions.padding.top;

                // NON-REPLACED inline (§10.6.1/§10.8.1): the rect is the
                // content area, but the LINE still advances by the child's
                // line-height, and the content box sits a half-leading below
                // the line top. Replaced/atomic inlines keep border-box line
                // sizing. Without the decoupling, shrinking the rect would
                // collapse every line to font height — the advance and the
                // report are different quantities.
                if matches!(child.box_type, BoxType::Inline) {
                    let (_, half_leading) = child.inline_content_area();
                    if half_leading > 0.0 {
                        child.shift_inline_content_area(half_leading);
                    }
                    line_height = line_height.max(child.get_line_height());
                    // The inline's text wrapped onto several line boxes: the
                    // flow must advance past ALL of them, exactly as the
                    // phase-5 split below does for a bare text run. Before
                    // n50 the line advanced by ONE line-height and the next
                    // block sibling was laid over lines 2..n (`<p><span>long
                    // text</span></p>` was one line tall; `<pre><code>` six
                    // lines painted over the following paragraph).
                    if let Some((n_lines, last_end, lh)) =
                        Self::inline_wrapped_tail(child, self.dimensions.content.x)
                    {
                        if let Some(start) = line_start_index {
                            lines.push((start, i + 1, line_width + child_width));
                        }
                        cursor_y += line_advance(line_height, line_below_baseline, line_extents, strut).max(lh)
                            + (n_lines as f32 - 2.0).max(0.0) * lh;
                        cursor_x = last_end;
                        line_width = last_end;
                        line_height = lh;
                        line_below_baseline = 0.0;
                        line_extents = (0.0, 0.0);
                        line_start_index = Some(i);
                        continue;
                    }
                } else {
                    line_height = line_height.max(child_height);
                }
                // Advance cursor
                cursor_x += child_width;
                line_width += child_width;
                // vertical-align: top|bottom boxes do not anchor to the
                // baseline at all (CSS2 §10.8): a top-aligned box hangs from
                // the line-box top, so a tall one SWALLOWS the strut instead
                // of stacking above its descent. Extending the strut under
                // them made every nowrap card row strut_descent too tall —
                // sticky-scroll's .horizontal-scroll read 156.8 where Chrome
                // reads 150 (+6.8 = descent + half-leading at line-height
                // 1.6), and the error cascaded into every later sibling's y.
                // Every baseline-anchored member raises the line's ascent or
                // deepens its descent; the line box is their sum, not the
                // tallest member (n54: `<label>Text:</label><input>` with no
                // whitespace between built a 19px line for Chrome's 24).
                let member_extents = match child.float {
                    Float::None => child.line_member_baseline_extents(),
                    _ => None,
                };
                if let Some((above, below)) = member_extents {
                    line_extents = (line_extents.0.max(above), line_extents.1.max(below));
                }
                let anchored_to_baseline = !matches!(
                    child.style.vertical_align,
                    rustkit_css::VerticalAlign::Top | rustkit_css::VerticalAlign::Bottom
                );
                if anchored_to_baseline && child.baseline_is_bottom_edge() {
                    // Box bottom sits ON the baseline; the strut extends the
                    // line box below it (CSS2 §10.8.1).
                    line_below_baseline = line_below_baseline.max(child_height + strut_descent);
                }

                if child.float != Float::None {
                    cursor_x -= child_width;
                    line_width -= child_width;
                }
            } else if Self::text_splits_inline(child, cursor_x) {
                // Phase 5 (IFC text splitting) — see layout_block_children.
                let cb = self.dimensions.clone();
                let line_top = self.dimensions.content.y + cursor_y;
                let (n_lines, last_w) = child.layout_text_in_flow(&cb, line_top, cursor_x);
                let lh = child.get_line_height();
                if n_lines <= 1 {
                    cursor_x += last_w;
                    line_width += last_w;
                    line_height = line_height.max(lh);
                } else {
                    // B2: closed lines aligned by align_split_close below.
                    split_records.push((line_start_index.unwrap_or(i), i));
                    cursor_y += line_advance(line_height, line_below_baseline, line_extents, strut).max(lh)
                        + (n_lines as f32 - 2.0).max(0.0) * lh;
                    cursor_x = last_w;
                    line_width = last_w;
                    line_height = lh;
                    line_below_baseline = 0.0;
                    line_extents = (0.0, 0.0);
                    line_start_index = Some(i);
                }
            } else {
                // Regular block layout with margin collapse
                // First, finish any inline-block line
                if cursor_x > 0.0 {
                    if let Some(start) = line_start_index {
                        lines.push((start, i, line_width));
                    }
                    cursor_y += line_advance(line_height, line_below_baseline, line_extents, strut);
                    cursor_x = 0.0;
                    line_height = 0.0;
                    line_below_baseline = 0.0;
                    line_extents = (0.0, 0.0);
                    line_start_index = None;
                    line_width = 0.0;
                }

                let mut cb = self.dimensions.clone();
                cb.content.height = cursor_y;
                match (container_is_definite_zero, &child.box_type) {
                    // Author `width: 0`: block-path text wraps against it
                    // (see layout_text_with_zero_wrap). Text has no margins
                    // to collapse, so bypassing the collapse entry is exact.
                    (true, BoxType::Text(t)) => {
                        let t = t.clone();
                        child.layout_text_with_zero_wrap(t, &cb, true);
                    }
                    _ => child.layout_with_collapse_in(
                        &cb,
                        margin_context,
                        float_context,
                        definite_height,
                    ),
                }

                // See layout_block_children: keep inline box decoration aligned.
                if matches!(child.box_type, BoxType::Inline) {
                    lines.push((i, i + 1, child.dimensions.margin_box().width));
                }
                // IFC Slice A: a text run laid on the block path (a lone or
                // first text child — the inline gate requires cursor_x > 0)
                // is a single-item line. Leaves no longer self-align, so
                // without this record centered headings would go left.
                if matches!(child.box_type, BoxType::Text(_)) {
                    lines.push((i, i + 1, child.dimensions.content.width));
                }

                if child.float == Float::None {
                    cursor_y = child.dimensions.border_box().bottom() - self.dimensions.content.y;
                    // An inline image on the baseline leaves the strut's
                    // descent + half-leading below it (see inline_strut_descent).
                    if matches!(child.box_type, BoxType::Image { .. })
                        && child.style.display == rustkit_css::Display::Inline
                    {
                        cursor_y += strut_descent;
                    }
                }
            }
        }

        // Record any remaining inline-block line
        if cursor_x > 0.0 {
            if let Some(start) = line_start_index {
                lines.push((start, self.children.len(), line_width));
            }
            cursor_y += line_advance(line_height, line_below_baseline, line_extents, strut);
        }

        // IFC Slice B2: align the CLOSED lines of each mid-line split —
        // see layout_block_children.
        for (start, ti) in split_records {
            Self::align_split_close(&mut self.children, start, ti, container_width, text_align);
        }

        // Apply text-align to all recorded lines
        let valign_font = self.style.clone();
        for (start, end, width) in lines {
            Self::apply_text_align_offset(
                &mut self.children[start..end],
                width,
                container_width,
                text_align,
            );
            // Slice C: vertical alignment about the line baseline.
            Self::apply_vertical_align(&mut self.children[start..end], &valign_font);
        }

        // CSS 2.1 §8.3.1: the last in-flow block child's bottom margin is
        // still pending in the context. When this box's own padding-bottom /
        // border-bottom / definite height blocks parent-child collapse, that
        // margin belongs INSIDE this box — materialize it into the content
        // height. (Every padded container was measuring 10px short: the
        // pending margin was silently dropped on return, and the form-control
        // bare-height blobs had calibrated themselves against the deficit.)
        // When collapse-through is allowed (the owner set the flag), the
        // margin stays pending: layout_block_with_collapse adjoins it to the
        // owner's own bottom margin. (It used to be silently dropped here —
        // `ul > li:last-child{margin-bottom:2px}` lost its 2px, and a plain
        // wrapper's last child lost its 4px on css-selectors.) Contexts
        // without the flag — flex/grid item re-layouts, closed bottom edges —
        // keep the margin inside the box, as a formatting root must.
        if !margin_context.last_child_collapses_through {
            cursor_y += margin_context.resolve();
            margin_context.reset();
        }

        self.dimensions.content.height = cursor_y;
    }

    /// CSS 2.1 §10.5: the content height a percentage-height CHILD of this
    /// box resolves against. `Some` only when this box's own height is
    /// definite WITHOUT consulting its children, so it is knowable before
    /// they lay out; `None` means "depends on content".
    ///
    /// This exists because the containing block a child is handed carries the
    /// parent's FLOW CURSOR in `content.height` (the static-position trick in
    /// `layout_block_children_with_collapse`), not the parent's height — so a
    /// percentage child saw 0 and `calculate_block_height` fell back to the
    /// viewport. The same shape as the abspos defect `reanchor_absolute_children`
    /// closes, on the in-flow side.
    ///
    /// `min-height`/`max-height` are deliberately not consulted: they clamp
    /// the used height after children flow, which is exactly the "depends on
    /// content" case the spec makes `auto`.
    fn definite_content_height_for_children(&self, containing_block_height: f32) -> Option<f32> {
        let specified = match self.style.height {
            Length::Px(px) => px,
            Length::Percent(pct) if containing_block_height > 0.0 => {
                pct / 100.0 * containing_block_height
            }
            // A `calc()` height is as definite as the terms it is made of: it
            // needs a base only where it carries a percentage term.
            Length::Calc(ref sum)
                if sum.percent == 0.0 || containing_block_height > 0.0 =>
            {
                self.length_to_px(&self.style.height, containing_block_height)
            }
            Length::Vh(vh) if self.viewport.1 > 0.0 => vh / 100.0 * self.viewport.1,
            Length::Em(em) => {
                let font_size = match self.style.font_size {
                    Length::Px(px) => px,
                    _ => 16.0,
                };
                em * font_size
            }
            Length::Rem(rem) => rem * 16.0,
            _ => return None,
        };
        let content = if self.style.box_sizing == BoxSizing::BorderBox {
            specified
                - (self.dimensions.padding.top
                    + self.dimensions.padding.bottom
                    + self.dimensions.border.top
                    + self.dimensions.border.bottom)
        } else {
            specified
        };
        // Defensive, and measured to be exactly that: every consumer of this
        // base runs through `calculate_block_height`, whose min-height pass
        // floors a negative content height at 0 anyway, so no assertion can
        // require this clamp (mutation probe M10, 2026-09-20, survived the
        // whole 415-test suite). Kept because a negative CONTENT BOX is not a
        // thing to hand anyone, not because a test needs it.
        Some(content.max(0.0))
    }

    /// Calculate block height.
    /// `percent_base` is the containing block's DEFINITE content height, used
    /// for resolving percentage heights. `None` means the containing block has
    /// no definite height; the historical viewport fallback still applies
    /// there (CSS 2.1 §10.5 makes the value `auto` — see
    /// `an_auto_height_parent_hands_its_percentage_child_no_definite_base`).
    /// `Some(0.0)` is a DEFINITE zero and resolves to zero: a bare `f32` could
    /// not tell the two apart, so a `height: 0` parent handed its percentage
    /// child the viewport.
    fn calculate_block_height(&mut self, percent_base: Option<f32>) {
        // Get padding and border for box-sizing calculations
        let padding_top = self.dimensions.padding.top;
        let padding_bottom = self.dimensions.padding.bottom;
        let border_top = self.dimensions.border.top;
        let border_bottom = self.dimensions.border.bottom;
        let padding_border_height = padding_top + padding_bottom + border_top + border_bottom;
        let is_border_box = self.style.box_sizing == BoxSizing::BorderBox;

        // Lifted out of the match so the arm below can read the sum without
        // borrowing `self.style` across the assignment to `self.dimensions`.
        let calc_height = match &self.style.height {
            Length::Calc(sum) => Some(**sum),
            _ => None,
        };

        // If height is explicitly set, use it
        match self.style.height {
            Length::Px(h) => {
                // With box-sizing: border-box, specified height includes padding and border
                self.dimensions.content.height = if is_border_box {
                    (h - padding_border_height).max(0.0)
                } else {
                    h
                };
            }
            Length::Percent(pct) => {
                // A definite base resolves the percentage, zero included; with
                // no definite base the viewport fallback stands (unchanged).
                let reference_height = match percent_base {
                    Some(h) => Some(h),
                    None if self.viewport.1 > 0.0 => Some(self.viewport.1),
                    None => None,
                };
                if let Some(reference_height) = reference_height {
                    let specified = pct / 100.0 * reference_height;
                    self.dimensions.content.height = if is_border_box {
                        (specified - padding_border_height).max(0.0)
                    } else {
                        specified
                    };
                }
            }
            Length::Calc(_) => {
                // css-values-3 §8.1: a `calc()` over lengths and percentages
                // IS a length once the percentage basis is known, so it takes
                // exactly the base a bare percentage takes — including the
                // viewport fallback for an indefinite parent. A calc with no
                // percentage term needs no base at all and must not be lost
                // with one: `calc(2em + 4px)` is definite everywhere.
                let sum = calc_height.unwrap_or_default();
                let reference_height = match percent_base {
                    Some(h) => Some(h),
                    None if sum.percent == 0.0 => Some(0.0),
                    None if self.viewport.1 > 0.0 => Some(self.viewport.1),
                    None => None,
                };
                if let Some(reference_height) = reference_height {
                    let specified = self.length_to_px(&self.style.height, reference_height);
                    self.dimensions.content.height = if is_border_box {
                        (specified - padding_border_height).max(0.0)
                    } else {
                        specified
                    };
                }
            }
            Length::Vh(vh) => {
                if self.viewport.1 > 0.0 {
                    let specified = vh / 100.0 * self.viewport.1;
                    self.dimensions.content.height = if is_border_box {
                        (specified - padding_border_height).max(0.0)
                    } else {
                        specified
                    };
                }
            }
            Length::Em(em) => {
                let font_size = match self.style.font_size {
                    Length::Px(px) => px,
                    _ => 16.0,
                };
                let specified = em * font_size;
                self.dimensions.content.height = if is_border_box {
                    (specified - padding_border_height).max(0.0)
                } else {
                    specified
                };
            }
            Length::Rem(rem) => {
                let specified = rem * 16.0; // Root font size
                self.dimensions.content.height = if is_border_box {
                    (specified - padding_border_height).max(0.0)
                } else {
                    specified
                };
            }
            _ => {
                // Auto or Zero - content.height was set by layout_block_children
                // But if aspect-ratio is set and we have a width, calculate height from it
                let padding_border_width = self.dimensions.padding.left
                    + self.dimensions.padding.right
                    + self.dimensions.border.left
                    + self.dimensions.border.right;
                if let Some(h) = aspect_ratio_content_height(
                    &self.style,
                    self.dimensions.content.width,
                    padding_border_width,
                    padding_border_height,
                ) {
                    self.dimensions.content.height = h;
                }
            }
        }

        // Apply min-height constraint (also respects box-sizing)
        let min_height_raw = match self.style.min_height {
            Length::Px(px) => px,
            Length::Vh(vh) => vh / 100.0 * self.viewport.1,
            Length::Percent(pct) => pct / 100.0 * self.viewport.1,
            // Same basis as the `Percent` arm above, deliberately: a `calc()`
            // is a length, so it must not resolve against a different
            // reference from the bare percentage sitting next to it. (That
            // basis is `self.viewport.1` rather than `percent_base` here —
            // a pre-existing inconsistency with the height arm, left as it is
            // so this change carries one rule and not two.)
            Length::Calc(_) => self.length_to_px(&self.style.min_height, self.viewport.1),
            _ => 0.0,
        };
        let min_height = if is_border_box && min_height_raw > 0.0 {
            (min_height_raw - padding_border_height).max(0.0)
        } else {
            min_height_raw
        };
        if self.dimensions.content.height < min_height {
            self.dimensions.content.height = min_height;
        }

        // Apply max-height constraint (also respects box-sizing)
        let max_height_raw = match self.style.max_height {
            Length::Px(px) => px,
            Length::Vh(vh) => vh / 100.0 * self.viewport.1,
            Length::Percent(pct) => pct / 100.0 * self.viewport.1,
            Length::Calc(_) => self.length_to_px(&self.style.max_height, self.viewport.1),
            _ => f32::INFINITY,
        };
        let max_height = if is_border_box && max_height_raw < f32::INFINITY {
            (max_height_raw - padding_border_height).max(0.0)
        } else {
            max_height_raw
        };
        if self.dimensions.content.height > max_height {
            self.dimensions.content.height = max_height;
        }
    }

    /// Convert a Length to pixels.
    fn length_to_px(&self, length: &Length, container_size: f32) -> f32 {
        let font_size = match &self.style.font_size {
            Length::Px(px) => *px,
            _ => 16.0,
        };
        length.to_px_with_viewport(
            font_size,
            16.0,
            container_size,
            self.viewport.0,
            self.viewport.1,
        )
    }

    /// Set viewport dimensions for this box and all children.
    pub fn set_viewport(&mut self, width: f32, height: f32) {
        self.viewport = (width, height);
        for child in &mut self.children {
            child.set_viewport(width, height);
        }
    }

    /// Get children sorted by z-index for painting.
    pub fn get_paint_order(&self) -> Vec<&LayoutBox> {
        let mut normal_flow: Vec<&LayoutBox> = Vec::new();
        let mut positioned: Vec<(&LayoutBox, i32)> = Vec::new();

        for child in &self.children {
            if child.position == Position::Static {
                normal_flow.push(child);
            } else {
                positioned.push((child, child.z_index));
            }
        }

        // Sort positioned elements by z-index
        positioned.sort_by(|a, b| a.1.cmp(&b.1));

        // Combine: negative z-index, normal flow, positive z-index
        let mut result: Vec<&LayoutBox> = Vec::new();

        // Add negative z-index positioned elements first
        for (child, z) in positioned.iter() {
            if *z < 0 {
                result.push(child);
            }
        }

        // Add normal flow elements
        result.extend(normal_flow);

        // Add zero and positive z-index positioned elements
        for (child, z) in positioned.iter() {
            if *z >= 0 {
                result.push(child);
            }
        }

        result
    }

    /// Perform hit testing at the given point.
    /// Returns the hit test result with information about the element at the point.
    pub fn hit_test(&self, x: f32, y: f32) -> Option<HitTestResult> {
        self.hit_test_internal(x, y, 0)
    }

    /// Internal hit test that tracks depth.
    fn hit_test_internal(&self, x: f32, y: f32, depth: u32) -> Option<HitTestResult> {
        // Get the border box for this element
        let border_box = self.dimensions.border_box();

        // Check if the point is within our border box
        if !border_box.contains(x, y) {
            return None;
        }

        // Check children in reverse paint order (topmost first)
        let paint_order = self.get_paint_order();
        for child in paint_order.iter().rev() {
            if let Some(mut result) = child.hit_test_internal(x, y, depth + 1) {
                // Found a hit in a child - add ourselves to the path
                result.ancestors.push(HitTestAncestor {
                    box_type: self.box_type.clone(),
                    border_box: self.dimensions.border_box(),
                    content_box: self.dimensions.content,
                    z_index: self.z_index,
                    position: self.position,
                });
                // Nearest link wins: only fill in from an ancestor if no
                // closer box already supplied one.
                if result.link_href.is_none() {
                    result.link_href = self.link_href.clone();
                }
                return Some(result);
            }
        }

        // Detect if element is scrollable by checking overflow style
        let is_scrollable = self.detect_overflow();

        // No child was hit, so we are the target
        Some(HitTestResult {
            box_type: self.box_type.clone(),
            border_box,
            content_box: self.dimensions.content,
            padding_box: self.dimensions.padding_box(),
            local_x: x - border_box.x,
            local_y: y - border_box.y,
            depth,
            ancestors: Vec::new(),
            z_index: self.z_index,
            position: self.position,
            is_scrollable,
            link_href: self.link_href.clone(),
            node_id: self.node_id,
        })
    }

    /// Detect if this element has overflow and should be scrollable.
    fn detect_overflow(&self) -> bool {
        use rustkit_css::Overflow;

        // Check if overflow is set to scroll or auto
        let has_overflow_style = matches!(self.style.overflow_x, Overflow::Scroll | Overflow::Auto)
            || matches!(self.style.overflow_y, Overflow::Scroll | Overflow::Auto);

        if !has_overflow_style {
            return false;
        }

        // Check if content actually overflows the container
        let padding_box = self.dimensions.padding_box();
        let mut content_width = 0.0_f32;
        let mut content_height = 0.0_f32;

        // Calculate total content size including children
        for child in &self.children {
            let child_box = child.dimensions.margin_box();
            content_width = content_width.max(child_box.x + child_box.width - padding_box.x);
            content_height = content_height.max(child_box.y + child_box.height - padding_box.y);
        }

        // Element is scrollable if content exceeds container
        content_width > padding_box.width || content_height > padding_box.height
    }

    /// Check if a point is within the border box.
    pub fn contains_point(&self, x: f32, y: f32) -> bool {
        self.dimensions.border_box().contains(x, y)
    }

    /// Get all elements at a point (including overlapping elements).
    pub fn hit_test_all(&self, x: f32, y: f32) -> Vec<HitTestResult> {
        let mut results = Vec::new();
        self.hit_test_all_internal(x, y, 0, &mut results);
        results
    }

    /// Internal hit test that collects all results.
    fn hit_test_all_internal(&self, x: f32, y: f32, depth: u32, results: &mut Vec<HitTestResult>) {
        let border_box = self.dimensions.border_box();

        if !border_box.contains(x, y) {
            return;
        }

        // Add this element
        results.push(HitTestResult {
            box_type: self.box_type.clone(),
            border_box,
            content_box: self.dimensions.content,
            padding_box: self.dimensions.padding_box(),
            local_x: x - border_box.x,
            local_y: y - border_box.y,
            depth,
            ancestors: Vec::new(),
            z_index: self.z_index,
            position: self.position,
            is_scrollable: false,
            link_href: self.link_href.clone(),
            node_id: self.node_id,
        });

        // Check all children
        for child in &self.children {
            child.hit_test_all_internal(x, y, depth + 1, results);
        }
    }
}

/// Result of a hit test operation.
#[derive(Debug, Clone)]
pub struct HitTestResult {
    /// The type of the hit box.
    pub box_type: BoxType,
    /// The border box of the hit element.
    pub border_box: Rect,
    /// The content box of the hit element.
    pub content_box: Rect,
    /// The padding box of the hit element.
    pub padding_box: Rect,
    /// X coordinate relative to the border box.
    pub local_x: f32,
    /// Y coordinate relative to the border box.
    pub local_y: f32,
    /// Depth in the layout tree (0 = root).
    pub depth: u32,
    /// Ancestor chain from parent to root.
    pub ancestors: Vec<HitTestAncestor>,
    /// Z-index of the hit element.
    pub z_index: i32,
    /// Position property of the hit element.
    pub position: Position,
    /// Whether the element is scrollable.
    pub is_scrollable: bool,
    /// `href` of the nearest `<a href>` ancestor (or the hit box itself).
    /// A click on a link's text, or on an image nested inside it, resolves
    /// to the same link — which is what makes a hit test navigable without
    /// walking back into the DOM.
    pub link_href: Option<String>,
    /// Raw `NodeId` of the hit box's DOM node, if it had one. Unlike
    /// `link_href` this is NOT inherited from ancestors: the caller wants
    /// the element actually under the cursor.
    pub node_id: Option<usize>,
}

impl HitTestResult {
    /// Check if the hit was in the content area.
    pub fn is_in_content(&self) -> bool {
        self.content_box.contains(
            self.border_box.x + self.local_x,
            self.border_box.y + self.local_y,
        )
    }

    /// Check if the hit was in the padding area.
    pub fn is_in_padding(&self) -> bool {
        let abs_x = self.border_box.x + self.local_x;
        let abs_y = self.border_box.y + self.local_y;
        self.padding_box.contains(abs_x, abs_y) && !self.content_box.contains(abs_x, abs_y)
    }

    /// Check if the hit was in the border area.
    pub fn is_in_border(&self) -> bool {
        let abs_x = self.border_box.x + self.local_x;
        let abs_y = self.border_box.y + self.local_y;
        self.border_box.contains(abs_x, abs_y) && !self.padding_box.contains(abs_x, abs_y)
    }
}

/// Information about an ancestor in the hit test path.
#[derive(Debug, Clone)]
pub struct HitTestAncestor {
    /// Box type.
    pub box_type: BoxType,
    /// Border box.
    pub border_box: Rect,
    /// Content box.
    pub content_box: Rect,
    /// Z-index.
    pub z_index: i32,
    /// Position property.
    pub position: Position,
}

/// Border radius values for each corner.
#[derive(Debug, Clone, Copy, Default)]
pub struct BorderRadius {
    pub top_left: f32,
    pub top_right: f32,
    pub bottom_right: f32,
    pub bottom_left: f32,
}

impl BorderRadius {
    /// Create uniform border radius.
    pub fn uniform(radius: f32) -> Self {
        Self {
            top_left: radius,
            top_right: radius,
            bottom_right: radius,
            bottom_left: radius,
        }
    }

    /// Check if all radii are zero (no rounding).
    pub fn is_zero(&self) -> bool {
        self.top_left == 0.0
            && self.top_right == 0.0
            && self.bottom_right == 0.0
            && self.bottom_left == 0.0
    }
}

/// A paint command for rendering.
#[derive(Debug, Clone)]
pub enum DisplayCommand {
    /// Fill a rectangle with a solid color.
    SolidColor(Color, Rect),
    /// Fill a rounded rectangle with a solid color.
    RoundedRect {
        color: Color,
        rect: Rect,
        radius: BorderRadius,
    },
    /// Draw a border.
    Border {
        color: Color,
        rect: Rect,
        top: f32,
        right: f32,
        bottom: f32,
        left: f32,
    },
    /// Draw text.
    Text {
        text: String,
        x: f32,
        y: f32,
        color: Color,
        font_size: f32,
        font_family: String,
        font_weight: u16,
        font_style: u8,
        /// ADVANCE CONTRACT (text-stack unification, 2026-07-11): per-char
        /// horizontal advances from the LAYOUT shaper. When present, paint
        /// places glyphs at these advances instead of re-deriving its own —
        /// layout width and painted ink stay in lockstep (the ~3-4% drift
        /// class). None = legacy paths (forms/svg placeholders); the
        /// renderer falls back to its own advances.
        advances: Option<Vec<f32>>,
        /// Baseline ascent from the layout shaper (same contract): paint
        /// positions the baseline at y + ascent instead of consulting a
        /// third per-glyph shaper.
        ascent: Option<f32>,
    },
    /// Draw text decoration line (underline, strikethrough, overline).
    TextDecoration {
        x: f32,
        y: f32,
        width: f32,
        thickness: f32,
        color: Color,
        style: TextDecorationStyleValue,
    },
    /// Draw an image.
    Image {
        /// URL or cache key of the image
        url: String,
        /// Source rectangle in the image (for sprites or cropping)
        src_rect: Option<Rect>,
        /// Destination rectangle on screen
        dest_rect: Rect,
        /// Object-fit mode
        object_fit: ObjectFit,
        /// Opacity (0.0 - 1.0)
        opacity: f32,
        /// The box's computed CSS `color` — what `currentColor` resolves to
        /// when the image is a vector document painted in place (an inline
        /// `<svg>` icon). Raster images ignore it.
        current_color: Color,
    },
    /// Draw a background image.
    BackgroundImage {
        /// URL or cache key of the image
        url: String,
        /// Destination rectangle
        rect: Rect,
        /// Background size
        size: BackgroundSize,
        /// Background position (0-1 range)
        position: (f32, f32),
        /// Background repeat
        repeat: BackgroundRepeat,
    },
    /// Draw a box shadow.
    BoxShadow {
        /// Shadow offset X
        offset_x: f32,
        /// Shadow offset Y
        offset_y: f32,
        /// Blur radius
        blur_radius: f32,
        /// Spread radius
        spread_radius: f32,
        /// Shadow color
        color: Color,
        /// Box rectangle (shadow is drawn outside this box, or inside if inset)
        rect: Rect,
        /// Whether this is an inset shadow
        inset: bool,
    },
    /// Apply a backdrop filter (blur, grayscale, etc.) to the pixels behind this rectangle.
    BackdropFilter {
        /// The rectangle to apply the filter to.
        rect: Rect,
        /// Border radius for clipping.
        border_radius: BorderRadius,
        /// The filter to apply.
        filter: rustkit_css::BackdropFilter,
    },
    /// Draw a linear gradient.
    LinearGradient {
        rect: Rect,
        direction: rustkit_css::GradientDirection,
        stops: Vec<rustkit_css::ColorStop>,
        repeating: bool,
        border_radius: BorderRadius,
    },
    /// Draw a radial gradient.
    RadialGradient {
        rect: Rect,
        shape: rustkit_css::RadialShape,
        size: rustkit_css::RadialSize,
        center: (f32, f32),
        stops: Vec<rustkit_css::ColorStop>,
        repeating: bool,
        border_radius: BorderRadius,
    },
    /// Draw a conic gradient.
    ConicGradient {
        rect: Rect,
        from_angle: f32,
        center: (f32, f32),
        stops: Vec<rustkit_css::ColorStop>,
        repeating: bool,
        border_radius: BorderRadius,
    },
    /// Draw a text input field.
    TextInput {
        rect: Rect,
        value: String,
        placeholder: String,
        font_size: f32,
        text_color: Color,
        placeholder_color: Color,
        background_color: Color,
        border_color: Color,
        border_width: f32,
        focused: bool,
        caret_position: Option<usize>,
        /// The control's computed font — the renderer used to paint every
        /// control in a hardcoded sans-serif 400.
        font_family: String,
        font_weight: u16,
        /// Resolved author padding `[top, right, bottom, left]` (px, zero for
        /// a bare UA control). `rect` is the border box; the text line is
        /// seated inside border + padding, as Chrome's inner editor is.
        padding: [f32; 4],
    },
    /// Draw a button.
    Button {
        rect: Rect,
        label: String,
        font_size: f32,
        text_color: Color,
        background_color: Color,
        border_color: Color,
        border_width: f32,
        border_radius: f32,
        pressed: bool,
        focused: bool,
        /// See `TextInput::font_family` / `padding`.
        font_family: String,
        font_weight: u16,
        padding: [f32; 4],
    },
    /// Draw a focus ring around an element.
    FocusRing {
        rect: Rect,
        color: Color,
        width: f32,
        offset: f32,
    },
    /// Draw a text caret (cursor).
    Caret {
        x: f32,
        y: f32,
        height: f32,
        color: Color,
    },
    /// Push a clip rect (for overflow handling).
    PushClip(Rect),
    /// Push a clip rect whose corners are rounded.
    ///
    /// `overflow` other than `visible` clips descendants to the padding box,
    /// and when the box has a border radius that clip is round. Emitting
    /// `PushClip` here instead loses the corner notches: a child painting its
    /// own background fills the square corner the parent's radius cut away.
    PushClipRounded { rect: Rect, radius: BorderRadius },
    /// Solid borders around a border box with rounded corners. `rect` is
    /// the border box, `widths`/`colors` are `[top, right, bottom, left]`,
    /// and `radius` is the outer (border-edge) radius. The inner (padding)
    /// edge curves with `radius − width` per axis (CSS Backgrounds 3 §5.2).
    /// The side strips alone painted a square frame around a rounded
    /// background.
    RoundedBorder {
        rect: Rect,
        widths: [f32; 4],
        colors: [Color; 4],
        radius: BorderRadius,
    },
    /// Pop clip rect.
    PopClip,
    /// Start stacking context.
    PushStackingContext { z_index: i32, rect: Rect },
    /// End stacking context.
    PopStackingContext,
    /// Push a 2D transform matrix.
    /// The matrix is [a, b, c, d, e, f] representing:
    /// | a c e |
    /// | b d f |
    /// | 0 0 1 |
    /// Origin is the point around which the transform is applied.
    PushTransform {
        matrix: [f32; 6],
        origin: (f32, f32),
    },
    /// Pop a transform matrix.
    PopTransform,

    /// Draw text with a gradient fill (for background-clip: text effect).
    GradientText {
        text: String,
        x: f32,
        y: f32,
        font_size: f32,
        font_family: String,
        font_weight: u16,
        font_style: u8,
        gradient: rustkit_css::Gradient,
        rect: Rect,
        /// ADVANCE CONTRACT — same semantics as Text: layout's per-char
        /// advances and ascent; paint places, never re-measures.
        advances: Option<Vec<f32>>,
        ascent: Option<f32>,
    },

    // SVG-specific commands
    /// Fill a rectangle with solid color.
    FillRect { rect: Rect, color: Color },
    /// Stroke a rectangle.
    StrokeRect {
        rect: Rect,
        color: Color,
        width: f32,
    },
    /// Fill a circle.
    FillCircle {
        cx: f32,
        cy: f32,
        radius: f32,
        color: Color,
    },
    /// Stroke a circle.
    StrokeCircle {
        cx: f32,
        cy: f32,
        radius: f32,
        color: Color,
        width: f32,
    },
    /// Fill an ellipse.
    FillEllipse { rect: Rect, color: Color },
    /// Draw a line.
    Line {
        x1: f32,
        y1: f32,
        x2: f32,
        y2: f32,
        color: Color,
        width: f32,
    },
    /// Draw a polyline (connected line segments).
    Polyline {
        points: Vec<(f32, f32)>,
        color: Color,
        width: f32,
    },
    /// Fill a polygon.
    FillPolygon {
        points: Vec<(f32, f32)>,
        color: Color,
    },
    /// Stroke a polygon.
    StrokePolygon {
        points: Vec<(f32, f32)>,
        color: Color,
        width: f32,
    },
}

/// Text decoration style for display commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextDecorationStyleValue {
    Solid,
    Double,
    Dotted,
    Dashed,
    Wavy,
}

/// CSS object-fit values for images.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ObjectFit {
    /// Fill the box, possibly distorting the image
    #[default]
    Fill,
    /// Scale to fit inside the box, preserving aspect ratio
    Contain,
    /// Scale to cover the box, preserving aspect ratio
    Cover,
    /// Don't scale the image
    None,
    /// Like contain but never scale up
    ScaleDown,
}

impl ObjectFit {
    /// Parse from CSS value
    pub fn from_css(value: &str) -> Self {
        match value.trim().to_lowercase().as_str() {
            "fill" => ObjectFit::Fill,
            "contain" => ObjectFit::Contain,
            "cover" => ObjectFit::Cover,
            "none" => ObjectFit::None,
            "scale-down" => ObjectFit::ScaleDown,
            _ => ObjectFit::default(),
        }
    }

    /// Calculate the image rectangle within a container
    pub fn compute_rect(
        &self,
        container: Rect,
        image_width: f32,
        image_height: f32,
        position: (f32, f32),
    ) -> ImageDrawRect {
        if image_width == 0.0 || image_height == 0.0 {
            return ImageDrawRect {
                dest: container,
                src: None,
            };
        }

        let image_aspect = image_width / image_height;
        let container_aspect = container.width / container.height;

        let (draw_width, draw_height) = match self {
            ObjectFit::Fill => (container.width, container.height),

            ObjectFit::Contain => {
                if image_aspect > container_aspect {
                    (container.width, container.width / image_aspect)
                } else {
                    (container.height * image_aspect, container.height)
                }
            }

            ObjectFit::Cover => {
                if image_aspect > container_aspect {
                    (container.height * image_aspect, container.height)
                } else {
                    (container.width, container.width / image_aspect)
                }
            }

            ObjectFit::None => (image_width, image_height),

            ObjectFit::ScaleDown => {
                if image_width <= container.width && image_height <= container.height {
                    (image_width, image_height)
                } else if image_aspect > container_aspect {
                    (container.width, container.width / image_aspect)
                } else {
                    (container.height * image_aspect, container.height)
                }
            }
        };

        let x = container.x + (container.width - draw_width) * position.0;
        let y = container.y + (container.height - draw_height) * position.1;

        ImageDrawRect {
            dest: Rect {
                x,
                y,
                width: draw_width,
                height: draw_height,
            },
            src: None,
        }
    }
}

/// Result of computing image draw rectangle
#[derive(Debug, Clone)]
pub struct ImageDrawRect {
    /// Destination rectangle
    pub dest: Rect,
    /// Source rectangle (for cropping, e.g., in cover mode)
    pub src: Option<Rect>,
}

/// CSS background-size values.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum BackgroundSize {
    /// Stretch to fill
    Cover,
    /// Scale to fit
    Contain,
    /// Explicit size
    Explicit {
        width: Option<f32>,
        height: Option<f32>,
    },
    /// Auto sizing
    #[default]
    Auto,
}

impl BackgroundSize {
    /// Parse from CSS value
    pub fn from_css(value: &str) -> Self {
        match value.trim().to_lowercase().as_str() {
            "cover" => BackgroundSize::Cover,
            "contain" => BackgroundSize::Contain,
            "auto" => BackgroundSize::Auto,
            _ => {
                // Try to parse explicit size
                let parts: Vec<&str> = value.split_whitespace().collect();
                let width = parts.first().and_then(|s| parse_length(s));
                let height = parts.get(1).and_then(|s| parse_length(s));
                BackgroundSize::Explicit { width, height }
            }
        }
    }

    /// Calculate the background image size
    pub fn compute_size(&self, container: Rect, image_width: f32, image_height: f32) -> (f32, f32) {
        if image_width == 0.0 || image_height == 0.0 {
            return (0.0, 0.0);
        }

        let image_aspect = image_width / image_height;
        let container_aspect = container.width / container.height;

        match self {
            BackgroundSize::Cover => {
                if image_aspect > container_aspect {
                    (container.height * image_aspect, container.height)
                } else {
                    (container.width, container.width / image_aspect)
                }
            }

            BackgroundSize::Contain => {
                if image_aspect > container_aspect {
                    (container.width, container.width / image_aspect)
                } else {
                    (container.height * image_aspect, container.height)
                }
            }

            BackgroundSize::Auto => (image_width, image_height),

            BackgroundSize::Explicit { width, height } => match (width, height) {
                (Some(w), Some(h)) => (*w, *h),
                (Some(w), None) => (*w, *w / image_aspect),
                (None, Some(h)) => (*h * image_aspect, *h),
                (None, None) => (image_width, image_height),
            },
        }
    }
}

/// CSS background-repeat values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BackgroundRepeat {
    /// Repeat in both directions
    #[default]
    Repeat,
    /// Repeat horizontally only
    RepeatX,
    /// Repeat vertically only
    RepeatY,
    /// No repeat
    NoRepeat,
    /// Space evenly
    Space,
    /// Round to fill
    Round,
}

impl BackgroundRepeat {
    /// Parse from CSS value
    pub fn from_css(value: &str) -> Self {
        match value.trim().to_lowercase().as_str() {
            "repeat" => BackgroundRepeat::Repeat,
            "repeat-x" => BackgroundRepeat::RepeatX,
            "repeat-y" => BackgroundRepeat::RepeatY,
            "no-repeat" => BackgroundRepeat::NoRepeat,
            "space" => BackgroundRepeat::Space,
            "round" => BackgroundRepeat::Round,
            _ => BackgroundRepeat::default(),
        }
    }

    /// Check if repeating on x-axis
    pub fn repeats_x(&self) -> bool {
        matches!(
            self,
            BackgroundRepeat::Repeat
                | BackgroundRepeat::RepeatX
                | BackgroundRepeat::Space
                | BackgroundRepeat::Round
        )
    }

    /// Check if repeating on y-axis
    pub fn repeats_y(&self) -> bool {
        matches!(
            self,
            BackgroundRepeat::Repeat
                | BackgroundRepeat::RepeatY
                | BackgroundRepeat::Space
                | BackgroundRepeat::Round
        )
    }
}

/// Parse a CSS length value to pixels
fn parse_length(value: &str) -> Option<f32> {
    // Delegates to rustkit-css (duplication audit P0: this was a third,
    // px-only parse_length). Only px-resolvable values map to f32 here —
    // percent/em stay None exactly as before, but units and unitless zero
    // now parse consistently with the rest of the engine.
    match rustkit_css::parse_length(value)? {
        rustkit_css::Length::Px(v) => Some(v),
        rustkit_css::Length::Zero => Some(0.0),
        _ => None,
    }
}

/// A display list of paint commands.
#[derive(Debug, Clone)]
pub struct DisplayList {
    pub commands: Vec<DisplayCommand>,
    /// Root element font size for rem unit calculations
    root_font_size: f32,
    /// Tree depth of the box being rendered (root = 0). The root's overflow
    /// propagates to the viewport, so the root box itself never clips.
    depth: u32,
    /// Whether the ROOT box's overflow is `visible`. When it is, `body`'s
    /// overflow propagates to the viewport instead (CSS 2.1 §11.1.1 /
    /// css-overflow-3 §3.3) and body does not clip either.
    root_overflow_visible: bool,
    /// Overflow clips pushed by NON-positioned boxes since the nearest
    /// positioned ancestor, outermost first. `overflow` only clips descendants
    /// whose containing block is the clipping box or lies inside it; an
    /// absolutely positioned box whose containing block is ABOVE a static
    /// clipper is not clipped by it (css-overflow-3 §3.1 — the classic
    /// dropdown escaping an `overflow: hidden` wrapper). Such a box pops
    /// exactly these clips before painting and re-pushes them after.
    escapable_clips: Vec<(Rect, BorderRadius)>,
    /// `text-overflow: ellipsis` scope of the nearest block container being
    /// painted (css-overflow-3 §5.1). Inline boxes, anonymous blocks and
    /// text runs paint inside it; a nested block container replaces it with
    /// its own (or none) for the duration of its subtree.
    ellipsis: Option<EllipsisScope>,
}

/// One block container's `text-overflow: ellipsis` state while its subtree
/// paints.
#[derive(Debug, Clone)]
struct EllipsisScope {
    /// End edge of the block's CONTENT box — the line box edge the
    /// ellipsis sits against. (The clip is at the padding edge; the
    /// ellipsis is not: css-overflow-3 §5.1 places it "at the end of the
    /// line box".)
    limit_x: f32,
    /// Line boxes (keyed by top) that already carry an ellipsis. Runs paint
    /// in inline order, so every run reaching a cut line AFTER the cut is
    /// past the ellipsis and does not paint — judged by order, not by x:
    /// a nowrap run's laid-out width is clamped to its container, so the
    /// sibling after an overflowing `<span>` is placed at the clamped end,
    /// short of where its content really is (repro: `Inline <b>bold run
    /// that overflows</b> the box` painted " th…" over the span's
    /// ellipsis when x decided).
    lines: Vec<f32>,
}

/// What `text-overflow` does to one painted run.
#[derive(Debug, Clone, PartialEq)]
enum TextOverflowCut {
    /// Fits (or cannot be measured): paint as laid out.
    Keep,
    /// Starts at or past an ellipsis already painted on this line.
    Hide,
    /// Cut so that `…` fits inside the block's content edge.
    Cut {
        text: String,
        advances: Vec<f32>,
        width: f32,
    },
}

impl EllipsisScope {
    /// css-overflow-3 §5.1 for one run painted at (`x`, line top
    /// `line_top`) with per-char `advances`: a run ending past the block's
    /// content edge is cut so that `…` (advance `ellipsis_advance`) fits;
    /// if not even the ellipsis fits, the ellipsis alone is painted (the
    /// clip cuts it — the spec says clip the ellipsis, not skip it). A run
    /// that fits is kept; a run reaching a line that already carries an
    /// ellipsis is hidden.
    fn cut(
        &mut self,
        text: &str,
        x: f32,
        line_top: f32,
        advances: &[f32],
        ellipsis_advance: f32,
    ) -> TextOverflowCut {
        const EPS: f32 = 0.01;
        if self.lines.iter().any(|&top| (top - line_top).abs() < 0.5) {
            return TextOverflowCut::Hide;
        }
        let total: f32 = advances.iter().sum();
        if x + total <= self.limit_x + EPS {
            return TextOverflowCut::Keep;
        }
        let available = self.limit_x - x - ellipsis_advance;
        let mut kept = 0usize;
        let mut kept_width = 0.0f32;
        for &adv in advances {
            if kept_width + adv > available + EPS {
                break;
            }
            kept_width += adv;
            kept += 1;
        }
        let mut cut: String = text.chars().take(kept).collect();
        cut.push('\u{2026}');
        let mut cut_advances: Vec<f32> = advances[..kept].to_vec();
        cut_advances.push(ellipsis_advance);
        self.lines.push(line_top);
        TextOverflowCut::Cut {
            text: cut,
            advances: cut_advances,
            width: kept_width + ellipsis_advance,
        }
    }
}

impl Default for DisplayList {
    fn default() -> Self {
        Self::new()
    }
}

impl DisplayList {
    /// Create an empty display list.
    pub fn new() -> Self {
        Self {
            commands: Vec::new(),
            root_font_size: 16.0,
            depth: 0,
            root_overflow_visible: true,
            escapable_clips: Vec::new(),
            ellipsis: None,
        }
    }

    fn for_root(root: &LayoutBox) -> Self {
        // Extract root font size from root element
        let root_font_size = match root.style.font_size {
            Length::Px(px) => px,
            Length::Em(em) => em * 16.0, // Relative to browser default
            Length::Rem(rem) => rem * 16.0, // Relative to browser default
            _ => 16.0,
        };
        DisplayList {
            commands: Vec::new(),
            root_font_size,
            depth: 0,
            root_overflow_visible: root.style.overflow_x == rustkit_css::Overflow::Visible
                && root.style.overflow_y == rustkit_css::Overflow::Visible,
            escapable_clips: Vec::new(),
            ellipsis: None,
        }
    }

    /// Build display list from a layout box with proper stacking order.
    pub fn build(root: &LayoutBox) -> Self {
        let mut list = DisplayList::for_root(root);
        list.render_stacking_context(root, 0, &mut 0);
        list
    }

    /// Build display list from a layout box with scroll state applied.
    ///
    /// This updates sticky positions based on the scroll state before building
    /// the display list. Use this when the scroll position has changed and
    /// sticky elements need to be repositioned.
    pub fn build_with_scroll(
        root: &mut LayoutBox,
        scroll_x: f32,
        scroll_y: f32,
        viewport: Rect,
    ) -> Self {
        // Update sticky positions based on scroll
        root.update_sticky_positions(scroll_x, scroll_y, viewport);

        // Build the display list
        let mut list = DisplayList::for_root(root);
        list.render_stacking_context(root, 0, &mut 0);
        list
    }

    /// Render a stacking context with proper z-ordering.
    fn render_stacking_context(&mut self, layout_box: &LayoutBox, parent_z: i32, layer: &mut u32) {
        let z_index = if layout_box.position != Position::Static {
            layout_box.z_index
        } else {
            parent_z
        };

        // A positioned box is the containing block for the absolutely
        // positioned boxes inside it, so clips pushed by static boxes ABOVE it
        // are not escapable from within its subtree: scope them out for the
        // duration. An absolutely/fixed positioned box additionally escapes
        // those clips itself — its containing block is above them.
        //
        // "Positioned" is read from the COMPUTED STYLE, not `LayoutBox::position`:
        // the engine deliberately transfers `position: relative` as
        // `Position::Static` (its paint-side stacking path is not ready for
        // relative boxes), but a relative box is still the containing block of
        // its absolute children — image-gallery's `.gallery-item { position:
        // relative; overflow: hidden }` must keep its `.image-overlay` inside
        // the clip. `LayoutBox::position` alone would let the overlay escape.
        let positioned = layout_box.position != Position::Static
            || layout_box.style.position != rustkit_css::Position::Static;
        let escapes = matches!(layout_box.position, Position::Absolute | Position::Fixed);
        let outer_escapable = if positioned {
            let outer = std::mem::take(&mut self.escapable_clips);
            if escapes {
                for _ in &outer {
                    self.commands.push(DisplayCommand::PopClip);
                }
            }
            Some(outer)
        } else {
            None
        };

        // Check if this creates a new stacking context
        let creates_context = layout_box
            .stacking_context
            .as_ref()
            .map(|ctx| ctx.creates_context)
            .unwrap_or(false);

        if creates_context {
            self.commands.push(DisplayCommand::PushStackingContext {
                z_index,
                rect: layout_box.dimensions.border_box(),
            });
        }

        // Check if this box has a transform
        let has_transform = !layout_box.style.transform.is_identity();
        if has_transform {
            let border_box = layout_box.dimensions.border_box();
            // Compute transform matrix
            let matrix = layout_box
                .style
                .transform
                .to_matrix(border_box.width, border_box.height);
            // Compute origin in absolute coordinates
            let origin_x = border_box.x
                + layout_box
                    .style
                    .transform_origin
                    .x
                    .to_px(16.0, 16.0, border_box.width);
            let origin_y = border_box.y
                + layout_box
                    .style
                    .transform_origin
                    .y
                    .to_px(16.0, 16.0, border_box.height);
            self.commands.push(DisplayCommand::PushTransform {
                matrix,
                origin: (origin_x, origin_y),
            });
        }

        // Render this box
        self.render_box_content(layout_box);

        // A block container starts (or ends) the `text-overflow: ellipsis`
        // scope its line boxes paint under; restored after its subtree.
        let outer_ellipsis = self.enter_ellipsis_scope(layout_box);

        // A box that clips its overflow clips its DESCENDANTS to its padding
        // box (rounded when it has a radius). Pushed after the box's own
        // content — the background rounds itself via BorderRadius on its own
        // commands and must not be clipped twice — and popped after every
        // child. Positioned children are inside the clip too, EXCEPT an
        // absolutely positioned descendant whose containing block is above a
        // static clipper: that one escapes via `escapable_clips` when it is
        // rendered (see the top of this function).
        let overflow_clip = self.overflow_clip(layout_box);
        if let Some((rect, radius)) = overflow_clip {
            if radius.is_zero() {
                self.commands.push(DisplayCommand::PushClip(rect));
            } else {
                self.commands
                    .push(DisplayCommand::PushClipRounded { rect, radius });
            }
            if !positioned {
                self.escapable_clips.push((rect, radius));
            }
        }
        self.depth += 1;

        // Collect children grouped by paint order
        let mut negative_z: Vec<(&LayoutBox, u32)> = Vec::new();
        let mut normal_flow: Vec<(&LayoutBox, u32)> = Vec::new();
        let mut positive_z: Vec<(&LayoutBox, u32)> = Vec::new();

        for child in &layout_box.children {
            *layer += 1;
            let child_layer = *layer;

            if child.position != Position::Static {
                if child.z_index < 0 {
                    negative_z.push((child, child_layer));
                } else {
                    positive_z.push((child, child_layer));
                }
            } else if child.float != Float::None {
                // Floats paint between normal flow and positioned
                positive_z.push((child, child_layer));
            } else {
                normal_flow.push((child, child_layer));
            }
        }

        // Sort by z-index, then by layer for stability
        negative_z.sort_by(|a, b| {
            let z_cmp = a.0.z_index.cmp(&b.0.z_index);
            if z_cmp == Ordering::Equal {
                a.1.cmp(&b.1)
            } else {
                z_cmp
            }
        });
        positive_z.sort_by(|a, b| {
            let z_cmp = a.0.z_index.cmp(&b.0.z_index);
            if z_cmp == Ordering::Equal {
                a.1.cmp(&b.1)
            } else {
                z_cmp
            }
        });

        // Render in correct order:
        // 1. Negative z-index positioned descendants
        for (child, _) in negative_z {
            self.render_stacking_context(child, z_index, layer);
        }

        // 2. Normal flow block children
        for (child, _) in &normal_flow {
            self.render_stacking_context(child, z_index, layer);
        }

        // 3. Floats and positive/zero z-index positioned descendants
        for (child, _) in positive_z {
            self.render_stacking_context(child, z_index, layer);
        }

        self.depth -= 1;
        if overflow_clip.is_some() {
            self.commands.push(DisplayCommand::PopClip);
            if !positioned {
                self.escapable_clips.pop();
            }
        }
        if let Some(outer) = outer_ellipsis {
            self.ellipsis = outer;
        }

        // Pop transform if we pushed one
        if has_transform {
            self.commands.push(DisplayCommand::PopTransform);
        }

        if creates_context {
            self.commands.push(DisplayCommand::PopStackingContext);
        }

        // Restore the clips this box's subtree was scoped out of; an escaping
        // box re-pushes the ones it popped so its later siblings are clipped
        // exactly as before.
        if let Some(outer) = outer_escapable {
            if escapes {
                for &(rect, radius) in &outer {
                    if radius.is_zero() {
                        self.commands.push(DisplayCommand::PushClip(rect));
                    } else {
                        self.commands
                            .push(DisplayCommand::PushClipRounded { rect, radius });
                    }
                }
            }
            self.escapable_clips = outer;
        }
    }

    /// css-overflow-3 §5.1: `text-overflow` applies to block containers
    /// whose `overflow` is other than `visible`, and governs THEIR line
    /// boxes only. Entering a block container therefore replaces the
    /// current scope — with an ellipsis scope at the block's content edge,
    /// or with none — and returns the outer scope for the caller to
    /// restore. Inline-level and anonymous boxes return `None`: they paint
    /// inside the nearest block container's scope.
    fn enter_ellipsis_scope(&mut self, layout_box: &LayoutBox) -> Option<Option<EllipsisScope>> {
        if !matches!(layout_box.box_type, BoxType::Block) {
            return None;
        }
        let s = &layout_box.style;
        let clips = s.overflow_x != rustkit_css::Overflow::Visible
            || s.overflow_y != rustkit_css::Overflow::Visible;
        let scope = if clips && s.text_overflow == rustkit_css::TextOverflow::Ellipsis {
            let c = &layout_box.dimensions.content;
            Some(EllipsisScope {
                limit_x: c.x + c.width,
                lines: Vec::new(),
            })
        } else {
            None
        };
        Some(std::mem::replace(&mut self.ellipsis, scope))
    }

    /// Render a layout box's own content (shadows, background, borders, text, images).
    fn render_box_content(&mut self, layout_box: &LayoutBox) {
        // A text run is not an element (CSS 2.1 §14.2: backgrounds, borders
        // and shadows belong to elements), so it paints glyphs only. Its
        // style can still carry box decorations: the engine copies
        // `background_gradient` onto every text child for gradient text,
        // and a pseudo-element's text child clones the whole pseudo style.
        // Painting them repainted the parent's gradient, rescaled to the
        // run's own rect (image-gallery: a gradient tile behind every emoji).
        if matches!(layout_box.box_type, BoxType::Text(_)) {
            self.render_text(layout_box);
            return;
        }
        // Box shadows (outer) are drawn first, behind the element
        self.render_box_shadows(layout_box);
        // Then background
        self.render_background(layout_box);
        // Then inset shadows (on top of background, inside the box)
        self.render_inset_shadows(layout_box);
        // Then borders
        self.render_borders(layout_box);
        // Then text
        self.render_text(layout_box);
        // Then images (replaced content)
        self.render_replaced_content(layout_box);
    }

    /// Render a layout box and its children (legacy method).
    #[allow(dead_code)]
    fn render_box(&mut self, layout_box: &LayoutBox) {
        // Box shadows (outer) are drawn first, behind the element
        self.render_box_shadows(layout_box);
        // Then background
        self.render_background(layout_box);
        // Then inset shadows (on top of background, inside the box)
        self.render_inset_shadows(layout_box);
        // Then borders
        self.render_borders(layout_box);
        // Then text
        self.render_text(layout_box);
        // Then images (replaced content)
        self.render_replaced_content(layout_box);

        for child in &layout_box.children {
            self.render_box(child);
        }
    }

    /// Render box shadows (must be called before background).
    fn render_box_shadows(&mut self, layout_box: &LayoutBox) {
        let box_rect = layout_box.dimensions.border_box();

        // Render outer shadows first (in order, first shadow is top-most)
        for shadow in &layout_box.style.box_shadows {
            if shadow.is_visible() && !shadow.inset {
                self.commands.push(DisplayCommand::BoxShadow {
                    offset_x: shadow.offset_x,
                    offset_y: shadow.offset_y,
                    blur_radius: shadow.blur_radius,
                    spread_radius: shadow.spread_radius,
                    color: shadow.color,
                    rect: box_rect,
                    inset: false,
                });
            }
        }
    }

    /// Render inset box shadows (called after background).
    fn render_inset_shadows(&mut self, layout_box: &LayoutBox) {
        let box_rect = layout_box.dimensions.border_box();

        for shadow in &layout_box.style.box_shadows {
            if shadow.is_visible() && shadow.inset {
                self.commands.push(DisplayCommand::BoxShadow {
                    offset_x: shadow.offset_x,
                    offset_y: shadow.offset_y,
                    blur_radius: shadow.blur_radius,
                    spread_radius: shadow.spread_radius,
                    color: shadow.color,
                    rect: box_rect,
                    inset: true,
                });
            }
        }
    }

    /// Render background.
    /// Supports multiple background layers painted bottom-to-top.
    /// Respects background-clip property (border-box, padding-box, content-box).
    /// The box's border-box corner radii, resolved to pixels.
    ///
    /// Percentages resolve against the border box WIDTH for every corner. That
    /// is not what CSS says (the vertical radius resolves against height), but
    /// it is what the background painter has always done, and the overflow clip
    /// has to round exactly where the background rounds or the two disagree by
    /// a pixel and the clip cuts into the paint it is supposed to contain.
    fn border_radius_px(&self, layout_box: &LayoutBox) -> BorderRadius {
        let s = &layout_box.style;
        let border_rect = layout_box.dimensions.border_box();
        let font_size = match s.font_size {
            Length::Px(px) => px,
            _ => 16.0,
        };
        let root_font_size = self.root_font_size;
        BorderRadius {
            top_left: s
                .border_top_left_radius
                .to_px(font_size, root_font_size, border_rect.width),
            top_right: s.border_top_right_radius.to_px(
                font_size,
                root_font_size,
                border_rect.width,
            ),
            bottom_right: s.border_bottom_right_radius.to_px(
                font_size,
                root_font_size,
                border_rect.width,
            ),
            bottom_left: s.border_bottom_left_radius.to_px(
                font_size,
                root_font_size,
                border_rect.width,
            ),
        }
    }

    /// The rounded clip a box imposes on its descendants, if any.
    ///
    /// The clip a box imposes on its descendants: `None` unless `overflow`
    /// is anything but `visible` on either axis (a single non-visible axis
    /// computes the other to `auto`, css-overflow-3 §3.1, so both clip). The
    /// rect is the padding box; the radius is zero for square corners.
    ///
    /// The root box never clips — its overflow propagates to the viewport —
    /// and neither does `body` while the root's overflow is `visible`, because
    /// then body's is what propagates (CSS 2.1 §11.1.1). Clipping either to its
    /// own padding box would cut the page at the first screen of content.
    fn overflow_clip(&self, layout_box: &LayoutBox) -> Option<(Rect, BorderRadius)> {
        let s = &layout_box.style;
        let clips = s.overflow_x != rustkit_css::Overflow::Visible
            || s.overflow_y != rustkit_css::Overflow::Visible;
        if !clips {
            return None;
        }

        if self.depth == 0 {
            return None;
        }
        let is_body = layout_box
            .identity
            .as_ref()
            .map(|id| id.tag == "body")
            .unwrap_or(false);
        if is_body && self.root_overflow_visible {
            return None;
        }

        let radius = self.border_radius_px(layout_box);

        // Overflow clips to the PADDING box, and the clip's radius is the
        // border radius shrunk inward by the border it sits inside.
        let d = &layout_box.dimensions;
        let border_rect = d.border_box();
        let padding_rect = Rect::new(
            border_rect.x + d.border.left,
            border_rect.y + d.border.top,
            border_rect.width - d.border.left - d.border.right,
            border_rect.height - d.border.top - d.border.bottom,
        );
        if padding_rect.width <= 0.0 || padding_rect.height <= 0.0 {
            return None;
        }

        // Each corner shrinks by the THICKER of its two borders. BorderRadius
        // is one scalar per corner, so an elliptical inner radius cannot be
        // expressed; taking the thicker border rounds less than Chrome would,
        // which errs toward clipping too little rather than eating paint that
        // belongs on screen. With no border — every case this currently fires
        // on — it is exact.
        let inset = |r: f32, a: f32, b: f32| (r - a.max(b)).max(0.0);
        let inner = BorderRadius {
            top_left: inset(radius.top_left, d.border.left, d.border.top),
            top_right: inset(radius.top_right, d.border.right, d.border.top),
            bottom_right: inset(radius.bottom_right, d.border.right, d.border.bottom),
            bottom_left: inset(radius.bottom_left, d.border.left, d.border.bottom),
        };

        Some((padding_rect, inner))
    }

    fn render_background(&mut self, layout_box: &LayoutBox) {
        let d = &layout_box.dimensions;
        let border_rect = d.border_box();
        let s = &layout_box.style;

        // Get font size for relative length calculations
        let font_size = match s.font_size {
            Length::Px(px) => px,
            _ => 16.0,
        };
        // Use actual root font size for rem unit calculations
        let root_font_size = self.root_font_size;

        // Calculate border radius once (used for both solid color and gradient clipping)
        let radius = self.border_radius_px(layout_box);

        // Calculate the clipped rect based on background-clip property
        let clip_rect = match s.background_clip {
            rustkit_css::BackgroundClip::BorderBox => border_rect,
            rustkit_css::BackgroundClip::PaddingBox => {
                // Clip to padding box (inside borders)
                Rect::new(
                    border_rect.x + d.border.left,
                    border_rect.y + d.border.top,
                    border_rect.width - d.border.left - d.border.right,
                    border_rect.height - d.border.top - d.border.bottom,
                )
            }
            rustkit_css::BackgroundClip::ContentBox => {
                // Clip to content box (inside padding)
                Rect::new(
                    border_rect.x + d.border.left + d.padding.left,
                    border_rect.y + d.border.top + d.padding.top,
                    border_rect.width
                        - d.border.left
                        - d.border.right
                        - d.padding.left
                        - d.padding.right,
                    border_rect.height
                        - d.border.top
                        - d.border.bottom
                        - d.padding.top
                        - d.padding.bottom,
                )
            }
            rustkit_css::BackgroundClip::Text => {
                // background-clip:text — the background shows ONLY through
                // glyph shapes (render_text emits GradientText for that).
                // Falling through here painted the full-box background as a
                // slab behind the text (about's hero: a giant cyan bar where
                // Chrome shows thin gradient letterforms).
                return;
            }
        };

        // Only proceed if clip_rect has positive dimensions
        if clip_rect.width <= 0.0 || clip_rect.height <= 0.0 {
            return;
        }

        // Apply clipping if not border-box (the default)
        let needs_clip = !matches!(s.background_clip, rustkit_css::BackgroundClip::BorderBox);
        if needs_clip {
            self.commands.push(DisplayCommand::PushClip(clip_rect));
        }

        // Use clip_rect for painting (not border_rect) to ensure proper bounds
        let paint_rect = clip_rect;

        // Step 0: Apply backdrop-filter if set (must be done BEFORE any background painting)
        // Backdrop filter applies to pixels that are already rendered behind this element
        if !s.backdrop_filter.is_none() {
            self.commands.push(DisplayCommand::BackdropFilter {
                rect: paint_rect,
                border_radius: radius,
                filter: s.backdrop_filter,
            });
        }

        // Step 1: Paint solid background color FIRST (bottom layer)
        // This must be painted even if there's a gradient, as the gradient may be semi-transparent
        let color = s.background_color;
        if color.a > 0.0 {
            if radius.is_zero() || needs_clip {
                // When clipping, use solid rect within the clipped area
                self.commands
                    .push(DisplayCommand::SolidColor(color, paint_rect));
            } else {
                self.commands.push(DisplayCommand::RoundedRect {
                    color,
                    rect: paint_rect,
                    radius,
                });
            }
        }

        // Step 2: Paint background layers (bottom-to-top, index 0 is bottommost)
        // This is the new multi-layer background support
        if !s.background_layers.is_empty() {
            for layer in &s.background_layers {
                self.render_background_layer(layer, paint_rect, font_size, root_font_size, radius);
            }
        } else if let Some(gradient) = &s.background_gradient {
            // Fallback to legacy single gradient for backwards compatibility
            self.render_gradient(gradient, paint_rect, radius);
        }

        // Pop the clip if we pushed one
        if needs_clip {
            self.commands.push(DisplayCommand::PopClip);
        }
    }

    /// Render a single background layer.
    fn render_background_layer(
        &mut self,
        layer: &rustkit_css::BackgroundLayer,
        container: Rect,
        _font_size: f32,
        _root_font_size: f32,
        border_radius: BorderRadius,
    ) {
        match &layer.image {
            rustkit_css::BackgroundImage::None => {
                // No image, nothing to render
            }
            rustkit_css::BackgroundImage::Gradient(gradient) => {
                // Calculate the positioned rect for this gradient based on size/position
                let positioned_rect = self.calculate_background_rect(
                    container,
                    &layer.size,
                    &layer.position,
                    container.width, // For gradients, use container size as "intrinsic" size
                    container.height,
                );

                // The background painting area is the CONTAINER, always.
                // `background-size: 400% 400%` scales the gradient IMAGE, not
                // where it may paint — Chrome shows a 4x-zoomed slice inside
                // the box. Without this clip the oversized rect painted in
                // full: square-cornered, covering the neighbouring grid cards
                // (the gradient-backgrounds "-45deg Rainbow" card, 14.44% of
                // that parity case). The old comment claimed the viewport
                // would clip it; the viewport clips at the VIEWPORT, which is
                // exactly how far the bleed reached. Tile edges in the repeat
                // arms had the same leak — tiles were intersect-tested against
                // the container but each intersecting tile painted full-size.
                // One clip closes every path.
                //
                // The clip carries the box's radius. A plain `PushClip` cuts
                // the oversized rect to a SQUARE container, so a scaled
                // gradient under `border-radius` paints its own fill into the
                // corner notches — the part of the border box the arc cuts
                // away, where Chrome shows what is behind. That is P1's named
                // residual, and Gate B reports it as `missing_clip`:
                // gradient-backgrounds' `.linear-6` (`background-size: 400%
                // 400%`, `border-radius: 16px`) auto-failed on three corners,
                // 36 notch px, on the macOS board of 2026-09-08 — the first
                // board on which the element's geometry was exact enough for
                // the discrete detector to be allowed to speak about it.
                //
                // The unscaled path needs no clip and keeps its rounding via
                // `border_radius` on the gradient rect itself; this is the
                // same radius, applied to the container the scaled rect is
                // being cut to.
                //
                // Limit, stated: `border_radius` is the border-box radius. On
                // a `background-clip` of padding-box or content-box the
                // container is inset and the exact answer is the inner
                // radius, which nothing on this path computes — the solid
                // fill above drops the radius entirely in that case. This
                // change makes the scaled gradient agree with the unscaled
                // one; it does not close the inner-radius gap.
                let needs_clip = positioned_rect.x < container.x
                    || positioned_rect.y < container.y
                    || positioned_rect.x + positioned_rect.width > container.x + container.width
                    || positioned_rect.y + positioned_rect.height > container.y + container.height;
                if needs_clip {
                    if border_radius.is_zero() {
                        self.commands.push(DisplayCommand::PushClip(container));
                    } else {
                        self.commands.push(DisplayCommand::PushClipRounded {
                            rect: container,
                            radius: border_radius,
                        });
                    }
                }

                // Handle background-repeat for gradients
                match layer.repeat {
                    rustkit_css::BackgroundRepeat::NoRepeat => {
                        // Render gradient at its positioned rect
                        // Note: Large gradients extending outside container bounds will be
                        // clipped by the viewport/wgpu. The gradient t-values are calculated
                        // relative to the positioned_rect which preserves correct color mapping.
                        self.render_gradient(gradient, positioned_rect, border_radius);
                    }
                    rustkit_css::BackgroundRepeat::Repeat
                    | rustkit_css::BackgroundRepeat::RepeatX
                    | rustkit_css::BackgroundRepeat::RepeatY => {
                        // Tile the gradient
                        let tile_width = positioned_rect.width.max(1.0);
                        let tile_height = positioned_rect.height.max(1.0);

                        // If tile is larger than container in both dimensions, just render once
                        if tile_width >= container.width && tile_height >= container.height {
                            self.render_gradient(gradient, positioned_rect, border_radius);
                        } else {
                            let repeat_x =
                                !matches!(layer.repeat, rustkit_css::BackgroundRepeat::RepeatY);
                            let repeat_y =
                                !matches!(layer.repeat, rustkit_css::BackgroundRepeat::RepeatX);

                            // Calculate starting tile position
                            // The first tile's origin aligns with positioned_rect's origin
                            // We need to find which tiles intersect the container
                            let start_x = if repeat_x && tile_width < container.width {
                                // Find the leftmost tile that intersects the container
                                let offset =
                                    (positioned_rect.x - container.x).rem_euclid(tile_width);
                                container.x - offset
                            } else {
                                positioned_rect.x
                            };

                            let start_y = if repeat_y && tile_height < container.height {
                                let offset =
                                    (positioned_rect.y - container.y).rem_euclid(tile_height);
                                container.y - offset
                            } else {
                                positioned_rect.y
                            };

                            // Calculate how many tiles we need
                            let end_x = container.x + container.width;
                            let end_y = container.y + container.height;

                            let cols = if repeat_x && tile_width < container.width {
                                ((end_x - start_x) / tile_width).ceil() as i32
                            } else {
                                1
                            };
                            let rows = if repeat_y && tile_height < container.height {
                                ((end_y - start_y) / tile_height).ceil() as i32
                            } else {
                                1
                            };

                            // Limit tiles to prevent performance issues
                            let max_tiles_per_dim = 50;
                            let cols = cols.min(max_tiles_per_dim).max(1);
                            let rows = rows.min(max_tiles_per_dim).max(1);

                            for row in 0..rows {
                                for col in 0..cols {
                                    let tile_x = start_x + col as f32 * tile_width;
                                    let tile_y = start_y + row as f32 * tile_height;

                                    // Only render tiles that intersect the container
                                    let tile_end_x = tile_x + tile_width;
                                    let tile_end_y = tile_y + tile_height;

                                    if tile_end_x > container.x
                                        && tile_x < end_x
                                        && tile_end_y > container.y
                                        && tile_y < end_y
                                    {
                                        let tile_rect =
                                            Rect::new(tile_x, tile_y, tile_width, tile_height);
                                        self.render_gradient(gradient, tile_rect, border_radius);
                                    }
                                }
                            }
                        }
                    }
                    rustkit_css::BackgroundRepeat::Space | rustkit_css::BackgroundRepeat::Round => {
                        // For now, treat Space and Round like Repeat
                        // TODO: Implement proper spacing/scaling
                        self.render_gradient(gradient, positioned_rect, border_radius);
                    }
                }

                if needs_clip {
                    self.commands.push(DisplayCommand::PopClip);
                }
            }
            rustkit_css::BackgroundImage::Url(url) => {
                // For URL backgrounds, emit a BackgroundImage command
                // The actual image dimensions would come from the image cache
                // For now, use container size as fallback
                let size = self.convert_background_size(&layer.size);
                let position = self.convert_background_position(&layer.position);
                let repeat = self.convert_background_repeat(layer.repeat);

                self.commands.push(DisplayCommand::BackgroundImage {
                    url: url.clone(),
                    rect: container,
                    size,
                    position,
                    repeat,
                });
            }
        }
    }

    /// Calculate the rect for a background image/gradient based on size and position.
    fn calculate_background_rect(
        &self,
        container: Rect,
        size: &rustkit_css::BackgroundSize,
        position: &rustkit_css::BackgroundPosition,
        intrinsic_width: f32,
        intrinsic_height: f32,
    ) -> Rect {
        // Calculate the background size
        let (bg_width, bg_height) = match size {
            rustkit_css::BackgroundSize::Auto => (intrinsic_width, intrinsic_height),
            rustkit_css::BackgroundSize::Cover => {
                let scale_x = container.width / intrinsic_width;
                let scale_y = container.height / intrinsic_height;
                let scale = scale_x.max(scale_y);
                (intrinsic_width * scale, intrinsic_height * scale)
            }
            rustkit_css::BackgroundSize::Contain => {
                let scale_x = container.width / intrinsic_width;
                let scale_y = container.height / intrinsic_height;
                let scale = scale_x.min(scale_y);
                (intrinsic_width * scale, intrinsic_height * scale)
            }
            rustkit_css::BackgroundSize::Explicit { width, height } => {
                let w = width
                    .map(|v| {
                        if v < 0.0 {
                            container.width * (-v / 100.0)
                        } else {
                            v
                        }
                    })
                    .unwrap_or(intrinsic_width);
                let h = height
                    .map(|v| {
                        if v < 0.0 {
                            container.height * (-v / 100.0)
                        } else {
                            v
                        }
                    })
                    .unwrap_or(intrinsic_height);
                (w, h)
            }
        };

        // Calculate position
        let x = container.x + position.x.to_px(container.width, bg_width);
        let y = container.y + position.y.to_px(container.height, bg_height);

        Rect::new(x, y, bg_width, bg_height)
    }

    /// Render a gradient to a rect with optional border-radius clipping.
    fn render_gradient(
        &mut self,
        gradient: &rustkit_css::Gradient,
        rect: Rect,
        border_radius: BorderRadius,
    ) {
        match gradient {
            rustkit_css::Gradient::Linear(linear) => {
                self.commands.push(DisplayCommand::LinearGradient {
                    rect,
                    direction: linear.direction,
                    stops: linear.stops.clone(),
                    repeating: linear.repeating,
                    border_radius,
                });
            }
            rustkit_css::Gradient::Radial(radial) => {
                self.commands.push(DisplayCommand::RadialGradient {
                    rect,
                    shape: radial.shape,
                    size: radial.size,
                    center: radial.center,
                    stops: radial.stops.clone(),
                    repeating: radial.repeating,
                    border_radius,
                });
            }
            rustkit_css::Gradient::Conic(conic) => {
                self.commands.push(DisplayCommand::ConicGradient {
                    rect,
                    from_angle: conic.from_angle,
                    center: conic.center,
                    stops: conic.stops.clone(),
                    repeating: conic.repeating,
                    border_radius,
                });
            }
        }
    }

    /// Convert rustkit_css::BackgroundSize to layout BackgroundSize.
    fn convert_background_size(&self, size: &rustkit_css::BackgroundSize) -> BackgroundSize {
        match size {
            rustkit_css::BackgroundSize::Auto => BackgroundSize::Auto,
            rustkit_css::BackgroundSize::Cover => BackgroundSize::Cover,
            rustkit_css::BackgroundSize::Contain => BackgroundSize::Contain,
            rustkit_css::BackgroundSize::Explicit { width, height } => BackgroundSize::Explicit {
                width: *width,
                height: *height,
            },
        }
    }

    /// Convert rustkit_css::BackgroundPosition to (f32, f32) tuple.
    fn convert_background_position(&self, pos: &rustkit_css::BackgroundPosition) -> (f32, f32) {
        let x = match &pos.x {
            rustkit_css::BackgroundPositionValue::Percent(p) => *p,
            rustkit_css::BackgroundPositionValue::Px(_) => 0.0, // Will be handled in rendering
        };
        let y = match &pos.y {
            rustkit_css::BackgroundPositionValue::Percent(p) => *p,
            rustkit_css::BackgroundPositionValue::Px(_) => 0.0,
        };
        (x, y)
    }

    /// Convert rustkit_css::BackgroundRepeat to layout BackgroundRepeat.
    fn convert_background_repeat(&self, repeat: rustkit_css::BackgroundRepeat) -> BackgroundRepeat {
        match repeat {
            rustkit_css::BackgroundRepeat::Repeat => BackgroundRepeat::Repeat,
            rustkit_css::BackgroundRepeat::RepeatX => BackgroundRepeat::RepeatX,
            rustkit_css::BackgroundRepeat::RepeatY => BackgroundRepeat::RepeatY,
            rustkit_css::BackgroundRepeat::NoRepeat => BackgroundRepeat::NoRepeat,
            rustkit_css::BackgroundRepeat::Space => BackgroundRepeat::Space,
            rustkit_css::BackgroundRepeat::Round => BackgroundRepeat::Round,
        }
    }

    /// Render borders.
    fn render_borders(&mut self, layout_box: &LayoutBox) {
        let d = &layout_box.dimensions;
        let s = &layout_box.style;
        let bb = d.border_box();

        // Rounded corners: one command, so the renderer can curve both
        // edges of the ring. The strips below painted a square frame around
        // a rounded background (rounded-corners §5). Solid rings only: the
        // ring command has no dash pattern, so a rounded dashed/dotted border
        // keeps the per-side path below (square corners, correct dashes).
        let radius = self.border_radius_px(layout_box);
        let widths = [d.border.top, d.border.right, d.border.bottom, d.border.left];
        let all_solid = [
            s.border_top_style,
            s.border_right_style,
            s.border_bottom_style,
            s.border_left_style,
        ]
        .iter()
        .all(|st| matches!(st, rustkit_css::BorderStyle::Solid | rustkit_css::BorderStyle::None));
        if !radius.is_zero() && all_solid && widths.iter().any(|w| *w > 0.0) {
            self.commands.push(DisplayCommand::RoundedBorder {
                rect: d.border_box(),
                widths,
                colors: [
                    s.border_top_color,
                    s.border_right_color,
                    s.border_bottom_color,
                    s.border_left_color,
                ],
                radius,
            });
            return;
        }

        // Render each border side separately for correct colors. A side is
        // a strip along the whole border-box edge (corners overlap, as the
        // solid path always has); dashed/dotted sides are split into dashes
        // along it (see border_dash_pattern).
        let sides = [
            // (thickness, color, style, strip rect, horizontal)
            (
                d.border.top,
                s.border_top_color,
                s.border_top_style,
                Rect::new(bb.x, bb.y, bb.width, d.border.top),
                true,
            ),
            (
                d.border.right,
                s.border_right_color,
                s.border_right_style,
                Rect::new(bb.right() - d.border.right, bb.y, d.border.right, bb.height),
                false,
            ),
            (
                d.border.bottom,
                s.border_bottom_color,
                s.border_bottom_style,
                Rect::new(bb.x, bb.bottom() - d.border.bottom, bb.width, d.border.bottom),
                true,
            ),
            (
                d.border.left,
                s.border_left_color,
                s.border_left_style,
                Rect::new(bb.x, bb.y, d.border.left, bb.height),
                false,
            ),
        ];
        for (thickness, color, style, strip, horizontal) in sides {
            if thickness <= 0.0 {
                continue;
            }
            let length = if horizontal { strip.width } else { strip.height };
            let Some((dash, gap)) = border_dash_pattern(style, thickness, length) else {
                self.commands.push(DisplayCommand::SolidColor(color, strip));
                continue;
            };
            let mut offset = 0.0;
            while offset < length - 0.01 {
                let run = dash.min(length - offset);
                let rect = if horizontal {
                    Rect::new(strip.x + offset, strip.y, run, strip.height)
                } else {
                    Rect::new(strip.x, strip.y + offset, strip.width, run)
                };
                self.commands.push(DisplayCommand::SolidColor(color, rect));
                offset += dash + gap;
            }
        }
    }

    /// Render text with decorations.
    fn render_text(&mut self, layout_box: &LayoutBox) {
        if let BoxType::Text(ref raw_text) = layout_box.box_type {
            let style = &layout_box.style;

            // Apply text-transform (uppercase, lowercase, capitalize)
            let text = apply_text_transform(raw_text, style.text_transform);

            let font_size = match &style.font_size {
                Length::Px(px) => *px,
                _ => 16.0,
            };

            let x = layout_box.dimensions.content.x;
            let content_y = layout_box.dimensions.content.y;
            let text_width = layout_box.dimensions.content.width;

            // Calculate half-leading for proper baseline alignment
            // CSS line-height creates extra space above and below the text content
            // The half-leading is split evenly above and below the text
            // (must resolve `normal` exactly as layout did, or the baseline moves)

            // Get font metrics for accurate baseline calculation
            let metrics = measure_text_advanced(
                &text,
                &style.font_family,
                font_size,
                style.font_weight,
                style.font_style,
            );
            let line_height = run_line_height(style, font_size, &metrics);

            // The face the baseline is seated on: united under `normal`,
            // the primary face under an explicit line-height (see
            // `seat_metrics`). Offset from a line's top to the y the text
            // command carries: paint seats the baseline at round(y +
            // seat_ascent), so this is the Blink seat (signed, floored
            // leading; see blink_baseline_offset) minus the fractional ascent.
            let (seat_ascent, seat_descent) = seat_metrics(style, font_size, &text, &metrics);
            let half_leading =
                blink_baseline_offset(line_height, seat_ascent, seat_descent) - seat_ascent;

            // Build the list of lines to emit: wrapped text boxes carry
            // per-line fragments (see LayoutBox::text_lines); single-run
            // boxes emit exactly one line, positioned as before.
            // Tuple: (text, x, y, width, line_top, justify_space) — shadowing
            // the outer names inside the loop keeps the emission code
            // identical for both cases. `line_top` is the line box's top (y
            // minus the half-leading), the key runs on one line share
            // regardless of their own font's leading. `justify_space` is the
            // per-word-separator expansion of a justified line (0 otherwise);
            // the line's width already includes it.
            let render_lines: Vec<(String, f32, f32, f32, f32, f32)> = match &layout_box.text_lines {
                Some(lines) => lines
                    .iter()
                    .enumerate()
                    .filter(|(_, l)| !l.text.is_empty())
                    .map(|(i, l)| {
                        let top = content_y + i as f32 * line_height;
                        let expansion = l.justify_space
                            * TextLine::justification_opportunities(l.text.trim_end()) as f32;
                        (
                            apply_text_transform(&l.text, style.text_transform),
                            x + l.x_offset,
                            top + half_leading,
                            l.width + expansion,
                            top,
                            l.justify_space,
                        )
                    })
                    .collect(),
                None => vec![(
                    text.clone(),
                    x,
                    content_y + half_leading,
                    text_width,
                    content_y,
                    0.0,
                )],
            };

            // PAINT-0 seating probe (RUSTKIT_PAINT_PROBE=1): log the layout
            // half of the glyph seating chain so flat-1.2 vs metrics-normal
            // builds can be diffed line-by-line (forensics 2026-07-16 §4.2).
            if paint0_probe() {
                for (t, _lx, ly, _lw, _top, _js) in &render_lines {
                    eprintln!(
                        "PAINT0 layout text={:?} fs={} lh={} asc={} desc={} half={} content_y={} y_cmd={}",
                        t.chars().take(16).collect::<String>(),
                        font_size,
                        line_height,
                        seat_ascent,
                        seat_descent,
                        half_leading,
                        content_y,
                        ly
                    );
                }
            }

            for (text, x, y, text_width, line_top, justify_space) in render_lines {
                // ADVANCE CONTRACT: ONE shape call feeds BOTH command types —
                // layout's per-char advances and ascent ride the command so
                // paint places glyphs exactly where layout measured them.
                // (GradientText was skipped by the old continue-before-shape
                // and re-owned pitch + baseline in paint — the last dual
                // text path.)
                let mut advances = shape_line_advances(&text, style, font_size);
                // A justified line widens each word separator by the slack
                // layout distributed (TextLine::justify_space). Only the
                // per-char advance path can carry it: when shaping fell back
                // (ligature clusters) the line paints at natural spacing.
                if justify_space > 0.0 {
                    if let Some(adv) = advances.as_mut() {
                        for (a, c) in adv.iter_mut().zip(text.chars()) {
                            if TextLine::is_word_separator(c) {
                                *a += justify_space;
                            }
                        }
                    }
                }

                // `text-overflow: ellipsis` on the enclosing block container
                // (css-overflow-3 §5.1): cut the run at the block's content
                // edge and paint `…` in the run's own font. Without per-char
                // advances there is nothing to cut against — the run paints
                // as laid out and the clip alone applies.
                let (text, advances, text_width) = match (self.ellipsis.as_mut(), &advances) {
                    (Some(scope), Some(adv)) => {
                        let ellipsis_advance = shape_line_advances("\u{2026}", style, font_size)
                            .and_then(|a| a.first().copied())
                            .unwrap_or_else(|| {
                                measure_text_with_spacing(
                                    "\u{2026}",
                                    &style.font_family,
                                    font_size,
                                    style.font_weight,
                                    style.font_style,
                                    0.0,
                                    0.0,
                                )
                                .width
                            });
                        match scope.cut(&text, x, line_top, adv, ellipsis_advance) {
                            TextOverflowCut::Keep => (text, advances, text_width),
                            TextOverflowCut::Hide => continue,
                            TextOverflowCut::Cut {
                                text,
                                advances,
                                width,
                            } => (text, Some(advances), width),
                        }
                    }
                    _ => (text, advances, text_width),
                };

                // Check if this is gradient text (background-clip: text with gradient and transparent fill)
                let is_gradient_text = style.background_clip == rustkit_css::BackgroundClip::Text
                    && style.webkit_text_fill_color == Some(rustkit_css::Color::TRANSPARENT)
                    && style.background_gradient.is_some();

                if is_gradient_text {
                    // Emit gradient text command
                    if let Some(gradient) = &style.background_gradient {
                        self.commands.push(DisplayCommand::GradientText {
                            text: text.clone(),
                            x,
                            y,
                            font_size,
                            font_family: style.font_family.clone(),
                            font_weight: style.font_weight.0,
                            font_style: match style.font_style {
                                rustkit_css::FontStyle::Normal => 0,
                                rustkit_css::FontStyle::Italic => 1,
                                rustkit_css::FontStyle::Oblique => 2,
                            },
                            gradient: gradient.clone(),
                            rect: Rect::new(x, y, text_width, line_height),
                            advances,
                            ascent: Some(seat_ascent),
                        });
                        continue; // Skip regular text rendering for this line
                    }
                }

                self.commands.push(DisplayCommand::Text {
                    text: text.clone(),
                    x,
                    y,
                    color: style.color,
                    font_size,
                    font_family: style.font_family.clone(),
                    font_weight: style.font_weight.0,
                    font_style: match style.font_style {
                        rustkit_css::FontStyle::Normal => 0,
                        rustkit_css::FontStyle::Italic => 1,
                        rustkit_css::FontStyle::Oblique => 2,
                    },
                    advances,
                    ascent: Some(seat_ascent),
                });

                // Draw text decorations
                let decoration_line = style.text_decoration_line;
                if decoration_line.underline
                    || decoration_line.overline
                    || decoration_line.line_through
                {
                    let decoration_color = style.text_decoration_color.unwrap_or(style.color);
                    let decoration_style = match style.text_decoration_style {
                        rustkit_css::TextDecorationStyle::Solid => TextDecorationStyleValue::Solid,
                        rustkit_css::TextDecorationStyle::Double => {
                            TextDecorationStyleValue::Double
                        }
                        rustkit_css::TextDecorationStyle::Dotted => {
                            TextDecorationStyleValue::Dotted
                        }
                        rustkit_css::TextDecorationStyle::Dashed => {
                            TextDecorationStyleValue::Dashed
                        }
                        rustkit_css::TextDecorationStyle::Wavy => TextDecorationStyleValue::Wavy,
                    };

                    // Get actual font metrics for accurate decoration positioning
                    let metrics = measure_text_advanced(
                        &text,
                        &style.font_family,
                        font_size,
                        style.font_weight,
                        style.font_style,
                    );

                    // Calculate thickness from style or font metrics
                    let thickness = match style.text_decoration_thickness {
                        Length::Px(px) => px,
                        Length::Em(em) => em * font_size,
                        _ => {
                            // Use font metrics if available, otherwise fallback
                            if metrics.underline_thickness > 0.0 {
                                metrics.underline_thickness
                            } else {
                                font_size / 14.0
                            }
                        }
                    };

                    // Use actual metrics for positioning
                    let ascent = if metrics.ascent > 0.0 {
                        metrics.ascent
                    } else {
                        font_size * 0.8
                    };

                    // Underline: position below baseline using font metrics
                    if decoration_line.underline {
                        let underline_y = if metrics.underline_offset != 0.0 {
                            // Font provides underline position (negative = below baseline)
                            y + ascent - metrics.underline_offset
                        } else {
                            // Fallback: position slightly below baseline
                            y + ascent + font_size * 0.1
                        };

                        self.commands.push(DisplayCommand::TextDecoration {
                            x,
                            y: underline_y,
                            width: text_width,
                            thickness,
                            color: decoration_color,
                            style: decoration_style,
                        });
                    }

                    // Overline: position at top of text
                    if decoration_line.overline {
                        let overline_y = if metrics.overline_offset != 0.0 {
                            y + ascent - metrics.overline_offset
                        } else {
                            y // At top of text box
                        };

                        self.commands.push(DisplayCommand::TextDecoration {
                            x,
                            y: overline_y,
                            width: text_width,
                            thickness,
                            color: decoration_color,
                            style: decoration_style,
                        });
                    }

                    // Line-through (strikethrough): position at middle of x-height
                    if decoration_line.line_through {
                        let strikethrough_y = if metrics.strikethrough_offset != 0.0 {
                            y + ascent - metrics.strikethrough_offset
                        } else {
                            // Fallback: approximately middle of x-height
                            y + ascent * 0.35
                        };

                        self.commands.push(DisplayCommand::TextDecoration {
                            x,
                            y: strikethrough_y,
                            width: text_width,
                            thickness,
                            color: decoration_color,
                            style: decoration_style,
                        });
                    }
                }
            } // end per-line loop
        }
    }

    /// Render replaced content (images).
    fn render_replaced_content(&mut self, layout_box: &LayoutBox) {
        match &layout_box.box_type {
            BoxType::Image {
                url,
                natural_width,
                natural_height,
            } => {
                let dims = &layout_box.dimensions;
                let container = Rect {
                    x: dims.content.x,
                    y: dims.content.y,
                    width: dims.content.width,
                    height: dims.content.height,
                };

                // Parse object-fit from style
                let object_fit = match layout_box.style.object_fit.as_str() {
                    "fill" => ObjectFit::Fill,
                    "contain" => ObjectFit::Contain,
                    "cover" => ObjectFit::Cover,
                    "none" => ObjectFit::None,
                    "scale-down" => ObjectFit::ScaleDown,
                    // Unknown keyword falls back to the INITIAL value (fill,
                    // CSS Images 3 §5.5), not to a different behaviour. The
                    // old `contain` fallback silently letterboxed anything
                    // whose object-fit failed to parse.
                    _ => ObjectFit::Fill,
                };

                let (pos_x, pos_y) = layout_box.style.object_position;

                // Generate image display command
                let cmd = crate::images::render_image(
                    url,
                    container,
                    *natural_width,
                    *natural_height,
                    object_fit,
                    (pos_x, pos_y),
                    layout_box.style.opacity,
                    layout_box.style.color,
                );

                self.commands.push(cmd);
            }
            BoxType::FormControl(control) => {
                self.render_form_control(layout_box, control);
            }
            _ => {}
        }
    }

    /// Render a form control.
    fn render_form_control(&mut self, layout_box: &LayoutBox, control: &FormControlType) {
        let dims = &layout_box.dimensions;
        let rect = Rect {
            x: dims.content.x,
            y: dims.content.y,
            width: dims.content.width,
            height: dims.content.height,
        };

        let font_size = match layout_box.style.font_size {
            Length::Px(px) => px,
            _ => 16.0,
        };

        let text_color = layout_box.style.color;
        let bg_color = layout_box.style.background_color;
        let border_color = layout_box.style.border_top_color;
        let font_family = layout_box.style.font_family.clone();
        let font_weight = layout_box.style.font_weight.0;
        // Same resolution layout_form_control composes the box from, so the
        // painter's text seat and the box's height agree on the padding.
        // `rem` resolves against the root font: settings' `padding: 0.5rem
        // 0.75rem` read as 0 here while the box was sized with it, so every
        // select/number label sat on the control's left border.
        let root_font_size = self.root_font_size;
        let padding = {
            let px = |l: &Length| match l {
                Length::Px(v) => *v,
                Length::Em(em) => em * font_size,
                Length::Rem(rem) => rem * root_font_size,
                _ => 0.0,
            };
            [
                px(&layout_box.style.padding_top),
                px(&layout_box.style.padding_right),
                px(&layout_box.style.padding_bottom),
                px(&layout_box.style.padding_left),
            ]
        };

        match control {
            FormControlType::TextInput {
                value, placeholder, ..
            } => {
                self.commands.push(DisplayCommand::TextInput {
                    rect,
                    value: value.clone(),
                    placeholder: placeholder.clone(),
                    font_size,
                    font_family: font_family.clone(),
                    font_weight,
                    padding,
                    text_color,
                    placeholder_color: Color::new(160, 160, 160, 1.0),
                    // The UA layer supplies the default background for form
                    // controls, so an authored `transparent` survives here
                    // instead of being replaced with white at paint time.
                    background_color: bg_color,
                    border_color: if border_color.a > 0.0 {
                        border_color
                    } else {
                        Color::new(200, 200, 200, 1.0)
                    },
                    border_width: 1.0,
                    // Focus and caret come from the engine's live edit state,
                    // carried on the box via `focused_caret` (unblocked by
                    // LayoutBox::node_id).
                    focused: layout_box.focused_caret.is_some(),
                    caret_position: layout_box.focused_caret,
                });
            }
            FormControlType::TextArea {
                value, placeholder, ..
            } => {
                self.commands.push(DisplayCommand::TextInput {
                    rect,
                    value: value.clone(),
                    placeholder: placeholder.clone(),
                    font_size,
                    font_family: font_family.clone(),
                    font_weight,
                    padding,
                    text_color,
                    placeholder_color: Color::new(160, 160, 160, 1.0),
                    // The UA layer supplies the default background for form
                    // controls, so an authored `transparent` survives here
                    // instead of being replaced with white at paint time.
                    background_color: bg_color,
                    border_color: if border_color.a > 0.0 {
                        border_color
                    } else {
                        Color::new(200, 200, 200, 1.0)
                    },
                    border_width: 1.0,
                    focused: layout_box.focused_caret.is_some(),
                    caret_position: layout_box.focused_caret,
                });
            }
            FormControlType::Button { label, .. } => {
                self.commands.push(DisplayCommand::Button {
                    rect,
                    label: label.clone(),
                    font_size,
                    font_family: font_family.clone(),
                    font_weight,
                    padding,
                    text_color: if text_color.a > 0.0 {
                        text_color
                    } else {
                        Color::BLACK
                    },
                    background_color: if bg_color.a > 0.0 {
                        bg_color
                    } else {
                        Color::new(239, 239, 239, 1.0)
                    },
                    border_color: if border_color.a > 0.0 {
                        border_color
                    } else {
                        Color::new(180, 180, 180, 1.0)
                    },
                    border_width: 1.0,
                    border_radius: 4.0,
                    pressed: false,
                    focused: false,
                });
            }
            FormControlType::Checkbox { checked } => {
                // Draw checkbox as a small rect with optional checkmark
                let check_color = if *checked {
                    text_color
                } else {
                    Color::TRANSPARENT
                };
                self.commands.push(DisplayCommand::SolidColor(
                    if bg_color.a > 0.0 {
                        bg_color
                    } else {
                        Color::WHITE
                    },
                    rect,
                ));
                self.commands.push(DisplayCommand::Border {
                    color: Color::new(150, 150, 150, 1.0),
                    rect,
                    top: 1.0,
                    right: 1.0,
                    bottom: 1.0,
                    left: 1.0,
                });
                if *checked {
                    // Draw a simple checkmark using lines (simplified)
                    let inner = Rect {
                        x: rect.x + 3.0,
                        y: rect.y + 3.0,
                        width: rect.width - 6.0,
                        height: rect.height - 6.0,
                    };
                    self.commands
                        .push(DisplayCommand::SolidColor(check_color, inner));
                }
            }
            FormControlType::Radio { checked, .. } => {
                // Draw radio as a circle (using ellipse)
                self.commands.push(DisplayCommand::FillEllipse {
                    rect,
                    color: if bg_color.a > 0.0 {
                        bg_color
                    } else {
                        Color::WHITE
                    },
                });
                // Outer ring
                self.commands.push(DisplayCommand::StrokeCircle {
                    cx: rect.x + rect.width / 2.0,
                    cy: rect.y + rect.height / 2.0,
                    radius: rect.width / 2.0 - 1.0,
                    color: Color::new(150, 150, 150, 1.0),
                    width: 1.0,
                });
                if *checked {
                    // Inner dot
                    self.commands.push(DisplayCommand::FillCircle {
                        cx: rect.x + rect.width / 2.0,
                        cy: rect.y + rect.height / 2.0,
                        radius: rect.width / 4.0,
                        color: text_color,
                    });
                }
            }
            FormControlType::Select {
                options,
                selected_index,
                ..
            } => {
                // Draw as a text input with dropdown arrow
                let display_text = selected_index
                    .and_then(|i| options.get(i))
                    .cloned()
                    .unwrap_or_default();

                self.commands.push(DisplayCommand::TextInput {
                    rect,
                    value: display_text,
                    placeholder: String::new(),
                    font_size,
                    font_family: font_family.clone(),
                    font_weight,
                    padding,
                    text_color,
                    placeholder_color: Color::new(160, 160, 160, 1.0),
                    // The UA layer supplies the default background for form
                    // controls, so an authored `transparent` survives here
                    // instead of being replaced with white at paint time.
                    background_color: bg_color,
                    border_color: if border_color.a > 0.0 {
                        border_color
                    } else {
                        Color::new(200, 200, 200, 1.0)
                    },
                    border_width: 1.0,
                    focused: false,
                    caret_position: None,
                });
            }
        }
    }
}

/// Measure text using the text shaper.
///
/// This provides accurate text measurement using DirectWrite on Windows.
pub fn measure_text_advanced(
    text: &str,
    font_family: &str,
    font_size: f32,
    font_weight: rustkit_css::FontWeight,
    font_style: rustkit_css::FontStyle,
) -> TextMetrics {
    measure_text_with_spacing(
        text,
        font_family,
        font_size,
        font_weight,
        font_style,
        0.0,
        0.0,
    )
}

/// Measure text using the text shaper with letter-spacing and word-spacing.
///
/// This provides accurate text measurement using DirectWrite on Windows,
/// with support for CSS letter-spacing and word-spacing properties.
pub fn measure_text_with_spacing(
    text: &str,
    font_family: &str,
    font_size: f32,
    font_weight: rustkit_css::FontWeight,
    font_style: rustkit_css::FontStyle,
    letter_spacing: f32,
    word_spacing: f32,
) -> TextMetrics {
    let shaper = TextShaper::new();
    let chain = FontFamilyChain::from_css_value(font_family);

    match shaper.shape(
        text,
        &chain,
        font_weight,
        font_style,
        rustkit_css::FontStretch::Normal,
        font_size,
    ) {
        Ok(mut run) => {
            // Apply letter-spacing and word-spacing
            run.apply_spacing(letter_spacing, word_spacing);
            run.metrics
        }
        Err(_) => {
            // Fallback to simple measurement with spacing
            let mut metrics = measure_text_simple(text, font_size);
            // Apply letter-spacing (one per character)
            let char_count = text.chars().count();
            metrics.width += letter_spacing * char_count as f32;
            // Apply word-spacing (one per whitespace)
            let space_count = text.chars().filter(|c| c.is_whitespace()).count();
            metrics.width += word_spacing * space_count as f32;
            metrics
        }
    }
}

/// Per-CHAR advances from the layout shaper, letter/word-spacing applied
/// (ADVANCE CONTRACT, text-stack unification 2026-07-11). Returns None when
/// shaping fails or when glyph count != char count (ligature clusters) — the
/// renderer then falls back to its own advances instead of misaligning.
pub fn shape_line_advances(
    text: &str,
    style: &rustkit_css::ComputedStyle,
    font_size: f32,
) -> Option<Vec<f32>> {
    let letter_spacing = match style.letter_spacing {
        Length::Px(px) => px,
        Length::Em(em) => em * font_size,
        Length::Rem(rem) => rem * 16.0,
        _ => 0.0,
    };
    let word_spacing = match style.word_spacing {
        Length::Px(px) => px,
        Length::Em(em) => em * font_size,
        Length::Rem(rem) => rem * 16.0,
        _ => 0.0,
    };
    let shaper = TextShaper::new();
    let chain = FontFamilyChain::from_css_value(&style.font_family);
    let mut run = shaper
        .shape(
            text,
            &chain,
            style.font_weight,
            style.font_style,
            style.font_stretch,
            font_size,
        )
        .ok()?;
    run.apply_spacing(letter_spacing, word_spacing);
    if run.glyphs.len() != text.chars().count() {
        return None;
    }
    Some(run.glyphs.iter().map(|g| g.advance).collect())
}

/// Simple text measurement (fallback when shaping is unavailable).
pub fn measure_text_simple(text: &str, font_size: f32) -> TextMetrics {
    // Approximate metrics based on font size
    // Typical Latin font has ~0.5em average character width
    let avg_char_width = font_size * 0.5;
    let width = text.chars().count() as f32 * avg_char_width;

    TextMetrics {
        width,
        ..TextMetrics::with_font_size(font_size)
    }
}

/// Measure text (simplified - uses average character width approximation).
///
/// For more accurate measurement, use `measure_text_advanced`.
#[deprecated(
    since = "0.1.0",
    note = "Use measure_text_advanced for accurate measurement"
)]
pub fn measure_text(text: &str, _font_family: &str, font_size: f32) -> text::TextMetrics {
    measure_text_simple(text, font_size)
}

/// Blink's dash pattern for one border side of `length` px
/// (`StrokeData::SetupPaintDashPathEffect` + `SelectBestDashGap`): a dashed
/// side draws dashes of 2x its thickness with 1x gaps (3x / 2x under 3px),
/// a dotted side 1x / 1x; the gap is then stretched or squeezed so the side
/// starts AND ends on a whole dash, choosing the dash count whose gap is
/// nearest the nominal one. A side too short for two dashes paints solid.
/// Returns `(dash, gap)`, or `None` for a solid side.
///
/// Thick dotted borders are round dots in Chrome; they are square here.
fn border_dash_pattern(
    style: rustkit_css::BorderStyle,
    thickness: f32,
    length: f32,
) -> Option<(f32, f32)> {
    let (dash, gap) = match style {
        // `None` never reaches paint with a width (the cascade zeroes it).
        rustkit_css::BorderStyle::Solid | rustkit_css::BorderStyle::None => return None,
        rustkit_css::BorderStyle::Dashed if thickness < 3.0 => (thickness * 3.0, thickness * 2.0),
        rustkit_css::BorderStyle::Dashed => (thickness * 2.0, thickness),
        rustkit_css::BorderStyle::Dotted => (thickness, thickness),
    };
    if dash <= 0.0 || length <= dash * 2.0 {
        return None;
    }
    let min_dashes = ((length + gap) / (dash + gap)).floor();
    let max_dashes = min_dashes + 1.0;
    let min_gap = if min_dashes > 1.0 {
        (length - min_dashes * dash) / (min_dashes - 1.0)
    } else {
        f32::INFINITY
    };
    let max_gap = (length - max_dashes * dash) / (max_dashes - 1.0);
    let best = if max_gap <= 0.0 || (min_gap - gap).abs() < (max_gap - gap).abs() {
        min_gap
    } else {
        max_gap
    };
    Some((dash, best))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_styled_button_composes_its_font_line_with_author_padding() {
        // flex-positioning `.btn { padding: 8px 16px; border: none; font-size:
        // 14px }`: Chrome builds label + 32 wide and 16 + 16 = 32 tall (the
        // control font's normal line, not font_size + 1), and the flex path
        // must size it from the same function as block flow.
        let mut style = ComputedStyle::new();
        style.font_size = Length::Px(14.0);
        style.padding_top = Length::Px(8.0);
        style.padding_bottom = Length::Px(8.0);
        style.padding_left = Length::Px(16.0);
        style.padding_right = Length::Px(16.0);
        let control = FormControlType::Button {
            label: "Save".to_string(),
            button_type: "button".to_string(),
        };
        let (w, h) = form_control_intrinsic_size(&style, &control);
        let label = measure_text_advanced(
            "Save",
            &style.font_family,
            14.0,
            style.font_weight,
            style.font_style,
        )
        .width;
        assert_eq!(w, label + 32.0);
        assert_eq!(h, normal_line_height(&style, 14.0) + 16.0);
        assert_eq!(h.fract(), 0.0, "a normal line is whole pixels, got {h}");
    }

    #[test]
    fn test_baseline_seat_floors_the_leading_above_whole_pixel_metrics() {
        // about's `.card p`: 16px system font (ascent 15.47, descent 3.38) on
        // a 27.2px line. Chrome's ink baseline is line top + 19 on all three
        // paragraphs above the fold (tops 430.0 / 469.19 / 535.56 -> rows
        // 449 / 488 / 555); the fractional seat 4.18 + 15.47 put the first
        // two one row low.
        let seat = blink_baseline_offset(27.2, 15.46875, 3.375);
        assert_eq!(seat, 19.0);
        assert_eq!((430.0_f32 + seat).round(), 449.0);
        assert_eq!((469.1875_f32 + seat).round(), 488.0);
        assert_eq!((535.5625_f32 + seat).round(), 555.0);
    }

    #[test]
    fn test_inline_content_area_shift_leaves_text_in_its_line_slot() {
        // `<span><strong>HxHx</strong></span>` on a 16px/32px line: both
        // inline rects sit a half-leading (7) below the line top, as in Chrome
        // (147 for a line at 140); the text leaf stays at the line top, where
        // paint seats it from the run's own leading. Translating it as well
        // painted the word 7px under the rest of the line.
        let style = ComputedStyle::new();
        let mut text = LayoutBox::new(BoxType::Text("HxHx".to_string()), style.clone());
        text.dimensions.content.y = 140.0;
        let mut strong = LayoutBox::new(BoxType::Inline, style.clone());
        strong.dimensions.content.y = 140.0;
        strong.children.push(text);
        let mut span = LayoutBox::new(BoxType::Inline, style);
        span.dimensions.content.y = 140.0;
        span.children.push(strong);

        span.shift_inline_content_area(7.0);

        assert_eq!(span.dimensions.content.y, 147.0);
        assert_eq!(span.children[0].dimensions.content.y, 147.0);
        assert_eq!(span.children[0].children[0].dimensions.content.y, 140.0);
    }

    #[test]
    fn test_small_font_inline_drops_to_the_line_baseline() {
        // `<p>HxHx <span class="small">HxHx</span></p>`, 16px/32px Arial with
        // an 11px span: Chrome seats the span's content area at line top + 11
        // (repro inline-baseline-drop.html: p at 20, span rect at 31) — its
        // slot one pixel below the line top, so both baselines share a row.
        // The align pass left every non-atomic inline at the line top.
        let mut body = ComputedStyle::new();
        body.font_size = Length::Px(16.0);
        body.line_height = rustkit_css::LineHeight::Px(32.0);
        let mut small = body.clone();
        small.font_size = Length::Px(11.0);

        let mut text = LayoutBox::new(BoxType::Text("HxHx ".to_string()), body.clone());
        text.dimensions.content.y = 100.0;
        text.dimensions.content.height = 32.0;
        let mut inner = LayoutBox::new(BoxType::Text("HxHx".to_string()), small.clone());
        inner.dimensions.content.y = 100.0;
        inner.dimensions.content.height = 32.0;
        let mut span = LayoutBox::new(BoxType::Inline, small.clone());
        span.dimensions.content.y = 100.0;
        span.children.push(inner);
        let (_, half_leading) = span.inline_content_area();
        span.shift_inline_content_area(half_leading);

        let mut line = vec![text, span];
        LayoutBox::apply_vertical_align(&mut line, &body);

        let drop = LayoutBox::text_line_box_extents(&body).0
            - LayoutBox::text_line_box_extents(&small).0;
        assert!(drop > 0.0, "the 16px strut sits above an 11px run, got {drop}");
        assert_eq!(drop.fract(), 0.0, "whole-pixel seats, got {drop}");
        assert_eq!(line[0].dimensions.content.y, 100.0, "the strut-font text does not move");
        assert_eq!(line[1].children[0].dimensions.content.y, 100.0 + drop);
        assert_eq!(line[1].dimensions.content.y, 100.0 + half_leading + drop);
    }

    #[test]
    fn test_line_of_only_small_inlines_keeps_its_line_top() {
        // A line whose only member is an inline: its rect sits a half-leading
        // below the slot, and reading the rect as the line top would push the
        // baseline — and the span — down by that half-leading again.
        let mut body = ComputedStyle::new();
        body.font_size = Length::Px(16.0);
        body.line_height = rustkit_css::LineHeight::Px(32.0);

        let mut inner = LayoutBox::new(BoxType::Text("HxHx".to_string()), body.clone());
        inner.dimensions.content.y = 100.0;
        let mut span = LayoutBox::new(BoxType::Inline, body.clone());
        span.dimensions.content.y = 100.0;
        span.children.push(inner);
        let (_, half_leading) = span.inline_content_area();
        span.shift_inline_content_area(half_leading);

        let mut line = vec![span];
        LayoutBox::apply_vertical_align(&mut line, &body);

        assert_eq!(line[0].children[0].dimensions.content.y, 100.0);
        assert_eq!(line[0].dimensions.content.y, 100.0 + half_leading);
    }

    #[test]
    fn test_inline_member_feeds_the_line_box_extents() {
        // The same line is 33px in Chrome, not 32: the small span's slot hangs
        // 12 below a baseline the strut puts 21 down.
        let mut body = ComputedStyle::new();
        body.font_size = Length::Px(16.0);
        body.line_height = rustkit_css::LineHeight::Px(32.0);
        let mut small = body.clone();
        small.font_size = Length::Px(11.0);
        let span = LayoutBox::new(BoxType::Inline, small.clone());
        assert_eq!(
            span.line_member_baseline_extents(),
            Some(LayoutBox::text_line_box_extents(&small))
        );
    }

    #[test]
    fn test_baseline_seat_without_leading_is_the_rounded_ascent() {
        // `normal` lines (no leading) already agreed with Chrome: the logo
        // (64px, 61.875 / 13.5 on a 76px line) and the 14px badge.
        assert_eq!(blink_baseline_offset(76.0, 61.875, 13.5), 62.0);
        assert_eq!(blink_baseline_offset(17.0, 13.535156, 2.953125), 14.0);
        // Negative leading is signed, then floored (n57 + #209): 15 + 3 on a
        // 16px line leaves -1 above, so the baseline is 14, not the clamped 15.
        assert_eq!(blink_baseline_offset(16.0, 15.46875, 3.375), 14.0);
    }

    #[test]
    fn test_oversized_gradient_paint_is_clipped_to_its_box() {
        // The gradient-backgrounds "-45deg Rainbow" card: `background-size:
        // 400% 400%` scales the gradient IMAGE; the paint stays inside the
        // border box. Without a clip the 4x rect painted in full — square
        // corners, covering the neighbouring cards. Geometry was verified
        // exact against Chrome (227x180 both sides); only paint escaped.
        let mut style = ComputedStyle::new();
        style.background_layers = vec![rustkit_css::BackgroundLayer {
            image: rustkit_css::BackgroundImage::Gradient(rustkit_css::Gradient::Linear(
                rustkit_css::LinearGradient {
                    direction: rustkit_css::GradientDirection::Angle(-45.0),
                    stops: vec![
                        rustkit_css::ColorStop {
                            color: Color {
                                r: 238,
                                g: 119,
                                b: 82,
                                a: 1.0,
                            },
                            position: None,
                        },
                        rustkit_css::ColorStop {
                            color: Color {
                                r: 35,
                                g: 213,
                                b: 171,
                                a: 1.0,
                            },
                            position: None,
                        },
                    ],
                    repeating: false,
                },
            )),
            // Percentages ride the Explicit variant as negative values:
            // -400.0 => container * 4.0 (see calculate_background_rect).
            size: rustkit_css::BackgroundSize::Explicit {
                width: Some(-400.0),
                height: Some(-400.0),
            },
            ..Default::default()
        }];

        let mut card = LayoutBox::new(BoxType::Block, style);
        card.dimensions.content = Rect::new(533.0, 400.0, 227.0, 180.0);

        let list = DisplayList::build(&card);
        let container = card.dimensions.border_box();

        let push = list.commands.iter().position(|c| matches!(c, DisplayCommand::PushClip(r) if (r.x - container.x).abs() < 0.5 && (r.width - container.width).abs() < 0.5));
        let grad = list.commands.iter().position(
            |c| matches!(c, DisplayCommand::LinearGradient { rect, .. } if rect.width > 900.0),
        );
        let pop = list
            .commands
            .iter()
            .position(|c| matches!(c, DisplayCommand::PopClip));

        let (push, grad, pop) = (
            push.expect("oversized gradient must push a clip at its own box"),
            grad.expect("gradient must still paint at the scaled 4x rect (the zoomed slice)"),
            pop.expect("clip must be popped"),
        );
        assert!(
            push < grad && grad < pop,
            "order must be PushClip < gradient < PopClip, got {push}/{grad}/{pop}"
        );
    }

    #[test]
    fn test_normal_size_gradient_pushes_no_clip() {
        // Control: an auto-sized gradient fills exactly its box; adding a
        // clip there would be noise (and would mask a future regression in
        // the needs_clip predicate by making the clip unconditional).
        let mut style = ComputedStyle::new();
        style.background_layers = vec![rustkit_css::BackgroundLayer {
            image: rustkit_css::BackgroundImage::Gradient(rustkit_css::Gradient::Linear(
                rustkit_css::LinearGradient {
                    direction: rustkit_css::GradientDirection::ToBottom,
                    stops: vec![
                        rustkit_css::ColorStop {
                            color: Color {
                                r: 0,
                                g: 0,
                                b: 0,
                                a: 1.0,
                            },
                            position: None,
                        },
                        rustkit_css::ColorStop {
                            color: Color {
                                r: 255,
                                g: 255,
                                b: 255,
                                a: 1.0,
                            },
                            position: None,
                        },
                    ],
                    repeating: false,
                },
            )),
            ..Default::default()
        }];
        let mut card = LayoutBox::new(BoxType::Block, style);
        card.dimensions.content = Rect::new(0.0, 0.0, 227.0, 180.0);

        let list = DisplayList::build(&card);
        assert!(
            !list
                .commands
                .iter()
                .any(|c| matches!(c, DisplayCommand::PushClip(_))),
            "a gradient that fits its box must not push a clip"
        );
    }

    // ---------- P1's residual: the scaled gradient's clip carries the radius ----------
    //
    // gradient-backgrounds' `.linear-6`: `background-size: 400% 400%` on a
    // `.gradient-box { border-radius: 16px }`. The clip that stops the 4x rect
    // bleeding over its neighbours was a plain rect, so the card painted its
    // own fill square into the four corner notches where Chrome shows the page
    // behind. Gate B auto-failed three of those corners as `missing_clip` on
    // the macOS board of 2026-09-08 — 36 notch px, the whole board's only
    // discrete failure.

    /// A `.gradient-box`-shaped card: `background-size: 400% 400%`, optionally
    /// under a uniform `border-radius`.
    fn scaled_gradient_card(radius_px: f32) -> LayoutBox {
        let mut style = ComputedStyle::new();
        if radius_px > 0.0 {
            style.border_top_left_radius = Length::Px(radius_px);
            style.border_top_right_radius = Length::Px(radius_px);
            style.border_bottom_right_radius = Length::Px(radius_px);
            style.border_bottom_left_radius = Length::Px(radius_px);
        }
        style.background_layers = vec![rustkit_css::BackgroundLayer {
            image: rustkit_css::BackgroundImage::Gradient(rustkit_css::Gradient::Linear(
                rustkit_css::LinearGradient {
                    direction: rustkit_css::GradientDirection::Angle(-45.0),
                    stops: vec![
                        rustkit_css::ColorStop {
                            color: Color {
                                r: 238,
                                g: 119,
                                b: 82,
                                a: 1.0,
                            },
                            position: None,
                        },
                        rustkit_css::ColorStop {
                            color: Color {
                                r: 35,
                                g: 213,
                                b: 171,
                                a: 1.0,
                            },
                            position: None,
                        },
                    ],
                    repeating: false,
                },
            )),
            // Percentages ride Explicit as negatives: -400.0 => container * 4.
            size: rustkit_css::BackgroundSize::Explicit {
                width: Some(-400.0),
                height: Some(-400.0),
            },
            ..Default::default()
        }];

        let mut card = LayoutBox::new(BoxType::Block, style);
        card.dimensions.content = Rect::new(533.0, 400.0, 227.0, 180.0);
        card
    }

    #[test]
    fn a_scaled_gradient_under_a_radius_is_clipped_at_that_radius() {
        let card = scaled_gradient_card(16.0);
        let container = card.dimensions.border_box();
        let list = DisplayList::build(&card);

        let rounded = list.commands.iter().find_map(|c| match c {
            DisplayCommand::PushClipRounded { rect, radius } => Some((*rect, *radius)),
            _ => None,
        });
        let (rect, radius) =
            rounded.expect("a scaled gradient under a radius must push a ROUNDED clip");

        assert!(
            (rect.x - container.x).abs() < 0.5
                && (rect.y - container.y).abs() < 0.5
                && (rect.width - container.width).abs() < 0.5
                && (rect.height - container.height).abs() < 0.5,
            "the clip is the box, not the 4x rect: {rect:?} vs {container:?}"
        );
        // Every corner, not just the one a single notch happens to expose.
        assert_eq!(
            (
                radius.top_left,
                radius.top_right,
                radius.bottom_right,
                radius.bottom_left
            ),
            (16.0, 16.0, 16.0, 16.0),
            "the clip must carry the box's own radius on all four corners"
        );
        assert!(
            !list
                .commands
                .iter()
                .any(|c| matches!(c, DisplayCommand::PushClip(_))),
            "a square clip at the same box would cut the notches back off"
        );
    }

    #[test]
    fn a_scaled_gradient_under_a_radius_still_paints_the_zoomed_slice() {
        // The clip must not become the paint rect: `background-size: 400%`
        // shows a 4x-zoomed slice, and a gradient re-fitted to the container
        // would be a different image that happens to have round corners.
        let card = scaled_gradient_card(16.0);
        let list = DisplayList::build(&card);

        let push = list
            .commands
            .iter()
            .position(|c| matches!(c, DisplayCommand::PushClipRounded { .. }))
            .expect("rounded clip");
        let grad = list
            .commands
            .iter()
            .position(
                |c| matches!(c, DisplayCommand::LinearGradient { rect, .. } if rect.width > 900.0),
            )
            .expect("gradient must still paint at the scaled 4x rect");
        let pop = list
            .commands
            .iter()
            .position(|c| matches!(c, DisplayCommand::PopClip))
            .expect("clip must be popped");
        assert!(
            push < grad && grad < pop,
            "order must be PushClipRounded < gradient < PopClip, got {push}/{grad}/{pop}"
        );
    }

    #[test]
    fn a_scaled_gradient_with_square_corners_keeps_its_square_clip() {
        // Control: the radius is what makes the clip rounded. A square card
        // must not start paying for a rounded clip it has no corners for.
        let card = scaled_gradient_card(0.0);
        let list = DisplayList::build(&card);
        assert!(
            list.commands
                .iter()
                .any(|c| matches!(c, DisplayCommand::PushClip(_))),
            "a scaled gradient with no radius still needs its square clip"
        );
        assert!(
            !list
                .commands
                .iter()
                .any(|c| matches!(c, DisplayCommand::PushClipRounded { .. })),
            "no radius, no rounded clip"
        );
    }

    // ==================== text-overflow: ellipsis ====================
    //
    // css-overflow-3 §5.1. The chrome's `.tab-title` / `.url-text`, the
    // shelf's `.command-item-name`, settings' `.cellar-item-*`: `white-space:
    // nowrap; overflow: hidden; text-overflow: ellipsis`. n35 made the clip
    // real, which cut those titles hard; the ellipsis is the visible half.

    const ELLIPSIS_TEXT: &str = "The quick brown fox jumps over the lazy dog";

    /// root → block(`width` wide, at x=10) → text. The block is NOT the root
    /// (the root never clips, §11.1.1).
    fn ellipsis_tree(
        width: f32,
        text_overflow: rustkit_css::TextOverflow,
        overflow: rustkit_css::Overflow,
        wrap_in_inline: bool,
    ) -> LayoutBox {
        let mut style = ComputedStyle::new();
        style.white_space = rustkit_css::WhiteSpace::Nowrap;
        let mut root = LayoutBox::new(BoxType::Block, style.clone());
        root.dimensions.content = Rect::new(0.0, 0.0, 800.0, 600.0);

        let mut block_style = style.clone();
        block_style.overflow_x = overflow;
        block_style.overflow_y = overflow;
        block_style.text_overflow = text_overflow;
        let mut block = LayoutBox::new(BoxType::Block, block_style);
        block.dimensions.content = Rect::new(10.0, 10.0, width, 20.0);

        let mut text = LayoutBox::new(BoxType::Text(ELLIPSIS_TEXT.to_string()), style.clone());
        text.dimensions.content = Rect::new(10.0, 10.0, width, 20.0);

        if wrap_in_inline {
            let mut inline = LayoutBox::new(BoxType::Inline, style);
            inline.dimensions.content = Rect::new(10.0, 10.0, width, 20.0);
            inline.children.push(text);
            block.children.push(inline);
        } else {
            block.children.push(text);
        }
        root.children.push(block);
        root
    }

    /// (text, x, advances) of every Text command.
    fn text_commands(list: &DisplayList) -> Vec<(String, f32, Option<Vec<f32>>)> {
        list.commands
            .iter()
            .filter_map(|c| match c {
                DisplayCommand::Text {
                    text, x, advances, ..
                } => Some((text.clone(), *x, advances.clone())),
                _ => None,
            })
            .collect()
    }

    // ==================== negative leading + seat face (n57) ====================

    /// root → block → text run in `family` at `font_size` with `line_height`;
    /// returns `(content_y, y_cmd, ascent_cmd)` of the one Text command.
    fn seat_probe(
        family: &str,
        font_size: f32,
        line_height: rustkit_css::LineHeight,
        text: &str,
    ) -> (f32, f32, f32) {
        let mut style = ComputedStyle::new();
        style.font_family = family.to_string();
        style.font_size = Length::Px(font_size);
        style.line_height = line_height;
        let mut root = LayoutBox::new(BoxType::Block, ComputedStyle::new());
        root.dimensions.content = Rect::new(0.0, 0.0, 800.0, 600.0);
        let mut text_box = LayoutBox::new(BoxType::Text(text.to_string()), style);
        text_box.dimensions.content = Rect::new(20.0, 100.0, 600.0, font_size);
        root.children.push(text_box);
        let list = DisplayList::build(&root);
        let cmd = list
            .commands
            .iter()
            .find_map(|c| match c {
                DisplayCommand::Text { y, ascent, .. } => Some((*y, ascent.expect("ascent"))),
                _ => None,
            })
            .expect("one Text command");
        (100.0, cmd.0, cmd.1)
    }

    #[test]
    fn test_half_leading_is_signed() {
        assert_eq!(half_leading(24.0, 15.0, 3.0), 3.0);
        assert_eq!(half_leading(16.0, 15.0, 3.0), -1.0);
        assert!((half_leading(40.0, 38.67, 8.44) + 3.555).abs() < 1e-3);
    }

    /// `line-height: 1` on a face whose content area exceeds 1em (Arial:
    /// 0.905 + 0.212 = 1.117em): the run's y_cmd sits ABOVE its content top
    /// by half the overflow, so the baseline lands where Chrome's does
    /// (32px bold Arial in a 32px line: top + 29 − 2 = top + 27; a zero
    /// floor put it at top + 29).
    #[test]
    fn test_negative_leading_seats_the_baseline_above_the_line_top() {
        let (top, y, ascent) = seat_probe("Arial", 32.0, rustkit_css::LineHeight::Number(1.0), "Arial");
        let m = measure_text_advanced("x", "Arial", 32.0, rustkit_css::FontWeight(400), rustkit_css::FontStyle::Normal);
        if m.ascent <= 0.0 {
            return; // no Arial on this machine: nothing to seat against
        }
        let content = m.ascent + m.descent;
        assert!(content > 32.0, "Arial's content area exceeds 1em: {content}");
        // Paint seats the baseline at round(y + ascent): Blink's whole-pixel
        // seat with the signed leading floored (29 + floor((32 - 36) / 2)).
        let expected = top + blink_baseline_offset(32.0, m.ascent, m.descent);
        assert_eq!((y + ascent).round(), expected, "y_cmd {y} + ascent {ascent}");
        assert!(expected < top + m.ascent.round(), "seated above the zero-floor baseline");
        assert!(y < top, "negative leading seats above the line top");
        assert!((ascent - m.ascent).abs() < 0.01);
    }

    /// An explicit line-height seats the run on the PRIMARY face: "🔒 x" at
    /// 16px Arial in a 20px line ships Arial's ascent (~14.5), not the emoji
    /// face's 20 — Chrome's baseline is top + 1 + 15 = 16 there. Under
    /// `normal` the united metrics still win (n40: "☕ coffee" is a 26px line
    /// seated 20 down).
    #[test]
    fn test_explicit_line_height_seats_on_the_primary_face() {
        let arial = measure_text_advanced("x", "Arial", 16.0, rustkit_css::FontWeight(400), rustkit_css::FontStyle::Normal);
        let united = measure_text_advanced("🔒 x", "Arial", 16.0, rustkit_css::FontWeight(400), rustkit_css::FontStyle::Normal);
        if arial.ascent <= 0.0 || united.ascent <= arial.ascent + 1.0 {
            return; // no emoji fallback face on this machine
        }
        let (top, y, ascent) = seat_probe("Arial", 16.0, rustkit_css::LineHeight::Px(20.0), "🔒 x");
        assert!((ascent - arial.ascent).abs() < 0.01, "explicit line-height: primary ascent {ascent} vs {}", arial.ascent);
        let expected = top + blink_baseline_offset(20.0, arial.ascent, arial.descent);
        assert_eq!((y + ascent).round(), expected, "y_cmd {y} + primary ascent {ascent}");

        let (_, _, ascent_normal) = seat_probe("Arial", 16.0, rustkit_css::LineHeight::Normal, "🔒 x");
        assert!((ascent_normal - united.ascent).abs() < 0.01, "normal: united ascent {ascent_normal} vs {}", united.ascent);
    }

    #[test]
    fn test_text_overflow_ellipsis_cuts_overflowing_run_inside_content_edge() {
        let root = ellipsis_tree(
            100.0,
            rustkit_css::TextOverflow::Ellipsis,
            rustkit_css::Overflow::Hidden,
            false,
        );
        let texts = text_commands(&DisplayList::build(&root));
        assert_eq!(texts.len(), 1, "one run paints: {texts:?}");
        let (text, x, advances) = &texts[0];
        assert!(
            text.ends_with('\u{2026}'),
            "run must end in U+2026, got {text:?}"
        );
        assert!(
            text.chars().count() > 3 && text.chars().count() < ELLIPSIS_TEXT.chars().count(),
            "some but not all characters survive the cut: {text:?}"
        );
        let advances = advances.as_ref().expect("cut run carries advances");
        assert_eq!(
            advances.len(),
            text.chars().count(),
            "one advance per char incl. the ellipsis"
        );
        let width: f32 = advances.iter().sum();
        assert!(
            *x + width <= 10.0 + 100.0 + 0.01,
            "ink (x={x}, w={width}) must end inside the content edge 110"
        );
        // The cut is tight: the next original character would not have fit.
        let full =
            shape_line_advances(ELLIPSIS_TEXT, &root.children[0].style, 16.0).expect("shape");
        let kept = text.chars().count() - 1;
        let next_width: f32 = full[..kept + 1].iter().sum::<f32>() + advances[kept];
        assert!(
            *x + next_width > 110.0,
            "keeping one more char ({}) would still have fit: {next_width}",
            kept + 1
        );
    }

    #[test]
    fn test_text_overflow_ellipsis_leaves_a_fitting_run_alone() {
        let root = ellipsis_tree(
            1000.0,
            rustkit_css::TextOverflow::Ellipsis,
            rustkit_css::Overflow::Hidden,
            false,
        );
        let texts = text_commands(&DisplayList::build(&root));
        assert_eq!(texts.len(), 1);
        assert_eq!(
            texts[0].0, ELLIPSIS_TEXT,
            "a run that fits is painted whole"
        );
    }

    #[test]
    fn test_text_overflow_needs_overflow_other_than_visible() {
        // §5.1: applies to block containers with overflow other than
        // visible. `text-overflow: ellipsis` alone does nothing.
        let root = ellipsis_tree(
            100.0,
            rustkit_css::TextOverflow::Ellipsis,
            rustkit_css::Overflow::Visible,
            false,
        );
        let texts = text_commands(&DisplayList::build(&root));
        assert_eq!(texts[0].0, ELLIPSIS_TEXT, "overflow: visible — no ellipsis");
    }

    #[test]
    fn test_text_overflow_clip_is_the_initial_value_and_cuts_nothing() {
        let root = ellipsis_tree(
            100.0,
            rustkit_css::TextOverflow::Clip,
            rustkit_css::Overflow::Hidden,
            false,
        );
        let texts = text_commands(&DisplayList::build(&root));
        assert_eq!(
            texts[0].0, ELLIPSIS_TEXT,
            "text-overflow: clip — the clip alone applies"
        );
        assert_eq!(
            rustkit_css::TextOverflow::default(),
            rustkit_css::TextOverflow::Clip
        );
    }

    #[test]
    fn test_text_overflow_scope_reaches_through_inline_boxes() {
        // The block owns its line boxes; a `<span>` inside it paints its
        // text under the block's ellipsis.
        let root = ellipsis_tree(
            100.0,
            rustkit_css::TextOverflow::Ellipsis,
            rustkit_css::Overflow::Hidden,
            true,
        );
        let texts = text_commands(&DisplayList::build(&root));
        assert_eq!(texts.len(), 1);
        assert!(
            texts[0].0.ends_with('\u{2026}'),
            "inline child is cut: {:?}",
            texts[0].0
        );
    }

    #[test]
    fn test_text_overflow_does_not_leak_into_a_nested_block_container() {
        // A nested block container establishes its own line boxes and has
        // its own (initial: clip) text-overflow.
        let mut root = ellipsis_tree(
            100.0,
            rustkit_css::TextOverflow::Ellipsis,
            rustkit_css::Overflow::Hidden,
            false,
        );
        let text = root.children[0].children.pop().expect("text child");
        let mut inner = LayoutBox::new(BoxType::Block, ComputedStyle::new());
        inner.dimensions.content = Rect::new(10.0, 10.0, 100.0, 20.0);
        inner.children.push(text);
        root.children[0].children.push(inner);

        let texts = text_commands(&DisplayList::build(&root));
        assert_eq!(texts.len(), 1);
        assert_eq!(
            texts[0].0, ELLIPSIS_TEXT,
            "nested block container: no ellipsis from the outer one"
        );
    }

    #[test]
    fn test_text_overflow_hides_a_later_run_behind_the_ellipsis() {
        // Two runs on one line: the first is cut and carries the ellipsis;
        // the second follows it in inline order and must not paint over it —
        // wherever layout put it. `x = 300` is the honest position; `x = 60`
        // is the engine-real one (the first run's laid-out width is clamped
        // to the 100px container, so its sibling starts short of the edge,
        // and short of the ellipsis).
        for second_x in [300.0_f32, 60.0] {
            let mut root = ellipsis_tree(
                100.0,
                rustkit_css::TextOverflow::Ellipsis,
                rustkit_css::Overflow::Hidden,
                false,
            );
            let mut second = LayoutBox::new(
                BoxType::Text("tail".to_string()),
                root.children[0].children[0].style.clone(),
            );
            second.dimensions.content = Rect::new(second_x, 10.0, 30.0, 20.0);
            root.children[0].children.push(second);

            let texts = text_commands(&DisplayList::build(&root));
            assert_eq!(
                texts.len(),
                1,
                "the second run (x={second_x}) is behind the ellipsis: {texts:?}"
            );
            assert!(texts[0].0.ends_with('\u{2026}'));
        }

        // Control: the same second run on ANOTHER line paints (it fits).
        let mut root = ellipsis_tree(
            100.0,
            rustkit_css::TextOverflow::Ellipsis,
            rustkit_css::Overflow::Hidden,
            false,
        );
        let mut second = LayoutBox::new(
            BoxType::Text("tail".to_string()),
            root.children[0].children[0].style.clone(),
        );
        second.dimensions.content = Rect::new(10.0, 30.0, 30.0, 20.0);
        root.children[0].children.push(second);
        let texts = text_commands(&DisplayList::build(&root));
        assert_eq!(
            texts.len(),
            2,
            "a run on the next line is its own line: {texts:?}"
        );
        assert_eq!(texts[1].0, "tail");
    }

    // ==================== Overflow rounded clip ====================
    //
    // image-gallery's shape, which Gate B reported 17 times on 2026-08-08 as
    // `missing_clip`: `.gallery-item { border-radius: 12px; overflow: hidden }`
    // wrapping a child that fills it exactly and paints its own background.
    // The child's paint has to be cut at the parent's arc.

    fn rounded_overflow_parent(radius_px: f32, hidden: bool) -> LayoutBox {
        let mut style = ComputedStyle::new();
        style.border_top_left_radius = Length::Px(radius_px);
        style.border_top_right_radius = Length::Px(radius_px);
        style.border_bottom_right_radius = Length::Px(radius_px);
        style.border_bottom_left_radius = Length::Px(radius_px);
        if hidden {
            style.overflow_x = rustkit_css::Overflow::Hidden;
            style.overflow_y = rustkit_css::Overflow::Hidden;
        }

        let mut child_style = ComputedStyle::new();
        child_style.background_color = Color {
            r: 102,
            g: 126,
            b: 234,
            a: 1.0,
        };
        let mut child = LayoutBox::new(BoxType::Block, child_style);
        child.dimensions.content = Rect::new(0.0, 0.0, 227.0, 180.0);

        let mut parent = LayoutBox::new(BoxType::Block, style);
        parent.dimensions.content = Rect::new(0.0, 0.0, 227.0, 180.0);
        parent.children.push(child);
        parent
    }

    fn rounded_clip(list: &DisplayList) -> Option<(Rect, BorderRadius)> {
        list.commands.iter().find_map(|c| match c {
            DisplayCommand::PushClipRounded { rect, radius } => Some((*rect, *radius)),
            _ => None,
        })
    }

    #[test]
    fn a_bordered_box_under_a_radius_paints_one_rounded_border() {
        let mut b = rounded_overflow_parent(12.0, false);
        b.children.clear();
        b.dimensions.border = EdgeSizes {
            top: 5.0,
            right: 5.0,
            bottom: 5.0,
            left: 5.0,
        };
        b.style.border_top_color = Color::BLACK;
        let list = DisplayList::build(&under_root(b));
        let rounded: Vec<_> = list
            .commands
            .iter()
            .filter_map(|c| match c {
                DisplayCommand::RoundedBorder { rect, widths, colors, radius } => {
                    Some((*rect, *widths, *colors, *radius))
                }
                _ => None,
            })
            .collect();
        assert_eq!(rounded.len(), 1, "one rounded border, not four square strips");
        let (rect, widths, colors, radius) = rounded[0];
        assert_eq!(rect.width, 237.0, "the border box");
        assert_eq!(widths, [5.0; 4]);
        assert_eq!(colors[0], Color::BLACK);
        assert_eq!(radius.bottom_left, 12.0);
        assert!(
            !list.commands.iter().any(|c| matches!(c, DisplayCommand::SolidColor(col, _) if *col == Color::BLACK)),
            "the square strips must not also paint"
        );

        // Square corners keep the strip path.
        let mut sq = rounded_overflow_parent(0.0, false);
        sq.children.clear();
        sq.dimensions.border.top = 5.0;
        let list = DisplayList::build(&under_root(sq));
        assert!(!list
            .commands
            .iter()
            .any(|c| matches!(c, DisplayCommand::RoundedBorder { .. })));
    }

    #[test]
    fn a_rounded_dashed_border_keeps_its_dashes() {
        // #217 x #227 seam: the ring command has no dash pattern, so a
        // dashed side under a radius stays on the per-side (dashed) path.
        let mut b = rounded_overflow_parent(12.0, false);
        b.children.clear();
        b.dimensions.border = EdgeSizes { top: 5.0, right: 5.0, bottom: 5.0, left: 5.0 };
        b.style.border_top_style = rustkit_css::BorderStyle::Dashed;
        let list = DisplayList::build(&under_root(b));
        assert!(
            !list.commands.iter().any(|c| matches!(c, DisplayCommand::RoundedBorder { .. })),
            "a dashed side must not collapse into a solid rounded ring"
        );
    }

    #[test]
    fn overflow_hidden_under_a_radius_pushes_a_rounded_clip_around_its_children() {
        let list = DisplayList::build(&under_root(rounded_overflow_parent(12.0, true)));
        let (rect, radius) = rounded_clip(&list).expect(
            "a rounded box that clips its overflow must push a rounded clip for its children",
        );
        assert_eq!(radius.top_left, 12.0);
        assert_eq!(rect.width, 227.0);

        let push = list
            .commands
            .iter()
            .position(|c| matches!(c, DisplayCommand::PushClipRounded { .. }))
            .unwrap();
        let child_bg = list
            .commands
            .iter()
            .position(|c| matches!(c, DisplayCommand::SolidColor(color, _) if color.b == 234))
            .expect("the child's background must be in the list");
        let pop = list
            .commands
            .iter()
            .rposition(|c| matches!(c, DisplayCommand::PopClip))
            .expect("the clip must be popped");
        assert!(
            push < child_bg && child_bg < pop,
            "the child must paint INSIDE the clip, got push={push} child={child_bg} pop={pop}"
        );
    }

    #[test]
    fn positioned_children_are_inside_the_clip_too() {
        // `overflow` clips EVERY descendant it contains, not just the in-flow
        // ones, so the pop belongs after the positioned pass. image-gallery's
        // `.image-overlay` is `position: absolute` inside a `position: relative`
        // item and sits on its bottom corners — pop it early and the overlay
        // paints square into exactly the notches this change exists to clear.
        // (A STATIC clipper is a different case: see the escape tests below.)
        let mut parent = rounded_overflow_parent(12.0, true);
        parent.position = Position::Relative;
        let mut overlay_style = ComputedStyle::new();
        overlay_style.background_color = Color {
            r: 0,
            g: 0,
            b: 0,
            a: 0.7,
        };
        let mut overlay = LayoutBox::new(BoxType::Block, overlay_style);
        overlay.position = Position::Absolute;
        overlay.dimensions.content = Rect::new(0.0, 120.0, 227.0, 60.0);
        parent.children.push(overlay);

        let list = DisplayList::build(&under_root(parent));
        let push = list
            .commands
            .iter()
            .position(|c| matches!(c, DisplayCommand::PushClipRounded { .. }))
            .expect("must push a clip");
        let overlay_bg = list
            .commands
            .iter()
            .position(|c| matches!(c, DisplayCommand::SolidColor(color, _) if color.a == 0.7))
            .expect("the positioned overlay's background must be in the list");
        let pop = list
            .commands
            .iter()
            .rposition(|c| matches!(c, DisplayCommand::PopClip))
            .expect("the clip must be popped");
        assert!(
            push < overlay_bg && overlay_bg < pop,
            "a positioned child must paint inside the clip, got push={push} overlay={overlay_bg} pop={pop}"
        );
    }

    #[test]
    fn overflow_visible_pushes_no_clip_however_round_the_box_is() {
        // A radius alone does not clip descendants — `overflow: visible` lets a
        // child paint past the arc, and Chrome agrees.
        let list = DisplayList::build(&rounded_overflow_parent(12.0, false));
        assert!(
            rounded_clip(&list).is_none(),
            "overflow: visible must not clip children"
        );
    }

    /// Wrap `inner` in a root box so the unit under test is not itself the
    /// root (the root's overflow propagates to the viewport and never clips).
    fn under_root(inner: LayoutBox) -> LayoutBox {
        let mut root = LayoutBox::new(BoxType::Block, ComputedStyle::new());
        root.dimensions.content = Rect::new(0.0, 0.0, 800.0, 600.0);
        root.children.push(inner);
        root
    }

    fn square_clip(list: &DisplayList) -> Option<Rect> {
        list.commands.iter().find_map(|c| match c {
            DisplayCommand::PushClip(rect) => Some(*rect),
            _ => None,
        })
    }

    #[test]
    fn overflow_hidden_with_square_corners_pushes_a_square_clip_around_its_children() {
        // The square half of overflow clipping (n35): `overflow: hidden` on a
        // box with no radius clips its descendants to its padding box. WPT
        // overflow-wrap-anywhere-002/003 are the exact-match probes — a
        // `height: 1em; overflow: hidden` div whose second line must not paint.
        let list = DisplayList::build(&under_root(rounded_overflow_parent(0.0, true)));
        assert!(
            rounded_clip(&list).is_none(),
            "no radius means no rounded clip"
        );
        let rect = square_clip(&list).expect("a square overflow clip must be pushed");
        assert_eq!((rect.width, rect.height), (227.0, 180.0));

        let push = list
            .commands
            .iter()
            .position(|c| matches!(c, DisplayCommand::PushClip(_)))
            .unwrap();
        let child_bg = list
            .commands
            .iter()
            .position(|c| matches!(c, DisplayCommand::SolidColor(color, _) if color.b == 234))
            .expect("the child's background must be in the list");
        let pop = list
            .commands
            .iter()
            .rposition(|c| matches!(c, DisplayCommand::PopClip))
            .expect("the clip must be popped");
        assert!(
            push < child_bg && child_bg < pop,
            "the child must paint INSIDE the clip, got push={push} child={child_bg} pop={pop}"
        );
    }

    #[test]
    fn the_root_never_clips_its_overflow_propagates_to_the_viewport() {
        let list = DisplayList::build(&rounded_overflow_parent(0.0, true));
        assert!(
            square_clip(&list).is_none() && rounded_clip(&list).is_none(),
            "the root box's overflow belongs to the viewport, not to a clip"
        );
    }

    #[test]
    fn body_does_not_clip_while_the_root_overflow_is_visible() {
        // CSS 2.1 §11.1.1: when the root's overflow is visible, body's
        // propagates to the viewport instead — `body { overflow: hidden }`
        // must not cut the page at body's padding box.
        let mut body = rounded_overflow_parent(0.0, true);
        body.identity = Some(Box::new(ElementIdentity {
            element_id: 1,
            tag: "body".to_string(),
            selector: "body".to_string(),
        }));
        let list = DisplayList::build(&under_root(body));
        assert!(
            square_clip(&list).is_none(),
            "body's overflow propagates to the viewport when the root's is visible"
        );

        // But with the root's overflow non-visible, body keeps its own.
        let mut body = rounded_overflow_parent(0.0, true);
        body.identity = Some(Box::new(ElementIdentity {
            element_id: 1,
            tag: "body".to_string(),
            selector: "body".to_string(),
        }));
        let mut root = under_root(body);
        root.style.overflow_y = rustkit_css::Overflow::Auto;
        let list = DisplayList::build(&root);
        assert!(
            square_clip(&list).is_some(),
            "body clips when the root's overflow is already non-visible"
        );
    }

    #[test]
    fn an_absolute_child_of_a_static_clipper_escapes_the_clip() {
        // css-overflow-3 §3.1: overflow clips descendants whose containing
        // block is the box or inside it. A static `overflow: hidden` wrapper is
        // NOT the containing block of an absolutely positioned child — the
        // dropdown-escapes-its-wrapper case — so that child paints outside the
        // clip, and the clip is back in force for the sibling after it.
        let mut parent = rounded_overflow_parent(0.0, true); // position: static
        let mut overlay_style = ComputedStyle::new();
        overlay_style.background_color = Color {
            r: 0,
            g: 0,
            b: 0,
            a: 0.7,
        };
        let mut overlay = LayoutBox::new(BoxType::Block, overlay_style);
        overlay.position = Position::Absolute;
        overlay.dimensions.content = Rect::new(0.0, 120.0, 227.0, 60.0);
        parent.children.push(overlay);

        let mut later_style = ComputedStyle::new();
        later_style.background_color = Color {
            r: 9,
            g: 9,
            b: 9,
            a: 1.0,
        };
        let mut later = LayoutBox::new(BoxType::Block, later_style);
        later.float = Float::Left; // paints in the positioned pass, after the overlay
        later.dimensions.content = Rect::new(0.0, 0.0, 10.0, 10.0);
        parent.children.push(later);

        let list = DisplayList::build(&under_root(parent));
        let cmds = &list.commands;
        let overlay_bg = cmds
            .iter()
            .position(|c| matches!(c, DisplayCommand::SolidColor(color, _) if color.a == 0.7))
            .expect("the overlay's background must be in the list");
        let later_bg = cmds
            .iter()
            .position(|c| matches!(c, DisplayCommand::SolidColor(color, _) if color.r == 9))
            .expect("the later sibling's background must be in the list");

        // Clip depth at a command index: pushes minus pops before it.
        let depth_at = |i: usize| {
            cmds[..i].iter().fold(0i32, |d, c| match c {
                DisplayCommand::PushClip(_) | DisplayCommand::PushClipRounded { .. } => d + 1,
                DisplayCommand::PopClip => d - 1,
                _ => d,
            })
        };
        assert_eq!(
            depth_at(overlay_bg),
            0,
            "the absolute child must paint with the static clip popped"
        );
        assert_eq!(
            depth_at(later_bg),
            1,
            "the clip must be re-pushed for the next sibling"
        );
        assert_eq!(depth_at(cmds.len()), 0, "the list must end balanced");
    }

    #[test]
    fn a_style_relative_clipper_is_a_containing_block_even_when_laid_out_static() {
        // The engine transfers `position: relative` onto the layout box as
        // `Position::Static` (transfer_positioning: the paint-side stacking
        // path is not ready for relative boxes). The containing-block rule
        // must read the computed style, or image-gallery's `.gallery-item {
        // position: relative; overflow: hidden }` lets its absolute overlay
        // paint square into the corner notch — measured as +74 px on the
        // campaign board the first time this lane ran.
        let mut parent = rounded_overflow_parent(12.0, true);
        parent.style.position = rustkit_css::Position::Relative; // layout position stays Static
        let mut overlay_style = ComputedStyle::new();
        overlay_style.background_color = Color {
            r: 0,
            g: 0,
            b: 0,
            a: 0.7,
        };
        let mut overlay = LayoutBox::new(BoxType::Block, overlay_style);
        overlay.position = Position::Absolute;
        overlay.dimensions.content = Rect::new(0.0, 120.0, 227.0, 60.0);
        parent.children.push(overlay);

        let list = DisplayList::build(&under_root(parent));
        let cmds = &list.commands;
        let overlay_bg = cmds
            .iter()
            .position(|c| matches!(c, DisplayCommand::SolidColor(color, _) if color.a == 0.7))
            .unwrap();
        let depth = cmds[..overlay_bg].iter().fold(0i32, |d, c| match c {
            DisplayCommand::PushClip(_) | DisplayCommand::PushClipRounded { .. } => d + 1,
            DisplayCommand::PopClip => d - 1,
            _ => d,
        });
        assert_eq!(
            depth, 1,
            "a style-relative clipper clips its absolute children"
        );
    }

    #[test]
    fn an_absolute_child_of_a_positioned_clipper_stays_inside_the_clip() {
        // The mirror image: a positioned clipper IS the containing block.
        let mut parent = rounded_overflow_parent(0.0, true);
        parent.position = Position::Relative;
        let mut overlay_style = ComputedStyle::new();
        overlay_style.background_color = Color {
            r: 0,
            g: 0,
            b: 0,
            a: 0.7,
        };
        let mut overlay = LayoutBox::new(BoxType::Block, overlay_style);
        overlay.position = Position::Absolute;
        overlay.dimensions.content = Rect::new(0.0, 120.0, 227.0, 60.0);
        parent.children.push(overlay);

        let list = DisplayList::build(&under_root(parent));
        let cmds = &list.commands;
        let overlay_bg = cmds
            .iter()
            .position(|c| matches!(c, DisplayCommand::SolidColor(color, _) if color.a == 0.7))
            .unwrap();
        let depth = cmds[..overlay_bg].iter().fold(0i32, |d, c| match c {
            DisplayCommand::PushClip(_) | DisplayCommand::PushClipRounded { .. } => d + 1,
            DisplayCommand::PopClip => d - 1,
            _ => d,
        });
        assert_eq!(depth, 1, "a positioned clipper clips its absolute children");
    }

    #[test]
    fn the_overflow_clip_is_the_padding_box_with_the_radius_shrunk_by_the_border() {
        // Overflow clips to the PADDING box. Clipping at the border box would
        // leave a child's paint sitting on top of the border it should be
        // behind, and the arc would be the outer one, a border-width off.
        let mut parent = rounded_overflow_parent(12.0, true);
        parent.dimensions.border = EdgeSizes {
            top: 4.0,
            right: 4.0,
            bottom: 4.0,
            left: 4.0,
        };
        let border_box = parent.dimensions.border_box();

        let list = DisplayList::build(&under_root(parent));
        let (rect, radius) = rounded_clip(&list).expect("must still clip");
        assert_eq!(rect.x, border_box.x + 4.0);
        assert_eq!(rect.width, border_box.width - 8.0);
        assert_eq!(
            radius.top_left, 8.0,
            "12px radius inside a 4px border is 8px"
        );
    }

    #[test]
    fn a_radius_swallowed_entirely_by_its_border_is_a_square_clip() {
        // Inner radius floors at zero rather than going negative, and a clip
        // with no rounding left is a square clip at the padding box — the
        // border has eaten the arc, so there is nothing round left to clip to.
        let mut parent = rounded_overflow_parent(4.0, true);
        parent.dimensions.border = EdgeSizes {
            top: 6.0,
            right: 6.0,
            bottom: 6.0,
            left: 6.0,
        };

        let list = DisplayList::build(&under_root(parent));
        assert!(
            rounded_clip(&list).is_none(),
            "no rounding survives a thicker border"
        );
        let rect = square_clip(&list).expect("the padding box still clips, square");
        assert_eq!(
            rect.width, 227.0,
            "the padding box is the content box here (no padding)"
        );
    }

    #[test]
    fn the_box_own_background_is_not_inside_its_own_overflow_clip() {
        // The background already rounds itself (RoundedRect / the gradient's
        // own border_radius). Clipping it again would double the antialiasing
        // at every corner and darken the arc by a visible amount.
        //
        // The parent needs its OWN background for this to test anything. An
        // earlier version asserted inside `if let Some(own_bg)` on a fixture
        // with no background, so the assertion never ran and moving the
        // content emission inside the clip left it green.
        let mut parent = rounded_overflow_parent(12.0, true);
        parent.style.background_color = Color {
            r: 45,
            g: 45,
            b: 68,
            a: 1.0,
        };

        let list = DisplayList::build(&under_root(parent));
        let push = list
            .commands
            .iter()
            .position(|c| matches!(c, DisplayCommand::PushClipRounded { .. }))
            .expect("must push a clip");
        let own_bg = list
            .commands
            .iter()
            .position(|c| matches!(c, DisplayCommand::RoundedRect { .. }))
            .expect("the parent's own rounded background must be in the list");
        assert!(
            own_bg < push,
            "the box's own background paints before its clip, got bg={own_bg} push={push}"
        );
    }

    #[test]
    fn test_rect() {
        let r = Rect::new(10.0, 20.0, 100.0, 50.0);
        assert_eq!(r.right(), 110.0);
        assert_eq!(r.bottom(), 70.0);
        assert!(r.contains(50.0, 30.0));
        assert!(!r.contains(0.0, 0.0));
    }

    #[test]
    fn test_advance_contract_letter_spacing_sum() {
        // GradientText advance-carry merge gate: with non-zero
        // letter-spacing, the shared helper's advances must sum to the
        // spaced measure within 0.5px (one test covers BOTH command types
        // at the contract root — GradientText uses the same helper).
        let mut style = ComputedStyle::new();
        style.font_size = Length::Px(16.0);
        style.letter_spacing = Length::Px(2.0);
        let text = "Spaced advance parity";
        let advances =
            shape_line_advances(text, &style, 16.0).expect("plain Latin text must shape");
        let measured = measure_text_with_spacing(
            text,
            &style.font_family,
            16.0,
            style.font_weight,
            style.font_style,
            2.0,
            0.0,
        )
        .width;
        let sum: f32 = advances.iter().sum();
        assert!(
            (sum - measured).abs() < 0.5,
            "spaced advance sum {sum} != spaced measure {measured}"
        );
    }

    #[test]
    fn test_advance_contract_sum_matches_measure() {
        // ADVANCE CONTRACT (text-stack unification): the per-char advances
        // shipped to paint must sum to the same width layout measured —
        // within 0.5px — or centering math and painted ink diverge again.
        let mut style = ComputedStyle::new();
        style.font_size = Length::Px(16.0);
        let text = "The quick brown fox jumps over 42 lazy dogs.";
        let advances = shape_line_advances(text, &style, 16.0)
            .expect("plain Latin text must shape to per-char advances");
        assert_eq!(advances.len(), text.chars().count());
        let measured = measure_text_advanced(
            text,
            &style.font_family,
            16.0,
            style.font_weight,
            style.font_style,
        )
        .width;
        let sum: f32 = advances.iter().sum();
        assert!(
            (sum - measured).abs() < 0.5,
            "advance sum {sum} != measured width {measured}"
        );
    }

    #[test]
    fn test_fixed_positions_against_viewport_not_flow() {
        // CSS2 §10.1 regression (R1 / viewport-probe, 2026-07-10): a fixed
        // element with right:0; bottom:0 must anchor to the VIEWPORT, not
        // the block that laid it out — bottom used to resolve against the
        // root's flow height, painting a 600px-viewport footer at y=280.
        let mut style = ComputedStyle::new();
        style.width = Length::Px(40.0);
        style.height = Length::Px(40.0);
        let mut fixed = LayoutBox::new(BoxType::Block, style);
        fixed.position = Position::Fixed;
        fixed.offsets.right = Some(0.0);
        fixed.offsets.bottom = Some(0.0);

        let mut root = LayoutBox::new(BoxType::Block, ComputedStyle::new());
        root.children.push(fixed);
        root.set_viewport(800.0, 600.0);

        // Lay out against a "flow" containing block much shorter than the
        // viewport — the old bug anchored bottom against this.
        let containing = Dimensions {
            content: Rect::new(0.0, 0.0, 800.0, 320.0),
            ..Default::default()
        };
        root.layout(&containing);

        let f = &root.children[0].dimensions.content;
        assert!(
            (f.x - 760.0).abs() < 0.5 && (f.y - 560.0).abs() < 0.5,
            "fixed right:0;bottom:0 must land at (760,560) in an 800x600 viewport, got ({}, {})",
            f.x,
            f.y
        );
    }

    #[test]
    fn test_dimensions_boxes() {
        let d = Dimensions {
            content: Rect::new(20.0, 20.0, 100.0, 50.0),
            padding: EdgeSizes {
                top: 5.0,
                right: 5.0,
                bottom: 5.0,
                left: 5.0,
            },
            border: EdgeSizes {
                top: 1.0,
                right: 1.0,
                bottom: 1.0,
                left: 1.0,
            },
            margin: EdgeSizes {
                top: 10.0,
                right: 10.0,
                bottom: 10.0,
                left: 10.0,
            },
        };

        let pb = d.padding_box();
        assert_eq!(pb.width, 110.0);
        assert_eq!(pb.height, 60.0);

        let bb = d.border_box();
        assert_eq!(bb.width, 112.0);
        assert_eq!(bb.height, 62.0);

        let mb = d.margin_box();
        assert_eq!(mb.width, 132.0);
        assert_eq!(mb.height, 82.0);
    }

    #[test]
    fn test_layout_box_creation() {
        let style = ComputedStyle::new();
        let layout_box = LayoutBox::new(BoxType::Block, style);
        assert!(matches!(layout_box.box_type, BoxType::Block));
        assert_eq!(layout_box.position, Position::Static);
        assert_eq!(layout_box.float, Float::None);
    }

    #[test]
    fn test_layout_box_with_position() {
        let style = ComputedStyle::new();
        let layout_box = LayoutBox::with_position(BoxType::Block, style, Position::Relative);
        assert_eq!(layout_box.position, Position::Relative);
        assert!(layout_box.stacking_context.is_some());
    }

    #[test]
    fn test_layout_box_with_float() {
        let style = ComputedStyle::new();
        let layout_box = LayoutBox::with_float(BoxType::Block, style, Float::Left);
        assert_eq!(layout_box.float, Float::Left);
    }

    fn containing_1000() -> Dimensions {
        let mut cb = Dimensions::default();
        cb.content = Rect::new(0.0, 0.0, 1000.0, 600.0);
        cb
    }

    #[test]
    fn test_auto_margins_center_fixed_width() {
        // width: 600px; margin: 0 auto → 200px each side (CSS 2.1 §10.3.3)
        let mut style = ComputedStyle::new();
        style.width = Length::Px(600.0);
        style.margin_left = Length::Auto;
        style.margin_right = Length::Auto;
        let mut layout_box = LayoutBox::new(BoxType::Block, style);
        layout_box.calculate_block_width(&containing_1000());
        assert_eq!(layout_box.dimensions.content.width, 600.0);
        assert_eq!(layout_box.dimensions.margin.left, 200.0);
        assert_eq!(layout_box.dimensions.margin.right, 200.0);
    }

    fn containing_200() -> Dimensions {
        let mut cb = Dimensions::default();
        cb.content = Rect::new(0.0, 0.0, 200.0, 600.0);
        cb
    }

    const LONG_TEXT: &str = "The quick brown fox jumps over the lazy dog again \
                             and again until the line has no choice but to wrap";

    #[test]
    fn test_text_wraps_into_line_boxes() {
        // Overflowing text in a 200px block wraps: multiple lines, each within
        // the container, box height = line_count * line-height (CSS2 §9.4.2).
        let style = ComputedStyle::new();
        let mut layout_box = LayoutBox::new(BoxType::Text(LONG_TEXT.to_string()), style);
        layout_box.layout_text(LONG_TEXT.to_string(), &containing_200());

        let lines = layout_box.text_lines.as_ref().expect("text should wrap");
        assert!(
            lines.len() > 1,
            "expected multiple lines, got {}",
            lines.len()
        );
        for line in lines {
            assert!(
                line.width <= 200.0 + 0.5,
                "line '{}' overflows container: {}px",
                line.text,
                line.width
            );
        }
        let line_height = layout_box.get_line_height();
        assert_eq!(
            layout_box.dimensions.content.height,
            lines.len() as f32 * line_height
        );
        // Wrapped fragment spans from the container origin (per-line offsets
        // handle text-align), never a single shifted run.
        assert_eq!(layout_box.dimensions.content.x, 0.0);
    }

    /// css-text-3 §7.3 receipt for a justified line: its natural width plus
    /// the distributed slack reaches the container edge.
    fn justified_width(line: &TextLine) -> f32 {
        let natural = TextLine::natural_ink_width(&line.text, &ComputedStyle::new(), 16.0)
            .expect("the test text shapes one glyph per char");
        assert!(
            natural <= line.width,
            "the ink width never exceeds the wrap width (which keeps the break-point space): {natural} vs {}",
            line.width
        );
        line.x_offset
            + natural
            + line.justify_space
                * TextLine::justification_opportunities(line.text.trim_end()) as f32
    }

    #[test]
    fn justified_wrapped_lines_fill_the_container_except_the_last() {
        // text-align: justify (css-text-3 §7.3): every soft-wrapped line
        // spreads its slack over its word separators and ends at the
        // container edge; the block's last line keeps natural spacing.
        // Before n49 the Justify arm was `0.0 // (complex)` and every
        // justified paragraph on every page painted start-aligned.
        let mut style = ComputedStyle::new();
        style.text_align = TextAlign::Justify;
        let mut layout_box = LayoutBox::new(BoxType::Text(LONG_TEXT.to_string()), style);
        layout_box.layout_text(LONG_TEXT.to_string(), &containing_200());
        let n = layout_box.text_lines.as_ref().expect("text should wrap").len();
        assert!(n >= 3, "need at least three lines, got {n}");

        let mut children = vec![layout_box];
        LayoutBox::apply_text_align_offset(&mut children, 200.0, 200.0, TextAlign::Justify);
        let lines = children[0].text_lines.as_ref().unwrap();
        for (k, line) in lines.iter().enumerate().take(n - 1) {
            assert_eq!(line.x_offset, 0.0, "justify never shifts a line (line {k})");
            let gaps = TextLine::justification_opportunities(line.text.trim_end());
            assert!(gaps > 0, "line {k} '{}' has no word separator", line.text);
            assert!(
                line.justify_space > 0.0,
                "line {k} '{}' must be justified, justify_space = {}",
                line.text,
                line.justify_space
            );
            assert!(
                (justified_width(line) - 200.0).abs() < 0.01,
                "line {k} '{}' must end at the container edge: {}",
                line.text,
                justified_width(line)
            );
        }
        let last = &lines[n - 1];
        assert_eq!(last.justify_space, 0.0, "the block's last line is never justified");
    }

    #[test]
    fn justify_leaves_left_lines_and_preserved_newlines_alone() {
        // Left: no expansion. pre-wrap: a wrapped line may end in a forced
        // break, which justification must not stretch (css-text-3 §7.3).
        for (align, ws) in [
            (TextAlign::Left, rustkit_css::WhiteSpace::Normal),
            (TextAlign::Justify, rustkit_css::WhiteSpace::PreWrap),
        ] {
            let mut style = ComputedStyle::new();
            style.text_align = align;
            style.white_space = ws;
            let mut layout_box = LayoutBox::new(BoxType::Text(LONG_TEXT.to_string()), style);
            layout_box.layout_text(LONG_TEXT.to_string(), &containing_200());
            assert!(layout_box.text_lines.as_ref().map_or(0, |l| l.len()) > 1);
            let mut children = vec![layout_box];
            LayoutBox::apply_text_align_offset(&mut children, 200.0, 200.0, align);
            for line in children[0].text_lines.as_ref().unwrap() {
                assert_eq!(
                    line.justify_space, 0.0,
                    "{align:?}/{ws:?}: line '{}' must not be justified",
                    line.text
                );
            }
        }
    }

    #[test]
    fn justify_midline_split_skips_line_zero_and_the_open_last_line() {
        // A run that starts after a sibling: line 0 is a mixed line (its
        // slack belongs to the sibling's spaces too — not distributed, it
        // stays start-aligned, ledgered), middle lines fill the container,
        // the open last line is left natural.
        let parent = midline_split_parent(TextAlign::Justify);
        assert_eq!(parent.children[0].dimensions.content.x, 0.0, "the prior sibling never moves");
        let t = &parent.children[1];
        let flow0 = t.text_flow_first_offset.unwrap();
        let tls = t.text_lines.as_ref().unwrap();
        let n = tls.len();
        assert!(n >= 3, "need line0 + middle + last, got {n}");
        assert!((tls[0].x_offset - flow0).abs() < 0.01, "line 0 keeps its FLOW offset");
        assert_eq!(tls[0].justify_space, 0.0, "line 0 of a split is a mixed line: not justified");
        for (k, tl) in tls.iter().enumerate().take(n - 1).skip(1) {
            assert_eq!(tl.x_offset, 0.0, "middle line {k} sits at the origin");
            assert!(
                (justified_width(tl) - 200.0).abs() < 0.01,
                "middle line {k} '{}' must end at the container edge: {}",
                tl.text,
                justified_width(tl)
            );
        }
        assert_eq!(tls[n - 1].justify_space, 0.0, "the open last line is never justified");
    }

    #[test]
    fn test_text_nowrap_stays_single_line() {
        let mut style = ComputedStyle::new();
        style.white_space = rustkit_css::WhiteSpace::Nowrap;
        let mut layout_box = LayoutBox::new(BoxType::Text(LONG_TEXT.to_string()), style);
        layout_box.layout_text(LONG_TEXT.to_string(), &containing_200());

        assert!(layout_box.text_lines.is_none(), "nowrap must not wrap");
        assert_eq!(
            layout_box.dimensions.content.height,
            layout_box.get_line_height()
        );
    }

    #[test]
    fn test_short_text_does_not_wrap() {
        let style = ComputedStyle::new();
        let mut layout_box = LayoutBox::new(BoxType::Text("short".to_string()), style);
        layout_box.layout_text("short".to_string(), &containing_200());

        assert!(layout_box.text_lines.is_none());
        assert_eq!(
            layout_box.dimensions.content.height,
            layout_box.get_line_height()
        );
    }

    #[test]
    fn test_wrapped_lines_center_align() {
        // IFC Slice A contract: layout_text NEVER self-aligns (x_offset
        // stays 0); the parent's apply_text_align_offset is the sole owner
        // and sets each visual line's offset against the container.
        let mut style = ComputedStyle::new();
        style.text_align = TextAlign::Center;
        let mut layout_box = LayoutBox::new(BoxType::Text(LONG_TEXT.to_string()), style);
        layout_box.layout_text(LONG_TEXT.to_string(), &containing_200());

        for line in layout_box.text_lines.as_ref().expect("text should wrap") {
            assert_eq!(
                line.x_offset, 0.0,
                "leaf must not self-align (line '{}')",
                line.text
            );
        }

        let mut children = vec![layout_box];
        LayoutBox::apply_text_align_offset(&mut children, 200.0, 200.0, TextAlign::Center);
        for line in children[0].text_lines.as_ref().unwrap() {
            let expected = ((200.0 - line.width) / 2.0).max(0.0);
            assert!(
                (line.x_offset - expected).abs() < 0.01,
                "line '{}': x_offset {} != centered {}",
                line.text,
                line.x_offset,
                expected
            );
        }
    }

    #[test]
    fn test_lone_text_centers_via_parent_line_record() {
        // A block with ONE text child and text-align:center — the leaf takes
        // the block path (inline gate needs cursor_x > 0), so centering must
        // arrive via the parent's single-item line record. This is the h1
        // headline case on every gradient page.
        let mut parent_style = ComputedStyle::new();
        parent_style.text_align = TextAlign::Center;
        let mut parent = LayoutBox::new(BoxType::Block, parent_style);
        parent.dimensions.content = Rect::new(0.0, 0.0, 700.0, 0.0);

        let mut text_style = ComputedStyle::new();
        text_style.text_align = TextAlign::Center; // inherited, but leaf must ignore it
        text_style.font_size = Length::Px(32.0);
        parent.children.push(LayoutBox::new(
            BoxType::Text("Hello".to_string()),
            text_style,
        ));

        parent.layout_block_children(None);

        let t = &parent.children[0];
        let w = t.dimensions.content.width;
        let expected = (700.0 - w) / 2.0;
        assert!(
            (t.dimensions.content.x - expected).abs() < 1.0,
            "lone centered text should sit at ({expected}), got {} (w={w})",
            t.dimensions.content.x
        );
    }

    #[test]
    fn test_inline_line_shifts_as_unit() {
        // [Inline(bold), Text(" world")] under center: the recorded line
        // shifts as ONE unit — relative spacing preserved, and the inline
        // box's inner text moves WITH the box (subtree shift; the old
        // box-only shift left descendants behind once leaves stopped
        // self-aligning).
        let mut parent_style = ComputedStyle::new();
        parent_style.text_align = TextAlign::Center;
        let mut parent = LayoutBox::new(BoxType::Block, parent_style);
        parent.dimensions.content = Rect::new(0.0, 0.0, 600.0, 0.0);

        let mut b_style = ComputedStyle::new();
        b_style.display = rustkit_css::Display::Inline;
        b_style.font_weight = rustkit_css::FontWeight(700);
        let mut b = LayoutBox::new(BoxType::Inline, b_style);
        b.children.push(LayoutBox::new(
            BoxType::Text("bold".to_string()),
            ComputedStyle::new(),
        ));
        parent.children.push(b);
        parent.children.push(LayoutBox::new(
            BoxType::Text(" world".to_string()),
            ComputedStyle::new(),
        ));

        parent.layout_block_children(None);

        let b = &parent.children[0];
        let t = &parent.children[1];
        let line_w = b.dimensions.margin_box().width + t.dimensions.content.width;
        let expected_start = (600.0 - line_w) / 2.0;
        assert!(
            (b.dimensions.content.x - expected_start).abs() < 1.5,
            "line should start near {expected_start}, got {}",
            b.dimensions.content.x
        );
        // Inner text of the inline box moved with it (subtree shift).
        let inner = &b.children[0];
        assert!(
            (inner.dimensions.content.x - b.dimensions.content.x).abs() < 0.5,
            "inline box inner text must move with the box: box.x={} inner.x={}",
            b.dimensions.content.x,
            inner.dimensions.content.x
        );
        // And the following text sits right after the inline box.
        assert!(
            t.dimensions.content.x > b.dimensions.content.x,
            "second run must stay to the right of the first"
        );
    }

    /// IFC Slice B2 test rig: `[Text("Hi"), Text(LONG)]` in a 200px block.
    /// "Hi" joins the line at cursor 0; the long run does not fit the
    /// remainder and takes the phase-5 mid-line split.
    fn midline_split_parent(text_align: TextAlign) -> LayoutBox {
        let mut parent_style = ComputedStyle::new();
        parent_style.text_align = text_align;
        let mut parent = LayoutBox::new(BoxType::Block, parent_style);
        parent.dimensions.content = Rect::new(0.0, 0.0, 200.0, 0.0);
        parent.children.push(LayoutBox::new(
            BoxType::Text("Hi".to_string()),
            ComputedStyle::new(),
        ));
        parent.children.push(LayoutBox::new(
            BoxType::Text(LONG_TEXT.to_string()),
            ComputedStyle::new(),
        ));
        parent.layout_block_children(None);
        parent
    }

    #[test]
    fn a_long_first_run_keeps_its_last_line_open_for_the_next_sibling() {
        // CSS2 §9.4.2: inline content after a wrapped run continues on the
        // run's LAST line box. Before n49 a run at cursor 0 took the block
        // path, which closed all its lines, so the sibling started a new
        // line under it ("one span wraps to a different line" on
        // article-typography: `…used as an` / `<span>ornamental…`).
        let mut parent = LayoutBox::new(BoxType::Block, ComputedStyle::new());
        parent.dimensions.content = Rect::new(0.0, 0.0, 200.0, 0.0);
        parent.children.push(LayoutBox::new(
            BoxType::Text(LONG_TEXT.to_string()),
            ComputedStyle::new(),
        ));
        parent.children.push(LayoutBox::new(
            BoxType::Text("Hi".to_string()),
            ComputedStyle::new(),
        ));
        parent.layout_block_children(None);

        let run = &parent.children[0];
        let tls = run.text_lines.as_ref().expect("the long run wraps");
        let n = tls.len();
        assert!(n >= 2);
        assert_eq!(
            run.text_flow_first_offset,
            Some(0.0),
            "a run at the line start is a flow split with FLOW offset 0"
        );
        assert_eq!(tls[0].x_offset, 0.0);
        let lh = run.get_line_height();
        let last_top = run.dimensions.content.y + (n as f32 - 1.0) * lh;
        let last_w = tls[n - 1].width;
        let hi = &parent.children[1];
        assert!(
            (hi.dimensions.content.y - last_top).abs() < 0.01,
            "the sibling must sit on the run's last line (top {last_top}), got y={}",
            hi.dimensions.content.y
        );
        assert!(
            (hi.dimensions.content.x - last_w).abs() < 0.01,
            "the sibling must continue after the last line's ink ({last_w}), got x={}",
            hi.dimensions.content.x
        );
        assert!(
            (parent.dimensions.content.height - n as f32 * lh).abs() < 0.01,
            "the block is exactly n line boxes tall, got {}",
            parent.dimensions.content.height
        );
    }

    #[test]
    fn center_midline_split_line0_uses_flow_plus_align() {
        // Line 0 = "Hi" + the run's first visual line, centered as ONE
        // unit: the prior sibling shifts by O₀ and the first fragment gets
        // FLOW ⊕ ALIGN — never pure-centered as if it owned the line.
        let parent = midline_split_parent(TextAlign::Center);
        let hi_w = parent.children[0].dimensions.content.width;
        let t = &parent.children[1];
        let flow0 = t
            .text_flow_first_offset
            .expect("long run must be laid out mid-line (phase-5 split)");
        assert!(
            (flow0 - hi_w).abs() < 0.5,
            "FLOW offset must be the cursor at entry"
        );
        let tls = t
            .text_lines
            .as_ref()
            .expect("split run must have visual lines");
        assert!(
            tls.len() >= 2,
            "run must span multiple lines, got {}",
            tls.len()
        );
        let line0_w = flow0 + tls[0].width;
        let o0 = ((200.0 - line0_w) / 2.0).max(0.0);
        assert!(
            o0 > 1.0,
            "fixture must leave real centering slack (o0={o0})"
        );
        assert!(
            (parent.children[0].dimensions.content.x - o0).abs() < 0.5,
            "prior sibling must shift by O0={o0}, got x={}",
            parent.children[0].dimensions.content.x
        );
        assert!(
            (tls[0].x_offset - (flow0 + o0)).abs() < 0.5,
            "line-0 fragment must be FLOW+ALIGN ({}), got {}",
            flow0 + o0,
            tls[0].x_offset
        );
    }

    #[test]
    fn center_midline_split_does_not_stomp_earlier_text_lines() {
        // The close of the OPEN last line must touch only the last visual
        // line; line 0 keeps FLOW⊕ALIGN and middles keep pure per-line
        // centering.
        let parent = midline_split_parent(TextAlign::Center);
        let t = &parent.children[1];
        let flow0 = t.text_flow_first_offset.unwrap();
        let tls = t.text_lines.as_ref().unwrap();
        let n = tls.len();
        assert!(n >= 3, "need line0 + middle + last, got {n}");
        let o0 = ((200.0 - (flow0 + tls[0].width)) / 2.0).max(0.0);
        assert!(
            (tls[0].x_offset - (flow0 + o0)).abs() < 0.5,
            "line 0 stomped: expected {}, got {}",
            flow0 + o0,
            tls[0].x_offset
        );
        for (k, tl) in tls.iter().enumerate().take(n - 1).skip(1) {
            let pure = ((200.0 - tl.width) / 2.0).max(0.0);
            assert!(
                (tl.x_offset - pure).abs() < 0.5,
                "middle line {k} must center independently: expected {pure}, got {}",
                tl.x_offset
            );
        }
        let pure_last = ((200.0 - tls[n - 1].width) / 2.0).max(0.0);
        assert!(
            (tls[n - 1].x_offset - pure_last).abs() < 0.5,
            "last line (no followers) must center on its own width: expected {pure_last}, got {}",
            tls[n - 1].x_offset
        );
    }

    #[test]
    fn left_midline_split_offsets_unchanged() {
        // Phase-5 regression guard: under Left the FLOW offsets are final —
        // line 0 starts at the cursor, every other line at the origin, and
        // the prior sibling never moves.
        let parent = midline_split_parent(TextAlign::Left);
        assert_eq!(parent.children[0].dimensions.content.x, 0.0);
        let t = &parent.children[1];
        let flow0 = t.text_flow_first_offset.unwrap();
        let tls = t.text_lines.as_ref().unwrap();
        assert!(tls.len() >= 2);
        assert!(
            (tls[0].x_offset - flow0).abs() < 0.01,
            "line 0 must keep its FLOW offset {flow0}, got {}",
            tls[0].x_offset
        );
        for (k, tl) in tls.iter().enumerate().skip(1) {
            assert_eq!(tl.x_offset, 0.0, "line {k} must sit at the flow origin");
        }
    }

    #[test]
    fn right_midline_split_line_ends_at_container() {
        // Every closed visual line's ink must end at the container's right
        // edge; the open last line right-aligns on its own width.
        let parent = midline_split_parent(TextAlign::Right);
        let t = &parent.children[1];
        let tls = t.text_lines.as_ref().unwrap();
        let n = tls.len();
        assert!(n >= 2);
        // Line 0 ends at the container edge (prior sibling + fragment
        // shifted together, so the fragment's own right edge is the line's).
        assert!(
            (tls[0].x_offset + tls[0].width - 200.0).abs() < 0.5,
            "line 0 must end at 200, ends at {}",
            tls[0].x_offset + tls[0].width
        );
        for (k, tl) in tls.iter().enumerate().skip(1) {
            assert!(
                (tl.x_offset + tl.width - 200.0).abs() < 0.5,
                "line {k} must end at 200, ends at {}",
                tl.x_offset + tl.width
            );
        }
        // The prior sibling sits immediately left of line-0's fragment.
        let hi = &parent.children[0];
        assert!(
            (hi.dimensions.content.x + hi.dimensions.content.width - tls[0].x_offset).abs() < 1.0,
            "prior sibling must abut the first fragment"
        );
    }

    #[test]
    fn test_image_max_width_preserves_aspect() {
        // 100x100 natural; max-width: 80px → 80x80 (CSS 2.1 §10.4)
        let mut style = ComputedStyle::new();
        style.max_width = Length::Px(80.0);
        let mut layout_box = LayoutBox::new(
            BoxType::Image {
                url: String::new(),
                natural_width: 100.0,
                natural_height: 100.0,
            },
            style,
        );
        layout_box.layout_image(100.0, 100.0, &containing_1000());
        assert_eq!(layout_box.dimensions.content.width, 80.0);
        assert_eq!(layout_box.dimensions.content.height, 80.0);
    }

    #[test]
    fn test_image_max_height_preserves_aspect() {
        // 200x100 natural; max-height: 60px → 120x60
        let mut style = ComputedStyle::new();
        style.max_height = Length::Px(60.0);
        let mut layout_box = LayoutBox::new(
            BoxType::Image {
                url: String::new(),
                natural_width: 200.0,
                natural_height: 100.0,
            },
            style,
        );
        layout_box.layout_image(200.0, 100.0, &containing_1000());
        assert_eq!(layout_box.dimensions.content.width, 120.0);
        assert_eq!(layout_box.dimensions.content.height, 60.0);
    }

    #[test]
    fn test_image_max_constraints_ignore_unconstrained() {
        // Natural size within both maxima → untouched
        let mut style = ComputedStyle::new();
        style.max_width = Length::Px(500.0);
        style.max_height = Length::Px(500.0);
        let mut layout_box = LayoutBox::new(
            BoxType::Image {
                url: String::new(),
                natural_width: 100.0,
                natural_height: 100.0,
            },
            style,
        );
        layout_box.layout_image(100.0, 100.0, &containing_1000());
        assert_eq!(layout_box.dimensions.content.width, 100.0);
        assert_eq!(layout_box.dimensions.content.height, 100.0);
    }

    /// A `.test-img` from `websuite/micro/images-intrinsic`: 1px border, and
    /// the page's `* { box-sizing: border-box }`.
    fn bordered_image_style(box_sizing: BoxSizing) -> ComputedStyle {
        let mut style = ComputedStyle::new();
        style.box_sizing = box_sizing;
        style.border_top_width = Length::Px(1.0);
        style.border_right_width = Length::Px(1.0);
        style.border_bottom_width = Length::Px(1.0);
        style.border_left_width = Length::Px(1.0);
        style
    }

    fn image_box(style: ComputedStyle) -> LayoutBox {
        LayoutBox::new(
            BoxType::Image {
                url: String::new(),
                natural_width: 100.0,
                natural_height: 100.0,
            },
            style,
        )
    }

    #[test]
    fn an_image_at_its_natural_size_gains_its_border() {
        // images-intrinsic test1: 100x100 natural, `border: 1px solid red`,
        // no specified size. Chrome's border box is 102x102 — an `auto` size
        // is the intrinsic CONTENT size and the border adds outside it, in
        // border-box mode too.
        let mut layout_box = image_box(bordered_image_style(BoxSizing::BorderBox));
        layout_box.layout_image(100.0, 100.0, &containing_1000());
        assert_eq!(layout_box.dimensions.content.width, 100.0);
        assert_eq!(layout_box.dimensions.content.height, 100.0);
        assert_eq!(layout_box.dimensions.border_box().width, 102.0);
        assert_eq!(layout_box.dimensions.border_box().height, 102.0);
    }

    #[test]
    fn a_border_box_image_takes_its_border_out_of_a_specified_size() {
        // images-intrinsic test4: `width: 150px; height: 75px` under
        // border-box. Chrome's border box is 150x75, content 148x73.
        let mut style = bordered_image_style(BoxSizing::BorderBox);
        style.width = Length::Px(150.0);
        style.height = Length::Px(75.0);
        let mut layout_box = image_box(style);
        layout_box.layout_image(100.0, 100.0, &containing_1000());
        assert_eq!(layout_box.dimensions.content.width, 148.0);
        assert_eq!(layout_box.dimensions.content.height, 73.0);
        assert_eq!(layout_box.dimensions.border_box().width, 150.0);
        assert_eq!(layout_box.dimensions.border_box().height, 75.0);
    }

    #[test]
    fn the_horizontal_and_vertical_decorations_are_not_interchangeable() {
        // Every other fixture here is symmetric, so a version of this code
        // that took the border out of the wrong axis would pass all of them.
        // 4px of horizontal border, 6px of vertical: `width: 150px` leaves a
        // 146px content box and `height: 75px` a 69px one.
        let mut style = ComputedStyle::new();
        style.box_sizing = BoxSizing::BorderBox;
        style.border_left_width = Length::Px(1.0);
        style.border_right_width = Length::Px(3.0);
        style.border_top_width = Length::Px(2.0);
        style.border_bottom_width = Length::Px(4.0);
        style.width = Length::Px(150.0);
        style.height = Length::Px(75.0);
        let mut layout_box = image_box(style);
        layout_box.layout_image(100.0, 100.0, &containing_1000());
        assert_eq!(layout_box.dimensions.content.width, 146.0);
        assert_eq!(layout_box.dimensions.content.height, 69.0);
        assert_eq!(layout_box.dimensions.border_box().width, 150.0);
        assert_eq!(layout_box.dimensions.border_box().height, 75.0);
        // …and the content box sits inside the LEFT/TOP border, not the mean.
        // `containing_1000()` carries a 600px flow cursor, so y is 600 + 2.
        assert_eq!(layout_box.dimensions.content.x, 1.0);
        assert_eq!(layout_box.dimensions.content.y, 602.0);
    }

    #[test]
    fn a_content_box_image_keeps_its_border_outside_a_specified_size() {
        // The other half of the same rule: under the initial `content-box`,
        // `width: 150px` IS the content box and the border box is 152.
        let mut style = bordered_image_style(BoxSizing::ContentBox);
        style.width = Length::Px(150.0);
        style.height = Length::Px(75.0);
        let mut layout_box = image_box(style);
        layout_box.layout_image(100.0, 100.0, &containing_1000());
        assert_eq!(layout_box.dimensions.content.width, 150.0);
        assert_eq!(layout_box.dimensions.content.height, 75.0);
        assert_eq!(layout_box.dimensions.border_box().width, 152.0);
        assert_eq!(layout_box.dimensions.border_box().height, 77.0);
    }

    #[test]
    fn a_border_box_image_max_width_names_the_border_box() {
        // images-intrinsic test5: `max-width: 80px` on a 100x100 natural
        // image under border-box. Chrome clamps the BORDER box to 80, so the
        // content box is 78 and the aspect ratio holds on 78, not 80.
        let mut style = bordered_image_style(BoxSizing::BorderBox);
        style.max_width = Length::Px(80.0);
        let mut layout_box = image_box(style);
        layout_box.layout_image(100.0, 100.0, &containing_1000());
        assert_eq!(layout_box.dimensions.content.width, 78.0);
        assert_eq!(layout_box.dimensions.content.height, 78.0);
        assert_eq!(layout_box.dimensions.border_box().width, 80.0);
        assert_eq!(layout_box.dimensions.border_box().height, 80.0);
    }

    #[test]
    fn an_images_border_box_starts_at_the_containing_blocks_content_edge() {
        // The border box is what Chrome's rects are keyed on, so the offsets
        // matter as much as the sizes: with 1px border and 4px padding the
        // content box sits 5px in, and the BORDER box still starts where the
        // containing block's content does.
        let mut style = bordered_image_style(BoxSizing::BorderBox);
        style.padding_left = Length::Px(4.0);
        style.padding_top = Length::Px(4.0);
        style.padding_right = Length::Px(4.0);
        style.padding_bottom = Length::Px(4.0);
        let mut cb = Dimensions::default();
        cb.content = Rect::new(32.0, 40.0, 1000.0, 0.0);
        let mut layout_box = image_box(style);
        layout_box.layout_image(100.0, 100.0, &cb);
        assert_eq!(layout_box.dimensions.content.x, 37.0);
        assert_eq!(layout_box.dimensions.content.y, 45.0);
        assert_eq!(layout_box.dimensions.border_box().x, 32.0);
        assert_eq!(layout_box.dimensions.border_box().y, 40.0);
        assert_eq!(layout_box.dimensions.border_box().width, 110.0);
    }

    #[test]
    fn an_undecorated_image_is_unchanged_by_the_border_box_rule() {
        // The overwhelming majority of images in the corpus carry no border
        // or padding at all; for them content box and border box coincide and
        // nothing about this path may move.
        let mut style = ComputedStyle::new();
        style.box_sizing = BoxSizing::BorderBox;
        style.width = Length::Px(200.0);
        let mut layout_box = image_box(style);
        layout_box.layout_image(100.0, 100.0, &containing_1000());
        assert_eq!(layout_box.dimensions.content.width, 200.0);
        assert_eq!(layout_box.dimensions.border_box().width, 200.0);
        assert_eq!(layout_box.dimensions.border_box().height, 200.0);
    }

    /// Every expected number in the `aspect_ratio` tests below was MEASURED
    /// against Chrome 148 on this seat (bundled chromium-1194), not derived:
    /// a 100x100 natural image with `border: 1px solid red` unless the test
    /// says otherwise. Chrome's rows are quoted in each test.
    fn ratio_image_box(box_sizing: BoxSizing, ratio: f32) -> LayoutBox {
        let mut style = bordered_image_style(box_sizing);
        style.aspect_ratio = Some(ratio);
        image_box(style)
    }

    /// 3px left/right and 5px top/bottom borders. Night 19's M5 survived every
    /// symmetric fixture in this file, so at least one aspect-ratio fixture
    /// has to be able to tell the two axes' decoration apart.
    fn asymmetric_ratio_style(box_sizing: BoxSizing, ratio: f32) -> ComputedStyle {
        let mut style = ComputedStyle::new();
        style.box_sizing = box_sizing;
        style.aspect_ratio = Some(ratio);
        style.border_left_width = Length::Px(3.0);
        style.border_right_width = Length::Px(3.0);
        style.border_top_width = Length::Px(5.0);
        style.border_bottom_width = Length::Px(5.0);
        style
    }

    #[test]
    fn a_specified_aspect_ratio_replaces_the_natural_one() {
        // images-intrinsic test11: `width: 160px; aspect-ratio: 16/9` on a
        // square image. Chrome: border box 160.0000 x 90.0000.
        // Before 2026-08-23 this built 160x160 — `style.aspect_ratio` was
        // parsed and never reached a replaced element.
        let mut layout_box = ratio_image_box(BoxSizing::BorderBox, 16.0 / 9.0);
        layout_box.style.width = Length::Px(160.0);
        layout_box.layout_image(100.0, 100.0, &containing_1000());
        assert_eq!(layout_box.dimensions.border_box().width, 160.0);
        assert_eq!(layout_box.dimensions.border_box().height, 90.0);
        assert_eq!(layout_box.dimensions.content.width, 158.0);
        assert_eq!(layout_box.dimensions.content.height, 88.0);
    }

    #[test]
    fn the_ratio_spans_the_box_named_by_box_sizing() {
        // Same declaration under `content-box`. Chrome: border box
        // 162.0000 x 92.0000 — the ratio spans the CONTENT box here, so the
        // border adds outside it. A ratio that always spanned the content box
        // would build the border-box case above 90.875 tall instead of 90.
        let mut layout_box = ratio_image_box(BoxSizing::ContentBox, 16.0 / 9.0);
        layout_box.style.width = Length::Px(160.0);
        layout_box.layout_image(100.0, 100.0, &containing_1000());
        assert_eq!(layout_box.dimensions.content.width, 160.0);
        assert_eq!(layout_box.dimensions.content.height, 90.0);
        assert_eq!(layout_box.dimensions.border_box().width, 162.0);
        assert_eq!(layout_box.dimensions.border_box().height, 92.0);
    }

    #[test]
    fn a_specified_height_derives_the_width_across_the_ratio() {
        // `height: 90px; aspect-ratio: 16/9`, border-box. Chrome: 160 x 90.
        // The ratio has to work in both directions, not just width -> height.
        let mut layout_box = ratio_image_box(BoxSizing::BorderBox, 16.0 / 9.0);
        layout_box.style.height = Length::Px(90.0);
        layout_box.layout_image(100.0, 100.0, &containing_1000());
        assert_eq!(layout_box.dimensions.border_box().width, 160.0);
        assert_eq!(layout_box.dimensions.border_box().height, 90.0);
    }

    #[test]
    fn two_specified_sizes_outrank_the_aspect_ratio() {
        // `width: 160px; height: 200px; aspect-ratio: 16/9`, border-box.
        // Chrome: 160 x 200 — the ratio does not get a vote.
        let mut layout_box = ratio_image_box(BoxSizing::BorderBox, 16.0 / 9.0);
        layout_box.style.width = Length::Px(160.0);
        layout_box.style.height = Length::Px(200.0);
        layout_box.layout_image(100.0, 100.0, &containing_1000());
        assert_eq!(layout_box.dimensions.border_box().width, 160.0);
        assert_eq!(layout_box.dimensions.border_box().height, 200.0);
    }

    #[test]
    fn an_auto_sized_image_keeps_its_natural_width_and_takes_the_ratio_height() {
        // `aspect-ratio: 16/9` with no specified size at all. Chrome:
        // border-box  102.0000 x 57.3750   (ratio spans the border box)
        // content-box 102.0000 x 58.2500   (ratio spans the content box)
        // The natural HEIGHT is discarded in both — that is what "replaces the
        // natural ratio" means, and it is the branch that separates this from
        // a rule that only fires when a size is specified.
        let mut bb = ratio_image_box(BoxSizing::BorderBox, 16.0 / 9.0);
        bb.layout_image(100.0, 100.0, &containing_1000());
        assert_eq!(bb.dimensions.border_box().width, 102.0);
        assert_eq!(bb.dimensions.border_box().height, 57.375);

        let mut cb = ratio_image_box(BoxSizing::ContentBox, 16.0 / 9.0);
        cb.layout_image(100.0, 100.0, &containing_1000());
        assert_eq!(cb.dimensions.border_box().width, 102.0);
        assert_eq!(cb.dimensions.border_box().height, 58.25);
    }

    #[test]
    fn the_ratio_takes_each_axis_own_decoration() {
        // 3px horizontal and 5px vertical borders, `aspect-ratio: 2/1`.
        // Chrome, all three specified/auto combinations:
        //   border-box  width:160px  -> 160 x 80   (content 154 x 70)
        //   content-box width:160px  -> 166 x 90   (content 160 x 80)
        //   border-box  height:80px  -> 160 x 80   (content 154 x 70)
        //   border-box  auto         -> 106 x 53   (content 100 x 43)
        // Swapping the two decorations passes every symmetric fixture above
        // and fails all four of these.
        let mut w_bb = image_box(asymmetric_ratio_style(BoxSizing::BorderBox, 2.0));
        w_bb.style.width = Length::Px(160.0);
        w_bb.layout_image(100.0, 100.0, &containing_1000());
        assert_eq!(w_bb.dimensions.content.width, 154.0);
        assert_eq!(w_bb.dimensions.content.height, 70.0);

        let mut w_cb = image_box(asymmetric_ratio_style(BoxSizing::ContentBox, 2.0));
        w_cb.style.width = Length::Px(160.0);
        w_cb.layout_image(100.0, 100.0, &containing_1000());
        assert_eq!(w_cb.dimensions.content.width, 160.0);
        assert_eq!(w_cb.dimensions.content.height, 80.0);

        let mut h_bb = image_box(asymmetric_ratio_style(BoxSizing::BorderBox, 2.0));
        h_bb.style.height = Length::Px(80.0);
        h_bb.layout_image(100.0, 100.0, &containing_1000());
        assert_eq!(h_bb.dimensions.content.width, 154.0);
        assert_eq!(h_bb.dimensions.content.height, 70.0);

        let mut auto_bb = image_box(asymmetric_ratio_style(BoxSizing::BorderBox, 2.0));
        auto_bb.layout_image(100.0, 100.0, &containing_1000());
        assert_eq!(auto_bb.dimensions.content.width, 100.0);
        assert_eq!(auto_bb.dimensions.content.height, 43.0);
    }

    #[test]
    fn an_absent_or_degenerate_ratio_leaves_the_natural_one_alone() {
        // The eleven sized tests on images-intrinsic and every other image in
        // the corpus have no `aspect-ratio` at all; none of them may move.
        assert_eq!(
            preferred_ratio_sizes(Some(158.0), None, Some(100.0), None, 2.0, 2.0, true),
            (Some(158.0), None)
        );
        // A ratio that cannot be divided by is not a ratio. `auto` already
        // parses to `None`; these are the values a bad declaration can reach.
        for bad in [0.0, -2.0, f32::NAN, f32::INFINITY] {
            assert_eq!(
                preferred_ratio_sizes(Some(158.0), None, Some(100.0), Some(bad), 2.0, 2.0, true),
                (Some(158.0), None),
                "ratio {bad} must be ignored, not divided by"
            );
        }
        // No specified size and no natural width: nothing to cross the ratio
        // from, so both stay absent rather than becoming zero.
        assert_eq!(
            preferred_ratio_sizes(None, None, None, Some(2.0), 2.0, 2.0, true),
            (None, None)
        );
    }

    #[test]
    fn a_ratio_narrower_than_its_own_decoration_floors_at_zero() {
        // 6px border box, 20px of vertical decoration, ratio 1: the derived
        // border box is 6px and giving 20px back is -14. `replaced_content_size`
        // floors the same way for the same reason — a negative content box is
        // read as a size by everything downstream.
        assert_eq!(
            ratio_cross_content_size(2.0, 4.0, 20.0, 1.0, true, true),
            0.0
        );
        // Nothing to floor in the ordinary case, and content-box never
        // subtracts at all.
        assert_eq!(
            ratio_cross_content_size(158.0, 2.0, 2.0, 16.0 / 9.0, true, true),
            88.0
        );
        assert_eq!(
            ratio_cross_content_size(160.0, 2.0, 2.0, 16.0 / 9.0, true, false),
            90.0
        );
    }

    #[test]
    fn an_auto_size_is_never_reduced_by_the_decoration() {
        // `replaced_content_size` must leave `None` alone: an absent size is
        // the intrinsic one, and subtracting the border from it is exactly
        // the mirror-image bug (an image at natural size measuring 98).
        assert_eq!(replaced_content_size(None, 2.0, true), None);
        assert_eq!(replaced_content_size(Some(200.0), 2.0, true), Some(198.0));
        assert_eq!(replaced_content_size(Some(200.0), 2.0, false), Some(200.0));
        // Decoration wider than the specified border box floors at zero
        // rather than producing a negative content box.
        assert_eq!(replaced_content_size(Some(1.0), 4.0, true), Some(0.0));
    }

    #[test]
    fn test_auto_margins_center_max_width_clamped() {
        // width: auto; max-width: 600px; margin: 0 auto → clamped then centered
        let mut style = ComputedStyle::new();
        style.width = Length::Auto;
        style.max_width = Length::Px(600.0);
        style.margin_left = Length::Auto;
        style.margin_right = Length::Auto;
        let mut layout_box = LayoutBox::new(BoxType::Block, style);
        layout_box.calculate_block_width(&containing_1000());
        assert_eq!(layout_box.dimensions.content.width, 600.0);
        assert_eq!(layout_box.dimensions.margin.left, 200.0);
        assert_eq!(layout_box.dimensions.margin.right, 200.0);
    }

    #[test]
    fn test_auto_margin_single_side_absorbs_free_space() {
        // width: 600px; margin-left: auto; margin-right: 100px
        let mut style = ComputedStyle::new();
        style.width = Length::Px(600.0);
        style.margin_left = Length::Auto;
        style.margin_right = Length::Px(100.0);
        let mut layout_box = LayoutBox::new(BoxType::Block, style);
        layout_box.calculate_block_width(&containing_1000());
        assert_eq!(layout_box.dimensions.margin.left, 300.0);
        assert_eq!(layout_box.dimensions.margin.right, 100.0);
    }

    #[test]
    fn test_auto_margins_zero_when_width_auto() {
        // width: auto fills the containing block; auto margins get nothing
        let mut style = ComputedStyle::new();
        style.width = Length::Auto;
        style.margin_left = Length::Auto;
        style.margin_right = Length::Auto;
        let mut layout_box = LayoutBox::new(BoxType::Block, style);
        layout_box.calculate_block_width(&containing_1000());
        assert_eq!(layout_box.dimensions.content.width, 1000.0);
        assert_eq!(layout_box.dimensions.margin.left, 0.0);
        assert_eq!(layout_box.dimensions.margin.right, 0.0);
    }

    #[test]
    fn test_text_align_center_shifts_inline_box() {
        // Regression: text-align:center must move an inline box (a styled
        // <span>/<a>, so its background/border/padding follow the text) — not
        // only inline-block items. Plain text already self-centers in
        // layout_text; this covers the box origin. (2026-07-08, macOS trench)
        let mut cb = Dimensions::default();
        cb.content = Rect::new(0.0, 0.0, 200.0, 0.0);

        let mut block_style = ComputedStyle::new();
        block_style.text_align = TextAlign::Center;

        // Plain text child stays centered (baseline behavior, must not regress).
        let mut block = LayoutBox::new(BoxType::Block, block_style.clone());
        let mut text_style = ComputedStyle::new();
        text_style.text_align = TextAlign::Center;
        block
            .children
            .push(LayoutBox::new(BoxType::Text("Hi".to_string()), text_style));
        block.layout(&cb);
        let text_x = block.children[0].dimensions.content.x;
        let text_w = block.children[0].dimensions.content.width;
        assert!(
            (text_x - (200.0 - text_w) / 2.0).abs() < 0.5,
            "plain text should stay centered, got x={text_x} w={text_w}"
        );

        // Inline span with an explicit 50px width is centered: (200-50)/2 = 75.
        let mut span_style = ComputedStyle::new();
        span_style.text_align = TextAlign::Center;
        span_style.width = Length::Px(50.0);
        let mut block2 = LayoutBox::new(BoxType::Block, block_style.clone());
        block2
            .children
            .push(LayoutBox::new(BoxType::Inline, span_style));
        block2.layout(&cb);
        assert_eq!(
            block2.children[0].dimensions.content.x, 75.0,
            "inline box should be centered"
        );

        // Realistic <div center><span>Hi</span></div>: the span box and its
        // inner text end up centered about the same point (no double-shift).
        let mut span3 = LayoutBox::new(BoxType::Inline, {
            let mut s = ComputedStyle::new();
            s.text_align = TextAlign::Center;
            s
        });
        let mut inner_style = ComputedStyle::new();
        inner_style.text_align = TextAlign::Center;
        span3
            .children
            .push(LayoutBox::new(BoxType::Text("Hi".to_string()), inner_style));
        let mut block3 = LayoutBox::new(BoxType::Block, block_style.clone());
        block3.children.push(span3);
        block3.layout(&cb);
        let span3 = &block3.children[0];
        let span3_center = span3.dimensions.content.x + span3.dimensions.content.width / 2.0;
        let inner = &span3.children[0];
        let inner_center = inner.dimensions.content.x + inner.dimensions.content.width / 2.0;
        assert!(
            (span3_center - inner_center).abs() < 1.0 && (span3_center - 100.0).abs() < 1.0,
            "span box and inner text should share a center near 100, got box_center={span3_center} text_center={inner_center}"
        );
    }

    #[test]
    fn test_inline_flex_children_share_a_line() {
        // Regression: display:inline-flex is an atomic inline (CSS Display 3
        // §2.4) — siblings flow on one line like inline-block, they don't
        // stack as blocks. Was: flow classification used is_inline_block(),
        // so every inline-flex child got its own line and containers grew
        // ~2x tall, landing bottom borders far below Chrome's.
        // (2026-07-09, macOS trench session 8)
        let mut cb = Dimensions::default();
        cb.content = Rect::new(0.0, 0.0, 500.0, 0.0);

        let mut parent = LayoutBox::new(BoxType::Block, ComputedStyle::new());
        for _ in 0..3 {
            let mut s = ComputedStyle::new();
            s.display = rustkit_css::Display::InlineFlex;
            s.width = Length::Px(100.0);
            s.height = Length::Px(40.0);
            parent.children.push(LayoutBox::new(BoxType::Block, s));
        }
        parent.layout(&cb);

        let ys: Vec<f32> = parent
            .children
            .iter()
            .map(|c| c.dimensions.content.y)
            .collect();
        let xs: Vec<f32> = parent
            .children
            .iter()
            .map(|c| c.dimensions.content.x)
            .collect();
        assert!(
            ys[0] == ys[1] && ys[1] == ys[2],
            "inline-flex siblings must share a line, got ys={ys:?}"
        );
        assert_eq!(
            xs,
            vec![0.0, 100.0, 200.0],
            "boxes should advance horizontally"
        );
        // One line tall = box height + strut descent below the baseline:
        // empty atomic inlines sit ON the baseline (CSS2 §10.8.1), so the
        // strut's descent extends the line box under them — Chrome does the
        // same (a lone 40px inline-block makes its container ~45px tall).
        let expected = 40.0 + parent.inline_strut_descent();
        assert!(
            (parent.dimensions.content.height - expected).abs() < 0.5,
            "parent should be one line tall ({expected}px incl. strut descent), got {}",
            parent.dimensions.content.height
        );

        // Same contract through the margin-collapse path the engine uses.
        let mut parent2 = LayoutBox::new(BoxType::Block, ComputedStyle::new());
        for _ in 0..3 {
            let mut s = ComputedStyle::new();
            s.display = rustkit_css::Display::InlineFlex;
            s.width = Length::Px(100.0);
            s.height = Length::Px(40.0);
            parent2.children.push(LayoutBox::new(BoxType::Block, s));
        }
        let mut mc = MarginCollapseContext::new();
        let mut fc = FloatContext::new();
        parent2.layout_with_collapse(&cb, &mut mc, &mut fc);
        let ys2: Vec<f32> = parent2
            .children
            .iter()
            .map(|c| c.dimensions.content.y)
            .collect();
        assert!(
            ys2[0] == ys2[1] && ys2[1] == ys2[2],
            "inline-flex siblings must share a line under margin collapse, got ys={ys2:?}"
        );
    }

    #[test]
    fn test_inline_block_border_box_position_and_line_strut() {
        // Two bugs found via the backgrounds y-table (macOS trench, 2026-07-10):
        // 1) a decorated inline-block's content rect was placed where its
        //    BORDER box belongs — border+padding painted up-left of Chrome.
        // 2) a wrapped line of empty inline-blocks advanced by margin-box
        //    height only; Chrome adds the strut's descent below the baseline
        //    (126px rows vs RustKit's 120px on the backgrounds page).
        let mut cb = Dimensions::default();
        cb.content = Rect::new(0.0, 0.0, 460.0, 0.0);

        let mk = || {
            let mut s = ComputedStyle::new();
            s.display = rustkit_css::Display::InlineBlock;
            s.width = Length::Px(200.0);
            s.height = Length::Px(100.0);
            s.margin_top = Length::Px(10.0);
            s.margin_bottom = Length::Px(10.0);
            s.margin_left = Length::Px(10.0);
            s.margin_right = Length::Px(10.0);
            s.border_top_width = Length::Px(2.0);
            s.border_bottom_width = Length::Px(2.0);
            s.border_left_width = Length::Px(2.0);
            s.border_right_width = Length::Px(2.0);
            LayoutBox::new(BoxType::Block, s)
        };
        // 460px container: two 224px margin boxes fit, the third wraps.
        let mut parent = LayoutBox::new(BoxType::Block, ComputedStyle::new());
        for _ in 0..3 {
            parent.children.push(mk());
        }
        parent.layout(&cb);

        // Border box = content minus border: starts at margin offset (10,10),
        // so content sits at margin + border = (12,12).
        let first = &parent.children[0].dimensions;
        assert_eq!(
            (first.content.x, first.content.y),
            (12.0, 12.0),
            "content rect must sit border-width inside the margin-box cursor"
        );
        assert!(
            (first.border_box().x - 10.0).abs() < 0.01
                && (first.border_box().y - 10.0).abs() < 0.01,
            "border box must start at the margin offset, got ({}, {})",
            first.border_box().x,
            first.border_box().y
        );

        // Row 2 starts one full line box down: margin-box height (124) plus
        // the strut descent below the empty inline-blocks' baseline.
        let sd = parent.inline_strut_descent();
        assert!(sd > 0.0, "strut descent must be positive");
        let second_row = &parent.children[2].dimensions;
        let expected_y = 124.0 + sd + 10.0 + 2.0;
        assert!(
            (second_row.content.y - expected_y).abs() < 0.5,
            "wrapped row must advance by line height incl. strut descent; expected content.y {expected_y}, got {}",
            second_row.content.y
        );
    }

    /// n53 fixtures: a 16px/24px row (the form-controls `.row`), a 12px
    /// inline-block `<label>` of the given text, whitespace, and a bare
    /// text input — the page's `label + input` idiom.
    fn n53_row_style() -> ComputedStyle {
        let mut s = ComputedStyle::new();
        s.font_family = "system-ui".to_string();
        s.font_size = Length::Px(16.0);
        s.line_height = rustkit_css::LineHeight::Px(24.0);
        s
    }

    fn n53_label(text: &str) -> LayoutBox {
        let mut s = n53_row_style();
        s.display = rustkit_css::Display::InlineBlock;
        s.width = Length::Px(120.0);
        s.font_size = Length::Px(12.0);
        s.line_height = rustkit_css::LineHeight::Px(18.0);
        let mut label = LayoutBox::new(BoxType::Block, s.clone());
        let mut text_style = s;
        text_style.display = rustkit_css::Display::Inline;
        text_style.width = Length::Auto;
        label
            .children
            .push(LayoutBox::new(BoxType::Text(text.to_string()), text_style));
        label
    }

    /// A bare control as the engine's UA arm styles it: inline-block, the
    /// 13.333px Arial control font, the page's inherited line-height 1.5.
    fn n53_control(control: FormControlType) -> LayoutBox {
        let mut s = ComputedStyle::new();
        s.display = rustkit_css::Display::InlineBlock;
        s.font_family = "Arial".to_string();
        s.font_size = Length::Px(13.333);
        s.line_height = rustkit_css::LineHeight::Number(1.5);
        LayoutBox::new(BoxType::FormControl(control), s)
    }

    fn n53_text_input() -> LayoutBox {
        n53_control(FormControlType::TextInput {
            value: String::new(),
            placeholder: "Default size".to_string(),
            input_type: "text".to_string(),
        })
    }

    #[test]
    fn inline_block_with_text_sits_on_its_last_line_baseline() {
        // form-controls (n53): `label { display: inline-block; width: 120px;
        // font-size: 12px }` beside a bare input in a 16px/24px row. Chrome
        // 148: row 24 tall, label top at +5 (its 12px text's baseline meets
        // the line baseline), input top at +4. RustKit placed the label at
        // the row top (+0) on all twelve rows of the page — the alignment
        // pass skipped every inline-block that had children.
        let mut cb = Dimensions::default();
        cb.content = Rect::new(0.0, 0.0, 736.0, 0.0);
        let mut row = LayoutBox::new(BoxType::Block, n53_row_style());
        row.children.push(n53_label("Text:"));
        row.children.push(LayoutBox::new(
            BoxType::Text(" ".to_string()),
            n53_row_style(),
        ));
        row.children.push(n53_text_input());
        row.layout(&cb);

        let row_top = row.dimensions.content.y;
        let label_top = row.children[0].dimensions.border_box().y - row_top;
        let input_top = row.children[2].dimensions.border_box().y - row_top;
        assert!(
            (label_top - 5.0).abs() <= 1.5,
            "label must sit on the line baseline: Chrome +5, got +{label_top}"
        );
        assert!(
            (input_top - 4.0).abs() <= 2.0,
            "input keeps its hang-model seat: Chrome +4, got +{input_top}"
        );
        assert!(
            (row.dimensions.content.height - 24.0).abs() <= 1.0,
            "row stays one 24px line, got {}",
            row.dimensions.content.height
        );
    }

    #[test]
    fn a_line_with_no_text_member_still_carries_the_strut() {
        // n54: `<label>Text:</label><input>` with NO whitespace between — the
        // same row as above without the text node that used to be the only
        // way the strut reached a line. CSS2 §10.8.1: every line box starts
        // with the container's strut, and its height is the largest extent
        // above the baseline plus the largest below, not the tallest member.
        // Chrome 148: 24; the tallest-member advance built 19 (the input).
        let mut cb = Dimensions::default();
        cb.content = Rect::new(0.0, 0.0, 736.0, 0.0);
        let mut row = LayoutBox::new(BoxType::Block, n53_row_style());
        row.children.push(n53_label("Text:"));
        row.children.push(n53_text_input());
        row.layout(&cb);

        assert!(
            (row.dimensions.content.height - 24.0).abs() <= 1.0,
            "an inline-block-only row is still one 24px line, got {}",
            row.dimensions.content.height
        );
        let mut with_ws = LayoutBox::new(BoxType::Block, n53_row_style());
        with_ws.children.push(n53_label("Text:"));
        with_ws.children.push(LayoutBox::new(
            BoxType::Text(" ".to_string()),
            n53_row_style(),
        ));
        with_ws.children.push(n53_text_input());
        with_ws.layout(&cb);
        assert!(
            (row.dimensions.content.height - with_ws.dimensions.content.height).abs() <= 0.01,
            "whitespace between the members must not change the line's height: {} vs {}",
            row.dimensions.content.height,
            with_ws.dimensions.content.height
        );
    }

    #[test]
    fn a_line_sums_whole_pixel_ascents_like_blink() {
        // form-controls §5 (n54): the row whose second label wraps to two
        // 18px lines. The line's ascent is that label's last-line baseline,
        // 18 + 13, and the 16px/24px strut hangs 6 below: Chrome 148 builds
        // 37. Raw metrics (13.54 / 5.95) summed to 37.58, and rounding the
        // label's geometric baseline UP built 38.
        assert_eq!(LayoutBox::text_line_box_extents(&n53_row_style()), (18.0, 6.0));
        let mut cb = Dimensions::default();
        cb.content = Rect::new(0.0, 0.0, 736.0, 0.0);
        let mut row = LayoutBox::new(BoxType::Block, n53_row_style());
        let ws = || LayoutBox::new(BoxType::Text(" ".to_string()), n53_row_style());
        row.children
            .push(n53_control(FormControlType::Checkbox { checked: false }));
        row.children.push(ws());
        row.children.push(n53_label("Checkbox 2 (checked)"));
        row.layout(&cb);
        assert!(
            (row.dimensions.content.height - 37.0).abs() <= 0.05,
            "wrapped-label row: Chrome 37, got {}",
            row.dimensions.content.height
        );
    }

    #[test]
    fn a_fixed_height_button_centres_its_line_about_the_baseline() {
        // form-controls §4 (n54): `button { width: 200px; height: 50px }`
        // alone in a 16px/24px container is a 50px line in Chrome 148 — its
        // label is centred, so ~20px of the box hangs below the baseline and
        // swallows the strut's 6. Seating the label at the box bottom left
        // 5.4 below: the strut poked out and the line read 50.6.
        let mut cb = Dimensions::default();
        cb.content = Rect::new(0.0, 0.0, 736.0, 0.0);
        let mut block = LayoutBox::new(BoxType::Block, n53_row_style());
        let mut button = n53_control(FormControlType::Button {
            label: "Fixed size".to_string(),
            button_type: "button".to_string(),
        });
        button.style.width = Length::Px(200.0);
        button.style.height = Length::Px(50.0);
        block.children.push(button);
        block.layout(&cb);
        assert!(
            (block.dimensions.content.height - 50.0).abs() <= 0.05,
            "fixed 50px button line: Chrome 50, got {}",
            block.dimensions.content.height
        );
    }

    #[test]
    fn a_text_only_line_is_exactly_one_line_height() {
        // The strut and a text member of the container's own font split the
        // SAME line-height about the baseline: summing them must not grow a
        // plain text line by the rounding sliver between a face's ascent +
        // descent and its `normal` line-height.
        let mut cb = Dimensions::default();
        cb.content = Rect::new(0.0, 0.0, 736.0, 0.0);
        for style in [ComputedStyle::new(), n53_row_style()] {
            let mut block = LayoutBox::new(BoxType::Block, style.clone());
            block
                .children
                .push(LayoutBox::new(BoxType::Text("Hi".to_string()), style.clone()));
            block
                .children
                .push(LayoutBox::new(BoxType::Text(" there".to_string()), style));
            block.layout(&cb);
            let lh = block.children[0].get_line_height();
            assert!(
                (block.dimensions.content.height - lh).abs() <= 0.01,
                "two runs on one line are one line-height ({lh}), got {}",
                block.dimensions.content.height
            );
        }
    }

    #[test]
    fn wrapped_inline_block_hangs_the_line_off_its_last_line() {
        // form-controls §5 (n53): `<input type=checkbox> <label>Checkbox 1
        // </label> <input type=checkbox> <label>Checkbox 2 (checked)</label>`
        // in a 120px-label row — the second label wraps to two 18px lines.
        // Chrome 148: the line's baseline is that label's SECOND line's, so
        // the checkboxes (bottom-edge baseline) and the one-line label all
        // sit at +18 from the row top; the wrapped label at +0.
        let mut cb = Dimensions::default();
        cb.content = Rect::new(0.0, 0.0, 736.0, 0.0);
        let mut row = LayoutBox::new(BoxType::Block, n53_row_style());
        let cb_box = || n53_control(FormControlType::Checkbox { checked: false });
        let ws = || LayoutBox::new(BoxType::Text(" ".to_string()), n53_row_style());
        row.children.push(cb_box());
        row.children.push(ws());
        row.children.push(n53_label("Checkbox 1"));
        row.children.push(ws());
        row.children.push(cb_box());
        row.children.push(ws());
        row.children.push(n53_label("Checkbox 2 (checked)"));
        row.layout(&cb);

        let row_top = row.dimensions.content.y;
        let top = |i: usize| row.children[i].dimensions.border_box().y - row_top;
        let wrapped = &row.children[6];
        assert!(
            (wrapped.dimensions.border_box().height - 36.0).abs() <= 1.0,
            "the long label must wrap to two 18px lines, got {}",
            wrapped.dimensions.border_box().height
        );
        assert!(
            top(6).abs() <= 1.0,
            "the wrapped label defines the line's ascent and stays at the top, got +{}",
            top(6)
        );
        for (i, what) in [
            (0, "first checkbox"),
            (2, "one-line label"),
            (4, "second checkbox"),
        ] {
            assert!(
                (top(i) - 18.0).abs() <= 1.5,
                "{what} must hang off the wrapped label's second line: Chrome +18, got +{}",
                top(i)
            );
        }
    }

    #[test]
    fn textarea_alone_on_a_line_hangs_the_strut_descent_below_it() {
        // form-controls §7 (n53): a bare 32px textarea as the only child of a
        // 16px/24px container. Chrome 148 builds a 38px line — the textarea
        // is a scroll container, its baseline is its bottom margin edge, and
        // the strut's descent + half-leading (6px here) hangs below. The hang
        // model built 32 and every section under it slid up by 6.
        let mut cb = Dimensions::default();
        cb.content = Rect::new(0.0, 0.0, 736.0, 0.0);
        let mut block = LayoutBox::new(BoxType::Block, n53_row_style());
        block.children.push(n53_control(FormControlType::TextArea {
            value: String::new(),
            placeholder: "Default textarea".to_string(),
            rows: 2,
            cols: 20,
        }));
        block.layout(&cb);

        let ta = &block.children[0].dimensions;
        assert!(
            (ta.border_box().height - 32.0).abs() <= 1.0,
            "bare two-row textarea stays 32px, got {}",
            ta.border_box().height
        );
        assert!(
            (ta.border_box().y - block.dimensions.content.y).abs() <= 0.5,
            "textarea top is the line top, got +{}",
            ta.border_box().y - block.dimensions.content.y
        );
        let sd = block.inline_strut_descent();
        let expected = 32.0 + sd;
        assert!(
            (block.dimensions.content.height - expected).abs() <= 0.5,
            "line = textarea + strut descent ({expected}; Chrome 38), got {}",
            block.dimensions.content.height
        );
    }

    #[test]
    fn bare_control_widths_match_chrome() {
        // n53 form-controls y-table vs Chrome 148 at the UA control font:
        // text input 149 (was the 12em blob, 160); textarea cols=20 178 and
        // cols=40 338 (0.6em per col + 18 border/scrollbar gutter; was 160 /
        // 320); a listbox is its widest option + 2 (39 for "Item 4"; was
        // 133), a dropdown its widest option + 24 (137 for "A longer option
        // text", 60 for "Select"; was 133).
        let mut cb = Dimensions::default();
        cb.content = Rect::new(0.0, 0.0, 736.0, 0.0);
        let width = |mut b: LayoutBox| {
            b.layout(&cb);
            b.dimensions.border_box().width
        };
        let w = width(n53_text_input());
        assert!(
            (w - 149.0).abs() <= 0.5,
            "bare text input width: Chrome 149, got {w}"
        );

        let ta = |cols: u32| {
            n53_control(FormControlType::TextArea {
                value: String::new(),
                placeholder: String::new(),
                rows: 2,
                cols,
            })
        };
        let w20 = width(ta(20));
        let w40 = width(ta(40));
        assert!(
            (w20 - 178.0).abs() <= 1.0,
            "textarea cols=20: Chrome 178, got {w20}"
        );
        assert!(
            (w40 - 338.0).abs() <= 1.0,
            "textarea cols=40: Chrome 338, got {w40}"
        );

        let sel = |options: &[&str], size: u32| {
            n53_control(FormControlType::Select {
                options: options.iter().map(|o| o.to_string()).collect(),
                selected_index: None,
                size,
            })
        };
        let listbox = width(sel(&["Item 1", "Item 2", "Item 3", "Item 4"], 3));
        assert!(
            (listbox - 39.0).abs() <= 3.0,
            "listbox = widest option + 2: Chrome 39, got {listbox}"
        );
        let dropdown = width(sel(&["Option 1", "Option 2", "A longer option text"], 0));
        assert!(
            (dropdown - 137.0).abs() <= 2.0,
            "dropdown = widest option + 24: Chrome 137, got {dropdown}"
        );
        let short = width(sel(&["Select"], 0));
        assert!(
            (short - 60.0).abs() <= 3.0,
            "dropdown 'Select': Chrome 60, got {short}"
        );
    }

    #[test]
    fn test_inline_block_auto_width_shrinks_to_fit() {
        // CSS2 §10.3.9: an atomic inline with width:auto shrinks to its
        // content, it does not fill the containing block. Found on the
        // about page (n39): span#versionBadge (inline-block, padding 4/12,
        // border 1) measured 672px wide vs Chrome's 104.6 — width:auto took
        // the block fill path, and text-align:center then centered the text
        // INSIDE the full-width badge instead of the badge itself.
        let mut cb = Dimensions::default();
        cb.content = Rect::new(0.0, 0.0, 672.0, 0.0);

        let mut parent_style = ComputedStyle::new();
        parent_style.text_align = TextAlign::Center;
        let mut parent = LayoutBox::new(BoxType::Block, parent_style);

        let mut badge_style = ComputedStyle::new();
        badge_style.display = rustkit_css::Display::InlineBlock;
        badge_style.padding_left = Length::Px(12.0);
        badge_style.padding_right = Length::Px(12.0);
        badge_style.border_left_width = Length::Px(1.0);
        badge_style.border_right_width = Length::Px(1.0);
        let mut badge = LayoutBox::new(BoxType::Block, badge_style);

        // Content stands in for a text run via an explicit-width block:
        // preferred (max-content) = 80.
        let mut inner_style = ComputedStyle::new();
        inner_style.width = Length::Px(80.0);
        inner_style.height = Length::Px(17.0);
        badge
            .children
            .push(LayoutBox::new(BoxType::Block, inner_style));
        parent.children.push(badge);

        parent.layout(&cb);

        let d = &parent.children[0].dimensions;
        assert!(
            (d.content.width - 80.0).abs() < 0.01,
            "auto-width inline-block must shrink to content (80), got {}",
            d.content.width
        );
        // Border box = 80 + 24 padding + 2 border = 106; text-align:center
        // on the parent centers the BOX: x = (672 - 106) / 2 = 283.
        let bb = d.border_box();
        assert!(
            (bb.width - 106.0).abs() < 0.01,
            "border box must be 106, got {}",
            bb.width
        );
        assert!(
            (bb.x - 283.0).abs() < 0.5,
            "centered inline-block border box must sit at x=283, got {}",
            bb.x
        );
    }

    #[test]
    fn test_inline_block_auto_width_floors_at_min_content() {
        // Shrink-to-fit = min(max(preferred_min, available), preferred):
        // a container narrower than min-content does not crush the box.
        let mut cb = Dimensions::default();
        cb.content = Rect::new(0.0, 0.0, 50.0, 0.0);

        let mut badge_style = ComputedStyle::new();
        badge_style.display = rustkit_css::Display::InlineBlock;
        badge_style.padding_left = Length::Px(12.0);
        badge_style.padding_right = Length::Px(12.0);
        let mut badge = LayoutBox::new(BoxType::Block, badge_style);
        let mut inner_style = ComputedStyle::new();
        inner_style.width = Length::Px(80.0);
        badge
            .children
            .push(LayoutBox::new(BoxType::Block, inner_style));

        let mut parent = LayoutBox::new(BoxType::Block, ComputedStyle::new());
        parent.children.push(badge);
        parent.layout(&cb);

        let d = &parent.children[0].dimensions;
        assert!(
            (d.content.width - 80.0).abs() < 0.01,
            "inline-block must not shrink below min-content (80), got {}",
            d.content.width
        );
    }

    #[test]
    fn test_text_align_left_is_noop_for_inline_box() {
        // Default (left) alignment must leave an inline box at the origin.
        let mut cb = Dimensions::default();
        cb.content = Rect::new(0.0, 0.0, 200.0, 0.0);
        let block_style = ComputedStyle::new(); // text_align defaults to Left
        let mut span_style = ComputedStyle::new();
        span_style.width = Length::Px(50.0);
        let mut block = LayoutBox::new(BoxType::Block, block_style);
        block
            .children
            .push(LayoutBox::new(BoxType::Inline, span_style));
        block.layout(&cb);
        assert_eq!(block.children[0].dimensions.content.x, 0.0);
    }

    #[test]
    fn test_auto_margins_ignored_on_floats() {
        // Floated boxes use auto margins as 0 (CSS 2.1 §10.3.5)
        let mut style = ComputedStyle::new();
        style.width = Length::Px(600.0);
        style.margin_left = Length::Auto;
        style.margin_right = Length::Auto;
        let mut layout_box = LayoutBox::with_float(BoxType::Block, style, Float::Left);
        layout_box.calculate_block_width(&containing_1000());
        assert_eq!(layout_box.dimensions.margin.left, 0.0);
        assert_eq!(layout_box.dimensions.margin.right, 0.0);
    }

    // ---- CSS 2.1 §10.3.7: out-of-flow `width: auto` is shrink-to-fit ----
    //
    // Driven through `calculate_block_width`, the function the engine calls,
    // rather than through the free helpers alone: `.footer { position: fixed;
    // bottom: 1rem }` on new_tab was 1280px wide against Chrome's 137.59
    // because the WIRING sent every auto width down §10.3.3's fill path, and
    // a helper that computes the right number but is never reached is worth
    // nothing. Every expected width below is derived from the shaper the
    // engine uses, never a hardcoded pixel count, so these do not encode this
    // seat's stub font metrics as if they were Chrome's.

    /// The text this measures must contain a space, so min-content (longest
    /// word) and max-content (whole run) are genuinely different numbers and
    /// a test can tell which one a formula returned.
    const STF_TEXT: &str = "aaaaaaaaaaaaaaaaaaaa bb";

    fn stf_measure(s: &str) -> f32 {
        let st = ComputedStyle::new();
        let fs = match st.font_size {
            Length::Px(px) => px,
            _ => 16.0,
        };
        measure_text_advanced(s, &st.font_family, fs, st.font_weight, st.font_style).width
    }

    /// An out-of-flow box carrying one text child, laid out against `cb_width`.
    fn stf_box(position: Position, left: Option<f32>, right: Option<f32>) -> LayoutBox {
        let mut b = LayoutBox::with_position(BoxType::Block, ComputedStyle::new(), position);
        b.set_offsets(None, right, None, left);
        b.children.push(LayoutBox::new(
            BoxType::Text(STF_TEXT.to_string()),
            ComputedStyle::new(),
        ));
        b
    }

    fn stf_width(mut b: LayoutBox, cb_width: f32) -> f32 {
        let mut cb = Dimensions::default();
        cb.content = Rect::new(0.0, 0.0, cb_width, 600.0);
        b.calculate_block_width(&cb);
        b.dimensions.content.width
    }

    #[test]
    fn an_inset_stretch_never_overrides_a_specified_height() {
        // CSS2 §10.6.4 stretches an out-of-flow box between `top` and
        // `bottom` only where `height` is auto; with a height specified the
        // constraint is over-determined and `bottom` is the one that gives.
        // `inset_definite_content_height` is the single copy of that rule
        // and BOTH its callers depend on the check: the positioning path
        // would otherwise resize a 40px overlay to its containing block.
        let mut style = ComputedStyle::new();
        style.height = Length::Px(40.0);
        let mut b = LayoutBox::with_position(BoxType::Block, style, Position::Absolute);
        b.set_offsets(Some(0.0), None, Some(0.0), None);
        b.dimensions.content.height = 40.0;
        let cb = Dimensions {
            content: Rect::new(0.0, 0.0, 300.0, 200.0),
            ..Default::default()
        };
        b.apply_position_offsets(&cb);
        assert_eq!(
            b.dimensions.content.height, 40.0,
            "a specified height survives inset: 0"
        );
    }

    #[test]
    fn an_out_of_flow_auto_width_box_shrinks_to_its_content() {
        // Room to spare: shrink-to-fit is the max-content width, NOT the
        // containing block. This is new_tab's footer defect in miniature.
        let max_content = stf_measure(STF_TEXT);
        for position in [Position::Fixed, Position::Absolute] {
            let w = stf_width(stf_box(position, None, None), 1000.0);
            assert!(
                (w - max_content).abs() < 0.01,
                "{position:?} with width:auto must shrink to its max-content \
                 width {max_content}, got {w}"
            );
            assert!(w < 1000.0, "{position:?} must not fill the containing block");
        }
    }

    #[test]
    fn an_in_flow_auto_width_block_still_fills_its_containing_block() {
        // §10.3.3 is a different rule wearing the same keyword and must not
        // have been dragged along.
        assert_eq!(stf_width(stf_box(Position::Static, None, None), 1000.0), 1000.0);
        assert_eq!(
            stf_width(stf_box(Position::Relative, None, None), 1000.0),
            1000.0
        );
    }

    #[test]
    fn an_out_of_flow_box_with_both_inline_offsets_still_stretches() {
        // `inset: 0` overlays (settings' toggle sliders) solve for width and
        // fill between left and right. Shrinking them would be a regression
        // dressed as a fix.
        let w = stf_width(stf_box(Position::Absolute, Some(0.0), Some(0.0)), 1000.0);
        assert_eq!(w, 1000.0, "left+right both set must keep the stretch");
    }

    #[test]
    fn one_inline_offset_alone_does_not_restore_the_stretch() {
        // §10.3.7 stretches only when BOTH are given; with exactly one, width
        // is shrink-to-fit and the other offset is solved afterwards.
        let max_content = stf_measure(STF_TEXT);
        for (l, r) in [(Some(10.0), None), (None, Some(10.0))] {
            let w = stf_width(stf_box(Position::Absolute, l, r), 1000.0);
            assert!(
                (w - max_content).abs() < 0.01,
                "left={l:?} right={r:?} must still shrink to fit, got {w}"
            );
        }
    }

    #[test]
    fn shrink_to_fit_is_clamped_by_the_available_width() {
        // min(max(min-content, available), max-content): with available
        // BETWEEN the two intrinsic sizes, available wins.
        let min_content = stf_measure("aaaaaaaaaaaaaaaaaaaa");
        let max_content = stf_measure(STF_TEXT);
        let available = (min_content + max_content) / 2.0;
        assert!(min_content < available && available < max_content, "fixture setup");
        let w = stf_width(stf_box(Position::Fixed, None, None), available);
        assert!(
            (w - available).abs() < 0.01,
            "available {available} must clamp the preferred width, got {w}"
        );
    }

    #[test]
    fn shrink_to_fit_is_floored_by_the_min_content_width() {
        // Available narrower than the longest unbreakable word: the floor
        // wins and the box overflows, rather than being squeezed to fit.
        let min_content = stf_measure("aaaaaaaaaaaaaaaaaaaa");
        let available = min_content / 4.0;
        let w = stf_width(stf_box(Position::Fixed, None, None), available);
        assert!(
            (w - min_content).abs() < 0.01,
            "min-content {min_content} is the floor, got {w} (available {available})"
        );
    }

    #[test]
    fn shrink_to_fit_measures_content_not_the_padding_box() {
        // The intrinsic estimators answer BORDER-box widths; the used value
        // here is a CONTENT width. Subtracting the wrong amount, or none,
        // moves the box by exactly its horizontal padding+border.
        let max_content = stf_measure(STF_TEXT);
        let mut b = stf_box(Position::Fixed, None, None);
        b.style.padding_left = Length::Px(10.0);
        b.style.padding_right = Length::Px(10.0);
        b.style.border_left_width = Length::Px(2.0);
        b.style.border_right_width = Length::Px(2.0);
        let w = stf_width(b, 1000.0);
        assert!(
            (w - max_content).abs() < 0.01,
            "content width must be the content's own max-content {max_content}, got {w}"
        );
    }

    #[test]
    fn the_out_of_flow_contribution_rule_does_not_size_the_box_itself() {
        // The split this rule needed: the estimators answer 0 for an
        // out-of-flow box because it contributes nothing to an ANCESTOR, and
        // sizing it from that would give a zero-width footer.
        let b = stf_box(Position::Fixed, None, None);
        assert_eq!(crate::grid::estimate_max_content_width(&b), 0.0);
        assert_eq!(crate::grid::estimate_min_content_width(&b), 0.0);
        assert!(crate::grid::own_max_content_width(&b) > 0.0);
        assert!(crate::grid::own_min_content_width(&b) > 0.0);
        // …and an out-of-flow CHILD still contributes nothing to its parent.
        let mut parent = LayoutBox::new(BoxType::Block, ComputedStyle::new());
        parent.children.push(stf_box(Position::Absolute, None, None));
        assert_eq!(crate::grid::own_max_content_width(&parent), 0.0);
    }

    #[test]
    fn shrink_to_fit_never_returns_a_negative_width() {
        // `own_*_content_width` has early returns that answer WITHOUT adding
        // padding+border back — `display: none` answers 0, a text box answers
        // its bare text width — so subtracting it can go negative. Neither is
        // reachable from `calculate_block_width` today (a `display: none` box
        // is never laid out, and a text box never takes the block-width
        // path), which is precisely why the clamps need a test that calls the
        // free function directly. Without them this returns -80.
        let mut b = stf_box(Position::Fixed, None, None);
        b.style.display = rustkit_css::Display::None;
        b.style.padding_left = Length::Px(40.0);
        b.style.padding_right = Length::Px(40.0);
        assert_eq!(shrink_to_fit_content_width(&b, 1000.0), 0.0);
    }

    #[test]
    fn the_shrink_to_fit_predicate_reads_position_and_both_offsets() {
        let none = PositionOffsets::default();
        let both = PositionOffsets {
            left: Some(0.0),
            right: Some(0.0),
            ..Default::default()
        };
        let left_only = PositionOffsets {
            left: Some(0.0),
            ..Default::default()
        };
        for p in [Position::Absolute, Position::Fixed] {
            assert!(auto_width_shrinks_to_fit(p, &none));
            assert!(auto_width_shrinks_to_fit(p, &left_only));
            assert!(!auto_width_shrinks_to_fit(p, &both));
        }
        for p in [Position::Static, Position::Relative, Position::Sticky] {
            assert!(!auto_width_shrinks_to_fit(p, &none));
            assert!(!auto_width_shrinks_to_fit(p, &left_only));
        }
    }

    #[test]
    fn a_percentage_offset_counts_as_specified_for_the_stretch_test() {
        // `set_offsets` drops percentages (they need the containing block);
        // `resolved_offsets` puts them back. Reading raw `offsets` here would
        // shrink a `left: 0%; right: 0%` overlay that must stretch.
        let mut b = stf_box(Position::Absolute, None, None);
        b.style.left = Some(Length::Percent(0.0));
        b.style.right = Some(Length::Percent(0.0));
        assert_eq!(
            stf_width(b, 1000.0),
            1000.0,
            "percentage insets must be seen as specified"
        );
    }

    #[test]
    fn test_auto_margins_overconstrained_stay_zero() {
        // Box wider than its container: no free space to distribute
        let mut style = ComputedStyle::new();
        style.width = Length::Px(1400.0);
        style.margin_left = Length::Auto;
        style.margin_right = Length::Auto;
        let mut layout_box = LayoutBox::new(BoxType::Block, style);
        layout_box.calculate_block_width(&containing_1000());
        assert_eq!(layout_box.dimensions.margin.left, 0.0);
        assert_eq!(layout_box.dimensions.margin.right, 0.0);
    }

    /// A vertical-align:top atomic inline taller than the strut SWALLOWS
    /// it (CSS2 §10.8): the line box is exactly the box's height. The old
    /// accounting extended strut_descent below every bottom-edge-baseline
    /// box regardless of alignment — sticky-scroll's nowrap card row read
    /// +6.8px tall and shifted every later sibling. Driven through BOTH
    /// children paths (the collapse twin the engine uses, and the plain
    /// twin flex item re-layout uses).
    #[test]
    fn vertical_align_top_atomic_swallows_the_strut() {
        for use_collapse in [true, false] {
            let mut parent = LayoutBox::new(BoxType::Block, ComputedStyle::new());
            let mut ib = ComputedStyle::new();
            ib.display = rustkit_css::Display::InlineBlock;
            ib.width = Length::Px(200.0);
            ib.height = Length::Px(120.0);
            ib.vertical_align = rustkit_css::VerticalAlign::Top;
            parent.children.push(LayoutBox::new(BoxType::Block, ib));

            let cb = Dimensions {
                content: Rect::new(0.0, 0.0, 800.0, 600.0),
                ..Default::default()
            };
            if use_collapse {
                let mut mc = MarginCollapseContext::new();
                let mut fc = FloatContext::new();
                parent.layout_with_collapse(&cb, &mut mc, &mut fc);
            } else {
                parent.layout(&cb);
            }
            let h = parent.dimensions.content.height;
            assert!(
                (h - 120.0).abs() < 0.1,
                "top-aligned 120px inline-block must make a 120px line \
                 (collapse={use_collapse}), got {h} — 123.7 means the strut \
                 descent was stacked under a box that swallows it"
            );
        }
    }

    /// NEGATIVE CONTROL: the BASELINE-aligned case keeps the strut descent
    /// under the box — the behaviour Chrome shows and the settings-toggle
    /// pin depends on. The vertical-align gate must not leak into it.
    #[test]
    fn baseline_aligned_atomic_still_extends_strut() {
        let mut parent = LayoutBox::new(BoxType::Block, ComputedStyle::new());
        let mut ib = ComputedStyle::new();
        ib.display = rustkit_css::Display::InlineBlock;
        ib.width = Length::Px(200.0);
        ib.height = Length::Px(120.0);
        parent.children.push(LayoutBox::new(BoxType::Block, ib));
        let strut = parent.inline_strut_descent();
        let cb = Dimensions {
            content: Rect::new(0.0, 0.0, 800.0, 600.0),
            ..Default::default()
        };
        let mut mc = MarginCollapseContext::new();
        let mut fc = FloatContext::new();
        parent.layout_with_collapse(&cb, &mut mc, &mut fc);
        let h = parent.dimensions.content.height;
        assert!(
            (h - (120.0 + strut)).abs() < 0.1,
            "baseline-aligned box must keep strut descent below it: \
             expected {}, got {h}",
            120.0 + strut
        );
    }

    /// A non-replaced inline's rect is its CONTENT AREA centered by
    /// half-leading (§10.6.1/§10.8.1) while the LINE still advances by
    /// line-height. Chrome's rects agree; reporting the line box instead
    /// put 24.9% of all Gate A rows on inline elements.
    #[test]
    fn inline_rect_is_content_area_centered_in_its_line() {
        let mut parent = LayoutBox::new(BoxType::Block, ComputedStyle::new());
        let mut a_style = ComputedStyle::new();
        a_style.display = rustkit_css::Display::Inline;
        a_style.line_height = rustkit_css::LineHeight::Number(1.5);
        a_style.font_size = Length::Px(16.0);
        // Width so the line has content and flushes; inline width is not
        // spec-honored but the advance mechanism under test does not care.
        a_style.width = Length::Px(50.0);
        let mut a = LayoutBox::new(BoxType::Inline, a_style);
        let (content, half) = a.inline_content_area();
        assert!(
            content < 24.0 * 0.9,
            "content area must be font-based, got {content}"
        );
        parent.children.push(a);

        let cb = Dimensions {
            content: Rect::new(0.0, 0.0, 800.0, 600.0),
            ..Default::default()
        };
        let mut mc = MarginCollapseContext::new();
        let mut fc = FloatContext::new();
        parent.layout_with_collapse(&cb, &mut mc, &mut fc);

        let parent_top = parent.dimensions.content.y;
        let child = &parent.children[0].dimensions;
        assert!(
            (child.content.height - content).abs() < 0.1,
            "inline rect height must be the content area {content}, got {}",
            child.content.height
        );
        assert!(
            (child.content.y - parent_top - half).abs() < 0.1,
            "inline rect must sit a half-leading ({half}) below the line top, \
             got relative y {}",
            child.content.y - parent_top
        );
        assert!(
            (parent.dimensions.content.height - 24.0).abs() < 0.1,
            "the LINE must still advance by line-height 24, got {} — \
             16-ish means the advance was coupled to the shrunken rect",
            parent.dimensions.content.height
        );
    }

    /// Sibling margins inside a FLEX ITEM must collapse (CSS 2.1 §8.3.1):
    /// the item establishes an independent formatting context, but its own
    /// in-flow children still collapse among themselves. Driven through
    /// layout_with_collapse — the entry the engine actually uses — so this
    /// exercises the flex re-layout path (flex.rs) that used to re-stack
    /// children with layout_block_children and re-sum every seam.
    #[test]
    fn flex_item_children_collapse_sibling_margins() {
        let mut root = LayoutBox::new(BoxType::Block, ComputedStyle::new());
        let mut flex_style = ComputedStyle::new();
        flex_style.display = rustkit_css::Display::Flex;
        let mut flex = LayoutBox::new(BoxType::Block, flex_style);
        let mut item = LayoutBox::new(BoxType::Block, ComputedStyle::new());
        let mut a_style = ComputedStyle::new();
        a_style.height = Length::Px(50.0);
        a_style.margin_bottom = Length::Px(20.0);
        let mut b_style = ComputedStyle::new();
        b_style.height = Length::Px(50.0);
        b_style.margin_top = Length::Px(20.0);
        item.children.push(LayoutBox::new(BoxType::Block, a_style));
        item.children.push(LayoutBox::new(BoxType::Block, b_style));
        flex.children.push(item);
        root.children.push(flex);

        let cb = Dimensions {
            content: Rect::new(0.0, 0.0, 800.0, 600.0),
            ..Default::default()
        };
        let mut mc = MarginCollapseContext::new();
        let mut fc = FloatContext::new();
        root.layout_with_collapse(&cb, &mut mc, &mut fc);

        let item = &root.children[0].children[0];
        let a = &item.children[0].dimensions;
        let b = &item.children[1].dimensions;
        let gap = b.content.y - (a.content.y + a.content.height);
        assert!(
            (gap - 20.0).abs() < 0.1,
            "adjacent 20/20 margins inside a flex item must collapse to 20, \
             got gap {gap} (40 = summed, the pre-fix behaviour)"
        );
    }

    /// Same contract for a GRID ITEM. The grid re-stack loop (grid.rs
    /// Phase 9) used to advance current_y by margin_bottom and then add the
    /// next child's full margin_top — block flow re-implemented without
    /// collapse, overwriting the collapsed pre-pass. Unequal margins pin
    /// max() semantics, not just "not summed".
    #[test]
    fn grid_item_children_collapse_sibling_margins() {
        let mut root = LayoutBox::new(BoxType::Block, ComputedStyle::new());
        let mut grid_style = ComputedStyle::new();
        grid_style.display = rustkit_css::Display::Grid;
        let mut grid = LayoutBox::new(BoxType::Block, grid_style);
        let mut item = LayoutBox::new(BoxType::Block, ComputedStyle::new());
        let mut a_style = ComputedStyle::new();
        a_style.height = Length::Px(50.0);
        a_style.margin_bottom = Length::Px(20.0);
        let mut b_style = ComputedStyle::new();
        b_style.height = Length::Px(50.0);
        b_style.margin_top = Length::Px(8.0);
        item.children.push(LayoutBox::new(BoxType::Block, a_style));
        item.children.push(LayoutBox::new(BoxType::Block, b_style));
        grid.children.push(item);
        root.children.push(grid);

        let cb = Dimensions {
            content: Rect::new(0.0, 0.0, 800.0, 600.0),
            ..Default::default()
        };
        let mut mc = MarginCollapseContext::new();
        let mut fc = FloatContext::new();
        root.layout_with_collapse(&cb, &mut mc, &mut fc);

        let item = &root.children[0].children[0];
        let a = &item.children[0].dimensions;
        let b = &item.children[1].dimensions;
        let gap = b.content.y - (a.content.y + a.content.height);
        assert!(
            (gap - 20.0).abs() < 0.1,
            "20/8 margins inside a grid item must collapse to max()=20, \
             got gap {gap} (28 = summed, 8 = wrong side won)"
        );
    }

    /// NEGATIVE CONTROL, both directions: plain block flow collapsed
    /// correctly before this fix and must keep doing so — a regression here
    /// means the fix leaked outside the flex/grid item paths it targets.
    #[test]
    fn plain_block_flow_still_collapses_sibling_margins() {
        let mut root = LayoutBox::new(BoxType::Block, ComputedStyle::new());
        let mut a_style = ComputedStyle::new();
        a_style.height = Length::Px(50.0);
        a_style.margin_bottom = Length::Px(20.0);
        let mut b_style = ComputedStyle::new();
        b_style.height = Length::Px(50.0);
        b_style.margin_top = Length::Px(20.0);
        root.children.push(LayoutBox::new(BoxType::Block, a_style));
        root.children.push(LayoutBox::new(BoxType::Block, b_style));

        let cb = Dimensions {
            content: Rect::new(0.0, 0.0, 800.0, 600.0),
            ..Default::default()
        };
        let mut mc = MarginCollapseContext::new();
        let mut fc = FloatContext::new();
        root.layout_with_collapse(&cb, &mut mc, &mut fc);

        let a = &root.children[0].dimensions;
        let b = &root.children[1].dimensions;
        let gap = b.content.y - (a.content.y + a.content.height);
        assert!(
            (gap - 20.0).abs() < 0.1,
            "plain-flow sibling collapse regressed: got gap {gap}"
        );
    }

    // ---- CSS 2.1 §8.3.1 parent/child through-collapse (n43) ----
    // Shapes are css-selectors §1: `.section-title{margin-bottom:10px}`
    // followed by an unpadded `.test-child` whose first child has
    // `margin-top:4px` — Chrome lays that child at max(10, 4) below the
    // title, RustKit laid it at 10 + 4.

    fn n43_block(height: Option<f32>, mt: f32, mb: f32) -> LayoutBox {
        let mut s = ComputedStyle::new();
        if let Some(h) = height {
            s.height = Length::Px(h);
        }
        s.margin_top = Length::Px(mt);
        s.margin_bottom = Length::Px(mb);
        LayoutBox::new(BoxType::Block, s)
    }

    fn n43_layout(root: &mut LayoutBox) {
        let cb = Dimensions {
            content: Rect::new(0.0, 0.0, 800.0, 0.0),
            ..Default::default()
        };
        let mut mc = MarginCollapseContext::new();
        let mut fc = FloatContext::new();
        root.layout_with_collapse(&cb, &mut mc, &mut fc);
    }

    #[test]
    fn a_first_childs_top_margin_collapses_through_an_open_parent() {
        let mut root = LayoutBox::new(BoxType::Block, ComputedStyle::new());
        root.children.push(n43_block(Some(20.0), 0.0, 10.0)); // title
        let mut wrapper = n43_block(None, 0.0, 0.0);
        wrapper.children.push(n43_block(Some(30.0), 4.0, 0.0));
        root.children.push(wrapper);
        n43_layout(&mut root);

        let wrapper = &root.children[1];
        let child = &wrapper.children[0];
        assert!(
            (wrapper.dimensions.content.y - 30.0).abs() < 0.1,
            "wrapper sits at title bottom + max(10, 4) = 30, got {}",
            wrapper.dimensions.content.y
        );
        assert!(
            (child.dimensions.content.y - 30.0).abs() < 0.1,
            "first child's 4px collapsed through the wrapper: y must be 30, got {} (34 = summed)",
            child.dimensions.content.y
        );
        assert!(
            (wrapper.dimensions.content.height - 30.0).abs() < 0.1,
            "the collapsed margin is outside the wrapper: height 30, got {}",
            wrapper.dimensions.content.height
        );
    }

    #[test]
    fn the_chain_collapses_through_two_open_wrappers() {
        // section-title (mb 10) / wrapper / wrapper / nested-child (mt 4)
        let mut root = LayoutBox::new(BoxType::Block, ComputedStyle::new());
        root.children.push(n43_block(Some(20.0), 0.0, 10.0));
        let mut outer = n43_block(None, 0.0, 0.0);
        let mut inner = n43_block(None, 0.0, 0.0);
        inner.children.push(n43_block(Some(30.0), 4.0, 0.0));
        outer.children.push(inner);
        root.children.push(outer);
        n43_layout(&mut root);

        let leaf = &root.children[1].children[0].children[0];
        assert!(
            (leaf.dimensions.content.y - 30.0).abs() < 0.1,
            "4px collapses through both wrappers to max(10, 4): y 30, got {}",
            leaf.dimensions.content.y
        );
        assert!(
            (root.children[1].dimensions.content.height - 30.0).abs() < 0.1,
            "neither wrapper grows by the escaped margin, got {}",
            root.children[1].dimensions.content.height
        );
    }

    #[test]
    fn a_last_childs_bottom_margin_collapses_through_and_adjoins_the_next_sibling() {
        // wrapper > child (h 30, mb 4), then next (mt 2): Chrome puts next at
        // 30 + max(4, 2) = 34 and the wrapper stays 30 tall.
        let mut root = LayoutBox::new(BoxType::Block, ComputedStyle::new());
        let mut wrapper = n43_block(None, 0.0, 0.0);
        wrapper.children.push(n43_block(Some(30.0), 0.0, 4.0));
        root.children.push(wrapper);
        root.children.push(n43_block(Some(10.0), 2.0, 0.0));
        n43_layout(&mut root);

        let wrapper = &root.children[0];
        let next = &root.children[1];
        assert!(
            (wrapper.dimensions.content.height - 30.0).abs() < 0.1,
            "wrapper height excludes the through-collapsed margin: 30, got {}",
            wrapper.dimensions.content.height
        );
        assert!(
            (next.dimensions.content.y - 34.0).abs() < 0.1,
            "next sibling at 30 + max(4, 2) = 34, got {} (32 = the 4px was dropped)",
            next.dimensions.content.y
        );
        // The ul case: two escaped margins (first + last) around a stack of items
        // must not inflate the parent — css-selectors §5 `.list-items` was +2.
        assert!(
            (root.dimensions.content.height - 44.0).abs() < 0.1,
            "root content = 34 + 10 = 44, got {}",
            root.dimensions.content.height
        );
    }

    #[test]
    fn a_padded_parent_keeps_its_first_childs_margin_inside() {
        // NEGATIVE CONTROL: padding-top closes the edge (§8.3.1).
        let mut root = LayoutBox::new(BoxType::Block, ComputedStyle::new());
        root.children.push(n43_block(Some(20.0), 0.0, 10.0));
        let mut wrapper = n43_block(None, 0.0, 0.0);
        wrapper.style.padding_top = Length::Px(15.0);
        wrapper.children.push(n43_block(Some(30.0), 4.0, 0.0));
        root.children.push(wrapper);
        n43_layout(&mut root);

        let wrapper = &root.children[1];
        let child = &wrapper.children[0];
        assert!(
            (wrapper.dimensions.content.y - 45.0).abs() < 0.1,
            "wrapper content at 20 + 10 + 15 = 45, got {}",
            wrapper.dimensions.content.y
        );
        assert!(
            (child.dimensions.content.y - 49.0).abs() < 0.1,
            "child keeps its 4px inside the padded wrapper: 49, got {}",
            child.dimensions.content.y
        );
    }

    #[test]
    fn a_flex_item_keeps_both_edge_margins_inside() {
        // A flex item is a formatting root: its children's margins never
        // escape it (css-flexbox-1 §4), in the pre-pass or the real pass.
        let mut root_style = ComputedStyle::new();
        root_style.display = rustkit_css::Display::Flex;
        let mut root = LayoutBox::new(BoxType::Block, root_style);
        let mut item = n43_block(None, 0.0, 0.0);
        item.children.push(n43_block(Some(20.0), 10.0, 6.0));
        root.children.push(item);
        n43_layout(&mut root);

        let item = &root.children[0];
        let child = &item.children[0];
        assert!(
            (item.dimensions.content.height - 36.0).abs() < 0.1,
            "flex item content = 10 + 20 + 6 = 36, got {}",
            item.dimensions.content.height
        );
        assert!(
            (child.dimensions.content.y - item.dimensions.content.y - 10.0).abs() < 0.1,
            "child sits 10px inside the item, got offset {}",
            child.dimensions.content.y - item.dimensions.content.y
        );
    }

    #[test]
    fn the_root_element_does_not_collapse_with_body() {
        // html > body(mt 8) > h1(mt 21.44): the engine marks html a
        // formatting root, so body sits at 21.44 under html's top edge and
        // html stays at 0 (Chrome: body.getBoundingClientRect().top = 21.44).
        let mut html = LayoutBox::new(BoxType::Block, ComputedStyle::new());
        let mut body = n43_block(None, 8.0, 8.0);
        body.children.push(n43_block(Some(30.0), 21.44, 0.0));
        html.children.push(body);
        let cb = Dimensions {
            content: Rect::new(0.0, 0.0, 800.0, 0.0),
            ..Default::default()
        };
        let mut mc = MarginCollapseContext::new();
        mc.children_are_formatting_roots = true;
        let mut fc = FloatContext::new();
        html.layout_with_collapse(&cb, &mut mc, &mut fc);

        assert!(
            (html.dimensions.content.y).abs() < 0.1,
            "html at 0, got {}",
            html.dimensions.content.y
        );
        let body = &html.children[0];
        assert!(
            (body.dimensions.content.y - 21.44).abs() < 0.1,
            "body's 8 and h1's 21.44 collapse to 21.44, got {}",
            body.dimensions.content.y
        );
        assert!(
            (body.children[0].dimensions.content.y - 21.44).abs() < 0.1,
            "h1 shares body's top edge, got {}",
            body.children[0].dimensions.content.y
        );
    }

    #[test]
    fn a_form_control_carries_its_author_margins_in_the_line() {
        // css-selectors §6: `button{padding:8px 16px; margin:4px}` — Chrome's
        // row is 4 + 31 + 4 = 39; the margins were never resolved (row 33.5).
        let mut root = LayoutBox::new(BoxType::Block, ComputedStyle::new());
        let mut s = ComputedStyle::new();
        s.display = rustkit_css::Display::InlineBlock;
        s.margin_top = Length::Px(4.0);
        s.margin_bottom = Length::Px(4.0);
        s.margin_left = Length::Px(4.0);
        s.margin_right = Length::Px(4.0);
        s.padding_top = Length::Px(8.0);
        s.padding_bottom = Length::Px(8.0);
        let button = LayoutBox::new(
            BoxType::FormControl(FormControlType::Button {
                label: "Active".to_string(),
                button_type: "button".to_string(),
            }),
            s,
        );
        root.children.push(button);
        n43_layout(&mut root);

        let b = &root.children[0];
        assert!(
            (b.dimensions.margin.top - 4.0).abs() < 0.01
                && (b.dimensions.margin.left - 4.0).abs() < 0.01,
            "margins resolved from style, got {:?}",
            b.dimensions.margin
        );
        assert!(
            (b.dimensions.content.y - 4.0).abs() < 0.1
                && (b.dimensions.content.x - 4.0).abs() < 0.1,
            "the control sits inside its margins, got ({}, {})",
            b.dimensions.content.x,
            b.dimensions.content.y
        );
        let control_h = b.dimensions.content.height;
        assert!(
            root.dimensions.content.height >= control_h + 8.0 - 0.1,
            "the line counts both vertical margins: >= {} + 8, got {}",
            control_h,
            root.dimensions.content.height
        );
    }

    #[test]
    fn test_margin_collapse_positive() {
        let mut ctx = MarginCollapseContext::new();
        ctx.add_margin(10.0);
        ctx.add_margin(20.0);
        assert_eq!(ctx.resolve(), 20.0); // Max of positive margins
    }

    #[test]
    fn test_margin_collapse_negative() {
        let mut ctx = MarginCollapseContext::new();
        ctx.add_margin(-10.0);
        ctx.add_margin(-20.0);
        assert_eq!(ctx.resolve(), -20.0); // Min of negative margins
    }

    #[test]
    fn test_margin_collapse_mixed() {
        let mut ctx = MarginCollapseContext::new();
        ctx.add_margin(20.0);
        ctx.add_margin(-10.0);
        assert_eq!(ctx.resolve(), 10.0); // Sum of max positive and min negative
    }

    #[test]
    fn test_abspos_sibling_does_not_swallow_block_margin() {
        // <div A height:30 margin-bottom:16> <div B position:absolute> <div C>
        // C must sit at A.bottom + 16, and B's static position must land at
        // the same y (CSS 2.1 §8.3.1: out-of-flow boxes do not participate in
        // margin collapsing). The regression: B resolved the pending margin
        // through the parent's live context for its own static position, then
        // reset it — so C landed at A.bottom + 0 (lba001: red line above the
        // green rect).
        let mut a_style = ComputedStyle::new();
        a_style.height = Length::Px(30.0);
        a_style.margin_bottom = Length::Px(16.0);
        let a = LayoutBox::new(BoxType::Block, a_style);

        let mut b_style = ComputedStyle::new();
        b_style.width = Length::Px(100.0);
        b_style.height = Length::Px(60.0);
        let b = LayoutBox::with_position(BoxType::Block, b_style, Position::Absolute);

        let mut c_style = ComputedStyle::new();
        c_style.height = Length::Px(10.0);
        let c = LayoutBox::new(BoxType::Block, c_style);

        let mut root = LayoutBox::new(BoxType::Block, ComputedStyle::new());
        root.children = vec![a, b, c];

        let mut cb = Dimensions::default();
        cb.content = Rect::new(0.0, 0.0, 1000.0, 0.0);
        let mut margin_context = MarginCollapseContext::new();
        let mut float_context = FloatContext::new();
        root.layout_with_collapse(&cb, &mut margin_context, &mut float_context);

        assert_eq!(root.children[0].dimensions.content.y, 0.0);
        assert_eq!(
            root.children[2].dimensions.content.y, 46.0,
            "in-flow C must still get A's 16px bottom margin with an abspos sibling between"
        );
        assert_eq!(
            root.children[1].dimensions.content.y, 46.0,
            "abspos B's static position includes the pending margin it would have had in flow"
        );
    }

    #[test]
    fn dashed_border_matches_blinks_dash_gap_selection() {
        // backgrounds test 3: 10px dashed on a 200x100 border box. Chrome:
        // top side 7 dashes of 20 with 10px gaps (7*20 + 6*10 = 200), left
        // side 4 dashes of 20 with gaps of 20/3.
        use rustkit_css::BorderStyle::{Dashed, Solid};
        assert_eq!(border_dash_pattern(Dashed, 10.0, 200.0), Some((20.0, 10.0)));
        let (dash, gap) = border_dash_pattern(Dashed, 10.0, 100.0).unwrap();
        assert_eq!(dash, 20.0);
        assert!((gap - 20.0 / 3.0).abs() < 1e-4, "gap {}", gap);
        assert_eq!(border_dash_pattern(Solid, 10.0, 200.0), None);
        assert_eq!(border_dash_pattern(Dashed, 10.0, 40.0), None, "too short: solid");

        let mut style = ComputedStyle::new();
        style.border_top_style = Dashed;
        style.border_top_color = Color::from_rgb(51, 51, 51);
        let mut b = LayoutBox::new(BoxType::Block, style);
        b.dimensions.content = Rect::new(10.0, 10.0, 180.0, 80.0);
        b.dimensions.border = EdgeSizes { top: 10.0, right: 0.0, bottom: 0.0, left: 10.0 };
        let list = DisplayList::build(&b);
        let top_dashes = list
            .commands
            .iter()
            .filter(|c| {
                matches!(c, DisplayCommand::SolidColor(_, r)
                    if r.y == 0.0 && r.height == 10.0 && r.width == 20.0)
            })
            .count();
        assert_eq!(top_dashes, 7);
    }

    #[test]
    fn test_float_context() {
        let mut ctx = FloatContext::new();

        // Add a left float
        ctx.add_left(Rect::new(0.0, 0.0, 100.0, 50.0));

        // Check available width at y=25 (within float)
        let (left, right) = ctx.available_width(25.0, 500.0);
        assert_eq!(left, 100.0); // Left edge is after the float
        assert_eq!(right, 500.0); // Right edge is container width

        // Check available width at y=60 (below float)
        let (left, right) = ctx.available_width(60.0, 500.0);
        assert_eq!(left, 0.0); // No float at this y
        assert_eq!(right, 500.0);
    }

    #[test]
    fn test_float_clear() {
        let mut ctx = FloatContext::new();

        ctx.add_left(Rect::new(0.0, 0.0, 100.0, 50.0));
        ctx.add_right(Rect::new(400.0, 0.0, 100.0, 80.0));

        assert_eq!(ctx.clear(Clear::Left), 50.0);
        assert_eq!(ctx.clear(Clear::Right), 80.0);
        assert_eq!(ctx.clear(Clear::Both), 80.0);
        assert_eq!(ctx.clear(Clear::None), 0.0);
    }

    #[test]
    fn test_float_clear_all() {
        let mut ctx = FloatContext::new();
        assert_eq!(ctx.clear_all(), 0.0);

        ctx.add_left(Rect::new(0.0, 0.0, 100.0, 50.0));
        assert_eq!(ctx.clear_all(), 50.0);

        ctx.add_right(Rect::new(400.0, 0.0, 100.0, 80.0));
        assert_eq!(ctx.clear_all(), 80.0);
    }

    #[test]
    fn test_float_find_position_left() {
        let mut ctx = FloatContext::new();
        let container_width = 500.0;

        // First float should go to left edge
        let (x, y) = ctx.find_float_position(Float::Left, 100.0, 50.0, 0.0, container_width);
        assert_eq!(x, 0.0);
        assert_eq!(y, 0.0);

        // Add the float
        ctx.add_left(Rect::new(0.0, 0.0, 100.0, 50.0));

        // Second float should stack beside or below
        let (x2, y2) = ctx.find_float_position(Float::Left, 100.0, 50.0, 0.0, container_width);
        // Should be to the right of first float
        assert_eq!(x2, 100.0);
        assert_eq!(y2, 0.0);
    }

    #[test]
    fn test_float_find_position_right() {
        let mut ctx = FloatContext::new();
        let container_width = 500.0;

        // First right float should go to right edge
        let (x, y) = ctx.find_float_position(Float::Right, 100.0, 50.0, 0.0, container_width);
        assert_eq!(x, 400.0); // 500 - 100
        assert_eq!(y, 0.0);

        // Add the float
        ctx.add_right(Rect::new(400.0, 0.0, 100.0, 50.0));

        // Second right float should stack beside or below
        let (x2, y2) = ctx.find_float_position(Float::Right, 100.0, 50.0, 0.0, container_width);
        // Should be to the left of first float
        assert_eq!(x2, 300.0);
        assert_eq!(y2, 0.0);
    }

    #[test]
    fn test_float_available_rect() {
        let mut ctx = FloatContext::new();
        let container_width = 500.0;

        ctx.add_left(Rect::new(0.0, 0.0, 100.0, 50.0));
        ctx.add_right(Rect::new(400.0, 0.0, 100.0, 50.0));

        // Available rect at y=0 should be between floats
        let (x, width) = ctx.available_rect(0.0, 25.0, container_width);
        assert_eq!(x, 100.0);
        assert_eq!(width, 300.0);

        // Available rect at y=60 (below both floats) should be full width
        let (x2, width2) = ctx.available_rect(60.0, 10.0, container_width);
        assert_eq!(x2, 0.0);
        assert_eq!(width2, 500.0);
    }

    #[test]
    fn test_float_has_floats_at() {
        let mut ctx = FloatContext::new();

        ctx.add_left(Rect::new(0.0, 10.0, 100.0, 50.0));

        assert!(!ctx.has_floats_at(0.0));
        assert!(!ctx.has_floats_at(5.0));
        assert!(ctx.has_floats_at(10.0));
        assert!(ctx.has_floats_at(30.0));
        assert!(ctx.has_floats_at(59.0));
        assert!(!ctx.has_floats_at(60.0));
    }

    #[test]
    fn test_float_fits() {
        let mut ctx = FloatContext::new();
        let container_width = 500.0;

        ctx.add_left(Rect::new(0.0, 0.0, 100.0, 50.0));

        // Should fit to the right of existing float
        assert!(ctx.float_fits(100.0, 0.0, 100.0, 50.0, container_width));

        // Should not fit overlapping existing float
        assert!(!ctx.float_fits(50.0, 25.0, 100.0, 50.0, container_width));

        // Should fit below existing float
        assert!(ctx.float_fits(0.0, 50.0, 100.0, 50.0, container_width));

        // Should not fit outside container
        assert!(!ctx.float_fits(450.0, 0.0, 100.0, 50.0, container_width));
    }

    #[test]
    fn test_float_floats_in_range() {
        let mut ctx = FloatContext::new();

        ctx.add_left(Rect::new(0.0, 0.0, 100.0, 50.0));
        ctx.add_right(Rect::new(400.0, 30.0, 100.0, 50.0));

        // Range that includes both floats
        let floats = ctx.floats_in_range(0.0, 80.0);
        assert_eq!(floats.len(), 2);

        // Range that only includes left float
        let floats = ctx.floats_in_range(0.0, 25.0);
        assert_eq!(floats.len(), 1);
        assert_eq!(floats[0].float_type, Float::Left);

        // Range below all floats
        let floats = ctx.floats_in_range(100.0, 150.0);
        assert!(floats.is_empty());
    }

    #[test]
    fn test_float_remove_floats_above() {
        let mut ctx = FloatContext::new();

        ctx.add_left(Rect::new(0.0, 0.0, 100.0, 50.0));
        ctx.add_left(Rect::new(0.0, 100.0, 100.0, 50.0));
        ctx.add_right(Rect::new(400.0, 0.0, 100.0, 30.0));

        assert_eq!(ctx.float_count(), 3);

        // Remove floats that end above y=50
        ctx.remove_floats_above(50.0);
        assert_eq!(ctx.float_count(), 1); // Only the second left float remains
    }

    #[test]
    fn test_float_is_empty() {
        let mut ctx = FloatContext::new();
        assert!(ctx.is_empty());

        ctx.add_left(Rect::new(0.0, 0.0, 100.0, 50.0));
        assert!(!ctx.is_empty());
    }

    #[test]
    fn test_float_count() {
        let mut ctx = FloatContext::new();
        assert_eq!(ctx.float_count(), 0);

        ctx.add_left(Rect::new(0.0, 0.0, 100.0, 50.0));
        assert_eq!(ctx.float_count(), 1);

        ctx.add_right(Rect::new(400.0, 0.0, 100.0, 50.0));
        assert_eq!(ctx.float_count(), 2);
    }

    #[test]
    fn test_rects_overlap() {
        let a = Rect::new(0.0, 0.0, 100.0, 100.0);

        // Overlapping
        assert!(rects_overlap(&a, &Rect::new(50.0, 50.0, 100.0, 100.0)));

        // Adjacent (not overlapping)
        assert!(!rects_overlap(&a, &Rect::new(100.0, 0.0, 100.0, 100.0)));
        assert!(!rects_overlap(&a, &Rect::new(0.0, 100.0, 100.0, 100.0)));

        // No overlap
        assert!(!rects_overlap(&a, &Rect::new(200.0, 200.0, 100.0, 100.0)));
    }

    #[test]
    fn test_position_offsets() {
        let style = ComputedStyle::new();
        let mut layout_box = LayoutBox::with_position(BoxType::Block, style, Position::Relative);
        layout_box.set_offsets(Some(10.0), None, None, Some(20.0));

        assert_eq!(layout_box.offsets.top, Some(10.0));
        assert_eq!(layout_box.offsets.left, Some(20.0));
        assert_eq!(layout_box.offsets.right, None);
        assert_eq!(layout_box.offsets.bottom, None);
    }


    /// The containing block a flow child is handed carries the parent's
    /// CURSOR in `content.height` (the static-position trick), so a
    /// percentage height read 0 and `calculate_block_height` fell back to the
    /// VIEWPORT. websuite/micro/rounded-corners test 7 is the corpus
    /// instance: `.test-box { height: 100px }` holding
    /// `.inner { height: 100% }` came out 1000 — the case's viewport height —
    /// against Chrome's 100, the largest single geometry error in the 26-case
    /// corpus. T-RED without the fix: 1000.
    #[test]
    fn a_percentage_height_child_resolves_against_its_definite_parent() {
        let mut parent_style = ComputedStyle::new();
        parent_style.width = Length::Px(150.0);
        parent_style.height = Length::Px(100.0);
        let mut parent = LayoutBox::new(BoxType::Block, parent_style);

        let mut inner_style = ComputedStyle::new();
        inner_style.width = Length::Percent(100.0);
        inner_style.height = Length::Percent(100.0);
        parent
            .children
            .push(LayoutBox::new(BoxType::Block, inner_style));
        parent.set_viewport(900.0, 1000.0);

        let viewport = Dimensions {
            content: Rect::new(0.0, 0.0, 900.0, 1000.0),
            ..Default::default()
        };
        parent.layout(&viewport);

        let inner = &parent.children[0];
        assert_eq!(
            inner.dimensions.content.height, 100.0,
            "height:100% of a 100px parent is 100"
        );
        assert!(
            (inner.dimensions.content.height - 1000.0).abs() > 0.5,
            "it must not be the viewport height"
        );
        assert_eq!(inner.dimensions.content.width, 150.0);
        assert_eq!(
            parent.dimensions.content.height, 100.0,
            "the parent keeps its own specified height"
        );
    }

    /// The same box inside an INLINE-BLOCK, which is the shape the corpus
    /// actually has (`.test-box { display: inline-block; height: 100px }`).
    /// The inline-block branch builds its own containing block with
    /// `content.height = 0` before laying the child out, so it needs the
    /// definite height passed explicitly too.
    #[test]
    fn a_percentage_height_child_of_an_inline_block_takes_the_inline_blocks_height() {
        let mut wrapper_style = ComputedStyle::new();
        wrapper_style.width = Length::Px(900.0);
        let mut wrapper = LayoutBox::new(BoxType::Block, wrapper_style);

        let mut box_style = ComputedStyle::new();
        box_style.display = rustkit_css::Display::InlineBlock;
        box_style.width = Length::Px(150.0);
        box_style.height = Length::Px(100.0);
        let mut test_box = LayoutBox::new(BoxType::Block, box_style);

        let mut inner_style = ComputedStyle::new();
        inner_style.width = Length::Percent(100.0);
        inner_style.height = Length::Percent(100.0);
        test_box
            .children
            .push(LayoutBox::new(BoxType::Block, inner_style));
        wrapper.children.push(test_box);
        wrapper.set_viewport(900.0, 1000.0);

        let viewport = Dimensions {
            content: Rect::new(0.0, 0.0, 900.0, 1000.0),
            ..Default::default()
        };
        let mut mc = MarginCollapseContext::new();
        let mut fc = FloatContext::new();
        wrapper.layout_with_collapse(&viewport, &mut mc, &mut fc);

        let inner = &wrapper.children[0].children[0];
        assert_eq!(inner.dimensions.content.height, 100.0);
        assert_eq!(
            wrapper.children[0].dimensions.content.height, 100.0,
            "the inline-block keeps its own height"
        );
    }

    /// `box-sizing: border-box` on the parent: the specified 100px is the
    /// BORDER box, so the content box a percentage child fills is
    /// 100 - padding - border. A fix that handed the child the specified
    /// length instead of the content height reads 100 here.
    #[test]
    fn a_percentage_height_child_fills_a_border_box_parents_content_box() {
        let mut parent_style = ComputedStyle::new();
        parent_style.width = Length::Px(150.0);
        parent_style.height = Length::Px(100.0);
        parent_style.box_sizing = BoxSizing::BorderBox;
        parent_style.padding_top = Length::Px(8.0);
        parent_style.padding_bottom = Length::Px(8.0);
        parent_style.border_top_width = Length::Px(2.0);
        parent_style.border_bottom_width = Length::Px(2.0);
        let mut parent = LayoutBox::new(BoxType::Block, parent_style);

        let mut inner_style = ComputedStyle::new();
        inner_style.height = Length::Percent(50.0);
        parent
            .children
            .push(LayoutBox::new(BoxType::Block, inner_style));
        parent.set_viewport(900.0, 1000.0);

        let viewport = Dimensions {
            content: Rect::new(0.0, 0.0, 900.0, 1000.0),
            ..Default::default()
        };
        parent.layout(&viewport);

        assert_eq!(parent.dimensions.content.height, 80.0);
        assert_eq!(
            parent.children[0].dimensions.content.height, 40.0,
            "50% of the parent's 80px CONTENT box"
        );
    }

    /// The boundary the fix must not cross: an auto-height parent's height
    /// DOES depend on its children, so it hands them no definite base and
    /// keeps its content height. A fix that handed children a base of 0
    /// (or the cursor) would collapse this parent to 0.
    #[test]
    fn an_auto_height_parent_still_takes_its_childrens_flow() {
        let mut parent_style = ComputedStyle::new();
        parent_style.width = Length::Px(150.0);
        let mut parent = LayoutBox::new(BoxType::Block, parent_style);

        let mut inner_style = ComputedStyle::new();
        inner_style.height = Length::Px(37.0);
        parent
            .children
            .push(LayoutBox::new(BoxType::Block, inner_style));
        parent.set_viewport(900.0, 1000.0);

        let viewport = Dimensions {
            content: Rect::new(0.0, 0.0, 900.0, 1000.0),
            ..Default::default()
        };
        parent.layout(&viewport);
        assert_eq!(parent.dimensions.content.height, 37.0);
        assert_eq!(parent.children[0].dimensions.content.height, 37.0);
    }

    /// The other half of the containing block's double duty: `content.height`
    /// on the child's containing block is still the parent's FLOW CURSOR, so
    /// two children of a definite-height parent stack instead of overlapping.
    /// A fix that overwrote the cursor with the definite height would lay the
    /// second child at the parent's bottom edge.
    #[test]
    fn a_definite_height_parent_still_stacks_its_children_on_the_cursor() {
        let mut parent_style = ComputedStyle::new();
        parent_style.width = Length::Px(150.0);
        parent_style.height = Length::Px(100.0);
        let mut parent = LayoutBox::new(BoxType::Block, parent_style);

        for _ in 0..2 {
            let mut kid_style = ComputedStyle::new();
            kid_style.height = Length::Percent(25.0);
            parent
                .children
                .push(LayoutBox::new(BoxType::Block, kid_style));
        }
        parent.set_viewport(900.0, 1000.0);

        let viewport = Dimensions {
            content: Rect::new(0.0, 0.0, 900.0, 1000.0),
            ..Default::default()
        };
        parent.layout(&viewport);

        let top = parent.dimensions.content.y;
        assert_eq!(parent.children[0].dimensions.content.height, 25.0);
        assert_eq!(parent.children[1].dimensions.content.height, 25.0);
        assert_eq!(parent.children[0].dimensions.content.y, top);
        assert_eq!(
            parent.children[1].dimensions.content.y,
            top + 25.0,
            "the second child stacks on the cursor, not on the definite height"
        );
    }

    /// The rule, not the example: a parent whose own height is a PERCENTAGE
    /// of a definite grandparent is itself definite, so the chain resolves
    /// all the way down. A helper that only answered for `Length::Px` would
    /// leave this child on the viewport fallback, and every guard written
    /// against the 100px example would stay green.
    #[test]
    fn a_percentage_height_chain_resolves_through_a_percentage_parent() {
        let mut grand_style = ComputedStyle::new();
        grand_style.width = Length::Px(150.0);
        grand_style.height = Length::Px(200.0);
        let mut grand = LayoutBox::new(BoxType::Block, grand_style);

        let mut parent_style = ComputedStyle::new();
        parent_style.height = Length::Percent(50.0);
        let mut parent = LayoutBox::new(BoxType::Block, parent_style);

        let mut child_style = ComputedStyle::new();
        child_style.height = Length::Percent(50.0);
        parent
            .children
            .push(LayoutBox::new(BoxType::Block, child_style));
        grand.children.push(parent);
        grand.set_viewport(900.0, 1000.0);

        let viewport = Dimensions {
            content: Rect::new(0.0, 0.0, 900.0, 1000.0),
            ..Default::default()
        };
        grand.layout(&viewport);

        assert_eq!(grand.children[0].dimensions.content.height, 100.0);
        assert_eq!(
            grand.children[0].children[0].dimensions.content.height,
            50.0,
            "50% of the parent's used 100px, not of the viewport"
        );
    }

    /// The boundary this change deliberately does NOT cross, pinned so the
    /// next unit has something to flip. An auto-height parent's height
    /// depends on its children, so it hands them no definite base — and the
    /// child then keeps the historical `self.viewport.1` fallback in
    /// `calculate_block_height`. CSS 2.1 §10.5 says the child computes to
    /// `auto` here (Chrome gives it its content height, 0), so this
    /// assertion records a KNOWN-WRONG value on purpose; what it guards is
    /// that the helper stays silent for `auto`, rather than handing the
    /// child a base of 0 and making the two cases indistinguishable.
    #[test]
    fn an_auto_height_parent_hands_its_percentage_child_no_definite_base() {
        let mut parent_style = ComputedStyle::new();
        parent_style.width = Length::Px(150.0);
        let mut parent = LayoutBox::new(BoxType::Block, parent_style);

        let mut child_style = ComputedStyle::new();
        child_style.height = Length::Percent(100.0);
        parent
            .children
            .push(LayoutBox::new(BoxType::Block, child_style));
        parent.set_viewport(900.0, 1000.0);

        let viewport = Dimensions {
            content: Rect::new(0.0, 0.0, 900.0, 1000.0),
            ..Default::default()
        };
        parent.layout(&viewport);
        assert_eq!(
            parent.children[0].dimensions.content.height, 1000.0,
            "unchanged by this unit: the viewport fallback, not a 0 base \
             (§10.5 wants `auto`, i.e. 0 — that is the next unit)"
        );
    }

    /// The inline-block branch builds its own containing block with
    /// `content.height = 0`, so an inline-block whose OWN height is a
    /// percentage has nothing to resolve against unless the parent's definite
    /// height is passed alongside it. The 100px-inline-block guard above
    /// cannot see this: a box with a `Px` height answers for its children
    /// whatever base it was handed.
    #[test]
    fn a_percentage_height_inline_block_resolves_against_its_definite_parent() {
        let mut wrapper_style = ComputedStyle::new();
        wrapper_style.width = Length::Px(900.0);
        wrapper_style.height = Length::Px(120.0);
        let mut wrapper = LayoutBox::new(BoxType::Block, wrapper_style);

        let mut box_style = ComputedStyle::new();
        box_style.display = rustkit_css::Display::InlineBlock;
        box_style.width = Length::Px(150.0);
        box_style.height = Length::Percent(50.0);
        wrapper
            .children
            .push(LayoutBox::new(BoxType::Block, box_style));
        wrapper.set_viewport(900.0, 1000.0);

        let viewport = Dimensions {
            content: Rect::new(0.0, 0.0, 900.0, 1000.0),
            ..Default::default()
        };
        let mut mc = MarginCollapseContext::new();
        let mut fc = FloatContext::new();
        wrapper.layout_with_collapse(&viewport, &mut mc, &mut fc);

        assert_eq!(
            wrapper.children[0].dimensions.content.height, 60.0,
            "50% of the wrapper's 120px, not of the viewport"
        );
    }

    fn calc_sum(value: &str) -> Length {
        rustkit_css::parse_length(value).unwrap_or_else(|| panic!("{value} did not parse"))
    }

    /// chrome_rustkit's `.sidebar`: `position: absolute; top: 84px;
    /// height: calc(100% - 84px)` in a 1280x100 chrome strip. Chrome gives it
    /// 16px. RustKit gave it 203 — its content height — because
    /// `calc(100% - 84px)` did not parse, so the declaration was dropped and
    /// the height was `auto`.
    #[test]
    fn a_calc_height_resolves_against_its_parents_definite_height() {
        let mut parent_style = ComputedStyle::new();
        parent_style.width = Length::Px(1280.0);
        parent_style.height = Length::Px(100.0);
        let mut parent = LayoutBox::new(BoxType::Block, parent_style);

        let mut child_style = ComputedStyle::new();
        child_style.width = Length::Px(220.0);
        child_style.height = calc_sum("calc(100% - 84px)");
        let mut child = LayoutBox::new(BoxType::Block, child_style);
        // Content taller than the calc asks for, so an `auto` fallback is
        // visibly different from the resolved value rather than coincidentally
        // equal to it.
        let mut filler_style = ComputedStyle::new();
        filler_style.height = Length::Px(203.0);
        child
            .children
            .push(LayoutBox::new(BoxType::Block, filler_style));
        parent.children.push(child);
        parent.set_viewport(1280.0, 100.0);

        let viewport = Dimensions {
            content: Rect::new(0.0, 0.0, 1280.0, 100.0),
            ..Default::default()
        };
        parent.layout(&viewport);

        assert_eq!(
            parent.children[0].dimensions.content.height, 16.0,
            "100% of the parent's 100px minus 84px, not the 203px of content"
        );
    }

    /// The percentage half and the absolute half must take the SAME base a
    /// bare percentage would. An `auto`-height parent hands no definite base,
    /// and the viewport fallback stands — the behaviour `Length::Percent`
    /// already has, which is what makes `calc()` a length rather than a
    /// second rule.
    #[test]
    fn a_calc_height_takes_the_same_base_a_bare_percentage_takes() {
        for (height, expected) in [
            (Length::Percent(50.0), 500.0),
            (calc_sum("calc(50% - 10px)"), 490.0),
        ] {
            let mut parent_style = ComputedStyle::new();
            parent_style.width = Length::Px(150.0);
            let mut parent = LayoutBox::new(BoxType::Block, parent_style);

            let mut child_style = ComputedStyle::new();
            child_style.height = height.clone();
            parent
                .children
                .push(LayoutBox::new(BoxType::Block, child_style));
            parent.set_viewport(900.0, 1000.0);

            let viewport = Dimensions {
                content: Rect::new(0.0, 0.0, 900.0, 1000.0),
                ..Default::default()
            };
            parent.layout(&viewport);
            assert_eq!(
                parent.children[0].dimensions.content.height, expected,
                "{height:?} must resolve against the same base as the percentage beside it"
            );
        }
    }

    /// A `calc()` with no percentage term is definite wherever it appears —
    /// including under a parent that has no definite height to offer. Falling
    /// back to the viewport there would make `calc(2em + 4px)` 1000px.
    #[test]
    fn a_calc_with_no_percentage_needs_no_base() {
        let mut parent_style = ComputedStyle::new();
        parent_style.width = Length::Px(150.0);
        let mut parent = LayoutBox::new(BoxType::Block, parent_style);

        let mut child_style = ComputedStyle::new();
        child_style.font_size = Length::Px(20.0);
        child_style.height = calc_sum("calc(2em + 4px)");
        parent
            .children
            .push(LayoutBox::new(BoxType::Block, child_style));
        parent.set_viewport(900.0, 1000.0);

        let viewport = Dimensions {
            content: Rect::new(0.0, 0.0, 900.0, 1000.0),
            ..Default::default()
        };
        parent.layout(&viewport);
        assert_eq!(parent.children[0].dimensions.content.height, 44.0);

        // …and with no viewport either. Mutation probe M11 (2026-09-21)
        // survived the case above: where the calc carries no percentage term
        // the basis cannot change the answer, so the `percent == 0` arm is
        // unobservable UNTIL the viewport fallback is also gone. This is the
        // only shape that tells `Some(0.0)` from `None` — and `None` leaves
        // the height unset, i.e. at its content.
        let mut bare_parent = LayoutBox::new(BoxType::Block, ComputedStyle::new());
        let mut bare_child = ComputedStyle::new();
        bare_child.font_size = Length::Px(20.0);
        bare_child.height = calc_sum("calc(2em + 4px)");
        bare_parent
            .children
            .push(LayoutBox::new(BoxType::Block, bare_child));
        bare_parent.set_viewport(0.0, 0.0);
        bare_parent.layout(&Dimensions::default());
        assert_eq!(
            bare_parent.children[0].dimensions.content.height, 44.0,
            "a calc with no percentage term is definite with no base at all"
        );
    }

    /// A calc-sized parent is a definite base for ITS percentage children —
    /// the rule `a_percentage_height_chain_resolves_through_a_percentage_parent`
    /// states for percentages, applied to the variant beside it.
    #[test]
    fn a_calc_height_parent_is_itself_a_definite_base() {
        let mut outer_style = ComputedStyle::new();
        outer_style.width = Length::Px(150.0);
        outer_style.height = Length::Px(200.0);
        let mut outer = LayoutBox::new(BoxType::Block, outer_style);

        let mut mid_style = ComputedStyle::new();
        mid_style.height = calc_sum("calc(100% - 40px)");
        let mut mid = LayoutBox::new(BoxType::Block, mid_style);

        let mut inner_style = ComputedStyle::new();
        inner_style.height = Length::Percent(50.0);
        mid.children
            .push(LayoutBox::new(BoxType::Block, inner_style));
        outer.children.push(mid);
        outer.set_viewport(900.0, 1000.0);

        let viewport = Dimensions {
            content: Rect::new(0.0, 0.0, 900.0, 1000.0),
            ..Default::default()
        };
        outer.layout(&viewport);
        assert_eq!(outer.children[0].dimensions.content.height, 160.0);
        assert_eq!(
            outer.children[0].children[0].dimensions.content.height, 80.0,
            "50% of the calc parent's 160px, not of the viewport"
        );
    }

    /// The abspos twin of `a_calc_height_resolves_against_its_parents_definite_height`.
    /// During flow layout an out-of-flow child is handed a stand-in whose
    /// `content.height` is the parent's flow cursor, so a percentage (or a
    /// calc carrying one) resolves against "content laid out so far" —
    /// chrome_rustkit's `.sidebar` read 84 and came out zero tall. Both
    /// heights are asserted in one test because the rule is one rule: a calc
    /// must take the same base the bare percentage beside it takes.
    #[test]
    fn an_out_of_flow_percentage_height_resolves_against_its_containing_block() {
        for (height, expected) in [
            (Length::Percent(100.0), 100.0),
            (calc_sum("calc(100% - 84px)"), 16.0),
        ] {
            let mut parent_style = ComputedStyle::new();
            parent_style.width = Length::Px(1280.0);
            parent_style.height = Length::Px(100.0);
            let mut parent = LayoutBox::new(BoxType::Block, parent_style);

            // Two in-flow siblings ahead of it, so the flow cursor (84) is a
            // different number from the containing block's height (100) and
            // the test can tell which one was used.
            for h in [40.0_f32, 44.0] {
                let mut sib = ComputedStyle::new();
                sib.height = Length::Px(h);
                parent.children.push(LayoutBox::new(BoxType::Block, sib));
            }

            let mut sidebar_style = ComputedStyle::new();
            sidebar_style.position = rustkit_css::Position::Absolute;
            sidebar_style.top = Some(Length::Px(84.0));
            sidebar_style.left = Some(Length::Px(0.0));
            sidebar_style.width = Length::Px(220.0);
            sidebar_style.height = height.clone();
            let mut sidebar = LayoutBox::new(BoxType::Block, sidebar_style);
            sidebar.position = Position::Absolute;
            parent.children.push(sidebar);
            parent.set_viewport(1280.0, 100.0);

            let viewport = Dimensions {
                content: Rect::new(0.0, 0.0, 1280.0, 100.0),
                ..Default::default()
            };
            parent.layout(&viewport);

            assert_eq!(
                parent.children[2].dimensions.content.height, expected,
                "{height:?} must resolve against the containing block's 100px, \
                 not the 84px flow cursor"
            );
        }
    }

    /// `min-height` and `max-height` read a calc through the same basis their
    /// `Percent` arm uses. Without their own arms a calc min-height is 0 and a
    /// calc max-height is infinite — both silently absent rather than wrong,
    /// which is the harder kind to notice.
    #[test]
    fn a_calc_min_and_max_height_clamp_like_the_percentage_beside_them() {
        let viewport = Dimensions {
            content: Rect::new(0.0, 0.0, 900.0, 1000.0),
            ..Default::default()
        };

        let mut floored_style = ComputedStyle::new();
        floored_style.width = Length::Px(150.0);
        floored_style.height = Length::Px(10.0);
        floored_style.min_height = calc_sum("calc(50% - 100px)");
        let mut floored = LayoutBox::new(BoxType::Block, floored_style);
        floored.set_viewport(900.0, 1000.0);
        floored.layout(&viewport);
        assert_eq!(
            floored.dimensions.content.height, 400.0,
            "50% of the 1000px viewport minus 100px, the basis the Percent arm uses"
        );

        let mut capped_style = ComputedStyle::new();
        capped_style.width = Length::Px(150.0);
        capped_style.height = Length::Px(900.0);
        capped_style.max_height = calc_sum("calc(50% - 100px)");
        let mut capped = LayoutBox::new(BoxType::Block, capped_style);
        capped.set_viewport(900.0, 1000.0);
        capped.layout(&viewport);
        assert_eq!(capped.dimensions.content.height, 400.0);
    }

    /// A calc width needs no arm of its own — `calculate_block_width` already
    /// funnels every non-`auto` width through `length_to_px`. Asserted rather
    /// than assumed, because "it already works" is the claim most likely to
    /// stop being true silently.
    #[test]
    fn a_calc_width_resolves_through_the_existing_width_path() {
        let mut parent_style = ComputedStyle::new();
        parent_style.width = Length::Px(400.0);
        let mut parent = LayoutBox::new(BoxType::Block, parent_style);

        let mut child_style = ComputedStyle::new();
        child_style.width = calc_sum("calc(100% - 40px)");
        parent
            .children
            .push(LayoutBox::new(BoxType::Block, child_style));
        parent.set_viewport(900.0, 1000.0);

        let viewport = Dimensions {
            content: Rect::new(0.0, 0.0, 900.0, 1000.0),
            ..Default::default()
        };
        parent.layout(&viewport);
        assert_eq!(parent.children[0].dimensions.content.width, 360.0);
    }

    /// A border-box parent whose padding exceeds its specified height has a
    /// content box of zero, never a negative one — and the base it hands a
    /// percentage child is that clamped value. Without the floor the child
    /// comes out NEGATIVE, which no other guard here can reach.
    #[test]
    fn a_percentage_child_of_an_over_padded_border_box_parent_is_never_negative() {
        let mut parent_style = ComputedStyle::new();
        parent_style.width = Length::Px(150.0);
        parent_style.height = Length::Px(10.0);
        parent_style.box_sizing = BoxSizing::BorderBox;
        parent_style.padding_top = Length::Px(20.0);
        parent_style.padding_bottom = Length::Px(20.0);
        let mut parent = LayoutBox::new(BoxType::Block, parent_style);

        let mut child_style = ComputedStyle::new();
        child_style.height = Length::Percent(100.0);
        parent
            .children
            .push(LayoutBox::new(BoxType::Block, child_style));
        parent.set_viewport(900.0, 1000.0);

        let viewport = Dimensions {
            content: Rect::new(0.0, 0.0, 900.0, 1000.0),
            ..Default::default()
        };
        parent.layout(&viewport);
        assert_eq!(parent.children[0].dimensions.content.height, 0.0);
    }

    /// The root case the public `f32` entry still has to get right: a
    /// containing block with NO height at all (the shape
    /// `layout(&Dimensions::default())` hands the root) is "no definite base",
    /// not a definite zero, so the viewport fallback stands. Converting it to
    /// `Some(0.0)` at that boundary would size every percentage-height root
    /// box to zero.
    #[test]
    fn a_zero_height_containing_block_is_absent_not_a_definite_zero() {
        let mut root_style = ComputedStyle::new();
        root_style.width = Length::Px(150.0);
        root_style.height = Length::Percent(100.0);
        let mut root = LayoutBox::new(BoxType::Block, root_style);
        root.set_viewport(900.0, 1000.0);

        let containing = Dimensions {
            content: Rect::new(0.0, 0.0, 900.0, 0.0),
            ..Default::default()
        };
        root.layout(&containing);
        assert_eq!(
            root.dimensions.content.height, 1000.0,
            "no containing-block height means the viewport fallback, not 0"
        );
    }

    /// WPT overflow-wrap-anywhere-001: `::after { position:absolute; inset:0 }`
    /// on a `height: 100px` div holding 54px of flow content was 54px tall —
    /// the abspos child resolved `bottom` against the parent's flow cursor
    /// (the static-position stand-in), not the parent's definite height.
    /// T-RED without `reanchor_absolute`: height reads 54, not 100.
    #[test]
    fn abspos_inset_fills_parents_definite_height_not_its_flow_cursor() {
        let mut parent_style = ComputedStyle::new();
        parent_style.width = Length::Px(100.0);
        parent_style.height = Length::Px(100.0);
        let mut parent = LayoutBox::with_position(BoxType::Block, parent_style, Position::Relative);

        let mut flow_style = ComputedStyle::new();
        flow_style.height = Length::Px(54.0);
        parent
            .children
            .push(LayoutBox::new(BoxType::Block, flow_style));

        let mut cover =
            LayoutBox::with_position(BoxType::Block, ComputedStyle::new(), Position::Absolute);
        cover.set_offsets(Some(0.0), Some(0.0), Some(0.0), Some(0.0));
        parent.children.push(cover);

        let viewport = Dimensions {
            content: Rect::new(0.0, 0.0, 800.0, 600.0),
            ..Default::default()
        };
        parent.layout(&viewport);

        let cover = &parent.children[1];
        assert_eq!(
            cover.dimensions.content.height, 100.0,
            "inset:0 must stretch to the containing block's definite height"
        );
        assert_eq!(cover.dimensions.content.y, parent.dimensions.content.y);
        assert_eq!(cover.dimensions.content.width, 100.0);
    }

    /// An auto-height parent anchors its abspos child to its FINAL content
    /// height (30px of flow here) — the same answer the flow cursor gave
    /// when the child came last, now guaranteed regardless of order.
    /// image-gallery's `.aspect-box > .content`, driven through the REAL
    /// entry point rather than through `layout_flex_container` directly.
    ///
    /// This guard exists because the flex-side guards cannot see the caller.
    /// They call `layout_flex_container_in` and pass the containing block
    /// themselves, so they hold the rule but not the wiring: a `lib.rs` call
    /// site that hands over `None` puts every item back at the top edge and
    /// leaves all six of them green. `LayoutBox::layout` is what grid Phase 9
    /// uses for an out-of-flow grandchild, so this is the path the page takes.
    ///
    /// 100px card, `inset: 0` column flex container, `justify-content: center`,
    /// two items summing 50: the free space is 50 and the stack starts at 25.
    /// Justifying in the 50px flow cursor instead starts it at 0.
    #[test]
    fn an_inset_overlays_flex_line_centres_in_the_card_through_the_layout_entry_point() {
        let mut card_style = ComputedStyle::new();
        card_style.width = Length::Px(100.0);
        card_style.height = Length::Px(100.0);
        let mut card = LayoutBox::with_position(BoxType::Block, card_style, Position::Relative);

        let mut overlay_style = ComputedStyle::new();
        overlay_style.display = rustkit_css::Display::Flex;
        overlay_style.flex_direction = rustkit_css::FlexDirection::Column;
        overlay_style.justify_content = rustkit_css::JustifyContent::Center;
        let mut overlay =
            LayoutBox::with_position(BoxType::Block, overlay_style, Position::Absolute);
        overlay.set_offsets(Some(0.0), Some(0.0), Some(0.0), Some(0.0));
        for h in [30.0, 20.0] {
            let mut item_style = ComputedStyle::new();
            item_style.height = Length::Px(h);
            overlay
                .children
                .push(LayoutBox::new(BoxType::Block, item_style));
        }
        card.children.push(overlay);

        let viewport = Dimensions {
            content: Rect::new(0.0, 0.0, 800.0, 600.0),
            ..Default::default()
        };
        card.layout(&viewport);

        let overlay = &card.children[0];
        assert_eq!(
            overlay.dimensions.content.height, 100.0,
            "the overlay itself must stretch to the card (the precondition)"
        );
        let lead = overlay.children[0].dimensions.content.y - overlay.dimensions.content.y;
        assert!(
            (lead - 25.0).abs() < 0.01,
            "50 of content centred in the card's 100 leaves 25 above, not {lead} \
             — the flex line was justified in the flow cursor"
        );
    }

    /// The re-anchor must not resize or re-justify a box that is NOT
    /// inset-definite. `top: 0` alone leaves the height indefinite (CSS2
    /// §10.6.4 needs both offsets), so such a flex container stays sized by
    /// its content and its line keeps the packing it already had — the
    /// re-justify has to be gated on the rule, not on being a flex container
    /// that happens to be out of flow.
    #[test]
    fn the_reanchor_leaves_a_flex_container_with_only_one_inset_alone() {
        let mut parent_style = ComputedStyle::new();
        parent_style.width = Length::Px(100.0);
        parent_style.height = Length::Px(200.0);
        let mut parent = LayoutBox::with_position(BoxType::Block, parent_style, Position::Relative);

        let mut overlay_style = ComputedStyle::new();
        overlay_style.display = rustkit_css::Display::Flex;
        overlay_style.flex_direction = rustkit_css::FlexDirection::Column;
        overlay_style.justify_content = rustkit_css::JustifyContent::Center;
        let mut overlay =
            LayoutBox::with_position(BoxType::Block, overlay_style, Position::Absolute);
        overlay.set_offsets(Some(0.0), None, None, None);
        for h in [30.0, 20.0] {
            let mut item_style = ComputedStyle::new();
            item_style.height = Length::Px(h);
            overlay
                .children
                .push(LayoutBox::new(BoxType::Block, item_style));
        }
        parent.children.push(overlay);

        let viewport = Dimensions {
            content: Rect::new(0.0, 0.0, 800.0, 600.0),
            ..Default::default()
        };
        parent.layout(&viewport);

        let overlay = &parent.children[0];
        assert_eq!(
            overlay.dimensions.content.height, 50.0,
            "one inset is not a definite height: the container is its content's 50"
        );
        let lead = overlay.children[0].dimensions.content.y - overlay.dimensions.content.y;
        assert!(
            lead.abs() < 0.01,
            "no free space in a content-sized container: expected 0, got {lead}"
        );
    }

    /// The re-anchor must not touch `position: fixed` (viewport containing
    /// block) or a parent whose height is auto (nothing definite to anchor
    /// to — the flow-cursor stand-in stays the best available answer).
    #[test]
    fn abspos_reanchor_uses_auto_height_parents_final_height() {
        let mut parent_style = ComputedStyle::new();
        parent_style.width = Length::Px(100.0);
        let mut parent = LayoutBox::with_position(BoxType::Block, parent_style, Position::Relative);
        let mut flow_style = ComputedStyle::new();
        flow_style.height = Length::Px(30.0);
        parent
            .children
            .push(LayoutBox::new(BoxType::Block, flow_style));
        let mut cover =
            LayoutBox::with_position(BoxType::Block, ComputedStyle::new(), Position::Absolute);
        cover.set_offsets(Some(0.0), Some(0.0), Some(0.0), Some(0.0));
        parent.children.push(cover);

        let viewport = Dimensions {
            content: Rect::new(0.0, 0.0, 800.0, 600.0),
            ..Default::default()
        };
        parent.layout(&viewport);
        assert_eq!(parent.children[1].dimensions.content.height, 30.0);
    }

    /// n46 (settings' toggle knobs 21px above their sliders): a `bottom: 2px`
    /// knob inside an `inset: 0` slider inside a 26px-tall positioned box.
    /// The slider's height comes from its inset stretch, which resolved
    /// AFTER its children laid out — so the knob's `bottom` was measured
    /// against a 0px-tall stand-in and sat against the slider's top. T-RED
    /// without the post-layout re-anchor: knob y = slider.y - 22.
    #[test]
    fn abspos_bottom_inside_inset_stretched_parent_anchors_to_stretched_height() {
        let mut toggle_style = ComputedStyle::new();
        toggle_style.width = Length::Px(48.0);
        toggle_style.height = Length::Px(26.0);
        let mut toggle = LayoutBox::with_position(BoxType::Block, toggle_style, Position::Relative);

        let mut slider_style = ComputedStyle::new();
        slider_style.border_top_width = Length::Px(1.0);
        slider_style.border_bottom_width = Length::Px(1.0);
        slider_style.border_left_width = Length::Px(1.0);
        slider_style.border_right_width = Length::Px(1.0);
        let mut slider = LayoutBox::with_position(BoxType::Block, slider_style, Position::Absolute);
        slider.set_offsets(Some(0.0), Some(0.0), Some(0.0), Some(0.0));

        let mut knob_style = ComputedStyle::new();
        knob_style.width = Length::Px(20.0);
        knob_style.height = Length::Px(20.0);
        let mut knob = LayoutBox::with_position(BoxType::Block, knob_style, Position::Absolute);
        knob.set_offsets(None, None, Some(2.0), Some(2.0));
        slider.children.push(knob);
        toggle.children.push(slider);

        let viewport = Dimensions {
            content: Rect::new(0.0, 0.0, 800.0, 600.0),
            ..Default::default()
        };
        toggle.layout(&viewport);

        // A bare root lays out at the stand-in's cursor (viewport bottom);
        // everything below is relative to the toggle's border box.
        let (tx, ty) = (
            toggle.dimensions.border_box().x,
            toggle.dimensions.border_box().y,
        );
        let slider = &toggle.children[0];
        assert_eq!(slider.dimensions.border_box().height, 26.0, "inset:0 fills the toggle");
        let knob = &slider.children[0];
        // Chrome 148: padding box of the slider is y 1..25; knob = 25 - 2 - 20 = 3.
        assert_eq!(
            knob.dimensions.content.y - ty,
            3.0,
            "knob sits 2px above the slider's padding-box bottom"
        );
        assert_eq!(knob.dimensions.content.x - tx, 3.0);
    }

    /// CSS 2.1 §10.1: the containing block is the positioned ancestor's
    /// PADDING box. A `bottom: 4px` knob in an auto-height parent with
    /// `padding: 8px 0` holding 48px of flow lands at 8 + 48 + 8 - 4 - 20,
    /// not 8 + 48 - 24 (content box).
    #[test]
    fn abspos_bottom_resolves_against_padding_box_of_auto_height_parent() {
        let mut parent_style = ComputedStyle::new();
        parent_style.width = Length::Px(380.0);
        parent_style.padding_top = Length::Px(8.0);
        parent_style.padding_bottom = Length::Px(8.0);
        let mut parent = LayoutBox::with_position(BoxType::Block, parent_style, Position::Relative);

        let mut knob_style = ComputedStyle::new();
        knob_style.width = Length::Px(20.0);
        knob_style.height = Length::Px(20.0);
        let mut knob = LayoutBox::with_position(BoxType::Block, knob_style, Position::Absolute);
        knob.set_offsets(None, Some(4.0), Some(4.0), None);
        // The knob comes FIRST so the flow cursor is 0 when it lays out.
        parent.children.push(knob);
        for _ in 0..2 {
            let mut line = ComputedStyle::new();
            line.height = Length::Px(24.0);
            parent.children.push(LayoutBox::new(BoxType::Block, line));
        }

        let viewport = Dimensions {
            content: Rect::new(0.0, 0.0, 800.0, 600.0),
            ..Default::default()
        };
        parent.layout(&viewport);

        let pb = parent.dimensions.border_box();
        assert_eq!(pb.height, 64.0);
        let knob = &parent.children[0];
        assert_eq!(
            knob.dimensions.content.y - pb.y,
            64.0 - 4.0 - 20.0,
            "bottom: against the padding box"
        );
        assert_eq!(
            knob.dimensions.content.x - pb.x,
            380.0 - 4.0 - 20.0,
            "right: against the padding box"
        );
    }

    #[test]
    fn test_z_index_stacking() {
        let style = ComputedStyle::new();
        let mut layout_box = LayoutBox::with_position(BoxType::Block, style, Position::Absolute);
        layout_box.set_z_index(5);

        assert_eq!(layout_box.z_index, 5);
        let ctx = layout_box.stacking_context.as_ref().unwrap();
        assert!(ctx.creates_context);
        assert_eq!(ctx.z_index, 5);
    }

    #[test]
    fn test_display_list_build() {
        let mut style = ComputedStyle::new();
        style.background_color = Color::from_rgb(255, 255, 255);

        let mut layout_box = LayoutBox::new(BoxType::Block, style);
        // Set dimensions - render_background skips zero-sized boxes
        layout_box.dimensions.content = Rect::new(0.0, 0.0, 100.0, 100.0);
        let display_list = DisplayList::build(&layout_box);

        assert!(!display_list.commands.is_empty());
    }

    #[test]
    fn text_run_paints_no_background_of_its_own() {
        // The engine copies box decorations onto text children (the gradient
        // for gradient text; a pseudo-element's whole style). Only the
        // element paints them.
        let mut text_style = ComputedStyle::new();
        text_style.background_color = Color::from_rgb(255, 0, 0);
        let mut text = LayoutBox::new(BoxType::Text("🌆".to_string()), text_style);
        text.dimensions.content = Rect::new(10.0, 10.0, 48.0, 63.0);
        let mut parent = LayoutBox::new(BoxType::Block, ComputedStyle::new());
        parent.dimensions.content = Rect::new(0.0, 0.0, 200.0, 100.0);
        parent.children.push(text);
        let display_list = DisplayList::build(&parent);
        let fills: Vec<String> = display_list
            .commands
            .iter()
            .map(|c| format!("{:?}", c))
            .filter(|c| c.starts_with("SolidColor") || c.starts_with("RoundedRect"))
            .collect();
        assert!(fills.is_empty(), "text run painted a box fill: {:?}", fills);
    }

    #[test]
    fn test_display_list_with_positioned() {
        let style = ComputedStyle::new();
        let mut parent = LayoutBox::new(BoxType::Block, style.clone());

        let mut child = LayoutBox::with_position(BoxType::Block, style, Position::Absolute);
        child.set_z_index(-1);
        parent.children.push(child);

        let display_list = DisplayList::build(&parent);

        // Should have commands for both parent and child
        assert!(!display_list.commands.is_empty());
    }

    #[test]
    fn test_paint_order() {
        let style = ComputedStyle::new();
        let mut parent = LayoutBox::new(BoxType::Block, style.clone());

        // Add normal flow child
        let normal = LayoutBox::new(BoxType::Block, style.clone());
        parent.children.push(normal);

        // Add positioned child with positive z-index
        let mut positive_z =
            LayoutBox::with_position(BoxType::Block, style.clone(), Position::Absolute);
        positive_z.set_z_index(1);
        parent.children.push(positive_z);

        // Add positioned child with negative z-index
        let mut negative_z = LayoutBox::with_position(BoxType::Block, style, Position::Absolute);
        negative_z.set_z_index(-1);
        parent.children.push(negative_z);

        let paint_order = parent.get_paint_order();

        // Order should be: negative z-index, normal flow, positive z-index
        assert_eq!(paint_order.len(), 3);
        assert_eq!(paint_order[0].z_index, -1);
        assert_eq!(paint_order[1].position, Position::Static);
        assert_eq!(paint_order[2].z_index, 1);
    }

    #[test]
    fn test_sticky_positioning_state_initialization() {
        let style = ComputedStyle::new();
        let mut layout_box = LayoutBox::with_position(BoxType::Block, style, Position::Sticky);

        // Set position offsets (sticky threshold)
        layout_box.set_offsets(Some(10.0), None, None, None);

        // Set dimensions as if layout happened
        layout_box.dimensions.content.x = 0.0;
        layout_box.dimensions.content.y = 100.0;
        layout_box.dimensions.content.width = 200.0;
        layout_box.dimensions.content.height = 50.0;

        // Create containing block
        let containing_block = Dimensions {
            content: Rect::new(0.0, 0.0, 800.0, 600.0),
            ..Default::default()
        };

        // Apply position offsets - this should initialize sticky_state
        layout_box.apply_position_offsets(&containing_block);

        // Verify sticky_state was created
        assert!(layout_box.sticky_state.is_some());

        let sticky = layout_box.sticky_state.as_ref().unwrap();
        assert!(!sticky.is_stuck);
        assert!(sticky.offsets.top.is_some());
        assert_eq!(sticky.offsets.top.unwrap(), 10.0);
    }

    #[test]
    fn test_sticky_update_not_stuck() {
        let style = ComputedStyle::new();
        let mut layout_box = LayoutBox::with_position(BoxType::Block, style, Position::Sticky);

        layout_box.set_offsets(Some(10.0), None, None, None);

        // Element at y=100
        layout_box.dimensions.content.y = 100.0;
        layout_box.dimensions.content.height = 50.0;
        layout_box.dimensions.content.width = 200.0;

        let containing_block = Dimensions::default();
        layout_box.apply_position_offsets(&containing_block);

        let container = Rect::new(0.0, 0.0, 800.0, 600.0);

        // Scroll position is 0 - element should not be stuck
        // Threshold is original_y - top_offset = 100 - 10 = 90
        layout_box.update_sticky_positions(0.0, 0.0, container);

        let sticky = layout_box.sticky_state.as_ref().unwrap();
        assert!(!sticky.is_stuck);
    }

    #[test]
    fn test_sticky_update_stuck() {
        let style = ComputedStyle::new();
        let mut layout_box = LayoutBox::with_position(BoxType::Block, style, Position::Sticky);

        layout_box.set_offsets(Some(10.0), None, None, None);

        // Element at y=100
        layout_box.dimensions.content.y = 100.0;
        layout_box.dimensions.content.height = 50.0;
        layout_box.dimensions.content.width = 200.0;

        let containing_block = Dimensions::default();
        layout_box.apply_position_offsets(&containing_block);

        let container = Rect::new(0.0, 0.0, 800.0, 600.0);

        // Scroll position is 150 - element should be stuck
        // Threshold is original_y - top_offset = 100 - 10 = 90
        // scroll_y (150) > threshold (90), so should stick
        layout_box.update_sticky_positions(0.0, 150.0, container);

        let sticky = layout_box.sticky_state.as_ref().unwrap();
        assert!(sticky.is_stuck);
    }

    #[test]
    fn test_sticky_reset() {
        let style = ComputedStyle::new();
        let mut layout_box = LayoutBox::with_position(BoxType::Block, style, Position::Sticky);

        layout_box.set_offsets(Some(10.0), None, None, None);

        layout_box.dimensions.content.y = 100.0;
        layout_box.dimensions.content.height = 50.0;
        layout_box.dimensions.content.width = 200.0;

        let containing_block = Dimensions::default();
        layout_box.apply_position_offsets(&containing_block);

        let container = Rect::new(0.0, 0.0, 800.0, 600.0);

        // First, make it stuck
        layout_box.update_sticky_positions(0.0, 150.0, container);
        assert!(layout_box.sticky_state.as_ref().unwrap().is_stuck);

        // Reset
        layout_box.reset_sticky_positions();
        assert!(!layout_box.sticky_state.as_ref().unwrap().is_stuck);
    }

    #[test]
    fn test_display_list_build_with_scroll() {
        let style = ComputedStyle::new();
        let mut layout_box = LayoutBox::new(BoxType::Block, style.clone());
        layout_box.dimensions.content = Rect::new(0.0, 0.0, 800.0, 600.0);

        // Add a sticky child
        let mut sticky_child = LayoutBox::with_position(BoxType::Block, style, Position::Sticky);
        sticky_child.set_offsets(Some(10.0), None, None, None);
        sticky_child.dimensions.content = Rect::new(0.0, 100.0, 200.0, 50.0);

        let containing_block = Dimensions::default();
        sticky_child.apply_position_offsets(&containing_block);

        layout_box.children.push(sticky_child);

        let viewport = Rect::new(0.0, 0.0, 800.0, 600.0);

        // Build with scroll = 0 (not stuck)
        let _display_list = DisplayList::build_with_scroll(&mut layout_box, 0.0, 0.0, viewport);

        // Verify sticky child is not stuck
        let sticky = layout_box.children[0].sticky_state.as_ref().unwrap();
        assert!(!sticky.is_stuck);

        // Build with scroll = 150 (should be stuck)
        let _display_list = DisplayList::build_with_scroll(&mut layout_box, 0.0, 150.0, viewport);

        // Verify sticky child is now stuck
        let sticky = layout_box.children[0].sticky_state.as_ref().unwrap();
        assert!(sticky.is_stuck);
    }
}

/// PAINT-0 seating probe gate (forensics 2026-07-16-paint0-glyph-seat).
/// RUSTKIT_PAINT_PROBE=1 logs the layout half of the glyph seating chain
/// (line_height -> half_leading -> y_cmd) so flat-1.2 and metrics-normal
/// builds can be diffed line-by-line. Zero cost when off.
fn paint0_probe() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("RUSTKIT_PAINT_PROBE").as_deref() == Ok("1"))
}

#[cfg(test)]
mod object_fit_default_tests {
    use super::*;

    /// CSS Images 3 §5.5: the initial value of `object-fit` is `fill`.
    ///
    /// #125 fixed the ComputedStyle initial value and the layout keyword
    /// fallback but MISSED the `#[default]` on the enum itself, in two
    /// crates (Prometheus caught it in the #110 tip R1). Same bug, four
    /// sites, and a partial fix is the more dangerous state: the visible
    /// path looks right while any code that goes through `Default` still
    /// letterboxes. This pins the enum default so the two can never drift.
    #[test]
    fn object_fit_derived_default_is_fill_not_contain() {
        assert_eq!(ObjectFit::default(), ObjectFit::Fill);
    }
}

#[cfg(test)]
mod w3_zero_width_wrap_tests {
    use super::*;
    use rustkit_css::{Display, WhiteSpace, WordBreak};

    fn text(t: &str, ws: WhiteSpace, wb: WordBreak) -> LayoutBox {
        let mut s = ComputedStyle::new();
        s.font_size = Length::Px(32.0);
        s.white_space = ws;
        s.word_break = wb;
        LayoutBox::new(BoxType::Text(t.to_string()), s)
    }

    /// `<div style="display:inline-block; width:0; font-size:32px">abc<span>xyz</span>def</div>`
    /// with `div { white-space: DIV_WS; word-break: DIV_WB }` and
    /// `span { white-space: SPAN_WS; word-break: SPAN_WB }` (text nodes carry
    /// their parent's computed values, as the engine's cascade would set).
    fn fixture(
        div_ws: WhiteSpace,
        div_wb: WordBreak,
        span_ws: WhiteSpace,
        span_wb: WordBreak,
    ) -> LayoutBox {
        let mut s = ComputedStyle::new();
        s.display = Display::InlineBlock;
        s.font_size = Length::Px(32.0);
        s.width = Length::Zero;
        s.white_space = div_ws;
        s.word_break = div_wb;
        let mut div = LayoutBox::new(BoxType::Block, s);
        div.children.push(text("abc", div_ws, div_wb));
        let mut ss = ComputedStyle::new();
        ss.display = Display::Inline;
        ss.font_size = Length::Px(32.0);
        ss.white_space = span_ws;
        ss.word_break = span_wb;
        let mut span = LayoutBox::new(BoxType::Inline, ss);
        span.children.push(text("xyz", span_ws, span_wb));
        div.children.push(span);
        div.children.push(text("def", div_ws, div_wb));
        let mut cb = Dimensions::default();
        cb.content = Rect::new(0.0, 0.0, 800.0, 0.0);
        div.layout(&cb);
        div
    }

    fn line_texts(b: &LayoutBox) -> Vec<String> {
        b.text_lines
            .as_ref()
            .map(|ls| ls.iter().map(|l| l.text.clone()).collect())
            .unwrap_or_default()
    }

    #[test]
    fn wpt_break_boundary_2_chars_001_shape_is_seven_line_boxes() {
        // css-text-3 §5.1, 7th bullet: the white-space of the NEAREST COMMON
        // ANCESTOR of two characters governs the opportunity between them.
        // div (normal, break-all) governs a|b|c, c|x, z|d, d|e|f; the span
        // (pre) governs x|y|z. Ref: a / b / c / xyz / d / e / f.
        let div = fixture(
            WhiteSpace::Normal,
            WordBreak::BreakAll,
            WhiteSpace::Pre,
            WordBreak::BreakAll,
        );
        let lh = div.children[0].get_line_height();

        assert_eq!(
            line_texts(&div.children[0]),
            ["a", "b", "c"],
            "abc wraps per grapheme at width 0"
        );
        assert!(
            div.children[1].children[0].text_lines.is_none(),
            "xyz is one run: the span is white-space: pre"
        );
        // def starts on the span's line (offset past the cursor) with NO
        // room: an empty first line closes that line box, then d / e / f.
        let def = &div.children[2];
        assert_eq!(line_texts(def), ["", "d", "e", "f"]);
        assert!(
            (def.dimensions.content.y - 3.0 * lh).abs() < 0.01,
            "def's line 0 is the xyz line"
        );
        assert!(
            (div.dimensions.content.height - 7.0 * lh).abs() < 0.01,
            "seven line boxes, got {} line-heights",
            div.dimensions.content.height / lh
        );
    }

    #[test]
    fn wpt_break_boundary_2_chars_002_inverse_stays_one_line() {
        // Inverse pair: break-all on the SPAN cannot create opportunities when
        // the NCA (div: pre / nowrap) disallows wrapping. One line: abcxyzdef.
        for div_ws in [WhiteSpace::Pre, WhiteSpace::Nowrap] {
            let div = fixture(div_ws, WordBreak::Normal, div_ws, WordBreak::BreakAll);
            let lh = div.children[0].get_line_height();
            assert!(
                div.children[0].text_lines.is_none(),
                "{div_ws:?}: abc must not wrap"
            );
            assert!(
                div.children[2].text_lines.is_none(),
                "{div_ws:?}: def must not wrap"
            );
            assert!(
                (div.dimensions.content.height - lh).abs() < 0.01,
                "{div_ws:?}: one line box, got {}",
                div.dimensions.content.height / lh
            );
        }
    }

    /// `<pre>` text: a preserved newline is a forced line break even though
    /// soft wrapping is off and the run fits its container (css-text-3
    /// §4.1.1). Before: the `white-space: pre` gate skipped the wrapper, so
    /// six source lines were one line box.
    #[test]
    fn pre_text_breaks_at_preserved_newlines_without_soft_wrapping() {
        let mut cb = Dimensions::default();
        cb.content = Rect::new(0.0, 0.0, 800.0, 0.0);
        let src = "body {\n    font-family: Georgia;\n\n}";
        let mut b = text(src, WhiteSpace::Pre, WordBreak::Normal);
        b.layout_text(src.into(), &cb);
        let lh = b.get_line_height();
        assert_eq!(
            line_texts(&b),
            ["body {", "    font-family: Georgia;", "", "}"],
            "one line box per segment, blank line kept, indentation kept"
        );
        assert!(
            (b.dimensions.content.height - 4.0 * lh).abs() < 0.01,
            "four line boxes, got {}",
            b.dimensions.content.height / lh
        );

        // Soft wrapping stays OFF under pre: a segment wider than the
        // container is still one line (overflow), never re-broken.
        cb.content = Rect::new(0.0, 0.0, 40.0, 0.0);
        let mut narrow = text(src, WhiteSpace::Pre, WordBreak::Normal);
        narrow.layout_text(src.into(), &cb);
        assert_eq!(line_texts(&narrow).len(), 4, "pre never soft-wraps");

        // A trailing newline ends the last line; it is not a fifth line box.
        let mut trailing = text("a\nb\n", WhiteSpace::Pre, WordBreak::Normal);
        trailing.layout_text("a\nb\n".into(), &cb);
        assert_eq!(line_texts(&trailing), ["a", "b"]);
    }

    /// `<p style="width:300px"><span>long…</span></p>` and
    /// `<pre><code>a\nb\nc</code></pre>`: the block must be as tall as the
    /// inline's wrapped lines, and a following sibling starts below them.
    /// Before n50 the line advanced by ONE line-height for any inline child,
    /// whatever its text had wrapped to.
    #[test]
    fn a_block_grows_past_an_inline_childs_wrapped_lines() {
        let make = |ws: WhiteSpace, width: f32, run: &str| {
            let mut ps = ComputedStyle::new();
            ps.font_size = Length::Px(16.0);
            ps.white_space = ws;
            ps.width = Length::Px(width);
            let mut p = LayoutBox::new(BoxType::Block, ps);
            let mut ss = ComputedStyle::new();
            ss.display = Display::Inline;
            ss.font_size = Length::Px(16.0);
            ss.white_space = ws;
            let mut span = LayoutBox::new(BoxType::Inline, ss);
            span.children.push(text(run, ws, WordBreak::Normal));
            p.children.push(span);
            let mut after = ComputedStyle::new();
            after.font_size = Length::Px(16.0);
            let mut sibling = LayoutBox::new(BoxType::Block, after);
            sibling
                .children
                .push(text("after", WhiteSpace::Normal, WordBreak::Normal));
            let mut body = LayoutBox::new(BoxType::Block, ComputedStyle::new());
            body.children.push(p);
            body.children.push(sibling);
            let mut cb = Dimensions::default();
            cb.content = Rect::new(0.0, 0.0, 800.0, 0.0);
            body.layout(&cb);
            body
        };
        let long = "The quick brown fox jumps over the lazy dog again and again \
                    until the line has no choice but to wrap around";
        let body = make(WhiteSpace::Normal, 200.0, long);
        let (p, sib) = (&body.children[0], &body.children[1]);
        let n = p.children[0].children[0].text_lines.as_ref().unwrap().len();
        let lh = p.children[0].children[0].get_line_height();
        assert!(n > 1, "precondition: the span's text wrapped");
        assert!(
            (p.dimensions.content.height - n as f32 * lh).abs() < 0.01,
            "p must be {n} lines tall, got {}",
            p.dimensions.content.height / lh
        );
        assert!(
            sib.dimensions.content.y >= p.dimensions.content.y + p.dimensions.content.height - 0.01,
            "the sibling starts below the wrapped span"
        );

        let body = make(WhiteSpace::Pre, 800.0, "one\ntwo\nthree");
        let p = &body.children[0];
        let lh = p.children[0].children[0].get_line_height();
        assert!(
            (p.dimensions.content.height - 3.0 * lh).abs() < 0.01,
            "pre > code with three source lines is three lines tall, got {}",
            p.dimensions.content.height / lh
        );
    }

    /// pre-line / pre-wrap: a newline breaks the line even when the whole
    /// run would have fit (the old "only wrap what overflows" test).
    #[test]
    fn pre_line_and_pre_wrap_break_at_newlines_when_the_run_fits() {
        let mut cb = Dimensions::default();
        cb.content = Rect::new(0.0, 0.0, 800.0, 0.0);
        for ws in [
            WhiteSpace::PreLine,
            WhiteSpace::PreWrap,
            WhiteSpace::BreakSpaces,
        ] {
            let mut b = text("one two\nthree", ws, WordBreak::Normal);
            b.layout_text("one two\nthree".into(), &cb);
            assert_eq!(line_texts(&b), ["one two", "three"], "{ws:?}");
        }
        // And a control: normal white-space text with no newline (the
        // engine has already collapsed them) still takes the old path.
        let mut normal = text("one two three", WhiteSpace::Normal, WordBreak::Normal);
        normal.layout_text("one two three".into(), &cb);
        assert!(normal.text_lines.is_none());
    }

    #[test]
    fn a_bare_zero_container_width_is_still_the_intrinsic_guard() {
        // The block path must keep reading an UNQUALIFIED 0 as "no resolved
        // width yet": only the definite-zero flag unlocks wrapping. Both
        // arms on the same text so a regression in either direction reds.
        let mut cb = Dimensions::default();
        cb.content = Rect::new(0.0, 0.0, 0.0, 0.0);
        let mut guarded = text("abc", WhiteSpace::Normal, WordBreak::BreakAll);
        guarded.layout_text("abc".into(), &cb);
        assert!(
            guarded.text_lines.is_none(),
            "bare 0 must not wrap (intrinsic pass)"
        );
        let mut definite = text("abc", WhiteSpace::Normal, WordBreak::BreakAll);
        definite.layout_text_with_zero_wrap("abc".into(), &cb, true);
        assert_eq!(line_texts(&definite), ["a", "b", "c"]);
    }

    /// n51: a text input's author padding composes with its content line
    /// in EVERY absolute unit. The unit closure took px and em only, so
    /// `padding: 1rem 1.5rem` (new_tab's search box, and most styled search
    /// fields) read as no author padding and the control fell to the bare
    /// 19px blob; Chrome builds 18 + 32 + 2 = 52.
    #[test]
    fn a_text_input_with_rem_padding_composes_it_into_its_height() {
        fn input(pad: Length) -> LayoutBox {
            let mut s = ComputedStyle::new();
            s.font_size = Length::Px(16.0);
            s.padding_top = pad.clone();
            s.padding_bottom = pad;
            s.border_top_width = Length::Px(1.0);
            s.border_bottom_width = Length::Px(1.0);
            LayoutBox::new(
                BoxType::FormControl(FormControlType::TextInput {
                    value: String::new(),
                    placeholder: String::new(),
                    input_type: "text".to_string(),
                }),
                s,
            )
        }
        let cb = Dimensions {
            content: Rect::new(0.0, 0.0, 500.0, 0.0),
            ..Default::default()
        };
        let mut px = input(Length::Px(16.0));
        px.layout(&cb);
        let mut rem = input(Length::Rem(1.0));
        rem.layout(&cb);
        let want = px.dimensions.content.height;
        assert!(
            want > 40.0,
            "setup: px padding must compose (17 + 32 + 2 = 51), got {want}"
        );
        assert!(
            (rem.dimensions.content.height - want).abs() < 0.01,
            "1rem padding must size the control like 16px padding ({want}), got {} \
             (19 is the bare blob: the rem was dropped)",
            rem.dimensions.content.height
        );
    }
}

// ── ported from hiwave-windows (#75): the display list emits RoundedRect for
//    a rounded background and keeps the cheap SolidColor path otherwise. ──
#[cfg(test)]
mod border_radius_emit_tests {
    use super::*;
    use rustkit_css::{Color, ComputedStyle, Length};


    fn box_with(radius: Length, bg: Color) -> LayoutBox {
        let mut s = ComputedStyle::new();
        s.background_color = bg;
        s.border_top_left_radius = radius.clone();
        s.border_top_right_radius = radius.clone();
        s.border_bottom_right_radius = radius.clone();
        s.border_bottom_left_radius = radius;
        let mut b = LayoutBox::new(BoxType::Block, s);
        b.dimensions.content.width = 80.0;
        b.dimensions.content.height = 40.0;
        b
    }

    fn kinds(b: &LayoutBox) -> Vec<String> {
        DisplayList::build(b)
            .commands
            .iter()
            .map(|c| {
                format!("{c:?}")
                    .chars()
                    .take_while(|ch| ch.is_alphanumeric())
                    .collect()
            })
            .collect()
    }

    /// A square box must keep the cheap path. Without this, "rounded works"
    /// could be satisfied by emitting RoundedRect unconditionally.
    #[test]
    fn a_square_box_still_emits_solid_color() {
        let k = kinds(&box_with(Length::Zero, Color::new(51, 102, 204, 1.0)));
        assert!(k.contains(&"SolidColor".to_string()), "got {k:?}");
        assert!(!k.contains(&"RoundedRect".to_string()), "got {k:?}");
    }

    /// The product: a rounded box emits RoundedRect INSTEAD of SolidColor.
    #[test]
    fn a_rounded_box_emits_roundedrect_through_the_live_path() {
        let k = kinds(&box_with(Length::Px(12.0), Color::new(51, 102, 204, 1.0)));
        assert!(k.contains(&"RoundedRect".to_string()), "got {k:?}");
        assert!(!k.contains(&"SolidColor".to_string()), "got {k:?}");
    }

    /// A transparent background emits nothing at all, rounded or not.
    #[test]
    fn a_transparent_box_emits_no_fill() {
        let k = kinds(&box_with(Length::Px(12.0), Color::TRANSPARENT));
        assert!(!k.contains(&"RoundedRect".to_string()), "got {k:?}");
        assert!(!k.contains(&"SolidColor".to_string()), "got {k:?}");
    }
}
