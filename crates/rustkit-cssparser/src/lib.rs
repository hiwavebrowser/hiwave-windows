//! # RustKit CSS Parser
//!
//! This crate provides a RustKit-owned CSS parsing layer, intended to replace the external
//! `cssparser` dependency over time.
//!
//! Current implementation is a **minimal** stylesheet parser suitable for RustKit's current
//! needs: parse basic rules `selector { prop: value; }` into an AST.

use thiserror::Error;

/// Errors that can occur while parsing CSS.
#[derive(Error, Debug, Clone)]
pub enum ParseError {
    #[error("Unexpected end of input")]
    UnexpectedEof,

    #[error("Parse error: {0}")]
    ParseError(String),
}

/// A parsed stylesheet AST.
#[derive(Debug, Default, Clone)]
pub struct StylesheetAst {
    pub rules: Vec<RuleAst>,
    /// Every cascade layer name the sheet declares, in the order it declares
    /// them: `@layer a, b;` statements and the name of each `@layer` block.
    /// Layer order is the order names are FIRST declared (CSS Cascade 5
    /// §6.4.3), and a statement can declare a layer before any rule is in it.
    pub layer_statements: Vec<LayerStatementAst>,
    /// What each `@keyframes` says about `opacity`, in source order.
    pub keyframes: Vec<KeyframesAst>,
}

/// An `@keyframes` rule: its keyframe blocks, in source order, for the
/// engine to run, and the `opacity` it sets at its two ends (`None` where
/// that keyframe does not set it) for the frame a parity capture, which
/// runs no animations, settles on (`ComputedStyle::settle_animation_opacity`).
#[derive(Debug, Clone, PartialEq)]
pub struct KeyframesAst {
    pub name: String,
    pub first: Option<f32>,
    pub last: Option<f32>,
    pub frames: Vec<KeyframeAst>,
}

/// One keyframe block: the offsets its selector names (fractions, `from`
/// is 0 and `to` is 1) and its declarations (`!important` ones dropped,
/// as they are invalid in a keyframe).
#[derive(Debug, Clone, PartialEq)]
pub struct KeyframeAst {
    pub offsets: Vec<f32>,
    pub declarations: Vec<DeclarationAst>,
}

/// A layer name declared at a point in the sheet.
#[derive(Debug, Clone, PartialEq)]
pub struct LayerStatementAst {
    /// How many rules of the sheet precede the declaration.
    pub position: usize,
    /// Full dotted layer name (`outer.inner` for a nested layer).
    pub name: String,
    /// Media query lists of the enclosing `@media` blocks, as on a rule.
    pub media: Vec<String>,
}

/// A parsed rule AST.
#[derive(Debug, Clone)]
pub struct RuleAst {
    pub selector: String,
    pub declarations: Vec<DeclarationAst>,
    /// Media query lists of the `@media` blocks enclosing this rule,
    /// outermost first. The rule applies only where every one matches.
    pub media: Vec<String>,
    /// Preludes of the `@container` blocks enclosing this rule, outermost
    /// first (`card (width >= 200px)`): an optional container name, then
    /// the size condition. The rule applies to an element only where each
    /// one holds for its query container.
    pub container: Vec<String>,
    /// Full dotted name of the cascade layer the rule is in, or `None` for
    /// an unlayered rule. An anonymous `@layer { }` block gets a name no
    /// author can write (see `anonymous_layer_name`).
    pub layer: Option<String>,
}

/// What an at-rule with a `{ ... }` block contributes to the stylesheet.
enum AtBlock {
    /// Its body is a list of rules (`@media`, `@supports`); an `@media`
    /// query list is recorded on each of them.
    Rules(Option<String>),
    /// An `@layer` block: its rules are in the named layer (anonymous when
    /// the prelude names none).
    Layer(String),
    /// An `@container` block: its prelude is recorded on each of its rules.
    Container(String),
    /// Its body is declarations (`@font-face`, `@page`): kept as one rule
    /// whose selector is the at-rule prelude, as before.
    Declarations,
    /// `@keyframes`, by name: only its `opacity` at each end is kept.
    Keyframes(String),
    /// Nothing that styles an element in the static frame (an `@container`
    /// with no condition, an `@supports` that is false, unknown at-rules):
    /// skipped whole.
    Skip,
}

fn at_block_kind(prelude: &str) -> AtBlock {
    let at = prelude.trim_start_matches('@');
    let name_end = at
        .find(|c: char| !(c.is_alphanumeric() || c == '-'))
        .unwrap_or(at.len());
    let name = at[..name_end].to_ascii_lowercase();
    let condition = at[name_end..].trim();
    match name.as_str() {
        "media" => AtBlock::Rules(Some(condition.to_string())),
        "supports" if supports_condition(condition) == Some(true) => AtBlock::Rules(None),
        "container" if !condition.is_empty() => AtBlock::Container(condition.to_string()),
        "layer" if condition.is_empty() => AtBlock::Layer(anonymous_layer_name()),
        "layer" => AtBlock::Layer(condition.to_string()),
        "font-face" | "page" | "property" | "counter-style" | "font-palette-values" => {
            AtBlock::Declarations
        }
        "keyframes" | "-webkit-keyframes" => {
            AtBlock::Keyframes(condition.trim_matches(|c| c == '"' || c == '\'').to_string())
        }
        _ => AtBlock::Skip,
    }
}

/// The `opacity` at each end of an `@keyframes` body. `None` when an end sets
/// a value this cannot read (`var()`): the rule is then as good as absent, and
/// the box keeps painting at full opacity rather than be hidden on a guess.
fn keyframes_opacity(name: String, body: &str) -> Option<KeyframesAst> {
    let mut frames = KeyframesAst {
        name,
        first: None,
        last: None,
        frames: Vec::new(),
    };
    if frames.name.is_empty() {
        return None;
    }
    for frame in parse_stylesheet(body).ok()?.rules {
        let offsets: Vec<f32> = frame
            .selector
            .split(',')
            .filter_map(|at| match at.trim().to_ascii_lowercase().as_str() {
                "from" => Some(0.0),
                "to" => Some(1.0),
                other => other
                    .strip_suffix('%')
                    .and_then(|p| p.trim().parse::<f32>().ok())
                    .map(|p| p / 100.0),
            })
            .filter(|o| (0.0..=1.0).contains(o))
            .collect();
        if !offsets.is_empty() {
            frames.frames.push(KeyframeAst {
                offsets,
                declarations: frame
                    .declarations
                    .iter()
                    .filter(|d| !d.important)
                    .cloned()
                    .collect(),
            });
        }
        // Keyframe declarations with `!important` are invalid and ignored.
        let Some(value) = frame
            .declarations
            .iter()
            .rev()
            .find(|d| d.property.eq_ignore_ascii_case("opacity") && !d.important)
        else {
            continue;
        };
        let value = value.value.trim();
        let opacity = match value.strip_suffix('%') {
            Some(percent) => percent.parse::<f32>().map(|p| p / 100.0),
            None => value.parse::<f32>(),
        };
        for at in frame.selector.split(',').map(|s| s.trim().to_ascii_lowercase()) {
            let offset = match at.as_str() {
                "from" => Some(0.0),
                "to" => Some(100.0),
                other => other.strip_suffix('%').and_then(|p| p.parse::<f32>().ok()),
            };
            let end = match offset {
                Some(o) if o == 0.0 => &mut frames.first,
                Some(o) if o == 100.0 => &mut frames.last,
                _ => continue,
            };
            *end = Some(opacity.as_ref().ok()?.clamp(0.0, 1.0));
        }
    }
    Some(frames)
}

