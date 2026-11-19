//! `@font-face` — the at-rule parse.
//!
//! MEASURED BEFORE WRITING (behavioural probe against the real parser, not a
//! reading of it): `@font-face { ... }` **already survives tokenization**. It
//! arrives as an ordinary [`Rule`] whose `selector` is the literal string
//! `"@font-face"`, with every descriptor intact in `declarations`. So the
//! missing piece was never a tokenizer — it is RECOGNITION plus
//! DESCRIPTOR-VALUE parsing, which is what lives here. Recorded because the
//! cost estimate for this quarter was built on "no parse exists at all", and
//! the truth is smaller in a specific way.

use crate::{Declaration, FontStretch, FontStyle, FontWeight, PropertyValue, Rule, Stylesheet};

/// `font-display` — how a face behaves while its file is still loading.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FontDisplayValue {
    #[default]
    Auto,
    Block,
    Swap,
    Fallback,
    Optional,
}

/// A parsed `@font-face` at-rule.
///
/// This is the CSS-level descriptor bag. `rustkit-layout` converts it into its
/// own `FontFaceRule` for the loader — the dependency runs one way only
/// (layout → css), so this crate cannot name the layout type.
///
/// `PartialEq` only, no `Eq`: `FontWeight` is not `Eq` in this crate, and
/// widening it here to satisfy a derive would change a shared type for the
/// convenience of one struct.
#[derive(Debug, Clone, PartialEq)]
pub struct FontFaceRule {
    pub family: String,
    pub src: String,
    pub weight: FontWeight,
    pub style: FontStyle,
    pub stretch: FontStretch,
    pub unicode_range: Option<String>,
    pub display: FontDisplayValue,
}

/// Split on commas that are not inside parentheses or quotes.
///
/// `url(data:font/woff2;base64,AAAA)` contains a comma INSIDE the parens, and a
/// naive `split(',')` tears such a data URI in half — producing two entries
/// that are each invalid, so a perfectly good inline font silently vanishes.
fn split_top_level_commas(value: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut quote: Option<char> = None;
    let mut current = String::new();
    for c in value.chars() {
        match c {
            '"' | '\'' if quote == Some(c) => {
                quote = None;
                current.push(c);
            }
            '"' | '\'' if quote.is_none() => {
                quote = Some(c);
                current.push(c);
            }
            '(' if quote.is_none() => {
                depth += 1;
                current.push(c);
            }
            ')' if quote.is_none() => {
                depth = depth.saturating_sub(1);
                current.push(c);
            }
            ',' if quote.is_none() && depth == 0 => {
                out.push(std::mem::take(&mut current));
            }
            _ => current.push(c),
        }
    }
    if !current.trim().is_empty() {
        out.push(current);
    }
    out
}

/// `name(` at the start of `s` (any case): what is between the parentheses,
/// trimmed, and what follows them.
fn function_arg<'a>(s: &'a str, name: &str) -> Option<(&'a str, &'a str)> {
    let head = s.get(..name.len())?;
    let body = s[name.len()..].strip_prefix('(')?;
    if !head.eq_ignore_ascii_case(name) {
        return None;
    }
    let close = body.find(')')?;
    Some((body[..close].trim(), &body[close + 1..]))
}

/// Whether a `format()` argument names a container the font loader decodes:
/// sfnt (TrueType, OpenType, a collection), WOFF and WOFF2
/// (`rustkit_text::webfonts`). `embedded-opentype` and `svg` are formats, but
/// nothing here reads them; anything else is no format at all. The answers
/// are Chrome 148's: it takes a string in any case or the keyword, and the
/// legacy `-variations` names as strings only.
fn format_is_decodable(arg: &str) -> bool {
    let quoted = arg.len() >= 2
        && [('"', '"'), ('\'', '\'')].iter().any(|(open, close)| arg.starts_with(*open) && arg.ends_with(*close));
    let name = if quoted { &arg[1..arg.len() - 1] } else { arg };
    match name.to_ascii_lowercase().as_str() {
        "woff" | "woff2" | "truetype" | "opentype" | "collection" => true,
        "woff-variations" | "woff2-variations" | "truetype-variations" | "opentype-variations" => quoted,
        _ => false,
    }
}

