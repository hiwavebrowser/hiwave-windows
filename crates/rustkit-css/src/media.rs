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

/// Serialize a media query list the way `MediaQueryList.media` reports it
/// (cssom §5.1 "serialize a media query list"): lowercased, whitespace
/// collapsed, each `(name: value)` feature spaced so, the queries joined
/// with `, `. A query that does not parse as a Media Queries 4
/// `<media-query>` (`foo bar`, `screen and`, `not(hover)`, `(a) and (b)
/// or (c)`, an unbalanced parenthesis) serializes as `not all`, which no
/// viewport matches; the rest of the list is unaffected. Evaluating the
/// serialized text with [`media_query_list_matches`] answers what the
/// original would in Chrome: an invalid query is false, not "unknown type
/// under `not`".
pub fn media_query_list_serialize(list: &str) -> String {
    let list = list.trim();
    if list.is_empty() {
        return String::new();
    }
    split_top_level(list, ',')
        .iter()
        .map(|q| {
            let lower = collapse_whitespace(&q.to_ascii_lowercase());
            serialize_query(&lower).unwrap_or_else(|| "not all".to_string())
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn collapse_whitespace(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A depth-0 token of a media query: a word, or the text inside one
/// balanced parenthesized group.
#[derive(Debug, Clone, Copy, PartialEq)]
enum MediaToken<'a> {
    Word(&'a str),
    Group(&'a str),
}

/// Tokenize a whitespace-collapsed query. `None` for an unbalanced
/// parenthesis or a word run straight into `(` (`not(`, `and(`: a function
/// token, which no media query grammar production accepts).
fn tokenize_media_query(s: &str) -> Option<Vec<MediaToken<'_>>> {
    let bytes = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b' ' => i += 1,
            b'(' => {
                let mut depth = 0i32;
                let mut j = i;
                loop {
                    if j >= bytes.len() {
                        return None;
                    }
                    match bytes[j] {
                        b'(' => depth += 1,
                        b')' => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                    j += 1;
                }
                out.push(MediaToken::Group(&s[i + 1..j]));
                i = j + 1;
                if i < bytes.len() && bytes[i] != b' ' {
                    return None;
                }
            }
            b')' => return None,
            _ => {
                let end = s[i..].find([' ', '(', ')']).map_or(bytes.len(), |k| i + k);
                out.push(MediaToken::Word(&s[i..end]));
                i = end;
                if i < bytes.len() && bytes[i] == b'(' {
                    return None;
                }
            }
        }
    }
    Some(out)
}

fn is_css_ident(s: &str) -> bool {
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == '_' || first == '-')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn is_media_type(s: &str) -> bool {
    is_css_ident(s) && !matches!(s, "not" | "and" | "or" | "only" | "layer")
}

/// `<media-query> = <media-condition> | [ not | only ]? <media-type> [ and
/// <media-condition-without-or> ]?`, serialized; `None` when invalid.
fn serialize_query(s: &str) -> Option<String> {
    use MediaToken::{Group, Word};
    let toks = tokenize_media_query(s)?;
    let (prefix, rest) = match toks.first()? {
        Word(kw @ ("not" | "only")) => match toks.get(1)? {
            // `not <media-condition>`: exactly one parenthesized query.
            Group(_) if *kw == "not" => {
                return serialize_condition(&toks, false);
            }
            Group(_) => return None,
            Word(_) => (Some(*kw), &toks[1..]),
        },
        Word(_) => (None, &toks[..]),
        Group(_) => return serialize_condition(&toks, false),
    };
    let Some(Word(media_type)) = rest.first() else {
        return None;
    };
    if !is_media_type(media_type) {
        return None;
    }
    let mut out = String::new();
    if let Some(kw) = prefix {
        out.push_str(kw);
        out.push(' ');
    }
    out.push_str(media_type);
    match rest.get(1) {
        None => Some(out),
        Some(Word("and")) => {
            let cond = serialize_condition(&rest[2..], true)?;
            out.push_str(" and ");
            out.push_str(&cond);
            Some(out)
        }
        Some(_) => None,
    }
}

/// `<media-condition> = <media-not> | <media-in-parens> [ <media-and>* |
/// <media-or>* ]`; with `without_or`, the `<media-condition-without-or>`
/// production (no `or`).
fn serialize_condition(toks: &[MediaToken<'_>], without_or: bool) -> Option<String> {
    use MediaToken::{Group, Word};
    match toks {
        [] => None,
        [Word("not"), Group(inner)] => Some(format!("not {}", serialize_in_parens(inner)?)),
        [Word("not"), ..] => None,
        [Group(first), rest @ ..] => {
            let mut out = serialize_in_parens(first)?;
            let mut connector: Option<&str> = None;
            let mut i = 0;
            while i < rest.len() {
                let (Word(word), Some(Group(inner))) = (rest[i], rest.get(i + 1)) else {
                    return None;
                };
                if !(word == "and" || (word == "or" && !without_or)) {
                    return None;
                }
                if connector.is_some_and(|c| c != word) {
                    return None;
                }
                connector = Some(word);
                out.push(' ');
                out.push_str(word);
                out.push(' ');
                out.push_str(&serialize_in_parens(inner)?);
                i += 2;
            }
            Some(out)
        }
        _ => None,
    }
}

/// `<media-in-parens>`: a nested condition, or a feature in plain, range or
/// boolean form, serialized with its parentheses.
fn serialize_in_parens(inner: &str) -> Option<String> {
    let inner = inner.trim();
    if inner.is_empty() {
        return None;
    }
    if inner.starts_with('(') || inner.starts_with("not ") {
        let toks = tokenize_media_query(inner)?;
        return Some(format!("({})", serialize_condition(&toks, false)?));
    }
    if inner.contains(['<', '>']) || (inner.contains('=') && !inner.contains(':')) {
        // Range syntax. Its operands are feature names, numbers, dimensions,
        // ratios and math functions, all ASCII: a non-ASCII char anywhere
        // makes the feature invalid (Chrome 148: `(ü > 1)` and
        // `(width > 600pxé)` are both `not all`). Page text, so the walk
        // below steps by char, never by byte.
        if !inner.is_ascii() {
            return None;
        }
        // One space around each comparison operator.
        let mut spaced = String::new();
        let mut skip = 0;
        for (i, c) in inner.char_indices() {
            if skip > 0 {
                skip -= 1;
                continue;
            }
            match ["<=", ">=", "<", ">", "="].iter().find(|op| inner[i..].starts_with(*op)) {
                Some(op) => {
                    spaced.push(' ');
                    spaced.push_str(op);
                    spaced.push(' ');
                    skip = op.len() - 1;
                }
                None => spaced.push(c),
            }
        }
        let spaced = collapse_whitespace(&spaced);
        return (!spaced.starts_with(['<', '>', '=']) && !spaced.ends_with(['<', '>', '='])
            && spaced.split(' ').any(is_css_ident))
            .then(|| format!("({spaced})"));
    }
    match inner.split_once(':') {
        Some((name, value)) => {
            let (name, value) = (name.trim(), value.trim());
            (is_css_ident(name) && !value.is_empty()).then(|| format!("({name}: {value})"))
        }
        None => is_css_ident(inner).then(|| format!("({inner})")),
    }
}

/// Font-relative and viewport units for an `@container` condition: em is
/// the query container's font size, rem the root element's, vw/vh the
/// viewport's (css-contain-3 §6.1: relative units resolve against the query
/// container's computed values, not its size).
#[derive(Debug, Clone, Copy)]
pub struct ContainerUnits {
    pub em: f32,
    pub rem: f32,
    pub vw: f32,
    pub vh: f32,
}

/// Does an `@container` size condition (css-contain-3 §6.3) hold for a query
/// container whose content box is `width` x `height` CSS px? `height` is
/// `None` for an `inline-size` container. `inline-size` and `block-size` are
/// `width` and `height` (horizontal writing mode).
///
/// Only size features are known: `width`, `height`, `aspect-ratio`,
/// `orientation` (with `min-`/`max-` and range syntax), compared with
/// lengths in absolute, font-relative or viewport units. Anything else (a
/// media feature such as `hover` or `color`, a `device-` feature, a style
/// query, a unit the walk cannot resolve such as `ch` or `cqw`, a block-size
/// feature of an `inline-size` container) is unknown, and an unknown part
/// makes the whole condition false, under `not` too: Chromium 148 applies
/// none of `not (hover)`, `(width > 299px) or (hover)`, `not (width >=
/// 30ch)` (#782 R2, docs/diagnostics/2026-10-26/css_container_query_r2.html).
/// Style queries Chromium does answer (`not style(--x)` holds there); here
/// they stay false.
pub fn container_condition_matches(
    condition: &str,
    width: f32,
    height: Option<f32>,
    units: ContainerUnits,
) -> bool {
    let cond = condition.to_ascii_lowercase();
    container_condition(cond.trim(), width, height, &units) == Some(true)
}

/// `Some(answer)`, or `None` when the condition is invalid (the rule is
/// dropped) or any part is unknown (see [`container_condition_matches`]).
fn container_condition(
    cond: &str,
    width: f32,
    height: Option<f32>,
    units: &ContainerUnits,
) -> Option<bool> {
    parse_container_query(cond)?.eval(width, height, units)
}

/// A parsed `<container-query>`.
#[derive(Debug)]
enum ContainerQuery<'a> {
    Not(Box<ContainerQuery<'a>>),
    And(Vec<ContainerQuery<'a>>),
    Or(Vec<ContainerQuery<'a>>),
    /// `( <size-feature> )`, the text between the parentheses.
    Feature(&'a str),
    /// `<general-enclosed>`: a function such as `style()`, or parentheses
    /// holding something that is neither a query nor a feature. Valid,
    /// always unknown.
    Unknown,
}

impl ContainerQuery<'_> {
    fn eval(&self, width: f32, height: Option<f32>, units: &ContainerUnits) -> Option<bool> {
        match self {
            Self::Not(q) => q.eval(width, height, units).map(|b| !b),
            // Every leg is evaluated: an unknown one anywhere is unknown.
            Self::And(legs) => legs.iter().try_fold(true, |acc, q| {
                Some(q.eval(width, height, units)? && acc)
            }),
            Self::Or(legs) => legs.iter().try_fold(false, |acc, q| {
                Some(q.eval(width, height, units)? || acc)
            }),
            Self::Feature(f) => container_feature(f, width, height, units),
            Self::Unknown => None,
        }
    }
}

/// Parse a lowercased `<container-query>`; `None` when it is invalid:
///
/// ```text
/// <container-query> = not <query-in-parens>
///                   | <query-in-parens> [ [ and <query-in-parens> ]* | [ or <query-in-parens> ]* ]
/// <query-in-parens> = ( <container-query> ) | ( <size-feature> ) | <general-enclosed>
/// ```
///
/// So `not` takes exactly one parenthesized query, each `and`/`or` leg is
/// one parenthesized query (no leading `not`), `and` and `or` do not mix,
/// and a keyword needs whitespace after it: `not(`, `and(` and `or(` are
/// function tokens. `not(...)` alone is a `<general-enclosed>` (unknown).
fn parse_container_query(s: &str) -> Option<ContainerQuery<'_>> {
    let mut p = QueryParser { s, i: 0 };
    p.ws();
    let query = if let Some(end) = p.keyword("not") {
        p.i = end;
        if !p.ws() {
            return None;
        }
        ContainerQuery::Not(Box::new(p.query_in_parens()?))
    } else {
        let first = p.query_in_parens()?;
        let mut legs = vec![first];
        let mut is_and = None;
        loop {
            p.ws();
            if p.i == s.len() {
                break;
            }
            let (and, end) = match (p.keyword("and"), p.keyword("or")) {
                (Some(end), _) => (true, end),
                (_, Some(end)) => (false, end),
                _ => return None,
            };
            if *is_and.get_or_insert(and) != and {
                return None;
            }
            p.i = end;
            if !p.ws() {
                return None;
            }
            legs.push(p.query_in_parens()?);
        }
        match (legs.len(), is_and) {
            (1, _) => legs.pop()?,
            (_, Some(true)) => ContainerQuery::And(legs),
            _ => ContainerQuery::Or(legs),
        }
    };
    p.ws();
    (p.i == s.len()).then_some(query)
}

struct QueryParser<'a> {
    s: &'a str,
    i: usize,
}

impl<'a> QueryParser<'a> {
    fn rest(&self) -> &'a str {
        &self.s[self.i..]
    }

    /// Skip whitespace; whether there was any.
    fn ws(&mut self) -> bool {
        let rest = self.rest();
        let trimmed = rest.trim_start();
        self.i += rest.len() - trimmed.len();
        trimmed.len() != rest.len()
    }

    /// The identifier at the cursor and the index just past it.
    fn ident(&self) -> Option<(&'a str, usize)> {
        let rest = self.rest();
        let len = rest
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'))
            .unwrap_or(rest.len());
        let word = &rest[..len];
        (word.starts_with(|c: char| c.is_ascii_alphabetic() || c == '-' || c == '_'))
            .then_some((word, self.i + len))
    }

    /// The end of `word` when it is the identifier at the cursor and not
    /// the name of a function (`word(`).
    fn keyword(&self, word: &str) -> Option<usize> {
        let (w, end) = self.ident()?;
        (w == word && !self.s[end..].starts_with('(')).then_some(end)
    }

    /// The text inside the parenthesized group at the cursor; moves past it.
    /// A parenthesis inside a quoted string (`style(--x: ")")`) neither
    /// opens nor closes one; a backslash escapes the next character there.
    fn group(&mut self) -> Option<&'a str> {
        let rest = self.rest();
        if !rest.starts_with('(') {
            return None;
        }
        let mut depth = 0i32;
        let mut quote: Option<char> = None;
        let mut escaped = false;
        for (at, c) in rest.char_indices() {
            if let Some(q) = quote {
                match c {
                    _ if escaped => escaped = false,
                    '\\' => escaped = true,
                    _ if c == q => quote = None,
                    _ => {}
                }
                continue;
            }
            match c {
                '"' | '\'' => quote = Some(c),
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        self.i += at + 1;
                        return Some(&rest[1..at]);
                    }
                }
                _ => {}
            }
        }
        None
    }

    fn query_in_parens(&mut self) -> Option<ContainerQuery<'a>> {
        if let Some(inner) = self.group() {
            let inner = inner.trim();
            let q = QueryParser { s: inner, i: 0 };
            let looks_like_query = inner.starts_with('(')
                || q.keyword("not").is_some()
                || q.ident().is_some_and(|(_, end)| inner[end..].starts_with('('));
            if !looks_like_query {
                return Some(ContainerQuery::Feature(inner));
            }
            return Some(parse_container_query(inner).unwrap_or(ContainerQuery::Unknown));
        }
        // A function such as `style(...)` or `not(...)`.
        let (_, end) = self.ident()?;
        if !self.s[end..].starts_with('(') {
            return None;
        }
        self.i = end;
        self.group()?;
        Some(ContainerQuery::Unknown)
    }
}

