//! Value parsers shared by the background and mask longhands
//! (css-backgrounds-3 §3, css-masking-1 §6): a layer's size, position,
//! repeat style and box. `mask-*` takes the same grammar as `background-*`
//! for these, so both property families parse through here.

/// Parse a background-size value.
pub fn parse_background_size(value: &str) -> crate::BackgroundSize {
    let value = value.trim().to_lowercase();
    match value.as_str() {
        "cover" => crate::BackgroundSize::Cover,
        "contain" => crate::BackgroundSize::Contain,
        "auto" => crate::BackgroundSize::Auto,
        _ => {
            // Parse explicit size (e.g., "100px 50px" or "50% auto")
            let parts: Vec<&str> = value.split_whitespace().collect();
            let width = parts
                .first()
                .and_then(|s| parse_background_size_dimension(s));
            let height = parts
                .get(1)
                .and_then(|s| parse_background_size_dimension(s));
            crate::BackgroundSize::Explicit { width, height }
        }
    }
}

/// Parse a single dimension for background-size (px, %, or auto).
pub fn parse_background_size_dimension(value: &str) -> Option<f32> {
    let value = value.trim();
    if value == "auto" {
        return None;
    }
    if value.ends_with("px") {
        return value.strip_suffix("px").and_then(|s| s.parse().ok());
    }
    if value.ends_with('%') {
        // Return percentage as negative value to indicate it's a percentage
        // (will be resolved during layout)
        return value
            .strip_suffix('%')
            .and_then(|s| s.parse::<f32>().ok())
            .map(|p| -p);
    }
    value.parse().ok()
}

/// Parse a background-repeat value.
pub fn parse_background_repeat(value: &str) -> crate::BackgroundRepeat {
    match value.trim().to_lowercase().as_str() {
        "repeat" => crate::BackgroundRepeat::Repeat,
        "repeat-x" => crate::BackgroundRepeat::RepeatX,
        "repeat-y" => crate::BackgroundRepeat::RepeatY,
        "no-repeat" => crate::BackgroundRepeat::NoRepeat,
        "space" => crate::BackgroundRepeat::Space,
        "round" => crate::BackgroundRepeat::Round,
        _ => crate::BackgroundRepeat::default(),
    }
}

/// Parse a background-position value (css-backgrounds-3 §3.6).
///
/// One value: the other axis is `center`, and `top` / `bottom` name the
/// vertical axis. Two values: horizontal then vertical, unless the keywords
/// say otherwise (`top right`). Three or four: `<edge> <offset>?` pairs; an
/// offset from `right` / `bottom` is measured back from that edge.
pub fn parse_background_position(value: &str) -> crate::BackgroundPosition {
    use crate::BackgroundPositionValue::{Calc, Percent, Px};
    let value = value.trim().to_lowercase();
    // A `calc()` has spaces of its own.
    let parts: Vec<&str> = split_top_level_whitespace(&value);
    let from_far_edge = |offset: crate::BackgroundPositionValue| match offset {
        Percent(p) => Percent(1.0 - p),
        Px(px) => Calc {
            percent: 1.0,
            px: -px,
        },
        Calc { percent, px } => Calc {
            percent: 1.0 - percent,
            px: -px,
        },
    };
    let vertical = |s: &str| matches!(s, "top" | "bottom");
    let horizontal = |s: &str| matches!(s, "left" | "right");

    let (x, y) = match parts.as_slice() {
        [] => (Percent(0.0), Percent(0.0)),
        [one] if vertical(one) => (Percent(0.5), parse_background_position_value(one)),
        [one] => (parse_background_position_value(one), Percent(0.5)),
        [a, b] if vertical(a) || horizontal(b) => (
            parse_background_position_value(b),
            parse_background_position_value(a),
        ),
        [a, b] => (
            parse_background_position_value(a),
            parse_background_position_value(b),
        ),
        many => {
            let (mut x, mut y) = (Percent(0.5), Percent(0.5));
            let mut i = 0;
            while i < many.len() {
                let edge = many[i];
                let offset = many
                    .get(i + 1)
                    .copied()
                    .filter(|next| !vertical(next) && !horizontal(next) && *next != "center");
                let at = match (edge, offset) {
                    ("left" | "top", Some(offset)) => parse_background_position_value(offset),
                    ("right" | "bottom", Some(offset)) => {
                        from_far_edge(parse_background_position_value(offset))
                    }
                    _ => parse_background_position_value(edge),
                };
                if vertical(edge) {
                    y = at;
                } else if horizontal(edge) {
                    x = at;
                }
                i += if offset.is_some() { 2 } else { 1 };
            }
            (x, y)
        }
    };

    crate::BackgroundPosition { x, y }
}