/// Whether every technology a `tech()` argument lists is one Chrome 148
/// accepts on macOS (the pinned oracle): a source that needs one it does not
/// have (`color-SVG`, `features-graphite`, `incremental`) or does not know is
/// passed over. `features-aat` is accepted there only on macOS.
fn tech_is_supported(arg: &str) -> bool {
    !arg.is_empty()
        && arg.split(',').all(|tech| {
            matches!(
                tech.trim().to_ascii_lowercase().as_str(),
                "variations"
                    | "palettes"
                    | "features-opentype"
                    | "features-aat"
                    | "color-colrv0"
                    | "color-colrv1"
                    | "color-sbix"
                    | "color-cbdt"
            )
        })
}

/// The URL of one `src` entry, when the entry is a source the loader can
/// use: `url(...)`, then an optional `format()` it decodes, then an optional
/// `tech()` it supports, and nothing else.
fn usable_source(entry: &str) -> Option<String> {
    let entry = entry.trim();
    let after_name = entry.get(..3).filter(|name| name.eq_ignore_ascii_case("url")).map(|_| &entry[3..])?;
    let inside = after_name.strip_prefix('(')?.trim_start();
    // A quoted URL ends at its closing quote, so it may hold a `)`.
    let (url, mut rest) = match inside.chars().next() {
        Some(quote @ ('"' | '\'')) => {
            let close = inside[1..].find(quote)?;
            (&inside[1..1 + close], inside[close + 2..].trim_start().strip_prefix(')')?)
        }
        _ => {
            let close = inside.find(')')?;
            (inside[..close].trim(), &inside[close + 1..])
        }
    };
    if url.is_empty() {
        return None;
    }
    rest = rest.trim_start();
    if let Some((format, after)) = function_arg(rest, "format") {
        if !format_is_decodable(format) {
            return None;
        }
        rest = after.trim_start();
    }
    if let Some((tech, after)) = function_arg(rest, "tech") {
        if !tech_is_supported(tech) {
            return None;
        }
        rest = after.trim_start();
    }
    rest.is_empty().then(|| url.to_string())
}

/// Extract the first usable font URL from a `src` descriptor.
///
/// `src` is a comma-separated PRIORITY LIST, and entries we cannot use must be
/// SKIPPED rather than aborting the descriptor: `local("Inter")` names an
/// installed face with no URL to fetch, and real stylesheets routinely list
/// `local(...)` first as an optimisation. Treating the first entry as
/// authoritative would drop the webfont on exactly the pages that took care to
/// offer one. The same goes for a `url()` whose `format()` no decoder here
/// reads: the list every icon-font generator writes opens with
/// `url(x.eot?#iefix) format("embedded-opentype")`, and taking it fetched the
/// .eot and never the woff2 after it. A source with no `format()` is tried.
fn first_fetchable_src(value: &str) -> Option<String> {
    split_top_level_commas(value).iter().find_map(|entry| usable_source(entry))
}

/// Parse an `@font-face` weight descriptor.
///
/// CSS Fonts 4 allows a RANGE here (`font-weight: 400 700`) for variable fonts.
/// We take the first value and record the simplification rather than rejecting
/// the rule: a variable font registered at its low end still renders text,
/// whereas a dropped rule renders none.
fn parse_font_face_weight(value: &str) -> FontWeight {
    let first = value.split_whitespace().next().unwrap_or("");
    match first.to_ascii_lowercase().as_str() {
        "normal" => FontWeight::NORMAL,
        "bold" => FontWeight::BOLD,
        n => n
            .parse::<u16>()
            .map(FontWeight)
            .unwrap_or(FontWeight::NORMAL),
    }
}

fn specified(decl: &Declaration) -> Option<&str> {
    match &decl.value {
        PropertyValue::Specified(v) => Some(v.trim()),
        _ => None,
    }
}

