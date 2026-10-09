//! CSS Masking 1 §6 mask layers: `mask-image`, `mask-size`,
//! `mask-position`, `mask-repeat`, `mask-origin`, `mask-clip`, the `mask`
//! shorthand, and the `-webkit-mask-*` aliases Blink still accepts.
//!
//! Sites draw monochrome icons as an element with a `background-color`
//! (often `currentColor`) seen through `mask-image: url(icon.svg)`. With
//! no mask the element painted as a solid square.
//!
//! Each longhand is kept as its own list, as the spec models it: the
//! number of layers is the number of `mask-image` values, and a shorter
//! list of sizes, positions, ... repeats to cover them (§6.1). So the
//! order in which the longhands were declared does not matter.

use crate::background::{
    parse_background_position, parse_background_repeat, parse_background_size,
};
use crate::{
    BackgroundClip, BackgroundImage, BackgroundLayer, BackgroundOrigin, BackgroundPosition,
    BackgroundRepeat, BackgroundSize, Gradient,
};

/// An element's computed mask layers.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Mask {
    /// `mask-image`, in CSS order (the first value is the topmost layer).
    /// Empty is the initial `none`.
    pub images: Vec<BackgroundImage>,
    /// `mask-size`; empty is the initial `auto`.
    pub sizes: Vec<BackgroundSize>,
    /// `mask-position`; empty is the initial `0% 0%`.
    pub positions: Vec<BackgroundPosition>,
    /// `mask-repeat`; empty is the initial `repeat`.
    pub repeats: Vec<BackgroundRepeat>,
    /// `mask-origin`; empty is the initial `border-box`.
    pub origins: Vec<BackgroundOrigin>,
    /// `mask-clip`; empty is the initial `border-box`.
    pub clips: Vec<BackgroundClip>,
}

/// The unprefixed mask property `name` stands for, if it is one this
/// module parses. `-webkit-mask-*` is an alias of `mask-*` in Blink.
/// `mask-mode` and `mask-composite` are accepted and not modelled.
pub fn canonical_mask_property(name: &str) -> Option<&'static str> {
    let name = name.strip_prefix("-webkit-").unwrap_or(name);
    Some(match name {
        "mask" => "mask",
        "mask-image" => "mask-image",
        "mask-size" => "mask-size",
        "mask-position" => "mask-position",
        "mask-repeat" => "mask-repeat",
        "mask-origin" => "mask-origin",
        "mask-clip" => "mask-clip",
        "mask-mode" => "mask-mode",
        "mask-composite" => "mask-composite",
        _ => return None,
    })
}

impl Mask {
    /// Whether any layer has an image: a mask of only `none` layers does
    /// not mask (§6.2).
    pub fn has_image(&self) -> bool {
        self.images
            .iter()
            .any(|i| !matches!(i, BackgroundImage::None))
    }

    /// The layers, bottommost first (the order `background_layers` uses),
    /// each longhand list repeated to the number of images.
    pub fn layers(&self) -> Vec<BackgroundLayer> {
        fn nth<T: Clone>(list: &[T], i: usize, initial: T) -> T {
            if list.is_empty() {
                initial
            } else {
                list[i % list.len()].clone()
            }
        }
        (0..self.images.len())
            .rev()
            .map(|i| BackgroundLayer {
                image: self.images[i].clone(),
                size: nth(&self.sizes, i, BackgroundSize::Auto),
                position: nth(&self.positions, i, BackgroundPosition::default()),
                repeat: nth(&self.repeats, i, BackgroundRepeat::Repeat),
                origin: nth(&self.origins, i, BackgroundOrigin::BorderBox),
                clip: nth(&self.clips, i, BackgroundClip::BorderBox),
            })
            .collect()
    }

    /// `property: initial` (and `unset`, masks not being inherited).
    pub fn reset(&mut self, property: &str) {
        match canonical_mask_property(property) {
            Some("mask") => *self = Mask::default(),
            Some("mask-image") => self.images.clear(),
            Some("mask-size") => self.sizes.clear(),
            Some("mask-position") => self.positions.clear(),
            Some("mask-repeat") => self.repeats.clear(),
            Some("mask-origin") => self.origins.clear(),
            Some("mask-clip") => self.clips.clear(),
            _ => {}
        }
    }

