//! Styled scrollbars (hub row CH2.E7): the standard `scrollbar-width` and
//! `scrollbar-color` properties, and what the `::-webkit-scrollbar*`
//! pseudo-elements of a scroll container cascade to.
//!
//! The pseudo-elements are matched by the engine (the same suffix path as
//! `::before`); this module only holds what they compute to and the
//! parsers for the two standard properties.

use crate::{Color, CornerRadius, ScrollbarWidth};

/// One `::-webkit-scrollbar-*` part: what Chrome paints for it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ScrollbarPart {
    /// `background-color` (or the colour of a `background` shorthand).
    /// `None` paints nothing for the part.
    pub background: Option<Color>,
    /// `border-radius` (top-left, top-right, bottom-right, bottom-left),
    /// resolved at paint against the part's own box.
    pub radius: [CornerRadius; 4],
    /// Whether a rule set `border-radius` (a `:hover` part keeps the
    /// thumb's radius when it does not).
    pub has_radius: bool,
    /// `display: none` on the part.
    pub hidden: bool,
}

/// What the `::-webkit-scrollbar*` rules matching one element give. Present
/// on a style only when some `::-webkit-scrollbar` rule matched the element:
/// Chrome draws a custom scrollbar only then, and the other parts style it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WebkitScrollbar {
    /// `::-webkit-scrollbar { width }`: the vertical bar's thickness.
    /// `None` is `auto` (Chrome then uses its platform width, 15px).
    pub width: Option<f32>,
    /// `::-webkit-scrollbar { height }`: the horizontal bar's thickness.
    pub height: Option<f32>,
    /// `::-webkit-scrollbar { display: none }` hides both bars and takes
    /// no space.
    pub hidden: bool,
    /// The `::-webkit-scrollbar` box itself (painted under the track).
    pub bar: ScrollbarPart,
    pub track: ScrollbarPart,
    pub thumb: ScrollbarPart,
    /// `::-webkit-scrollbar-thumb:hover`: replaces the thumb's background
    /// (and radius when it sets one) while the pointer is over the thumb.
    pub thumb_hover: Option<ScrollbarPart>,
    pub corner: ScrollbarPart,
}

/// The platform (non-overlay, classic) scrollbar thickness Chrome lays out
/// for `scrollbar-width: auto` and for a `::-webkit-scrollbar` without a
/// `width`/`height`.
pub const CLASSIC_SCROLLBAR_PX: f32 = 15.0;
/// What `scrollbar-width: thin` lays out (Chrome 153, Linux classic).
pub const THIN_SCROLLBAR_PX: f32 = 10.0;

impl WebkitScrollbar {
    /// The vertical bar's thickness, 0 when hidden.
    pub fn vertical_thickness(&self) -> f32 {
        if self.hidden {
            return 0.0;
        }
        self.width.unwrap_or(CLASSIC_SCROLLBAR_PX).max(0.0)
    }

    /// The horizontal bar's thickness, 0 when hidden.
    pub fn horizontal_thickness(&self) -> f32 {
        if self.hidden {
            return 0.0;
        }
        self.height.unwrap_or(CLASSIC_SCROLLBAR_PX).max(0.0)
    }
}

/// `scrollbar-width: auto | thin | none`. `None` for anything else (the
/// declaration is invalid and dropped).
pub fn parse_scrollbar_width(value: &str) -> Option<ScrollbarWidth> {
    match value.trim().to_ascii_lowercase().as_str() {
        "auto" => Some(ScrollbarWidth::Auto),
        "thin" => Some(ScrollbarWidth::Thin),
        "none" => Some(ScrollbarWidth::None),
        _ => None,
    }
}

/// `scrollbar-color: auto | <thumb> <track>`. `Some(None)` is `auto`;
/// `None` is an invalid value (dropped).
pub fn parse_scrollbar_color(value: &str) -> Option<Option<(Color, Color)>> {
    let value = value.trim();
    if value.eq_ignore_ascii_case("auto") {
        return Some(None);
    }
    let parts = split_top_level_whitespace(value);
    if parts.len() != 2 {
        return None;
    }
    let thumb = crate::parse_color(parts[0])?;
    let track = crate::parse_color(parts[1])?;
    Some(Some((thumb, track)))
}

/// Split on whitespace outside parentheses: `rgba(1, 2, 3, .5) transparent`
/// is two colours.
fn split_top_level_whitespace(value: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let (mut depth, mut start) = (0usize, None);
    for (i, ch) in value.char_indices() {
        match ch {
            '(' => {
                depth += 1;
                start.get_or_insert(i);
            }
            ')' => depth = depth.saturating_sub(1),
            c if c.is_whitespace() && depth == 0 => {
                if let Some(s) = start.take() {
                    parts.push(&value[s..i]);
                }
            }
            _ => {
                start.get_or_insert(i);
            }
        }
    }
    if let Some(s) = start {
        parts.push(&value[s..]);
    }
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrollbar_width_keywords() {
        assert_eq!(parse_scrollbar_width("auto"), Some(ScrollbarWidth::Auto));
        assert_eq!(parse_scrollbar_width(" THIN "), Some(ScrollbarWidth::Thin));
        assert_eq!(parse_scrollbar_width("none"), Some(ScrollbarWidth::None));
        assert_eq!(parse_scrollbar_width("6px"), None);
    }

    #[test]
    fn scrollbar_color_two_colours_or_auto() {
        assert_eq!(parse_scrollbar_color("auto"), Some(None));
        assert_eq!(
            parse_scrollbar_color("rgba(100, 116, 139, 0.3) transparent"),
            Some(Some((Color::new(100, 116, 139, 0.3), Color::TRANSPARENT)))
        );
        assert_eq!(
            parse_scrollbar_color("#888 #eee"),
            Some(Some((Color::new(0x88, 0x88, 0x88, 1.0), Color::new(0xee, 0xee, 0xee, 1.0))))
        );
        assert_eq!(parse_scrollbar_color("red"), None);
        assert_eq!(parse_scrollbar_color("red blue green"), None);
    }

    #[test]
    fn webkit_thickness_defaults_to_the_classic_width_and_hides() {
        let mut bar = WebkitScrollbar { width: Some(6.0), ..Default::default() };
        assert_eq!(bar.vertical_thickness(), 6.0);
        assert_eq!(bar.horizontal_thickness(), CLASSIC_SCROLLBAR_PX);
        bar.hidden = true;
        assert_eq!(bar.vertical_thickness(), 0.0);
    }
}