/// Decide an `@supports` condition (CSS Conditional 3 §6.1, with
/// `selector()` and `font-tech()`/`font-format()` from level 4) as Chrome
/// does. `None` means the prelude does not parse, and the rule is dropped.
///
/// A declaration counts as supported unless it is plainly not: a property
/// no Chromium build has, an empty value, or a `display`/`position` value
/// outside the keywords. RustKit drops a property it lacks at apply time
/// anyway, so a page's Chrome branch stays the one RustKit gets.
fn supports_condition(s: &str) -> Option<bool> {
    let s = s.trim();
    if let Some(rest) = strip_keyword(s, "not") {
        let (term, tail) = split_term(rest.trim_start())?;
        if !tail.trim().is_empty() {
            return None;
        }
        return supports_in_parens(term).map(|v| !v);
    }
    let (mut all, mut any, mut op, mut rest) = (true, false, None, s);
    loop {
        let (term, tail) = split_term(rest)?;
        let v = supports_in_parens(term)?;
        (all, any) = (all && v, any || v);
        let tail = tail.trim_start();
        if tail.is_empty() {
            return Some(if op == Some("or") { any } else { all });
        }
        let (kw, after) = match (strip_keyword(tail, "and"), strip_keyword(tail, "or")) {
            (Some(a), _) => ("and", a),
            (_, Some(a)) => ("or", a),
            _ => return None,
        };
        // `and` and `or` never mix without parentheses.
        if op.is_some_and(|o| o != kw) {
            return None;
        }
        (op, rest) = (Some(kw), after.trim_start());
    }
}

/// `s` without a leading case-insensitive keyword that whitespace follows.
fn strip_keyword<'a>(s: &'a str, kw: &str) -> Option<&'a str> {
    let head = s.get(..kw.len())?;
    let rest = &s[kw.len()..];
    (head.eq_ignore_ascii_case(kw) && rest.starts_with(char::is_whitespace)).then_some(rest)
}