    /// Apply one declaration. Returns false, changing nothing, when
    /// `property` is not a mask property or `value` does not parse (the
    /// declaration is then dropped and the earlier value stays).
    /// `gradient` parses a `<gradient>` image.
    pub fn apply(
        &mut self,
        property: &str,
        value: &str,
        gradient: &dyn Fn(&str) -> Option<Gradient>,
    ) -> bool {
        let Some(property) = canonical_mask_property(property) else {
            return false;
        };
        let layers = split_layers(value.trim());
        if layers.is_empty() || layers.iter().any(|l| l.is_empty()) {
            return false;
        }
        match property {
            "mask-image" => {
                let Some(images) = layers.iter().map(|l| parse_image(l, gradient)).collect() else {
                    return false;
                };
                self.images = images;
            }
            "mask-size" => {
                self.sizes = layers.iter().map(|l| parse_background_size(l)).collect();
            }
            "mask-position" => {
                self.positions = layers
                    .iter()
                    .map(|l| parse_background_position(l))
                    .collect();
            }
            "mask-repeat" => {
                let Some(repeats) = layers.iter().map(|l| parse_repeat(l)).collect() else {
                    return false;
                };
                self.repeats = repeats;
            }
            "mask-origin" => {
                let Some(origins) = layers.iter().map(|l| parse_origin(l)).collect() else {
                    return false;
                };
                self.origins = origins;
            }
            "mask-clip" => {
                let Some(clips) = layers.iter().map(|l| parse_clip(l)).collect() else {
                    return false;
                };
                self.clips = clips;
            }
            "mask" => {
                let Some(parsed): Option<Vec<ShorthandLayer>> = layers
                    .iter()
                    .map(|l| parse_shorthand_layer(l, gradient))
                    .collect()
                else {
                    return false;
                };
                *self = Mask {
                    images: parsed.iter().map(|l| l.image.clone()).collect(),
                    sizes: parsed.iter().map(|l| l.size.clone()).collect(),
                    positions: parsed.iter().map(|l| l.position.clone()).collect(),
                    repeats: parsed.iter().map(|l| l.repeat).collect(),
                    origins: parsed.iter().map(|l| l.origin).collect(),
                    clips: parsed.iter().map(|l| l.clip).collect(),
                };
            }
            // mask-mode, mask-composite: accepted, not modelled.
            _ => {}
        }
        true
    }
}

/// One layer of the `mask` shorthand.
struct ShorthandLayer {
    image: BackgroundImage,
    position: BackgroundPosition,
    size: BackgroundSize,
    repeat: BackgroundRepeat,
    origin: BackgroundOrigin,
    clip: BackgroundClip,
}