/// Parse an `@font-face` rule out of a stylesheet rule.
///
/// Returns `None` for any rule that is not `@font-face`, and for an
/// `@font-face` that is INVALID. Per CSS Fonts, a face with no `font-family`
/// or no usable `src` must be IGNORED — dropping it is the specified
/// behaviour, not defensive coding, and keeping it would register a family
/// name that can never resolve to glyphs.
pub fn parse_font_face(rule: &Rule) -> Option<FontFaceRule> {
    // At-rule names are case-insensitive (`@FONT-FACE` is legal) and the
    // tokenizer preserves author whitespace ahead of the block.
    if !rule.selector.trim().eq_ignore_ascii_case("@font-face") {
        return None;
    }

    let mut family: Option<String> = None;
    let mut src: Option<String> = None;
    let mut weight = FontWeight::NORMAL;
    let mut style = FontStyle::Normal;
    let mut stretch = FontStretch::Normal;
    let mut unicode_range: Option<String> = None;
    let mut display = FontDisplayValue::Auto;

    for decl in &rule.declarations {
        let Some(value) = specified(decl) else {
            continue;
        };
        match decl.property.trim().to_ascii_lowercase().as_str() {
            "font-family" => {
                let name = value.trim_matches(|c| c == '"' || c == '\'').trim();
                if !name.is_empty() {
                    family = Some(name.to_string());
                }
            }
            "src" => src = first_fetchable_src(value),
            "font-weight" => weight = parse_font_face_weight(value),
            "font-style" => {
                style = match value.to_ascii_lowercase().as_str() {
                    "italic" => FontStyle::Italic,
                    "oblique" => FontStyle::Oblique,
                    _ => FontStyle::Normal,
                }
            }
            "font-stretch" => {
                stretch = match value.to_ascii_lowercase().as_str() {
                    "ultra-condensed" => FontStretch::UltraCondensed,
                    "extra-condensed" => FontStretch::ExtraCondensed,
                    "condensed" => FontStretch::Condensed,
                    "semi-condensed" => FontStretch::SemiCondensed,
                    "semi-expanded" => FontStretch::SemiExpanded,
                    "expanded" => FontStretch::Expanded,
                    "extra-expanded" => FontStretch::ExtraExpanded,
                    "ultra-expanded" => FontStretch::UltraExpanded,
                    _ => FontStretch::Normal,
                }
            }
            "unicode-range" => {
                if !value.is_empty() {
                    unicode_range = Some(value.to_string());
                }
            }
            "font-display" => {
                display = match value.to_ascii_lowercase().as_str() {
                    "block" => FontDisplayValue::Block,
                    "swap" => FontDisplayValue::Swap,
                    "fallback" => FontDisplayValue::Fallback,
                    "optional" => FontDisplayValue::Optional,
                    _ => FontDisplayValue::Auto,
                }
            }
            _ => {}
        }
    }

    Some(FontFaceRule {
        family: family?,
        src: src?,
        weight,
        style,
        stretch,
        unicode_range,
        display,
    })
}

impl Stylesheet {
    /// Every valid `@font-face` rule in this stylesheet, in source order.
    ///
    /// Source order is load-bearing: when two faces declare the same family,
    /// the LATER one wins, so a caller that iterates in a different order
    /// registers the wrong file.
    pub fn font_face_rules(&self) -> Vec<FontFaceRule> {
        self.rules.iter().filter_map(parse_font_face).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sheet(css: &str) -> Stylesheet {
        Stylesheet::parse(css).expect("parse")
    }

    #[test]
    fn a_plain_font_face_parses_every_descriptor() {
        let s = sheet(
            r#"@font-face {
                font-family: "Inter";
                src: url(/fonts/inter.woff2) format("woff2");
                font-weight: 700;
                font-style: italic;
                font-stretch: condensed;
                unicode-range: U+0000-00FF;
                font-display: swap;
            }"#,
        );
        let faces = s.font_face_rules();
        assert_eq!(faces.len(), 1);
        let f = &faces[0];
        assert_eq!(f.family, "Inter", "quotes are stripped from the family");
        assert_eq!(
            f.src, "/fonts/inter.woff2",
            "the URL is extracted from url()"
        );
        assert_eq!(f.weight, FontWeight(700));
        assert_eq!(f.style, FontStyle::Italic);
        assert_eq!(f.stretch, FontStretch::Condensed);
        assert_eq!(f.unicode_range.as_deref(), Some("U+0000-00FF"));
        assert_eq!(f.display, FontDisplayValue::Swap);
    }