/// A size feature's name: `width`, `height`, `aspect-ratio`, `orientation`.
fn size_feature(name: &str) -> Option<&'static str> {
    match name.trim() {
        "width" | "inline-size" => Some("width"),
        "height" | "block-size" => Some("height"),
        "aspect-ratio" => Some("aspect-ratio"),
        "orientation" => Some("orientation"),
        _ => None,
    }
}

fn container_actual(base: &str, width: f32, height: Option<f32>) -> Option<f32> {
    match base {
        "width" => Some(width),
        "height" => height,
        "aspect-ratio" => {
            let h = height?;
            (h > 0.0).then(|| width / h)
        }
        _ => None,
    }
}

fn container_value(base: &str, v: &str, units: &ContainerUnits) -> Option<f32> {
    match base {
        "width" | "height" => container_length(v, units),
        "aspect-ratio" => {
            let (a, b) = match v.split_once('/') {
                Some((a, b)) => (parse_number(a)?, parse_number(b)?),
                None => (parse_number(v)?, 1.0),
            };
            (a >= 0.0 && b > 0.0).then(|| a / b)
        }
        _ => None,
    }
}

fn container_feature(
    feature: &str,
    width: f32,
    height: Option<f32>,
    units: &ContainerUnits,
) -> Option<bool> {
    if feature.contains(['<', '>']) || (feature.contains('=') && !feature.contains(':')) {
        return container_range(feature, width, height, units);
    }
    let (name, value) = match feature.split_once(':') {
        Some((n, v)) => (n.trim(), Some(v.trim())),
        None => (feature.trim(), None),
    };
    let (prefix, name) = match name.strip_prefix("min-") {
        Some(b) => (Some(true), b),
        None => match name.strip_prefix("max-") {
            Some(b) => (Some(false), b),
            None => (None, name),
        },
    };
    let base = size_feature(name)?;
    if base == "orientation" {
        let h = height?;
        let actual = if h >= width { "portrait" } else { "landscape" };
        return match (prefix, value) {
            (None, Some(v @ ("portrait" | "landscape"))) => Some(v == actual),
            (None, None) => Some(true),
            _ => None,
        };
    }
    let actual = container_actual(base, width, height);
    match value {
        // Boolean context: the feature's value is not zero. Chromium 148
        // answers `(width)` false on a 0px-wide container.
        None if prefix.is_none() => match base {
            "aspect-ratio" => Some(height? > 0.0 && width > 0.0),
            _ => Some(actual? != 0.0),
        },
        None => None,
        Some(v) => {
            let wanted = container_value(base, v, units)?;
            Some(compare(actual?, wanted, prefix))
        }
    }
}