/// `<mask-layer> = <mask-reference> || <position> [ / <bg-size> ]? ||
/// <repeat-style> || <geometry-box> || [ <geometry-box> | no-clip ] ||
/// <compositing-operator> || <masking-mode>` (§6.10), in any order.
fn parse_shorthand_layer(
    layer: &str,
    gradient: &dyn Fn(&str) -> Option<Gradient>,
) -> Option<ShorthandLayer> {
    // `center/contain` is one whitespace token: give the slash its own.
    let mut spaced = String::with_capacity(layer.len() + 2);
    let mut depth = 0usize;
    let mut quote = None;
    for ch in layer.chars() {
        match (quote, ch) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => quote = Some(ch),
            (None, '(') => depth += 1,
            (None, ')') => depth = depth.saturating_sub(1),
            _ => {}
        }
        if ch == '/' && depth == 0 && quote.is_none() {
            spaced.push_str(" / ");
        } else {
            spaced.push(ch);
        }
    }

    let numeric = |lower: &str| {
        let digits = lower.strip_prefix(['-', '+']).unwrap_or(lower);
        digits.starts_with(|c: char| c.is_ascii_digit() || c == '.') || lower.starts_with("calc(")
    };

    let mut image = None;
    let mut position: Vec<&str> = Vec::new();
    let mut size: Vec<&str> = Vec::new();
    let mut repeat: Vec<&str> = Vec::new();
    let mut boxes: Vec<&str> = Vec::new();
    let mut in_size = false;

    for token in split_tokens(&spaced) {
        let lower = token.to_ascii_lowercase();
        if token == "/" {
            if position.is_empty() || !size.is_empty() {
                return None;
            }
            in_size = true;
            continue;
        }
        if in_size {
            let takes = match size.len() {
                0 => matches!(lower.as_str(), "auto" | "cover" | "contain") || numeric(&lower),
                1 => {
                    !matches!(size[0].to_ascii_lowercase().as_str(), "cover" | "contain")
                        && (lower == "auto" || numeric(&lower))
                }
                _ => false,
            };
            if takes {
                size.push(token);
                continue;
            }
            in_size = false;
        }
        match lower.as_str() {
            "repeat" | "repeat-x" | "repeat-y" | "no-repeat" | "space" | "round" => {
                repeat.push(token)
            }
            "border-box" | "padding-box" | "content-box" | "fill-box" | "stroke-box"
            | "view-box" | "no-clip" | "margin-box" => boxes.push(token),
            "left" | "right" | "top" | "bottom" | "center" => position.push(token),
            // <compositing-operator>, <masking-mode>: not modelled.
            "add" | "subtract" | "intersect" | "exclude" | "alpha" | "luminance"
            | "match-source" => {}
            _ if numeric(&lower) => position.push(token),
            _ => {
                if image.is_some() {
                    return None;
                }
                image = Some(parse_image(token, gradient)?);
            }
        }
    }
    if repeat.len() > 2 || boxes.len() > 2 {
        return None;
    }
    let origin = match boxes.first() {
        Some(b) => parse_origin(b)?,
        None => BackgroundOrigin::BorderBox,
    };
    // One box sets both origin and clip; a second is the clip.
    let clip = match boxes.last() {
        Some(b) => parse_clip(b)?,
        None => BackgroundClip::BorderBox,
    };
    Some(ShorthandLayer {
        image: image.unwrap_or(BackgroundImage::None),
        position: if position.is_empty() {
            BackgroundPosition::default()
        } else {
            parse_background_position(&position.join(" "))
        },
        size: if size.is_empty() {
            BackgroundSize::Auto
        } else {
            parse_background_size(&size.join(" "))
        },
        repeat: if repeat.is_empty() {
            BackgroundRepeat::Repeat
        } else {
            parse_repeat(&repeat.join(" "))?
        },
        origin,
        clip,
    })
}

/// A `<mask-reference>`: `none`, a `url()`, or a gradient.
fn parse_image(
    value: &str,
    gradient: &dyn Fn(&str) -> Option<Gradient>,
) -> Option<BackgroundImage> {
    let value = value.trim();
    if value.eq_ignore_ascii_case("none") {
        return Some(BackgroundImage::None);
    }
    if let Some(url) = parse_css_url(value) {
        return Some(BackgroundImage::Url(url));
    }
    gradient(value).map(BackgroundImage::Gradient)
}

/// The address in a `url()` token: `url(a.svg)`, `url("a.svg")` or
/// `url('a.svg')`, the quoted forms free to hold spaces, commas and
/// parentheses (a `data:` SVG does). `None` when `token` is not one url.
pub fn parse_css_url(token: &str) -> Option<String> {
    let token = token.trim();
    let head = token.get(..4)?;
    if !head.eq_ignore_ascii_case("url(") || !token.ends_with(')') {
        return None;
    }
    let inner = token[4..token.len() - 1].trim();
    let url = match inner.chars().next() {
        Some(q @ ('"' | '\'')) => {
            let body = inner.strip_prefix(q)?.strip_suffix(q)?;
            // A url token is one string, not two (`url("a" "b")`).
            if body.contains(q) {
                return None;
            }
            body
        }
        _ => inner,
    };
    Some(url.to_string())
}