/// Split one `( ... )` or `name( ... )` off the front of `s`.
fn split_term(s: &str) -> Option<(&str, &str)> {
    let open = s.find('(')?;
    if !s[..open].chars().all(|c| c.is_alphanumeric() || c == '-') {
        return None;
    }
    let (mut depth, mut quote, mut escaped) = (0usize, None, false);
    for (i, c) in s.char_indices().skip_while(|&(i, _)| i < open) {
        if escaped {
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else if let Some(q) = quote {
            if c == q {
                quote = None;
            }
        } else {
            match c {
                '"' | '\'' => quote = Some(c),
                '(' => depth += 1,
                ')' if depth == 1 => return Some((&s[..=i], &s[i + 1..])),
                ')' => depth -= 1,
                _ => {}
            }
        }
    }
    None
}

/// One `<supports-in-parens>`, or a `selector()`/`font-*()` function.
fn supports_in_parens(term: &str) -> Option<bool> {
    let open = term.find('(')?;
    let name = term[..open].to_ascii_lowercase();
    let arg = term[open + 1..term.len() - 1].trim();
    Some(match name.as_str() {
        "" => {
            if let Some(v) = supports_condition(arg) {
                v
            } else if let Some((prop, value)) = arg.split_once(':') {
                supports_declaration(prop.trim(), value.trim())
            } else {
                false // <general-enclosed>: valid, but false
            }
        }
        // Chrome knows every selector a page tests for except other
        // engines' prefixed pseudo-classes (`:-moz-focusring`).
        "selector" => {
            let a = arg.to_ascii_lowercase();
            !a.is_empty() && !a.contains(":-moz-") && !a.contains(":-ms-")
        }
        "font-format" => matches!(
            arg.to_ascii_lowercase().as_str(),
            "collection" | "opentype" | "truetype" | "woff" | "woff2"
        ),
        "font-tech" => matches!(
            arg.to_ascii_lowercase().as_str(),
            "features-opentype" | "variations" | "palettes" | "color-colrv0" | "color-colrv1"
        ),
        _ => false,
    })
}

fn supports_declaration(prop: &str, value: &str) -> bool {
    let value = value
        .trim_end_matches("!important")
        .trim()
        .to_ascii_lowercase();
    if prop.starts_with("--") {
        return prop.len() > 2;
    }
    let prop = prop.to_ascii_lowercase();
    let ident = prop
        .trim_start_matches('-')
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-');
    if value.is_empty() || prop.is_empty() || !ident {
        return false;
    }
    // Other engines' prefixes, and properties Chromium never shipped.
    if ["-moz-", "-ms-", "-o-", "-khtml-"]
        .iter()
        .any(|p| prop.starts_with(p))
        || matches!(
            prop.as_str(),
            "margin-trim"
                | "hanging-punctuation"
                | "-webkit-touch-callout"
                | "-webkit-overflow-scrolling"
        )
    {
        return false;
    }
    if matches!(
        value.as_str(),
        "inherit" | "initial" | "unset" | "revert" | "revert-layer"
    ) {
        return true;
    }
    match prop.as_str() {
        "display" => supports_display(&value),
        "position" => matches!(
            value.as_str(),
            "static" | "relative" | "absolute" | "fixed" | "sticky"
        ),
        _ => true,
    }
}

/// Chrome's `display` grammar: one legacy keyword, or up to one each of an
/// outer type, an inner type and `list-item` (CSS Display 3 §2).
fn supports_display(value: &str) -> bool {
    const ONE: &[&str] = &[
        "none",
        "contents",
        "inline-block",
        "inline-flex",
        "inline-grid",
        "inline-table",
        "table-row-group",
        "table-header-group",
        "table-footer-group",
        "table-row",
        "table-cell",
        "table-column-group",
        "table-column",
        "table-caption",
        "ruby-text",
        "-webkit-box",
        "-webkit-inline-box",
        "-webkit-flex",
        "-webkit-inline-flex",
    ];
    if ONE.contains(&value) {
        return true;
    }
    let (mut outer, mut inner, mut item) = (false, None, false);
    for t in value.split_whitespace() {
        match t {
            "block" | "inline" if !outer => outer = true,
            "flow" | "flow-root" | "flex" | "grid" | "table" | "ruby" | "math"
                if inner.is_none() =>
            {
                inner = Some(t)
            }
            "list-item" if !item => item = true,
            _ => return false,
        }
    }
    !item || matches!(inner, None | Some("flow" | "flow-root"))
}

/// A name for an anonymous `@layer { }` block. Each anonymous block is a
/// layer of its own (CSS Cascade 5 §6.4.2), unique across every sheet the
/// process parses; the control character keeps it from colliding with an
/// author's identifier, and it has no `.` so it is one path segment.
fn anonymous_layer_name() -> String {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("\u{1}anonymous{n}")
}

/// `inner` as a sublayer of `outer`.
fn sublayer(outer: &str, inner: Option<&str>) -> String {
    match inner {
        Some(inner) => format!("{outer}.{inner}"),
        None => outer.to_string(),
    }
}

/// Consume a block body up to its matching `}` (which is consumed too),
/// honouring nested blocks, strings, comments and escapes. If the input
/// ends first, the block ends there, as CSS Syntax §5.4 closes every open
/// block at EOF.
fn take_block(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> String {
    let mut body = String::new();
    let mut depth = 0usize;
    let mut quote: Option<char> = None;
    while let Some(c) = chars.next() {
        // An escape is one code point in or out of a string. Outside one,
        // Tailwind's arbitrary-value classes are full of them
        // (`.bg-\[url\(\'https\:...\'\)\]`): read as a quote, `\'` opened a
        // string that swallowed the block's closing `}`, and the whole sheet
        // (linkedin's only one, 341 KB) was dropped at EOF.
        if c == '\\' {
            body.push(c);
            if let Some(n) = chars.next() {
                body.push(n);
            }
            continue;
        }
        if let Some(q) = quote {
            if c == q {
                quote = None;
            }
            body.push(c);
            continue;
        }
        match c {
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                while let Some(cc) = chars.next() {
                    if cc == '*' && chars.peek() == Some(&'/') {
                        chars.next();
                        break;
                    }
                }
                continue;
            }
            '"' | '\'' => quote = Some(c),
            '{' => depth += 1,
            '}' if depth == 0 => return body,
            '}' => depth -= 1,
            _ => {}
        }
        body.push(c);
    }
    body
}

/// A parsed declaration AST.
#[derive(Debug, Clone, PartialEq)]
pub struct DeclarationAst {
    pub property: String,
    pub value: String,
    pub important: bool,
}

/// Parse a stylesheet into an AST.
///
/// Notes:
/// - This is not a full CSS parser.
/// - At-rule blocks are handled by `at_block_kind`: the rules inside
///   `@media`/`@supports`/`@layer` are parsed (each carrying its media
///   query lists), declaration blocks like `@font-face` stay one rule, and
///   the rest are skipped. Statement at-rules (`@charset`, `@import`) end
///   at their `;`. Until this, `@media` had no block structure: every rule
///   in the block but the first leaked out and applied at every width, and
///   the `}` closing the block was glued onto the next selector, so the
///   first rule after every `@media` block (and after `@charset`) was lost.
/// - Nested style rules and nested `@media`/`@supports`/`@layer` (CSS
///   Nesting 1) are flattened into ordinary rules with `&` resolved.
/// - It does not do complex tokenization.
/// - It attempts to be robust for common author CSS and RustKit test inputs.
pub fn parse_stylesheet(css: &str) -> Result<StylesheetAst, ParseError> {
    let mut out = StylesheetAst::default();

    let mut current_selector = String::new();
    let mut current_property = String::new();
    let mut current_value = String::new();
    let mut current_decls: Vec<DeclarationAst> = Vec::new();

    let mut in_block = false;
    let mut in_value = false;

    // Paren depth and quote state. WITHOUT THESE, a data URI destroys TWO
    // declarations: `background-image: url(data:image/png;base64,AAAA); color: red`
    // truncates to `url(data:image/png` AND swallows `color: red`, because the
    // `;` inside url() ends the declaration and the remainder is re-read as a
    // new property. Data URIs are ordinary on real pages (inline icons, inline
    // fonts), and nothing anywhere reports an error. Found by an @font-face
    // test whose base64 payload contained a comma; the semicolon was the
    // deeper defect underneath it.
    let mut depth = 0usize;
    let mut quote: Option<char> = None;

    let mut chars = css.chars().peekable();
    while let Some(c) = chars.next() {
        // Very small comment skipper: /* ... */
        if c == '/' && chars.peek() == Some(&'*') {
            // consume '*'
            chars.next();
            // consume until */
            while let Some(cc) = chars.next() {
                if cc == '*' && chars.peek() == Some(&'/') {
                    chars.next();
                    break;
                }
            }
            continue;
        }

        // An escaped code point is text, never structure: `\'` must not open
        // a string, `\;` must not end a declaration (see `take_block`).
        if c == '\\' {
            let text = if !in_block {
                &mut current_selector
            } else if in_value {
                &mut current_value
            } else {
                &mut current_property
            };
            text.push(c);
            if let Some(n) = chars.next() {
                text.push(n);
            }
            continue;
        }

        if !in_block {
            let at_rule = current_selector.trim_start().starts_with('@');
            if c == ';' && at_rule {
                // A statement at-rule (`@charset "UTF-8";`, `@import ...;`)
                // ends here and styles nothing. `@layer a, b;` styles nothing
                // either, but it fixes the order of the layers it names.
                let statement = current_selector.trim();
                if let Some(names) = statement
                    .strip_prefix('@')
                    .filter(|s| {
                        s.get(..5).is_some_and(|k| k.eq_ignore_ascii_case("layer"))
                            && s[5..].starts_with(char::is_whitespace)
                    })
                    .map(|s| &s[5..])
                {
                    for name in names.split(',').map(str::trim).filter(|n| !n.is_empty()) {
                        out.layer_statements.push(LayerStatementAst {
                            position: out.rules.len(),
                            name: name.to_string(),
                            media: Vec::new(),
                        });
                    }
                }
                current_selector.clear();
                continue;
            }
            if c == '{' && at_rule {
                let prelude = current_selector.trim().to_string();
                match at_block_kind(&prelude) {
                    AtBlock::Declarations => {}
                    kind => {
                        let body = take_block(&mut chars);
                        let (media, container, layer) = match kind {
                            AtBlock::Rules(media) => (media, None, None),
                            AtBlock::Container(c) => (None, Some(c), None),
                            AtBlock::Layer(name) => (None, None, Some(name)),
                            AtBlock::Keyframes(name) => {
                                out.keyframes.extend(keyframes_opacity(name, &body));
                                current_selector.clear();
                                continue;
                            }
                            _ => {
                                current_selector.clear();
                                continue;
                            }
                        };
                        let inner = parse_stylesheet(&body)?;
                        merge_block(&mut out, inner, media, container, layer);
                        current_selector.clear();
                        continue;
                    }
                }
            }
            if c == '{' {
                in_block = true;
                current_selector = current_selector.trim().to_string();
                current_property.clear();
                current_value.clear();
                current_decls.clear();
                in_value = false;
            } else {
                current_selector.push(c);
            }
            continue;
        }

        // In block
        if c == '}' {
            flush_decl(
                &mut current_property,
                &mut current_value,
                &mut current_decls,
                in_value,
            );
            let selector = current_selector.trim().to_string();
            if !selector.is_empty() && !current_decls.is_empty() {
                out.rules.push(RuleAst {
                    selector,
                    declarations: current_decls.clone(),
                    media: Vec::new(),
                    container: Vec::new(),
                    layer: None,
                });
            }

            // reset for next rule
            in_block = false;
            current_selector.clear();
            current_property.clear();
            current_value.clear();
            current_decls.clear();
            in_value = false;
            depth = 0;
            quote = None;
            continue;
        }

        // Quote and paren tracking runs for BOTH halves of a declaration: a
        // property name never contains them, but starting the accounting only
        // once a value begins would miss `url(` opened on the property side by
        // malformed input and leave depth wrong for the rest of the block.
        match c {
            '"' | '\'' if quote == Some(c) => quote = None,
            '"' | '\'' if quote.is_none() => quote = Some(c),
            '(' if quote.is_none() => depth += 1,
            ')' if quote.is_none() => depth = depth.saturating_sub(1),
            _ => {}
        }

        let structural = quote.is_none() && depth == 0;

        // A `{` at the top level of a declaration block opens a nested rule
        // (CSS Nesting 1): `& > * { ... }`, `&:hover { ... }` (whose `:` put
        // the reader in value position), `.child { ... }`, `@media (...) { ... }`.
        // Before, its `}` closed the PARENT rule, and the parent's own `}`
        // then glued onto the next selector, losing that rule as well.
        // A custom property's value may hold a `{}` block; that stays a value.
        if c == '{' && structural && in_value && current_property.trim().starts_with("--") {
            current_value.push('{');
            current_value.push_str(&take_block(&mut chars));
            current_value.push('}');
            continue;
        }
        if c == '{' && structural {
            let prelude = if in_value {
                format!("{}:{}", current_property, current_value)
            } else {
                current_property.clone()
            };
            current_property.clear();
            current_value.clear();
            in_value = false;
            // Declarations before a nested rule stay with the parent, and
            // come before the nested rule in source order.
            let parent = current_selector.trim().to_string();
            if !parent.is_empty() && !current_decls.is_empty() {
                out.rules.push(RuleAst {
                    selector: parent.clone(),
                    declarations: std::mem::take(&mut current_decls),
                    media: Vec::new(),
                    container: Vec::new(),
                    layer: None,
                });
            }
            let body = take_block(&mut chars);
            let prelude = prelude.trim();
            if parent.is_empty() || prelude.is_empty() {
                continue;
            }
            let (css, media, container, layer) = if prelude.starts_with('@') {
                // A conditional group rule inside a style rule: its bare
                // declarations apply to the parent selector.
                match at_block_kind(prelude) {
                    AtBlock::Rules(media) => (format!("{parent}{{{body}}}"), media, None, None),
                    AtBlock::Container(c) => (format!("{parent}{{{body}}}"), None, Some(c), None),
                    AtBlock::Layer(name) => (format!("{parent}{{{body}}}"), None, None, Some(name)),
                    _ => continue,
                }
            } else {
                let selector = nest_selector(&parent, prelude);
                (format!("{selector}{{{body}}}"), None, None, None)
            };
            let inner = parse_stylesheet(&css)?;
            merge_block(&mut out, inner, media, container, layer);
            continue;
        }

        if !in_value {
            // NOTE: no `structural` check here, deliberately. It looks like it
            // belongs -- a colon inside url(data:...) is part of the scheme --
            // but falsification proved it dead: with the `;` fix below, a value
            // is never re-entered as a property, so a URL's colon is always
            // read in value position. A guard whose removal turns nothing red
            // is decoration, and decoration in a parser reads as intent.
            if c == ':' {
                in_value = true;
            } else if c == ';' && structural {
                // A `;` before any `:` ends an empty declaration (`;;`) or
                // one with no colon (`foo;`): both are skipped and the next
                // declaration starts clean (CSS Syntax 3 §5.4.5). Kept as
                // text, the `;` became part of the next property name, so
                // `color:red;;background:blue` lost the background (CH2.E28).
                current_property.clear();
            } else {
                current_property.push(c);
            }
            continue;
        }

        // In value
        if c == ';' && structural {
            flush_decl(
                &mut current_property,
                &mut current_value,
                &mut current_decls,
                true,
            );
            in_value = false;
            continue;
        }

        current_value.push(c);
    }

    if in_block {
        // EOF closes an unclosed block (CSS Syntax §5.4). Failing here threw
        // away every rule already parsed; one defect cost the whole sheet.
        flush_decl(
            &mut current_property,
            &mut current_value,
            &mut current_decls,
            in_value,
        );
        let selector = current_selector.trim().to_string();
        if !selector.is_empty() && !current_decls.is_empty() {
            out.rules.push(RuleAst {
                selector,
                declarations: current_decls,
                media: Vec::new(),
                container: Vec::new(),
                layer: None,
            });
        }
    }

    Ok(out)
}

/// Append the rules and layer statements of a block's body to `out`, each
/// inside the block's `@media` query list, `@container` condition and
/// cascade layer.
fn merge_block(
    out: &mut StylesheetAst,
    inner: StylesheetAst,
    media: Option<String>,
    container: Option<String>,
    layer: Option<String>,
) {
    let base = out.rules.len();
    // An `@keyframes` is carried whatever `@media` it sits in: only a box that
    // names it reads it, and a wrong width costs less than a lost rule.
    out.keyframes.extend(inner.keyframes);
    if let Some(name) = &layer {
        // The block declares its layer where it opens, even when it is empty.
        out.layer_statements.push(LayerStatementAst {
            position: base,
            name: name.clone(),
            media: media.iter().cloned().collect(),
        });
    }
    for mut statement in inner.layer_statements {
        statement.position += base;
        if let Some(m) = &media {
            statement.media.insert(0, m.clone());
        }
        if let Some(outer) = &layer {
            statement.name = sublayer(outer, Some(&statement.name));
        }
        out.layer_statements.push(statement);
    }
    for mut rule in inner.rules {
        if let Some(m) = &media {
            rule.media.insert(0, m.clone());
        }
        if let Some(c) = &container {
            rule.container.insert(0, c.clone());
        }
        if let Some(outer) = &layer {
            rule.layer = Some(sublayer(outer, rule.layer.as_deref()));
        }
        out.rules.push(rule);
    }
}

/// Split a selector list at its top-level commas (not inside `()`, `[]`,
/// strings or escapes).
fn split_selector_list(list: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let (mut depth, mut quote, mut start) = (0usize, None::<char>, 0usize);
    let mut chars = list.char_indices();
    while let Some((i, c)) = chars.next() {
        match c {
            '\\' => {
                chars.next();
            }
            _ if quote == Some(c) => quote = None,
            _ if quote.is_some() => {}
            '"' | '\'' => quote = Some(c),
            '(' | '[' => depth += 1,
            ')' | ']' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                parts.push(list[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(list[start..].trim());
    parts.retain(|p| !p.is_empty());
    parts
}

/// Past this many selectors, a nested rule's parent list is kept as one
/// `:is(...)` instead of being expanded (see `nest_selector`).
const MAX_NESTED_EXPANSION: usize = 64;

/// The selector a nested style rule matches (CSS Nesting 1 §3): `&` stands
/// for the parent's selector list, and a nested selector without `&` is
/// relative to it (`.c` is `& .c`, `> .c` is `& > .c`).
///
/// The spec reads `&` as `:is(<parent>)`, but the matcher's `:is()` takes
/// compound arguments only, so a parent list is expanded instead, one
/// selector per (parent, nested) pair, as preprocessors do: `.a, .b > p
/// { & span {} }` is `.a span, .b > p span`. That matches the same elements
/// wherever `&` leads its selector (every nested rule on the board's sites).
/// Two known differences: each expanded selector keeps its own specificity
/// rather than the list's maximum, and a `&` after a combinator
/// (`.c &` under `.a .b`) requires `.c` above `.a` rather than anywhere
/// above `.b`.
fn nest_selector(parent: &str, nested: &str) -> String {
    let parents = split_selector_list(parent);
    let nested = split_selector_list(nested);
    let parents: Vec<String> = if parents.len() * nested.len() <= MAX_NESTED_EXPANSION {
        parents.into_iter().map(str::to_string).collect()
    } else {
        vec![format!(":is({})", parents.join(", "))]
    };
    nested
        .iter()
        .flat_map(|sel| parents.iter().map(move |amp| substitute_nesting(sel, amp)))
        .collect::<Vec<_>>()
        .join(", ")
}

/// `sel` with `amp` in place of each `&`, or `amp sel` when it has none.
fn substitute_nesting(sel: &str, amp: &str) -> String {
    if !has_nesting_selector(sel) {
        return format!("{amp} {sel}");
    }
    let mut out = String::with_capacity(sel.len() + amp.len());
    let mut quote = None::<char>;
    let mut chars = sel.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                out.push(c);
                if let Some(n) = chars.next() {
                    out.push(n);
                }
                continue;
            }
            _ if quote == Some(c) => quote = None,
            _ if quote.is_some() => {}
            '"' | '\'' => quote = Some(c),
            '&' => {
                out.push_str(amp);
                continue;
            }
            _ => {}
        }
        out.push(c);
    }
    out
}

/// Whether a selector contains `&` outside strings and escapes.
fn has_nesting_selector(sel: &str) -> bool {
    let mut quote = None::<char>;
    let mut chars = sel.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                chars.next();
            }
            _ if quote == Some(c) => quote = None,
            _ if quote.is_some() => {}
            '"' | '\'' => quote = Some(c),
            '&' => return true,
            _ => {}
        }
    }
    false
}