fn container_range(
    feature: &str,
    width: f32,
    height: Option<f32>,
    units: &ContainerUnits,
) -> Option<bool> {
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
    let named: Vec<usize> = (0..parts.len()).filter(|i| size_feature(&parts[*i]).is_some()).collect();
    let [at] = named.as_slice() else {
        return None;
    };
    let base = size_feature(&parts[*at])?;
    if base == "orientation" {
        return None;
    }
    match ops.len() {
        1 => {}
        // `a < width < b`: the name in the middle, both one way.
        2 if *at == 1 => {
            let less = |o: &str| o.starts_with('<');
            let more = |o: &str| o.starts_with('>');
            if !(ops.iter().all(|o| less(o)) || ops.iter().all(|o| more(o))) {
                return None;
            }
        }
        _ => return None,
    }
    let actual = container_actual(base, width, height);
    let mut values = Vec::with_capacity(parts.len());
    for (i, p) in parts.iter().enumerate() {
        values.push(if i == *at { actual? } else { container_value(base, p, units)? });
    }
    let mut answer = true;
    for (i, op) in ops.iter().enumerate() {
        let (a, b) = (values[i], values[i + 1]);
        answer &= match op.as_str() {
            "<" => a < b,
            "<=" => a <= b,
            ">" => a > b,
            ">=" => a >= b,
            "=" => (a - b).abs() < 0.001,
            _ => return None,
        };
    }
    Some(answer)
}

