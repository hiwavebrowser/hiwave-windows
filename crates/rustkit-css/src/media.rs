//! Media query evaluation (Media Queries 4) against a static viewport.
//!
//! The environment is the one the real-site board pins Chrome to: a screen
//! of the given size, device pixel ratio 1, light colour scheme, a fine
//! hover-capable pointer, no reduced-motion preference. Anything this
//! evaluator does not understand does not match, which is what Chrome does
//! with an unknown media feature.

/// Does a media query list (the prelude of `@media`) match a viewport of
/// `width` x `height` CSS px? Comma-separated queries match if any does.
pub fn media_query_list_matches(list: &str, width: f32, height: f32) -> bool {
    let list = list.trim();
    if list.is_empty() {
        return true;
    }
    split_top_level(list, ',')
        .iter()
        .any(|q| query_matches(q.trim(), width, height))
}

fn query_matches(query: &str, width: f32, height: f32) -> bool {
    let lower = query.to_ascii_lowercase();
    let mut q = lower.trim();
    let mut negate = false;
    if let Some(rest) = strip_word(q, "not") {
        // `not screen and (...)`; a leading `not (` is a condition, handled below.
        if !rest.starts_with('(') {
            negate = true;
            q = rest;
        }
    }
    if let Some(rest) = strip_word(q, "only") {
        q = rest;
    }
    let matched = if q.starts_with('(') || q.starts_with("not") {
        condition_matches(q, width, height)
    } else {
        // A media type, optionally `and` a condition.
        let type_end = q.find(char::is_whitespace).unwrap_or(q.len());
        let (media_type, rest) = (&q[..type_end], q[type_end..].trim());
        let type_ok = matches!(media_type, "all" | "screen");
        let cond_ok = match strip_word(rest, "and") {
            Some(cond) => condition_matches(cond, width, height),
            None => rest.is_empty(),
        };
        type_ok && cond_ok
    };
    matched != negate
}

/// `<media-condition>`: `not X`, `X and Y ...`, `X or Y ...`, where each X
/// is a parenthesized feature or nested condition.
fn condition_matches(cond: &str, width: f32, height: f32) -> bool {
    let cond = cond.trim();
    if let Some(rest) = strip_word(cond, "not") {
        return !condition_matches(rest, width, height);
    }
    let ands = split_top_level_word(cond, "and");
    if ands.len() > 1 {
        return ands.iter().all(|c| condition_matches(c, width, height));
    }
    let ors = split_top_level_word(cond, "or");
    if ors.len() > 1 {
        return ors.iter().any(|c| condition_matches(c, width, height));
    }
    match cond.strip_prefix('(').and_then(|c| c.strip_suffix(')')) {
        Some(inner) => {
            let inner = inner.trim();
            if inner.starts_with('(') || inner.starts_with("not ") {
                condition_matches(inner, width, height)
            } else {
                feature_matches(inner, width, height)
            }
        }
        None => false,
    }
}