/// `<repeat-style>`: one keyword, or two (horizontal then vertical).
fn parse_repeat(value: &str) -> Option<BackgroundRepeat> {
    use BackgroundRepeat::{RepeatX, RepeatY};
    let words: Vec<String> = value
        .split_whitespace()
        .map(|w| w.to_ascii_lowercase())
        .collect();
    let known = |w: &str| matches!(w, "repeat" | "no-repeat" | "space" | "round");
    match words.as_slice() {
        [one] if matches!(one.as_str(), "repeat-x" | "repeat-y") || known(one) => {
            Some(parse_background_repeat(one))
        }
        [x, y] if known(x) && known(y) => Some(match (x.as_str(), y.as_str()) {
            (a, b) if a == b => parse_background_repeat(a),
            ("repeat", "no-repeat") => RepeatX,
            ("no-repeat", "repeat") => RepeatY,
            // Other mixed pairs: the horizontal style, approximately.
            (a, _) => parse_background_repeat(a),
        }),
        _ => None,
    }
}

/// `<geometry-box>` as an origin. The SVG boxes (`fill-box`, `stroke-box`,
/// `view-box`) are the border box for an HTML element (§6.3).
fn parse_origin(value: &str) -> Option<BackgroundOrigin> {
    Some(match value.trim().to_ascii_lowercase().as_str() {
        "content-box" => BackgroundOrigin::ContentBox,
        "padding-box" => BackgroundOrigin::PaddingBox,
        "border-box" | "margin-box" | "fill-box" | "stroke-box" | "view-box" => {
            BackgroundOrigin::BorderBox
        }
        _ => return None,
    })
}

/// `<geometry-box> | no-clip` as a clip. `no-clip` is approximated by the
/// border box.
fn parse_clip(value: &str) -> Option<BackgroundClip> {
    Some(match value.trim().to_ascii_lowercase().as_str() {
        "content-box" => BackgroundClip::ContentBox,
        "padding-box" => BackgroundClip::PaddingBox,
        "border-box" | "margin-box" | "fill-box" | "stroke-box" | "view-box" | "no-clip" => {
            BackgroundClip::BorderBox
        }
        // Legacy `-webkit-mask-clip: text` etc. do not parse.
        _ => return None,
    })
}

/// `value` split at commas outside parentheses and quotes.
fn split_layers(value: &str) -> Vec<&str> {
    split_outside(value, |c| c == ',')
        .into_iter()
        .map(str::trim)
        .collect()
}

/// `value` split at whitespace outside parentheses and quotes, empty
/// pieces dropped.
fn split_tokens(value: &str) -> Vec<&str> {
    split_outside(value, char::is_whitespace)
        .into_iter()
        .filter(|t| !t.is_empty())
        .collect()
}