/// A length in px for a container condition: unitless 0, or a number with
/// an absolute, font-relative (`em`, `rem`) or viewport (`vw`, `vh`,
/// `vmin`, `vmax`) unit; inside `calc()`/`min()`/`max()`/`clamp()` px,
/// font-relative and viewport units. Any other unit (`ch`, `ex`, `lh`,
/// `cq*`, `dvw`, a percentage) is unknown.
fn container_length(v: &str, units: &ContainerUnits) -> Option<f32> {
    let v = v.trim();
    if v.contains('%') {
        return None;
    }
    // min()/max() with any number of arguments; the crate's length parser
    // reads only two.
    for (name, min) in [("min(", true), ("max(", false)] {
        if let Some(inner) = v.strip_prefix(name).and_then(|r| r.strip_suffix(')')) {
            if balanced(inner) {
                let mut out: Option<f32> = None;
                for arg in split_top_level(inner, ',') {
                    let x = container_length(arg, units)?;
                    out = Some(out.map_or(x, |o| if min { o.min(x) } else { o.max(x) }));
                }
                return out;
            }
        }
    }
    if has_long_min_max(v) {
        // A nested min()/max() with three or more arguments: unknown.
        return None;
    }
    if let Some(n) = parse_number(v) {
        return (n == 0.0).then_some(0.0);
    }
    let per_unit = |unit: &str, math: bool| -> Option<f32> {
        Some(match unit {
            "px" => 1.0,
            "em" => units.em,
            "rem" => units.rem,
            "vw" => units.vw / 100.0,
            "vh" => units.vh / 100.0,
            "vmin" => units.vw.min(units.vh) / 100.0,
            "vmax" => units.vw.max(units.vh) / 100.0,
            _ if math => return None,
            "in" => 96.0,
            "cm" => 96.0 / 2.54,
            "mm" => 96.0 / 25.4,
            "pt" => 96.0 / 72.0,
            "pc" => 16.0,
            "q" => 96.0 / 101.6,
            _ => return None,
        })
    };
    let split = v
        .find(|c: char| c.is_ascii_alphabetic())
        .filter(|at| *at > 0)
        .map(|at| v.split_at(at));
    if let Some((n, unit)) = split {
        if let Some(n) = parse_number(n) {
            return Some(n * per_unit(unit, false)?);
        }
    }
    // A math function: every unit in it must be one this resolves.
    let bytes = v.as_bytes();
    let mut i = 0;
    let mut saw_unit = false;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() || bytes[i] == b'.' {
            let mut j = i;
            while j < bytes.len() && (bytes[j].is_ascii_digit() || bytes[j] == b'.') {
                j += 1;
            }
            let mut k = j;
            while k < bytes.len() && bytes[k].is_ascii_alphabetic() {
                k += 1;
            }
            if j < k {
                per_unit(&v[j..k], true)?;
                saw_unit = true;
            }
            i = k;
        } else {
            i += 1;
        }
    }
    if !saw_unit {
        return None;
    }
    match crate::parse_length(v)? {
        crate::Length::Auto | crate::Length::FitContent | crate::Length::Percent(_) => None,
        l => Some(l.to_px_with_viewport(units.em, units.rem, 0.0, units.vw, units.vh)),
    }
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
            let Some(wanted) = parse_value(base, v, width, height) else {
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

fn parse_value(base: &str, v: &str, width: f32, height: f32) -> Option<f32> {
    match base {
        "width" | "height" => parse_length(v, width, height),
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

/// A length in px: unitless 0, an absolute unit, em/rem (16px, the initial
/// font size, whatever the page sets), a viewport unit, or `calc()`,
/// `min()`, `max()`, `clamp()` over those. A percentage is not a length here.
fn parse_length(v: &str, width: f32, height: f32) -> Option<f32> {
    let v = v.trim();
    if v.contains('%') {
        return None;
    }
    if let Some(n) = parse_number(v) {
        return Some(n).filter(|n| *n == 0.0);
    }
    const ABSOLUTE: [(&str, f32); 6] = [
        ("in", 96.0),
        ("cm", 96.0 / 2.54),
        ("mm", 96.0 / 25.4),
        ("pt", 96.0 / 72.0),
        ("pc", 16.0),
        ("q", 96.0 / 101.6),
    ];
    for (unit, px) in ABSOLUTE {
        if let Some(n) = v.strip_suffix(unit).and_then(parse_number) {
            return Some(n * px);
        }
    }
    // A bare number inside a math function (`calc(10)`) is not a length.
    let has_unit = v
        .as_bytes()
        .windows(2)
        .any(|w| w[0].is_ascii_digit() && w[1].is_ascii_alphabetic());
    match crate::parse_length(v)? {
        crate::Length::Auto | crate::Length::FitContent => None,
        _ if !has_unit => None,
        length => Some(length.to_px_with_viewport(16.0, 16.0, 0.0, width, height)),
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
            parse_value(&name, p, width, height)
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

/// Whether the parentheses in `s` balance and never close past depth 0.
fn balanced(s: &str) -> bool {
    let mut depth = 0i32;
    for c in s.chars() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            _ => {}
        }
        if depth < 0 {
            return false;
        }
    }
    depth == 0
}

/// Whether `s` holds a `min(`/`max(` call with three or more arguments.
fn has_long_min_max(s: &str) -> bool {
    ["min(", "max("].iter().any(|name| {
        s.match_indices(name).any(|(at, _)| {
            let body = &s[at + name.len()..];
            let mut depth = 1i32;
            let mut commas = 0;
            for c in body.chars() {
                match c {
                    '(' => depth += 1,
                    ')' => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    ',' if depth == 1 => commas += 1,
                    _ => {}
                }
            }
            commas >= 2
        })
    })
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

    const U: super::ContainerUnits = super::ContainerUnits {
        em: 16.0,
        rem: 16.0,
        vw: 800.0,
        vh: 600.0,
    };

    fn c(cond: &str, width: f32, height: Option<f32>) -> bool {
        super::container_condition_matches(cond, width, height, U)
    }

    /// #782 R1 (review 5654936187): `not` takes one parenthesized query,
    /// each `and`/`or` leg is one parenthesized query, and `not(`/`and(`
    /// are function tokens, not keywords. Chromium 148 drops each of the
    /// six invalid rules below (they are absent from `cssRules`) except
    /// `not(...)`, which it keeps as an unknown general-enclosed part; none
    /// applies. On a 300px container each would be true if misparsed.
    /// Rows match docs/diagnostics/2026-10-26/css_container_query_grammar.html.
    #[test]
    fn invalid_not_and_or_grammar_never_applies() {
        assert!(!c("not (width > 400px) or (width > 500px)", 300.0, None));
        assert!(!c("(width > 100px) and not (width > 400px)", 300.0, None));
        assert!(!c("(width > 100px) or not (width > 400px)", 300.0, None));
        assert!(!c("not not (width > 100px)", 300.0, None));
        assert!(!c("not(width > 400px)", 300.0, None));
        assert!(!c("(width > 100px) and(width < 400px)", 300.0, None));
        assert!(!c("(width > 100px) or (width < 400px) and (width > 200px)", 300.0, None));
        // Valid controls.
        assert!(c("not (width > 400px)", 300.0, None));
        assert!(c("(width > 100px) and (width < 400px)", 300.0, None));
        assert!(c("(width > 400px) or (width > 100px)", 300.0, None));
        assert!(c("((width > 100px) and (width < 400px))", 300.0, None));
        assert!(c("(not (width > 400px))", 300.0, None));
        assert!(c("(width > 100px) and ((width < 400px) or (width > 900px))", 300.0, None));
        assert!(c("(width > 100px) and (not (width > 400px))", 300.0, None));
        assert!(c("(width > 100px) and (width < 400px) and (width > 200px)", 300.0, None));
        assert!(c("not ((width > 400px) or (width > 500px))", 300.0, None));
        assert!(c("not   (width > 400px)", 300.0, None));
        assert!(c("NOT (width > 400px)", 300.0, None));
        assert!(c("(width > 100px) AND (width < 400px)", 300.0, None));
        // `)and` is fine: the keyword only needs whitespace after it.
        assert!(c("(width > 100px)and (width < 400px)", 300.0, None));
    }

    /// The parser drops what Chromium 148 drops from `cssRules`: `None`
    /// for invalid grammar, a query for valid grammar (`not(...)` is a
    /// valid, unknown `<general-enclosed>`).
    #[test]
    fn invalid_container_grammar_parses_to_none() {
        use super::parse_container_query as q;
        for bad in [
            "not (a: 1px) or (b: 1px)",
            "(a: 1px) and not (b: 1px)",
            "(a: 1px) or not (b: 1px)",
            "not not (a: 1px)",
            "(a: 1px) and(b: 1px)",
            "(a: 1px) or(b: 1px)",
            "not",
            "(a: 1px) and",
            "(a: 1px) (b: 1px)",
            "(a: 1px) or (b: 1px) and (c: 1px)",
            "(a: 1px",
            "card",
            "",
        ] {
            assert!(q(bad).is_none(), "{bad:?} should be invalid");
        }
        for good in [
            "not(a: 1px)",
            "not (a: 1px)",
            "(a: 1px) and (b: 1px)",
            "(a: 1px) or (b: 1px)",
            "((a: 1px) and (b: 1px))",
            "(a: 1px)and (b: 1px)",
            "style(--x: 1)",
            "not style(--x: 1)",
            "(a: 1px) and style(--x: 1)",
            "((a: 1px) and not (b: 1px))",
        ] {
            assert!(q(good).is_some(), "{good:?} should be valid");
        }
    }

    /// min()/max() take any number of arguments; the third one counts.
    #[test]
    fn container_min_max_read_every_argument() {
        assert!(!c("(max-width: min(500px, 400px, 100px))", 300.0, None));
        assert!(!c("(min-width: max(100px, 200px, 400px))", 300.0, None));
        assert!(c("(max-width: min(500px, 400px))", 300.0, None));
        assert!(c("(min-width: max(100px, 200px))", 300.0, None));
        assert!(c("(min-width: clamp(100px, 200px, 400px))", 300.0, None));
        assert!(c("(max-width: min(500px, 40em, 20rem))", 300.0, None));
        assert!(!c("(max-width: min(500px, 40em, 10rem))", 300.0, None));
    }

    /// #782 follow-up (Prometheus on #782): a parenthesis inside a quoted
    /// string does not open or close a group, so `style(--x: ")")` is one
    /// general-enclosed part and the query stays valid (Chromium 148 keeps
    /// `(width > 1px) or style(--x: ")")` in `cssRules` and applies it, see
    /// docs/diagnostics/2026-10-27/css_container_query_nits.html k3). The
    /// style part is still unknown here, so the answer is still false.
    #[test]
    fn a_quoted_paren_does_not_end_a_query_group() {
        let q = |s: &str| super::parse_container_query(s).is_some();
        for good in [
            r#"style(--x: ")")"#,
            r#"style(--x: "(")"#,
            r#"style(--x: ')')"#,
            r#"style(--x: "\")")"#,
            r#"(width > 1px) or style(--x: ")")"#,
            r#"style(--x: "a)b") and (width > 1px)"#,
            r#"not style(--x: "((")"#,
        ] {
            assert!(q(good), "{good:?} should be valid");
        }
        for bad in [r#"style(--x: ")""#, r#"(width > 1px) or style(--x: "("#] {
            assert!(!q(bad), "{bad:?} should be invalid");
        }
        assert!(!c(r#"(width > 1px) or style(--x: ")")"#, 300.0, None));
        assert!(c(r#"(width > 1px) and ((width > 2px) or (width > 3px))"#, 300.0, None));
    }

    #[test]
    fn container_conditions_read_the_container_size() {
        assert!(c("(width >= 200px)", 300.0, None));
        assert!(c("(min-width: 250px)", 300.0, None));
        assert!(!c("(min-width: 400px)", 300.0, None));
        assert!(c("(inline-size > 299px) and (not (width > 300px))", 300.0, None));
        assert!(c("(200px < width <= 300px) or (width > 1000px)", 300.0, None));
        // An inline-size container has no queryable block size.
        assert!(!c("(height > 0px)", 300.0, None));
        assert!(!c("(orientation: portrait)", 300.0, None));
        assert!(c("(block-size >= 100px)", 300.0, Some(100.0)));
        assert!(c("(orientation: landscape)", 300.0, Some(100.0)));
        // Style queries and bare words are not size conditions.
        assert!(!c("style(--x: 1)", 300.0, None));
        assert!(!c("card", 300.0, None));
    }

    /// #782 R2 (fix 1): `not` over a part this evaluator does not know was
    /// true, and media-only features answered for a container. Each row is
    /// Chromium 148's answer on docs/diagnostics/2026-10-26/
    /// css_container_query_r2.html (a 300px inline-size container, 800x600
    /// viewport), except `not style(--x)`, which Chromium applies (it
    /// answers style queries) and which stays false here.
    #[test]
    fn an_unknown_part_makes_a_container_condition_false_even_under_not() {
        assert!(!c("not style(--x)", 300.0, None));
        assert!(!c("not (width >= 30ch)", 300.0, None));
        assert!(!c("(hover)", 300.0, None));
        assert!(!c("not (hover)", 300.0, None));
        assert!(!c("(color)", 300.0, None));
        assert!(!c("(prefers-color-scheme: light)", 300.0, None));
        assert!(!c("(min-device-width: 1px)", 300.0, None));
        assert!(!c("(width > 299px) or (hover)", 300.0, None));
        assert!(!c("(width > 400px) or not (hover)", 300.0, None));
        assert!(!c("(width > 299px) and (hover)", 300.0, None));
        assert!(!c("not (width > 299px) or (hover)", 300.0, None));
        assert!(c("not (not (width > 299px))", 300.0, None));
        assert!(c("(width > 299px)", 300.0, None));
        // A block-size feature of an inline-size container is unknown too.
        assert!(!c("not (height > 0px)", 300.0, None));
        assert!(!c("not (orientation: portrait)", 300.0, None));
        // Units: cq*, ch, % are unknown; vw is the viewport's, em the
        // container's font size.
        assert!(!c("not (width > 10cqw)", 300.0, None));
        assert!(!c("not (width > 50%)", 300.0, None));
        assert!(!c("(width > 50vw)", 300.0, None));
        assert!(c("(width < 50vw)", 300.0, None));
        assert!(c("(width > 10em)", 300.0, None));
        let big = super::ContainerUnits { em: 40.0, ..U };
        assert!(!super::container_condition_matches("(width > 10em)", 300.0, None, big));
        assert!(c("(min-width: 3in)", 300.0, None));
        assert!(!c("(min-width: 3.2in)", 300.0, None));
        assert!(c("(min-width: calc(100px + 10em))", 300.0, None));
        assert!(!c("(min-width: calc(100px + 1in))", 300.0, None));
        assert!(!c("(min-width: calc(100px + 1ch))", 300.0, None));
        // Mixed `and`/`or` without parentheses is not a condition.
        assert!(!c("(width > 1px) and (width > 2px) or (width > 3px)", 300.0, None));
    }

    /// #782 R2 nit: `(width)` is the boolean context, false at 0 (Chromium
    /// 148 applies none of it on a 0px-wide container).
    #[test]
    fn a_bare_size_feature_is_false_at_zero() {
        assert!(!c("(width)", 0.0, None));
        assert!(c("(width)", 300.0, None));
        assert!(!c("(height)", 300.0, None));
        assert!(!c("(height)", 300.0, Some(0.0)));
        assert!(c("(height)", 300.0, Some(10.0)));
    }

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

    /// A length in a media feature may be a math function or carry any
    /// length unit. Every row is what the oracle Chromium (143) answers from
    /// `matchMedia` at 1280x800; the first four are en.wikipedia.org's.
    #[test]
    fn math_functions_and_other_units_in_a_feature_length() {
        let (w, h) = (1280.0, 800.0);
        assert!(m("(min-width: calc(639px))", w, h));
        assert!(!m("(max-width: calc(1119px))", w, h));
        assert!(!m("(max-width: calc(639px))", w, h));
        assert!(m("screen and (min-width: calc(639px)) and (max-width: calc(1679px))", w, h));
        assert!(m("(min-width: calc(1280px))", w, h));
        assert!(!m("(min-width: calc(1280px + 1px))", w, h));
        assert!(m("(max-width: calc(1281px - 1px))", w, h));
        assert!(!m("(max-width: calc(640px - 1px))", w, h));
        assert!(m("(min-width: calc(40em + 1px))", w, h));
        assert!(m("(min-width: calc(80em))", w, h));
        assert!(!m("(min-width: calc(80em + 1px))", w, h));
        assert!(m("(min-width: calc(2 * 640px))", w, h));
        assert!(!m("(min-width: calc(2 * 640.5px))", w, h));
        assert!(m("(min-width: min(1000px, 2000px))", w, h));
        assert!(!m("(min-width: max(1000px, 2000px))", w, h));
        assert!(!m("(min-width: clamp(100px, 1300px, 1290px))", w, h));
        assert!(m("(min-width: 100vw)", w, h));
        assert!(!m("(min-width: 101vw)", w, h));
        assert!(m("(min-height: 100vh)", w, h));
        assert!(m("(min-width: calc(50vw + 640px))", w, h));
        assert!(!m("(min-width: calc(50vw + 641px))", w, h));
        assert!(m("(width >= calc(1000px + 280px))", w, h));
        assert!(!m("(width > calc(1000px + 280px))", w, h));
        assert!(m("(calc(600px) <= width < calc(1280px + 1px))", w, h));
        assert!(m("(min-height: calc(799px + 1px))", w, h));
        assert!(!m("(max-height: calc(100px * 7))", w, h));
        assert!(m("(min-width: CALC(639PX))", w, h));
        assert!(m("(min-width: calc( 639px ))", w, h));
        assert!(m("(min-width: 1in)", w, h));
        assert!(!m("(min-width: 14in)", w, h));
        assert!(m("(min-width: 960pt)", w, h));
        assert!(!m("(min-width: 961pt)", w, h));
        assert!(m("(min-width: 33.8cm)", w, h));
        assert!(!m("(min-width: 33.9cm)", w, h));
        // Not lengths: a percentage, a bare number other than 0.
        assert!(!m("(min-width: calc(50%))", w, h));
        assert!(!m("(min-width: calc(10))", w, h));
        assert!(!m("(min-width: 10)", w, h));
        // Narrower viewports, where the `max-` forms are the ones that hold.
        assert!(m("(max-width: calc(1119px))", 1000.0, h));
        assert!(m("(max-width: calc(639px))", 600.0, h));
        assert!(!m("(min-width: calc(639px))", 600.0, h));
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

    /// `MediaQueryList.media` as Chrome 148 serializes it: case and
    /// whitespace normalized, an invalid query `not all`, and evaluating the
    /// serialization answers what the original would.
    #[test]
    fn serialization_normalizes_and_marks_invalid_queries_not_all() {
        use super::media_query_list_serialize as ser;
        assert_eq!(ser("(min-width: 600px)"), "(min-width: 600px)");
        assert_eq!(ser("(MIN-WIDTH:600PX)"), "(min-width: 600px)");
        assert_eq!(ser("  ( min-width :  600px )  "), "(min-width: 600px)");
        assert_eq!(ser("screen and (min-width: 960px)"), "screen and (min-width: 960px)");
        assert_eq!(ser("only screen and (max-width: 640px)"), "only screen and (max-width: 640px)");
        assert_eq!(ser("not print"), "not print");
        assert_eq!(ser("print"), "print");
        assert_eq!(ser("all"), "all");
        assert_eq!(ser(""), "");
        assert_eq!(ser("(width>=600px)"), "(width >= 600px)");
        assert_eq!(ser("(400px <= width < 700px)"), "(400px <= width < 700px)");
        assert_eq!(ser("(color)"), "(color)");
        assert_eq!(ser("not (max-width: 500px)"), "not (max-width: 500px)");
        assert_eq!(ser("(min-width: 600px) and (max-width: 1300px)"), "(min-width: 600px) and (max-width: 1300px)");
        assert_eq!(ser("((min-width: 600px) or (hover)) and (color)"), "((min-width: 600px) or (hover)) and (color)");
        assert_eq!(ser("print, (min-width: 1000px)"), "print, (min-width: 1000px)");
        assert_eq!(ser("(prefers-color-scheme: dark)"), "(prefers-color-scheme: dark)");

        // Invalid queries, each as Chrome 148 answers them.
        assert_eq!(ser("foo bar"), "not all");
        assert_eq!(ser("screen and"), "not all");
        assert_eq!(ser("not(hover)"), "not all");
        assert_eq!(ser("(min-width: 600px"), "not all");
        assert_eq!(ser("min-width: 600px)"), "not all");
        assert_eq!(ser("()"), "not all");
        assert_eq!(ser("(a) and (b) or (c)"), "not all");
        assert_eq!(ser("not (a) and (b)"), "not all");
        assert_eq!(ser("only (hover)"), "not all");
        assert_eq!(ser("screen and (hover) or (color)"), "not all");
        assert_eq!(ser("screen, foo bar"), "screen, not all");
        assert_eq!(ser("not foo bar"), "not all");

        // The serialization evaluates as the original would in Chrome: an
        // invalid query is false, even under `not`.
        let (w, h) = (1280.0, 800.0);
        assert!(!m(&ser("not foo bar"), w, h));
        assert!(!m(&ser("not(hover)"), w, h));
        assert!(m(&ser("screen, foo bar"), w, h));
        assert!(m(&ser("(MIN-WIDTH:600PX)"), w, h));
        assert!(!m(&ser("(min-width: 1281px)"), w, h));
        assert!(m(&ser(""), w, h));
    }

    /// A non-ASCII char in a range feature is page text the serializer must
    /// walk by char: #818 R1 H1 stepped it by byte and panicked on `(ü > 1)`
    /// ("byte index 1 is not a char boundary"), a page-controlled panic
    /// through `window.matchMedia`. Such a feature is invalid (`not all`, as
    /// Chrome 148 answers), and a valid range still serializes as before.
    #[test]
    fn range_serialization_is_utf8_safe() {
        use super::media_query_list_serialize as ser;
        assert_eq!(ser("(ü > 1)"), "not all");
        assert_eq!(ser("(ü>1)"), "not all");
        assert_eq!(ser("(width > 600pxé)"), "not all");
        assert_eq!(ser("(é <= width <= ü)"), "not all");
        assert_eq!(ser("(width = ü)"), "not all");
        assert_eq!(ser("screen, (ü > 1)"), "screen, not all");
        assert_eq!(ser("(width > 600px)"), "(width > 600px)");
        assert_eq!(ser("(width>600px)"), "(width > 600px)");
        assert_eq!(ser("(400px <= width < 700px)"), "(400px <= width < 700px)");
        assert_eq!(ser("(aspect-ratio >= 16/9)"), "(aspect-ratio >= 16/9)");
        assert!(!m(&ser("(ü > 1)"), 1280.0, 800.0));
        assert!(m(&ser("(width > 600px)"), 1280.0, 800.0));
    }
}