/// Parse a single background-position dimension.
pub fn parse_background_position_value(value: &str) -> crate::BackgroundPositionValue {
    let value = value.trim().to_lowercase();
    match value.as_str() {
        "left" | "top" => crate::BackgroundPositionValue::Percent(0.0),
        "center" => crate::BackgroundPositionValue::Percent(0.5),
        "right" | "bottom" => crate::BackgroundPositionValue::Percent(1.0),
        // A sum of a percentage and px; one in font or viewport units has
        // nothing to resolve against here and is the start edge.
        _ if value.starts_with("calc(") => match crate::parse_length(&value) {
            Some(crate::Length::Px(px)) => crate::BackgroundPositionValue::Px(px),
            Some(crate::Length::Zero) => crate::BackgroundPositionValue::Px(0.0),
            Some(crate::Length::Percent(p)) => crate::BackgroundPositionValue::Percent(p / 100.0),
            Some(crate::Length::Calc(sum))
                if [sum.em, sum.rem, sum.vw, sum.vh, sum.vmin, sum.vmax]
                    .iter()
                    .all(|c| *c == 0.0) =>
            {
                crate::BackgroundPositionValue::Calc {
                    percent: sum.percent / 100.0,
                    px: sum.px,
                }
            }
            _ => crate::BackgroundPositionValue::Percent(0.0),
        },
        _ if value.ends_with('%') => value
            .strip_suffix('%')
            .and_then(|s| s.parse::<f32>().ok())
            .map(|p| crate::BackgroundPositionValue::Percent(p / 100.0))
            .unwrap_or(crate::BackgroundPositionValue::Percent(0.0)),
        _ if value.ends_with("px") => value
            .strip_suffix("px")
            .and_then(|s| s.parse::<f32>().ok())
            .map(crate::BackgroundPositionValue::Px)
            .unwrap_or(crate::BackgroundPositionValue::Percent(0.0)),
        _ => {
            // Try parsing as a number (assumed px)
            value
                .parse::<f32>()
                .ok()
                .map(crate::BackgroundPositionValue::Px)
                .unwrap_or(crate::BackgroundPositionValue::Percent(0.0))
        }
    }
}

/// Parse a background-origin value.
pub fn parse_background_origin(value: &str) -> crate::BackgroundOrigin {
    match value.trim().to_lowercase().as_str() {
        "border-box" => crate::BackgroundOrigin::BorderBox,
        "padding-box" => crate::BackgroundOrigin::PaddingBox,
        "content-box" => crate::BackgroundOrigin::ContentBox,
        _ => crate::BackgroundOrigin::default(),
    }
}

/// `value` split at whitespace outside parentheses.
pub fn split_top_level_whitespace(value: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut start = None;
    for (i, ch) in value.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            _ => {}
        }
        if depth == 0 && ch.is_whitespace() {
            if let Some(from) = start.take() {
                parts.push(&value[from..i]);
            }
        } else if start.is_none() {
            start = Some(i);
        }
    }
    if let Some(from) = start {
        parts.push(&value[from..]);
    }
    parts
}