fn flush_decl(
    current_property: &mut String,
    current_value: &mut String,
    decls: &mut Vec<DeclarationAst>,
    saw_colon: bool,
) {
    let property = current_property.trim();
    let value_raw = current_value.trim();
    // An empty value is valid for a custom property (CSS Variables 1 §2):
    // `--toggle: ;` is how "space toggles" switch on, and dropping it left
    // the toggle unset, which is the OFF state.
    let empty_custom = saw_colon && property.starts_with("--");
    if property.is_empty() || (value_raw.is_empty() && !empty_custom) {
        current_property.clear();
        current_value.clear();
        return;
    }

    let (value, important) = strip_important(value_raw);
    decls.push(DeclarationAst {
        property: property.to_string(),
        value: value.to_string(),
        important,
    });

    current_property.clear();
    current_value.clear();
}

/// Split a declaration list -- the text of a `style` attribute -- into its
/// declarations, the way CSS Syntax 3 §5.4.5 ("consume a list of
/// declarations") does: only a `;` at the top level ends one. A `;` inside a
/// quoted string, inside `()`/`[]`/`{}` (`calc()`, `var()` fallbacks,
/// custom-property blocks), inside an unquoted `url(...)` or inside a
/// comment does not, and an escaped `\;` is text. Comments are dropped (a
/// tokenizer never yields them); one becomes a space so `1px/**/2px` stays
/// two tokens. A closer ends only the block it matches (`(]` leaves the `(`
/// open). Each declaration comes back trimmed; empty ones (`;;`, a trailing
/// `;`) are skipped, as Chromium skips them.
///
/// An unquoted `url(` is ONE url token (§4.3.6): it runs to the first
/// unescaped `)` with no string, comment or block inside it, so
/// `url(a/*b)`, `url(it's)` and `url(a(b.png)` end at their `)` (the last
/// two are bad urls, which the value parser rejects) and the next
/// declaration survives.
///
/// End of input: a custom property keeps its text as written (Chromium reads
/// `--s:'abc` back as `'abc`). Any other declaration gets its open url,
/// string and blocks closed, which is what its value parser needs and what
/// Chromium's tokenizer does: `url(data:...` at the end of the attribute
/// still loads.
///
/// `rustkit-bindings`' `splitDecls` (dom.rs) is the JS copy of this rule;
/// both run `declaration_list_cases.json`.
///
/// Splitting at every `;` cut `url(data:image/svg+xml;base64,...)` at its
/// `;base64,`, so no inline data-URL background or mask ever applied (Z plan
/// CH2.E27).
pub fn split_declaration_list(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut closers: Vec<char> = Vec::new();
    let mut quote: Option<char> = None;
    let mut open_url = false;
    let mut chars = text.chars().peekable();
    let finish = |current: &mut String, out: &mut Vec<String>| {
        let declaration = current.trim();
        if !declaration.is_empty() {
            out.push(declaration.to_string());
        }
        current.clear();
    };
    while let Some(c) = chars.next() {
        if c == '\\' {
            current.push(c);
            if let Some(n) = chars.next() {
                current.push(n);
            }
            continue;
        }
        if let Some(q) = quote {
            if c == q {
                quote = None;
            }
            current.push(c);
            continue;
        }
        match c {
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                while let Some(cc) = chars.next() {
                    if cc == '*' && chars.peek() == Some(&'/') {
                        chars.next();
                        break;
                    }
                }
                current.push(' ');
                continue;
            }
            '(' if ends_with_url_ident(&current) => {
                current.push(c);
                let quoted = {
                    let mut ahead = chars.clone();
                    loop {
                        match ahead.next() {
                            Some(w) if w.is_whitespace() => continue,
                            Some('"' | '\'') => break true,
                            _ => break false,
                        }
                    }
                };
                if quoted {
                    // `url("...")` is an ordinary function holding a string.
                    closers.push(')');
                    continue;
                }
                open_url = true;
                while let Some(u) = chars.next() {
                    current.push(u);
                    if u == '\\' {
                        if let Some(n) = chars.next() {
                            current.push(n);
                        }
                    } else if u == ')' {
                        open_url = false;
                        break;
                    }
                }
                continue;
            }
            '"' | '\'' => quote = Some(c),
            '(' => closers.push(')'),
            '[' => closers.push(']'),
            '{' => closers.push('}'),
            ')' | ']' | '}' if closers.last() == Some(&c) => {
                closers.pop();
            }
            ';' if closers.is_empty() => {
                finish(&mut current, &mut out);
                continue;
            }
            _ => {}
        }
        current.push(c);
    }
    if !current.trim_start().starts_with("--") {
        if open_url {
            current.push(')');
        }
        current.extend(quote);
        current.extend(closers.iter().rev());
    }
    finish(&mut current, &mut out);
    out
}