fn feature_matches(feature: &str, width: f32, height: f32) -> bool {
    // Range syntax: `width >= 600px`, `400px <= width < 700px`.
    if feature.contains(['<', '>']) || (feature.contains('=') && !feature.contains(':')) {
        return range_matches(feature, width, height);
    }
    let (name, value) = match feature.split_once(':') {
        Some((n, v)) => (n.trim(), Some(v.trim())),
        None => (feature.trim(), None),
    };
    // `-webkit-min-device-pixel-ratio` puts its prefix inside the vendor one.
    let vendor;
    let name = match name.strip_prefix("-webkit-") {
        Some(rest) if rest.starts_with("min-") || rest.starts_with("max-") => {
            vendor = format!("{}-webkit-{}", &rest[..4], &rest[4..]);
            vendor.as_str()
        }
        _ => name,
    };
    let (prefix, base) = match name.strip_prefix("min-") {
        Some(b) => (Some(true), b),
        None => match name.strip_prefix("max-") {
            Some(b) => (Some(false), b),
            None => (None, name),
        },
    };
    let base = base.trim_start_matches("device-");
    match (base, value) {
        ("width" | "height" | "aspect-ratio" | "resolution" | "-webkit-device-pixel-ratio"
        | "-moz-device-pixel-ratio", None) => prefix.is_none(),
        ("width" | "height" | "aspect-ratio" | "resolution" | "-webkit-device-pixel-ratio"
        | "-moz-device-pixel-ratio", Some(v)) => {
            let Some(actual) = actual_value(base, width, height) else {
                return false;
            };
            let Some(wanted) = parse_value(base, v) else {
                return false;
            };
            compare(actual, wanted, prefix)
        }
        ("orientation", Some(v)) => {
            v == if height >= width { "portrait" } else { "landscape" }
        }
        ("prefers-color-scheme", Some(v)) => v == "light",
        ("prefers-reduced-motion" | "prefers-reduced-transparency" | "prefers-contrast", Some(v)) => {
            v == "no-preference"
        }
        ("prefers-reduced-motion" | "prefers-reduced-transparency" | "prefers-contrast", None) => {
            false
        }
        ("hover" | "any-hover", Some(v)) => v == "hover",
        ("pointer" | "any-pointer", Some(v)) => v == "fine",
        ("hover" | "any-hover" | "pointer" | "any-pointer", None) => true,
        ("forced-colors" | "inverted-colors", Some(v)) => v == "none",
        ("forced-colors" | "inverted-colors", None) => false,
        ("display-mode", Some(v)) => v == "browser",
        ("scripting", Some(v)) => v == "enabled",
        ("update", Some(v)) => v == "fast",
        ("dynamic-range" | "video-dynamic-range", Some(v)) => v == "standard",
        ("color-gamut", Some(v)) => v == "srgb",
        ("color", None) => true,
        ("color", Some(v)) => parse_number(v).is_some_and(|n| compare(8.0, n, prefix)),
        ("monochrome" | "grid", None) => false,
        ("monochrome" | "grid", Some(v)) => parse_number(v).is_some_and(|n| compare(0.0, n, prefix)),
        _ => false,
    }
}

fn actual_value(base: &str, width: f32, height: f32) -> Option<f32> {
    match base {
        "width" => Some(width),
        "height" => Some(height),
        "aspect-ratio" if height > 0.0 => Some(width / height),
        "resolution" | "-webkit-device-pixel-ratio" | "-moz-device-pixel-ratio" => Some(1.0),
        _ => None,
    }
}

fn parse_value(base: &str, v: &str) -> Option<f32> {
    match base {
        "width" | "height" => parse_length(v),
        "aspect-ratio" => match v.split_once('/') {
            Some((a, b)) => Some(parse_number(a)? / parse_number(b)?),
            None => parse_number(v),
        },
        "resolution" => {
            if let Some(n) = v.strip_suffix("dppx").or_else(|| v.strip_suffix('x')) {
                parse_number(n)
            } else if let Some(n) = v.strip_suffix("dpi") {
                Some(parse_number(n)? / 96.0)
            } else if let Some(n) = v.strip_suffix("dpcm") {
                Some(parse_number(n)? * 2.54 / 96.0)
            } else {
                None
            }
        }
        _ => parse_number(v),
    }
}

/// px, em/rem (16px, the initial font size), or unitless 0.
fn parse_length(v: &str) -> Option<f32> {
    let v = v.trim();
    if let Some(n) = v.strip_suffix("px") {
        parse_number(n)
    } else if let Some(n) = v.strip_suffix("rem").or_else(|| v.strip_suffix("em")) {
        Some(parse_number(n)? * 16.0)
    } else {
        parse_number(v).filter(|n| *n == 0.0)
    }
}

fn parse_number(v: &str) -> Option<f32> {
    v.trim().parse::<f32>().ok()
}

/// `min-` is actual >= wanted, `max-` is actual <= wanted, plain is equal.
fn compare(actual: f32, wanted: f32, min: Option<bool>) -> bool {
    match min {
        Some(true) => actual >= wanted,
        Some(false) => actual <= wanted,
        None => (actual - wanted).abs() < 0.001,
    }
}

fn range_matches(feature: &str, width: f32, height: f32) -> bool {
    // Tokenize into operands and operators: a op b [op c].
    let mut parts: Vec<String> = Vec::new();
    let mut ops: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut chars = feature.chars().peekable();
    while let Some(c) = chars.next() {
        if matches!(c, '<' | '>' | '=') {
            let mut op = c.to_string();
            if c != '=' && chars.peek() == Some(&'=') {
                op.push('=');
                chars.next();
            }
            parts.push(cur.trim().to_string());
            cur.clear();
            ops.push(op);
        } else {
            cur.push(c);
        }
    }
    parts.push(cur.trim().to_string());
    if ops.is_empty() || parts.len() != ops.len() + 1 {
        return false;
    }
    let names = ["width", "height", "aspect-ratio", "resolution"];
    let Some(name) = parts.iter().find(|p| names.contains(&p.as_str())).cloned() else {
        return false;
    };
    let Some(actual) = actual_value(&name, width, height) else {
        return false;
    };
    let value_of = |p: &str| -> Option<f32> {
        if p == name {
            Some(actual)
        } else {
            parse_value(&name, p)
        }
    };
    ops.iter().enumerate().all(|(i, op)| {
        let (Some(a), Some(b)) = (value_of(&parts[i]), value_of(&parts[i + 1])) else {
            return false;
        };
        match op.as_str() {
            "<" => a < b,
            "<=" => a <= b,
            ">" => a > b,
            ">=" => a >= b,
            "=" => (a - b).abs() < 0.001,
            _ => false,
        }
    })
}