fn split_outside(value: &str, at: impl Fn(char) -> bool) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut quote = None;
    let mut start = 0;
    for (i, ch) in value.char_indices() {
        match (quote, ch) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => quote = Some(ch),
            (None, '(') => depth += 1,
            (None, ')') => depth = depth.saturating_sub(1),
            (None, c) if depth == 0 && at(c) => {
                parts.push(&value[start..i]);
                start = i + c.len_utf8();
            }
            _ => {}
        }
    }
    parts.push(&value[start..]);
    if parts.len() == 1 && parts[0].trim().is_empty() {
        parts.clear();
    }
    parts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BackgroundPositionValue::{Percent, Px};

    fn no_gradient(_: &str) -> Option<Gradient> {
        None
    }

    fn applied(decls: &[(&str, &str)]) -> Mask {
        let mut mask = Mask::default();
        for (p, v) in decls {
            mask.apply(p, v, &no_gradient);
        }
        mask
    }

    /// The board's repro: a quoted `data:` SVG with spaces, `<`, `=` and
    /// `%22` in it, under both the prefixed and unprefixed names.
    const REPRO_URL: &str = "data:image/svg+xml,<svg xmlns=%22http://www.w3.org/2000/svg%22 viewBox=%220 0 10 10%22><circle cx=%225%22 cy=%225%22 r=%225%22/></svg>";

    #[test]
    fn mask_image_takes_a_quoted_data_svg_url() {
        for name in ["mask-image", "-webkit-mask-image"] {
            let mask = applied(&[(name, &format!("url('{REPRO_URL}')"))]);
            assert_eq!(
                mask.images,
                vec![BackgroundImage::Url(REPRO_URL.to_string())],
                "{name}"
            );
            assert!(mask.has_image());
        }
    }

    #[test]
    fn mask_image_url_forms() {
        let m = applied(&[("mask-image", "url(icons/a.svg)")]);
        assert_eq!(m.images, vec![BackgroundImage::Url("icons/a.svg".into())]);
        let m = applied(&[("mask-image", "url(\"a(1), b.svg\")")]);
        assert_eq!(m.images, vec![BackgroundImage::Url("a(1), b.svg".into())]);
    }

    #[test]
    fn mask_image_none_is_a_layer_that_does_not_mask() {
        let m = applied(&[("mask-image", "none")]);
        assert_eq!(m.images, vec![BackgroundImage::None]);
        assert!(!m.has_image());
        assert!(!Mask::default().has_image());
    }

    #[test]
    fn an_invalid_mask_image_is_dropped_and_the_earlier_value_stays() {
        let mut m = applied(&[("mask-image", "url(a.svg)")]);
        assert!(!m.apply("mask-image", "bogus(1)", &no_gradient));
        assert_eq!(m.images, vec![BackgroundImage::Url("a.svg".into())]);
    }

    #[test]
    fn mask_longhands_parse_like_their_background_counterparts() {
        let m = applied(&[
            ("-webkit-mask-size", "16px 12px"),
            ("mask-position", "center"),
            ("-webkit-mask-repeat", "no-repeat"),
            ("mask-image", "url(a.svg)"),
        ]);
        let layers = m.layers();
        assert_eq!(layers.len(), 1);
        let l = &layers[0];
        assert_eq!(
            l.size,
            BackgroundSize::Explicit {
                width: Some(16.0),
                height: Some(12.0)
            }
        );
        assert_eq!(
            l.position,
            BackgroundPosition {
                x: Percent(0.5),
                y: Percent(0.5)
            }
        );
        assert_eq!(l.repeat, BackgroundRepeat::NoRepeat);
        assert_eq!(
            l.origin,
            BackgroundOrigin::BorderBox,
            "mask-origin's initial is border-box"
        );
        assert_eq!(l.clip, BackgroundClip::BorderBox);
    }

    #[test]
    fn layers_are_bottom_first_and_shorter_lists_repeat() {
        let m = applied(&[
            ("mask-image", "url(top.svg), url(mid.svg), url(bottom.svg)"),
            ("mask-size", "contain, 10px"),
        ]);
        let layers = m.layers();
        let urls: Vec<_> = layers
            .iter()
            .map(|l| match &l.image {
                BackgroundImage::Url(u) => u.as_str(),
                _ => "",
            })
            .collect();
        assert_eq!(urls, ["bottom.svg", "mid.svg", "top.svg"]);
        // CSS index 2 (bottom) takes sizes[2 % 2] = contain.
        assert_eq!(layers[0].size, BackgroundSize::Contain);
        assert_eq!(
            layers[1].size,
            BackgroundSize::Explicit {
                width: Some(10.0),
                height: None
            }
        );
        assert_eq!(layers[2].size, BackgroundSize::Contain);
    }

    #[test]
    fn the_mask_shorthand_sets_every_longhand() {
        let m = applied(&[(
            "-webkit-mask",
            "url(\"i.svg\") no-repeat right 4px top / contain content-box",
        )]);
        let layers = m.layers();
        assert_eq!(layers.len(), 1);
        let l = &layers[0];
        assert_eq!(l.image, BackgroundImage::Url("i.svg".into()));
        assert_eq!(l.repeat, BackgroundRepeat::NoRepeat);
        assert_eq!(l.size, BackgroundSize::Contain);
        assert_eq!(l.position.y, Percent(0.0));
        assert_eq!(
            l.position.x,
            crate::BackgroundPositionValue::Calc {
                percent: 1.0,
                px: -4.0
            }
        );
        assert_eq!(l.origin, BackgroundOrigin::ContentBox);
        assert_eq!(l.clip, BackgroundClip::ContentBox);
    }

    #[test]
    fn the_shorthand_resets_longhands_it_does_not_name() {
        let m = applied(&[("mask-size", "8px"), ("mask", "url(a.svg) 2px 3px")]);
        let l = &m.layers()[0];
        assert_eq!(l.size, BackgroundSize::Auto);
        assert_eq!(
            l.position,
            BackgroundPosition {
                x: Px(2.0),
                y: Px(3.0)
            }
        );
        assert_eq!(l.repeat, BackgroundRepeat::Repeat);
        let m = applied(&[("mask-image", "url(a.svg)"), ("mask", "none")]);
        assert!(!m.has_image());
    }

    #[test]
    fn the_shorthand_accepts_mode_and_composite_keywords() {
        let m = applied(&[("mask", "url(a.svg) center/16px 16px no-repeat alpha add")]);
        let l = &m.layers()[0];
        assert_eq!(
            l.size,
            BackgroundSize::Explicit {
                width: Some(16.0),
                height: Some(16.0)
            }
        );
        assert_eq!(l.repeat, BackgroundRepeat::NoRepeat);
    }

    #[test]
    fn the_shorthand_rejects_two_images_in_one_layer() {
        let mut m = Mask::default();
        assert!(!m.apply("mask", "url(a.svg) url(b.svg)", &no_gradient));
        assert!(!m.has_image());
    }

    #[test]
    fn gradients_go_through_the_callers_parser() {
        let g = |v: &str| {
            v.starts_with("linear-gradient(").then(|| {
                Gradient::Linear(crate::LinearGradient::new(
                    crate::GradientDirection::Angle(180.0),
                    vec![],
                ))
            })
        };
        let mut m = Mask::default();
        assert!(m.apply("mask-image", "linear-gradient(black, transparent)", &g));
        assert!(matches!(m.images[0], BackgroundImage::Gradient(_)));
    }

    #[test]
    fn initial_resets_one_longhand_or_all() {
        let mut m = applied(&[("mask-image", "url(a.svg)"), ("mask-size", "4px")]);
        m.reset("-webkit-mask-size");
        assert!(m.sizes.is_empty());
        assert!(m.has_image());
        m.reset("mask");
        assert_eq!(m, Mask::default());
    }

    #[test]
    fn non_mask_properties_are_not_taken() {
        let mut m = Mask::default();
        assert!(!m.apply("mask-border", "url(a.svg)", &no_gradient));
        assert!(!m.apply("-webkit-mask-box-image", "url(a.svg)", &no_gradient));
        assert!(!m.apply("background-image", "url(a.svg)", &no_gradient));
        assert_eq!(
            canonical_mask_property("-webkit-mask-position"),
            Some("mask-position")
        );
    }

    #[test]
    fn a_stylesheet_hands_the_whole_data_url_to_the_mask() {
        // The url's spaces, `<`, `/` and `=` must survive declaration parsing.
        let css = format!(
            ".i {{ -webkit-mask: url('{REPRO_URL}') no-repeat center / contain; mask-size: 50%; }}"
        );
        let sheet = crate::Stylesheet::parse(&css).expect("css");
        let mut mask = Mask::default();
        for decl in &sheet.rules[0].declarations {
            let crate::PropertyValue::Specified(v) = &decl.value else {
                panic!("{decl:?}");
            };
            assert!(mask.apply(&decl.property, v, &no_gradient), "{decl:?}");
        }
        let l = &mask.layers()[0];
        assert_eq!(l.image, BackgroundImage::Url(REPRO_URL.to_string()));
        assert_eq!(l.repeat, BackgroundRepeat::NoRepeat);
        assert_eq!(
            l.size,
            BackgroundSize::Explicit {
                width: Some(-50.0),
                height: None
            }
        );
    }

    #[test]
    fn computed_style_carries_the_mask() {
        let style = crate::ComputedStyle::default();
        assert!(!style.mask.has_image());
    }
}