    #[test]
    fn a_face_with_no_family_is_dropped() {
        let s = sheet(r#"@font-face { src: url(/a.woff2); }"#);
        assert!(
            s.font_face_rules().is_empty(),
            "a family-less face can never resolve to glyphs; CSS says ignore it"
        );
    }

    #[test]
    fn a_face_with_no_fetchable_src_is_dropped() {
        let s = sheet(r#"@font-face { font-family: "Inter"; src: local("Inter"); }"#);
        assert!(
            s.font_face_rules().is_empty(),
            "local() names an installed face with nothing to fetch"
        );
    }

    #[test]
    fn local_sources_are_skipped_not_fatal() {
        // The shape real stylesheets ship: local() first as an optimisation,
        // url() as the fallback. Treating entry one as authoritative would
        // drop the webfont on exactly the pages that offered one.
        let s = sheet(
            r#"@font-face {
                font-family: Inter;
                src: local("Inter"), url(/fonts/inter.woff2) format("woff2");
            }"#,
        );
        let faces = s.font_face_rules();
        assert_eq!(faces.len(), 1);
        assert_eq!(faces[0].src, "/fonts/inter.woff2");
    }

    /// The `src` a face with this descriptor value ends up with.
    fn picked(src: &str) -> Option<String> {
        let s = sheet(&format!("@font-face {{ font-family: T; src: {src}; }}"));
        s.font_face_rules().into_iter().next().map(|f| f.src)
    }

    #[test]
    fn a_source_in_a_format_nothing_here_decodes_is_skipped() {
        // The list every icon-font generator writes. Taking the first url()
        // fetched the .eot, which no decoder here reads, and never the woff2.
        assert_eq!(
            picked(
                r#"url(/a.eot?#iefix) format("embedded-opentype"), url(/a.woff2) format("woff2"),
                   url(/a.woff) format("woff"), url(/a.ttf) format("truetype")"#
            )
            .as_deref(),
            Some("/a.woff2")
        );
        assert_eq!(picked(r#"url(/a.svg#T) format("svg"), url(/a.ttf) format("truetype")"#).as_deref(), Some("/a.ttf"));
        assert_eq!(picked(r#"url(/a.xyz) format("xyz"), url(/a.woff) format("woff")"#).as_deref(), Some("/a.woff"));
        assert_eq!(picked(r#"url(/a.xyz) format(""), url(/a.woff) format("woff")"#).as_deref(), Some("/a.woff"));
        // The first format that decodes wins, not the smallest file.
        assert_eq!(picked(r#"url(/a.woff) format("woff"), url(/a.woff2) format("woff2")"#).as_deref(), Some("/a.woff"));
        // Nothing usable: the face is dropped, as one with no src.
        assert_eq!(picked(r#"url(/a.eot) format("embedded-opentype"), url(/a.svg) format("svg")"#), None);
    }

    #[test]
    fn every_format_the_loader_decodes_is_taken() {
        for (format, file) in [
            (r#""truetype""#, "/a.ttf"),
            (r#""opentype""#, "/a.otf"),
            (r#""collection""#, "/a.ttc"),
            (r#""woff""#, "/a.woff"),
            (r#""woff2""#, "/a.woff2"),
            (r#""WOFF2""#, "/a.woff2"),
            ("'woff2'", "/a.woff2"),
            ("woff2", "/a.woff2"),
            ("truetype", "/a.ttf"),
            (r#""woff2-variations""#, "/a.woff2"),
            (r#""woff-variations""#, "/a.woff"),
            (r#""truetype-variations""#, "/a.ttf"),
            (r#""opentype-variations""#, "/a.otf"),
        ] {
            let src = format!("url({file}) format({format}), url(/fallback.bin)");
            assert_eq!(picked(&src).as_deref(), Some(file), "format({format})");
        }
        // The legacy `-variations` names are strings only; a keyword that is
        // no format, an empty format() and two formats are not sources.
        for format in ["woff2-variations", "warp", "", r#""woff2", "woff""#, "embedded-opentype", "svg"] {
            let src = format!("url(/a.woff2) format({format}), url(/b.woff)");
            assert_eq!(picked(&src).as_deref(), Some("/b.woff"), "format({format})");
        }
    }

    #[test]
    fn a_source_with_no_format_is_tried() {
        assert_eq!(picked(r#"url(/a.woff2), url(/b.woff) format("woff")"#).as_deref(), Some("/a.woff2"));
        assert_eq!(picked(r#"url(/a.eot) format("embedded-opentype"), url(/b.woff2)"#).as_deref(), Some("/b.woff2"));
        assert_eq!(picked(r#"URL(/a.woff) FORMAT("woff")"#).as_deref(), Some("/a.woff"));
    }

    #[test]
    fn tech_is_a_requirement_the_source_must_meet() {
        for tech in [
            "variations",
            "VARIATIONS",
            "palettes",
            "features-opentype",
            "color-COLRv0",
            "color-COLRv1",
            "color-sbix",
            "color-CBDT",
            "variations, palettes",
        ] {
            let src = format!("url(/a.woff2) format(woff2) tech({tech}), url(/b.woff)");
            assert_eq!(picked(&src).as_deref(), Some("/a.woff2"), "tech({tech})");
        }
        for tech in ["color-SVG", "features-graphite", "incremental", "warp-drive", "variations, incremental", ""] {
            let src = format!("url(/a.woff2) format(woff2) tech({tech}), url(/b.woff)");
            assert_eq!(picked(&src).as_deref(), Some("/b.woff"), "tech({tech})");
        }
        assert_eq!(picked("url(/a.woff2) tech(variations), url(/b.woff)").as_deref(), Some("/a.woff2"));
    }

    #[test]
    fn an_entry_that_is_no_source_is_skipped_alone() {
        assert_eq!(picked(r#"warp(/a.woff2), url(/b.woff) format("woff")"#).as_deref(), Some("/b.woff"));
        assert_eq!(picked(r#"url(/a.woff2) format("woff2") warp, url(/b.woff)"#).as_deref(), Some("/b.woff"));
        // A `)` inside a quoted url does not end it.
        assert_eq!(
            picked(r#"url("/a).eot") format("embedded-opentype"), url("/b).woff") format("woff")"#).as_deref(),
            Some("/b).woff")
        );
        assert_eq!(
            picked(r#"url(data:application/vnd.ms-fontobject;base64,AAAA,BBBB) format("embedded-opentype"), url(/b.woff)"#)
                .as_deref(),
            Some("/b.woff")
        );
    }

    #[test]
    fn a_data_uri_survives_its_own_commas() {
        // base64 payloads contain commas inside url(); a naive split(',')
        // tears the URI in half and the font vanishes with no error anywhere.
        let s = sheet(
            r#"@font-face {
                font-family: Embedded;
                src: url(data:font/woff2;base64,AAAA,BBBB) format("woff2");
            }"#,
        );
        let faces = s.font_face_rules();
        assert_eq!(faces.len(), 1);
        assert_eq!(faces[0].src, "data:font/woff2;base64,AAAA,BBBB");
    }

    #[test]
    fn a_weight_range_takes_its_first_value() {
        // CSS Fonts 4 variable-font range. Documented simplification: a face
        // registered at the low end renders; a rejected rule renders nothing.
        let s =
            sheet(r#"@font-face { font-family: Var; src: url(/v.woff2); font-weight: 400 700; }"#);
        assert_eq!(s.font_face_rules()[0].weight, FontWeight(400));
    }

    #[test]
    fn the_at_rule_name_is_case_insensitive() {
        let s = sheet(r#"@FONT-FACE { font-family: Loud; src: url(/l.woff2); }"#);
        assert_eq!(s.font_face_rules().len(), 1, "@FONT-FACE is legal CSS");
    }

    #[test]
    fn ordinary_rules_are_not_font_faces() {
        let s = sheet(r#"body { color: red; } @font-face { font-family: A; src: url(/a.woff2); }"#);
        let faces = s.font_face_rules();
        assert_eq!(faces.len(), 1, "only the at-rule is a face");
        assert_eq!(faces[0].family, "A");
    }

    #[test]
    fn faces_come_back_in_source_order() {
        // Later wins for a duplicate family, so order is not cosmetic.
        let s = sheet(
            r#"@font-face { font-family: Dup; src: url(/first.woff2); }
               @font-face { font-family: Dup; src: url(/second.woff2); }"#,
        );
        let faces = s.font_face_rules();
        assert_eq!(faces.len(), 2);
        assert_eq!(faces[0].src, "/first.woff2");
        assert_eq!(faces[1].src, "/second.woff2");
    }

    #[test]
    fn omitted_descriptors_take_their_initial_values() {
        let s = sheet(r#"@font-face { font-family: Bare; src: url(/b.woff2); }"#);
        let f = &s.font_face_rules()[0];
        assert_eq!(f.weight, FontWeight::NORMAL);
        assert_eq!(f.style, FontStyle::Normal);
        assert_eq!(f.stretch, FontStretch::Normal);
        assert_eq!(f.unicode_range, None);
        assert_eq!(f.display, FontDisplayValue::Auto);
    }
}