/// Whether `text` ends with the identifier `url` (ASCII case-insensitive), so
/// a `(` after it opens a url token. `myurl(` and `my-url(` are other
/// functions: the character before `url` must not continue an identifier.
fn ends_with_url_ident(text: &str) -> bool {
    let Some(head) = text.len().checked_sub(3).and_then(|i| text.get(i..).map(|t| (i, t))) else {
        return false;
    };
    let (at, tail) = head;
    if !tail.eq_ignore_ascii_case("url") {
        return false;
    }
    !text[..at]
        .chars()
        .next_back()
        .is_some_and(|p| p.is_alphanumeric() || p == '-' || p == '_' || p == '\\' || !p.is_ascii())
}

fn strip_important(value: &str) -> (&str, bool) {
    let lower = value.to_ascii_lowercase();
    if let Some(idx) = lower.rfind("!important") {
        let before = value[..idx].trim_end();
        (before, true)
    } else {
        (value, false)
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn a_data_uri_does_not_end_its_own_declaration() {
        // THE DEFECT: `;` inside url() ended the declaration, so the URI was
        // truncated at `url(data:image/png` AND the remainder was re-read as a
        // new property -- which SWALLOWED the declaration after it. One data
        // URI corrupted two declarations, with no error reported anywhere.
        let css = "a { background-image: url(data:image/png;base64,AAAA); color: red; }";
        let ast = parse_stylesheet(css).expect("parse");
        let decls = &ast.rules[0].declarations;
        assert_eq!(decls.len(), 2, "the following declaration must survive");
        assert_eq!(decls[0].property, "background-image");
        assert_eq!(decls[0].value, "url(data:image/png;base64,AAAA)");
        assert_eq!(decls[1].property, "color", "color: red was being eaten");
        assert_eq!(decls[1].value, "red");
    }

    #[test]
    fn a_prefixed_mask_with_a_quoted_svg_data_uri_parses_whole() {
        // The icon idiom on the board sites: a `-webkit-` mask whose quoted
        // SVG has `;`, `:`, `{`-free markup and quotes of the other kind.
        let svg = "data:image/svg+xml;charset=utf-8,<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16'><path d='M0 0h16v16z'/></svg>";
        let css = format!(
            ".icon {{ -webkit-mask-image: url(\"{svg}\"); mask: url(\"{svg}\") no-repeat 50% / contain; background-color: currentColor; }}"
        );
        let ast = parse_stylesheet(&css).expect("parse");
        let decls = &ast.rules[0].declarations;
        let names: Vec<&str> = decls.iter().map(|d| d.property.as_str()).collect();
        assert_eq!(names, ["-webkit-mask-image", "mask", "background-color"]);
        assert_eq!(decls[0].value, format!("url(\"{svg}\")"));
        assert_eq!(decls[1].value, format!("url(\"{svg}\") no-repeat 50% / contain"));
    }

    #[test]
    fn a_url_containing_colons_parses_whole() {
        // Documents behaviour; NOT a guard. The obvious `structural` check on
        // the property/value colon was removed after falsification showed this
        // test stays green with or without it.
        let css = "a { background: url(https://example.com/x.png) no-repeat; }";
        let ast = parse_stylesheet(css).expect("parse");
        let d = &ast.rules[0].declarations[0];
        assert_eq!(d.property, "background");
        assert_eq!(d.value, "url(https://example.com/x.png) no-repeat");
    }

    #[test]
    fn a_semicolon_inside_a_quoted_string_is_not_a_terminator() {
        let css = "a { content: \"a;b\"; color: red; }";
        let ast = parse_stylesheet(css).expect("parse");
        let decls = &ast.rules[0].declarations;
        assert_eq!(decls.len(), 2);
        assert_eq!(decls[0].value, "\"a;b\"");
        assert_eq!(decls[1].property, "color");
    }

    use super::*;

    /// CH2.E28: an empty declaration, or one with no colon, is skipped and
    /// the next one is kept (Chromium 148.0.7778.216 on the same rules:
    /// tools/parity_oracle/empty_declaration_cases.json).
    #[test]
    fn an_empty_or_colonless_declaration_does_not_eat_the_next_one() {
        let decls = |css: &str| -> Vec<(String, String, bool)> {
            parse_stylesheet(css).unwrap().rules[0]
                .declarations
                .iter()
                .map(|d| (d.property.clone(), d.value.clone(), d.important))
                .collect()
        };
        let d = |p: &str, v: &str, i: bool| (p.to_string(), v.to_string(), i);
        assert_eq!(decls("a{color:red;;background:blue}"), [d("color", "red", false), d("background", "blue", false)]);
        assert_eq!(decls("a{;;color:red}"), [d("color", "red", false)]);
        assert_eq!(decls("a{color:red;;background:blue !important}"), [d("color", "red", false), d("background", "blue", true)]);
        assert_eq!(decls("a{foo;color:red}"), [d("color", "red", false)]);
        assert_eq!(decls("a{width:1px;}"), [d("width", "1px", false)]);
        assert_eq!(decls("a{b:url(x;y);;w:1}"), [d("b", "url(x;y)", false), d("w", "1", false)]);
        assert_eq!(decls("a{--t: ;;w:1}"), [d("--t", "", false), d("w", "1", false)]);
        assert_eq!(decls("a{f(;);color:red}"), [d("color", "red", false)]);
    }

    /// The case list both copies of the rule run (this one and the bindings'
    /// JS `splitDecls`), so they cannot drift apart (#972 R1 nit 3).
    #[test]
    fn the_shared_declaration_list_cases_split_as_listed() {
        let doc: serde_json::Value =
            serde_json::from_str(include_str!("declaration_list_cases.json")).expect("cases");
        let mut wrong = Vec::new();
        for case in doc["cases"].as_array().expect("cases") {
            let input = case["input"].as_str().unwrap();
            let want: Vec<(String, String)> = case["decls"]
                .as_array()
                .unwrap()
                .iter()
                .map(|d| (d[0].as_str().unwrap().to_string(), d[1].as_str().unwrap().to_string()))
                .collect();
            let got: Vec<(String, String)> = split_declaration_list(input)
                .iter()
                .filter_map(|d| d.split_once(':'))
                .map(|(n, v)| (n.trim().to_string(), v.trim().to_string()))
                .collect();
            if got != want {
                wrong.push(format!("{input:?}: got {got:?}, want {want:?}"));
            }
        }
        assert!(wrong.is_empty(), "{} wrong\n{}", wrong.len(), wrong.join("\n"));
    }

    #[test]
    fn a_declaration_list_splits_only_at_top_level_semicolons() {
        let split = |s: &str| split_declaration_list(s);
        assert_eq!(
            split("background-image:url(data:image/svg+xml;base64,AA==);width:10px"),
            ["background-image:url(data:image/svg+xml;base64,AA==)", "width:10px"]
        );
        assert_eq!(split("font-family:'a;b', serif;width:1px"), ["font-family:'a;b', serif", "width:1px"]);
        assert_eq!(split("a:url(\"x;y\");b:1"), ["a:url(\"x;y\")", "b:1"]);
        assert_eq!(split("--k:f(;);w:1"), ["--k:f(;)", "w:1"]);
        assert_eq!(split("--k:[;];w:1"), ["--k:[;]", "w:1"]);
        assert_eq!(split("--k:{;};w:1"), ["--k:{;}", "w:1"]);
        assert_eq!(split("a:'it\\'s;x';b:1"), ["a:'it\\'s;x'", "b:1"]);
        assert_eq!(split("a:x\\;y;b:1"), ["a:x\\;y", "b:1"]);
        assert_eq!(split("width:20px /* ; */;height:2px"), ["width:20px", "height:2px"]);
        assert_eq!(split("margin:1px/**/2px"), ["margin:1px 2px"]);
        assert_eq!(split(" ;; color:red !important ;; ;"), ["color:red !important"]);
        assert_eq!(split("w:1;b:url(data:x;y"), ["w:1", "b:url(data:x;y)"]);
        assert_eq!(split("a:'open;b:1"), ["a:'open;b:1'"]);
        assert_eq!(split("--a:'open;b:1"), ["--a:'open;b:1"]);
        assert_eq!(split("a:f(];b:1"), ["a:f(];b:1)"]);
        assert_eq!(split("a:1);b:2"), ["a:1)", "b:2"]);
        assert!(split("").is_empty());
    }

    #[test]
    fn parse_simple_stylesheet() {
        let css = r#"
            body { color: black; }
            .container { width: 100%; height: 10px !important; }
        "#;
        let ast = parse_stylesheet(css).unwrap();
        assert_eq!(ast.rules.len(), 2);
        assert_eq!(ast.rules[0].selector, "body");
        assert_eq!(ast.rules[0].declarations.len(), 1);
        assert_eq!(ast.rules[1].selector, ".container");
        assert_eq!(ast.rules[1].declarations.len(), 2);
        assert!(ast.rules[1].declarations[1].important);
    }

    #[test]
    fn an_empty_custom_property_is_kept_and_an_empty_property_is_not() {
        let ast = parse_stylesheet("a{--on: ;color:;--last:}b{--nocolon}").unwrap();
        let decls: Vec<(&str, &str)> = ast.rules[0]
            .declarations
            .iter()
            .map(|d| (d.property.as_str(), d.value.as_str()))
            .collect();
        assert_eq!(decls, vec![("--on", ""), ("--last", "")]);
        assert_eq!(ast.rules.len(), 1, "`--nocolon` is not a declaration");
    }

    #[test]
    fn parse_with_comments() {
        let css = r#"
            /* comment */
            body { color: black; /* inside */ width: 10px; }
        "#;
        let ast = parse_stylesheet(css).unwrap();
        assert_eq!(ast.rules.len(), 1);
        assert_eq!(ast.rules[0].declarations.len(), 2);
    }

    #[test]
    fn eof_closes_an_unclosed_rule_and_keeps_the_rules_before_it() {
        // Was Err(UnexpectedEof), which dropped `.a` too: one defect anywhere
        // cost the whole sheet.
        let css = ".a { color: red } body { color: black;";
        let ast = parse_stylesheet(css).expect("EOF closes the block");
        assert_eq!(ast.rules.len(), 2);
        assert_eq!(ast.rules[1].selector, "body");
        assert_eq!(ast.rules[1].declarations[0].value, "black");
    }

    #[test]
    fn an_escaped_quote_in_a_selector_does_not_open_a_string() {
        // linkedin, x, yahoo and weather each lost a whole sheet to this:
        // Tailwind's `\'` read as a quote swallowed the `@media` block's `}`.
        let css = "@media (min-width: 640px) { .bg-\\[url\\(\\'x\\'\\)\\] { color: red } } .after { color: green }";
        assert_eq!(
            summary(css),
            vec![
                rule(".bg-\\[url\\(\\'x\\'\\)\\]", &["(min-width: 640px)"]),
                rule(".after", &[]),
            ]
        );
    }

    #[test]
    fn escapes_are_text_inside_declarations() {
        let css = ".a { content: \"x\\\"; y\"; color: red } .b { --v: a\\;b; color: blue }";
        let ast = parse_stylesheet(css).expect("parse");
        assert_eq!(ast.rules[0].declarations[0].value, "\"x\\\"; y\"");
        assert_eq!(ast.rules[0].declarations[1].property, "color");
        assert_eq!(ast.rules[1].declarations[0].value, "a\\;b");
        assert_eq!(ast.rules[1].declarations[1].value, "blue");
    }

    fn summary(css: &str) -> Vec<(String, Vec<String>)> {
        parse_stylesheet(css)
            .expect("parse")
            .rules
            .into_iter()
            .map(|r| (r.selector, r.media))
            .collect()
    }

    fn rule(selector: &str, media: &[&str]) -> (String, Vec<String>) {
        (selector.to_string(), media.iter().map(|m| m.to_string()).collect())
    }

    #[test]
    fn media_blocks_keep_their_rules_inside_and_the_next_rule_survives() {
        // Before: `@media (x) {` became a selector and `.a{color` a property,
        // `.b` leaked out to apply at EVERY width, and `}.c` swallowed the
        // first rule after the block (apple's nav lost its 12px font to this).
        let css = "@media (max-width: 1px){.a{color:red}.b{color:blue}}.c{color:green}.d{color:black}";
        assert_eq!(
            summary(css),
            vec![
                rule(".a", &["(max-width: 1px)"]),
                rule(".b", &["(max-width: 1px)"]),
                rule(".c", &[]),
                rule(".d", &[]),
            ]
        );
    }

    #[test]
    fn statement_at_rules_do_not_eat_the_first_rule() {
        let css = "@charset \"UTF-8\";@import url(x.css);@layer base, top;#a html{color:red}";
        assert_eq!(summary(css), vec![rule("#a html", &[])]);
    }

    #[test]
    fn nested_group_rules_accumulate_media_and_skip_what_cannot_apply() {
        let css = r#"
            @media screen { @media (min-width: 800px) { .wide { color: red } } }
            @supports (display: grid) { .grid { display: grid } }
            @supports not (display: grid) { .fallback { float: left } }
            @layer base { .layered { color: blue } }
            @keyframes spin { from { opacity: 0 } to { opacity: 1 } }
            @container card (min-width: 400px) { .in-card { color: red } }
            @font-face { font-family: X; src: url(x.woff2) }
            .after { color: green }
        "#;
        assert_eq!(
            summary(css),
            vec![
                rule(".wide", &["screen", "(min-width: 800px)"]),
                rule(".grid", &[]),
                rule(".layered", &[]),
                rule(".in-card", &[]),
                rule("@font-face", &[]),
                rule(".after", &[]),
            ]
        );
    }

    #[test]
    fn supports_blocks_apply_only_when_their_condition_holds() {
        // #747's css_supports.html (#a-#e), then and/or/not, Tailwind v4's
        // property-defaults query, and a group rule nested in a style rule.
        // Before, every positive query applied and every `not` was skipped.
        let css = r#"
            @supports (display: grid) { .a { color: green } }
            @supports not (display: frobnicate) { .b { color: green } }
            @supports (display: frobnicate) { .c { color: red } }
            @supports not (display: block) { .d { color: red } }
            @supports not selector(:popover-open) { .e { color: red } }
            @supports (display: grid) and (not (margin-trim: inline)) { .f { color: green } }
            @supports (-moz-appearance: none) or (color: oklch(0 0 0)) { .g { color: green } }
            @supports (display: grid) and (gap: 1px) or (color: red) { .h { color: red } }
            @supports ((-webkit-hyphens: none) and (not (margin-trim: inline))) or ((-moz-orient: inline) and (not (color:rgb(from red r g b)))) { .tw { color: green } }
            .n { color: green; @supports (display: frobnicate) { color: red } }
            .after { color: green }
        "#;
        let selectors: Vec<String> = parse_stylesheet(css)
            .expect("parse")
            .rules
            .into_iter()
            .map(|r| r.selector)
            .collect();
        assert_eq!(selectors, [".a", ".b", ".f", ".g", ".tw", ".n", ".after"]);
    }

    #[test]
    fn supports_conditions_follow_the_grammar() {
        assert_eq!(supports_condition("(display: block flow-root)"), Some(true));
        assert_eq!(
            supports_condition("(display: inline list-item flex)"),
            Some(false)
        );
        assert_eq!(
            supports_condition("(--x: {a}) and font-format(woff2)"),
            Some(true)
        );
        assert_eq!(supports_condition("display: grid"), None);
    }
    #[test]
    fn layer_blocks_name_their_rules_and_statements_declare_the_order() {
        let css = r#"
            @layer reset, theme;
            .plain { color: black }
            @layer theme { .t { color: red } @layer dark { .d { color: blue } } }
            @media (min-width: 1px) { @layer reset { .r { margin: 0 } } }
            @LAYER x.y;
        "#;
        let ast = parse_stylesheet(css).expect("parse");
        let layers: Vec<(&str, Option<&str>)> = ast
            .rules
            .iter()
            .map(|r| (r.selector.as_str(), r.layer.as_deref()))
            .collect();
        assert_eq!(
            layers,
            vec![
                (".plain", None),
                (".t", Some("theme")),
                (".d", Some("theme.dark")),
                (".r", Some("reset")),
            ]
        );
        assert_eq!(ast.rules[3].media, vec!["(min-width: 1px)".to_string()]);
        let statements: Vec<(usize, &str, usize)> = ast
            .layer_statements
            .iter()
            .map(|s| (s.position, s.name.as_str(), s.media.len()))
            .collect();
        assert_eq!(
            statements,
            vec![
                (0, "reset", 0),
                (0, "theme", 0),
                (1, "theme", 0),
                (2, "theme.dark", 0),
                (3, "reset", 1),
                (4, "x.y", 0),
            ]
        );
    }

    fn selectors_and_decls(css: &str) -> Vec<(String, Vec<String>)> {
        parse_stylesheet(css)
            .expect("parse")
            .rules
            .into_iter()
            .map(|r| {
                let decls = r
                    .declarations
                    .into_iter()
                    .map(|d| format!("{}:{}", d.property, d.value))
                    .collect();
                (r.selector, decls)
            })
            .collect()
    }

    fn sd(selector: &str, decls: &[&str]) -> (String, Vec<String>) {
        (selector.to_string(), decls.iter().map(|d| d.to_string()).collect())
    }

    #[test]
    fn nested_style_rules_resolve_against_the_parent_and_the_next_rule_survives() {
        // linkedin's layered bundle stacks its hero with exactly this. Before,
        // the nested `}` closed `.stack`, `grid-area` became part of a garbage
        // property, and the stray `}` swallowed `.next`.
        let css = ".stack { display: grid; & > * { grid-area: 1/-1; min-width: 0 } \
                   & > [popover]:popover-open { grid-area: auto } } .next { color: red }";
        assert_eq!(
            selectors_and_decls(css),
            vec![
                sd(".stack", &["display:grid"]),
                sd(".stack > *", &["grid-area:1/-1", "min-width:0"]),
                sd(".stack > [popover]:popover-open", &["grid-area:auto"]),
                sd(".next", &["color:red"]),
            ]
        );
    }

    #[test]
    fn nested_selector_forms() {
        let css = ".a { &:hover { color: red } &.b, .c & { color: blue } \
                   .d { color: green } + .e { color: black } }";
        assert_eq!(
            selectors_and_decls(css),
            vec![
                sd(".a:hover", &["color:red"]),
                sd(".a.b, .c .a", &["color:blue"]),
                sd(".a .d", &["color:green"]),
                sd(".a + .e", &["color:black"]),
            ]
        );
        // A parent list expands per parent (the matcher's `:is()` takes
        // compounds only), and nesting goes deep.
        let css = ".x, .y > p { & span { color: red; &:first-child { color: blue } } }";
        assert_eq!(
            selectors_and_decls(css),
            vec![
                sd(".x span, .y > p span", &["color:red"]),
                sd(".x span:first-child, .y > p span:first-child", &["color:blue"]),
            ]
        );
        // Past the expansion cap, the parent list stays one `:is()`.
        let parents: Vec<String> = (0..9).map(|i| format!(".p{i}")).collect();
        let nested: Vec<String> = (0..8).map(|i| format!("& .n{i}")).collect();
        let css = format!("{} {{ {} {{ color: red }} }}", parents.join(", "), nested.join(", "));
        let ast = parse_stylesheet(&css).expect("parse");
        assert!(ast.rules[0].selector.starts_with(":is(.p0, .p1,"), "{}", ast.rules[0].selector);
        assert_eq!(split_selector_list(&ast.rules[0].selector).len(), 8);
        // `&` inside a string or escaped is not the nesting selector.
        let css = ".a { [data-x=\"&\"] & { color: red } }";
        assert_eq!(selectors_and_decls(css), vec![sd("[data-x=\"&\"] .a", &["color:red"])]);
    }

    #[test]
    fn declarations_after_a_nested_rule_stay_with_the_parent_in_source_order() {
        let css = ".a { color: red; & .b { color: blue } width: 1px }";
        assert_eq!(
            selectors_and_decls(css),
            vec![
                sd(".a", &["color:red"]),
                sd(".a .b", &["color:blue"]),
                sd(".a", &["width:1px"]),
            ]
        );
    }

    #[test]
    fn nested_group_rules_apply_to_the_parent() {
        let css = ".a { color: red; @media (min-width: 800px) { color: blue; & .b { color: green } } \
                   @layer top { width: 1px } @container (min-width: 1px) { width: 2px } } .c { color: black }";
        let ast = parse_stylesheet(css).expect("parse");
        let got: Vec<(&str, Vec<String>, Vec<String>, Option<&str>)> = ast
            .rules
            .iter()
            .map(|r| {
                (
                    r.selector.as_str(),
                    r.declarations.iter().map(|d| d.value.clone()).collect(),
                    r.media.clone(),
                    r.layer.as_deref(),
                )
            })
            .collect();
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(
            got,
            vec![
                (".a", s(&["red"]), vec![], None),
                (".a", s(&["blue"]), s(&["(min-width: 800px)"]), None),
                (".a .b", s(&["green"]), s(&["(min-width: 800px)"]), None),
                (".a", s(&["1px"]), vec![], Some("top")),
                (".a", s(&["2px"]), vec![], None),
                (".c", s(&["black"]), vec![], None),
            ]
        );
        assert_eq!(ast.layer_statements[0].name, "top");
    }

    #[test]
    fn container_blocks_record_their_condition_on_each_rule() {
        let css = "@container card (width >= 200px) { .a { color: red } \
                   @container (min-width: 1px) { .b { color: blue } } } \
                   @media print { @container x (width > 0px) { .p { color: red } } } \
                   @container { .bare { color: red } } .c { color: black }";
        let ast = parse_stylesheet(css).expect("parse");
        let got: Vec<(&str, Vec<&str>, Vec<&str>)> = ast
            .rules
            .iter()
            .map(|r| {
                (
                    r.selector.as_str(),
                    r.container.iter().map(String::as_str).collect(),
                    r.media.iter().map(String::as_str).collect(),
                )
            })
            .collect();
        assert_eq!(
            got,
            vec![
                (".a", vec!["card (width >= 200px)"], vec![]),
                (".b", vec!["card (width >= 200px)", "(min-width: 1px)"], vec![]),
                (".p", vec!["x (width > 0px)"], vec!["print"]),
                (".c", vec![], vec![]),
            ]
        );
    }

    #[test]
    fn a_custom_property_value_may_hold_a_block() {
        let css = ".a { --x: { b: c }; color: red } .n { color: blue }";
        assert_eq!(
            selectors_and_decls(css),
            vec![sd(".a", &["--x:{ b: c }", "color:red"]), sd(".n", &["color:blue"])]
        );
    }

    #[test]
    fn each_anonymous_layer_block_is_its_own_layer() {
        let ast = parse_stylesheet("@layer { .a { color: red } } @layer { .b { color: blue } }")
            .expect("parse");
        let (a, b) = (ast.rules[0].layer.clone(), ast.rules[1].layer.clone());
        assert!(a.is_some() && b.is_some());
        assert_ne!(a, b);
    }

    #[test]
    fn braces_in_strings_and_comments_do_not_end_a_media_block() {
        // (A `}` inside a declaration's string is a separate, older limit of
        // the rule parser; `{` exercises the block scanner's string state.)
        let css = "@media print { .a::after { content: \"{\" } /* } */ .b { color: red } } .c { color: blue }";
        assert_eq!(
            summary(css),
            vec![rule(".a::after", &["print"]), rule(".b", &["print"]), rule(".c", &[])]
        );
    }

    #[test]
    fn eof_closes_an_unclosed_media_block_like_an_unclosed_rule() {
        assert_eq!(
            summary(".z { color: blue } @media screen { .a { color: red }"),
            vec![rule(".z", &[]), rule(".a", &["screen"])]
        );
    }

    #[test]
    fn parse_hsl_values() {
        let css = r#"
            .hsl1 { background-color: hsl(0, 100%, 50%); }
            .hsl2 { background-color: hsl(120, 100%, 50%); }
        "#;
        let ast = parse_stylesheet(css).unwrap();
        assert_eq!(ast.rules.len(), 2);
        assert_eq!(ast.rules[0].declarations[0].value, "hsl(0, 100%, 50%)");
        assert_eq!(ast.rules[1].declarations[0].value, "hsl(120, 100%, 50%)");
    }

    /// #833 RD3: `@keyframes` opacity ends are what settle paints from. The
    /// engine companion is headless-gated, so default Linux CI never sees it;
    /// pin the parser contract here.
    #[test]
    fn keyframes_keep_opacity_at_each_end() {
        let frames = |css: &str| -> Vec<(String, Option<f32>, Option<f32>)> {
            parse_stylesheet(css)
                .expect("parse")
                .keyframes
                .into_iter()
                .map(|k| (k.name, k.first, k.last))
                .collect()
        };
        assert_eq!(
            frames("@keyframes fade { from { opacity: 0 } to { opacity: 1 } }"),
            vec![("fade".to_string(), Some(0.0), Some(1.0))]
        );
        assert_eq!(
            frames("@keyframes p { 0% { opacity: 20% } 100% { opacity: .5 } }"),
            vec![("p".to_string(), Some(0.2), Some(0.5))]
        );
        // `!important` in a keyframe is invalid and ignored; an end that
        // only had that declaration stays unset.
        assert_eq!(
            frames("@keyframes i { from { opacity: 0 !important } to { opacity: 1 } }"),
            vec![("i".to_string(), None, Some(1.0))]
        );
        // `var()` cannot be read: the `?` on the parse drops the whole
        // `@keyframes` (as good as absent), so settle leaves the box at
        // full opacity rather than guess.
        assert!(
            frames("@keyframes v { from { opacity: var(--o) } to { opacity: var(--o) } }").is_empty(),
            "unreadable opacity must drop the rule"
        );
        assert_eq!(
            frames("@-webkit-keyframes w { to { opacity: .25 } }"),
            vec![("w".to_string(), None, Some(0.25))]
        );
    }

    /// S13b: the engine runs `@keyframes`, so every block is kept with its
    /// offsets and declarations (`!important` ones dropped).
    #[test]
    fn keyframes_keep_every_block() {
        let sheet = parse_stylesheet(
            "@keyframes k { from { opacity: 0; transform: none } 25%, 75% { opacity: .5 !important; \
             animation-timing-function: linear } to { opacity: 1 } }",
        )
        .expect("parse");
        let k = &sheet.keyframes[0];
        let blocks: Vec<(Vec<f32>, Vec<(String, String)>)> = k
            .frames
            .iter()
            .map(|f| {
                (
                    f.offsets.clone(),
                    f.declarations
                        .iter()
                        .map(|d| (d.property.clone(), d.value.clone()))
                        .collect(),
                )
            })
            .collect();
        assert_eq!(
            blocks,
            vec![
                (
                    vec![0.0],
                    vec![("opacity".to_string(), "0".to_string()), ("transform".to_string(), "none".to_string())]
                ),
                (
                    vec![0.25, 0.75],
                    vec![("animation-timing-function".to_string(), "linear".to_string())]
                ),
                (vec![1.0], vec![("opacity".to_string(), "1".to_string())]),
            ]
        );
    }
}