/// `word` followed by whitespace or `(` at the start of `s`; the rest.
fn strip_word<'a>(s: &'a str, word: &str) -> Option<&'a str> {
    let rest = s.strip_prefix(word)?;
    if rest.starts_with(char::is_whitespace) || rest.starts_with('(') {
        Some(rest.trim_start())
    } else {
        None
    }
}

fn split_top_level(s: &str, sep: char) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut start = 0;
    for (i, c) in s.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            c if c == sep && depth == 0 => {
                out.push(&s[start..i]);
                start = i + c.len_utf8();
            }
            _ => {}
        }
    }
    out.push(&s[start..]);
    out
}

/// Split on a keyword (`and`/`or`) surrounded by whitespace at paren depth 0.
fn split_top_level_word<'a>(s: &'a str, word: &str) -> Vec<&'a str> {
    let bytes = s.as_bytes();
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut start = 0;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'(' => depth += 1,
            b')' => depth -= 1,
            _ => {}
        }
        if depth == 0
            && s[i..].starts_with(word)
            && i > 0
            && bytes[i - 1].is_ascii_whitespace()
            && s[i + word.len()..].starts_with(|c: char| c.is_whitespace() || c == '(')
        {
            out.push(s[start..i].trim());
            i += word.len();
            start = i;
            continue;
        }
        i += 1;
    }
    out.push(s[start..].trim());
    out
}

#[cfg(test)]
mod tests {
    use super::media_query_list_matches as m;

    #[test]
    fn widths_and_types_at_the_board_viewport() {
        let (w, h) = (1280.0, 800.0);
        assert!(m("(min-width: 834px)", w, h));
        assert!(!m("(max-width: 833px)", w, h));
        assert!(!m("only screen and (max-width: 640px)", w, h));
        assert!(m("only screen and (min-width: 1069px)", w, h));
        assert!(m("screen", w, h));
        assert!(!m("print", w, h));
        assert!(m("print, (min-width: 1000px)", w, h));
        assert!(!m("not screen", w, h));
        assert!(m("not print", w, h));
        assert!(m("(min-width: 48em)", w, h));
        assert!(!m("(max-width: 47.9375rem)", w, h));
        assert!(m("(width >= 1024px)", w, h));
        assert!(!m("(width < 1024px)", w, h));
        assert!(m("(768px <= width < 1440px)", w, h));
        assert!(m("(min-width: 600px) and (max-width: 1300px)", w, h));
        assert!(!m("(min-width: 600px) and (max-width: 1000px)", w, h));
        assert!(m("(max-width: 500px) or (min-width: 1200px)", w, h));
        assert!(m("not (max-width: 500px)", w, h));
        assert!(m("(orientation: landscape)", w, h));
        assert!(m("(min-aspect-ratio: 16/10)", w, h));
        assert!(!m("(min-aspect-ratio: 16/9)", w, h));
    }

    #[test]
    fn environment_features_match_the_pinned_chrome() {
        let (w, h) = (1280.0, 800.0);
        assert!(m("(prefers-color-scheme: light)", w, h));
        assert!(!m("(prefers-color-scheme: dark)", w, h));
        assert!(!m("(prefers-reduced-motion: reduce)", w, h));
        assert!(m("(prefers-reduced-motion: no-preference)", w, h));
        assert!(m("(hover: hover) and (pointer: fine)", w, h));
        assert!(!m("(-webkit-min-device-pixel-ratio: 2), (min-resolution: 192dpi)", w, h));
        assert!(m("(min-resolution: 1dppx)", w, h));
        assert!(m("(-webkit-min-device-pixel-ratio: 1)", w, h));
        assert!(m("(min--moz-device-pixel-ratio: 1)", w, h));
        assert!(!m("(frobnicate: yes)", w, h));
        assert!(m("", w, h));
    }
}
