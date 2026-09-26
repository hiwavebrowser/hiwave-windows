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
}

/// A parsed rule AST.
#[derive(Debug, Clone)]
pub struct RuleAst {
    pub selector: String,
    pub declarations: Vec<DeclarationAst>,
    /// Media query lists of the `@media` blocks enclosing this rule,
    /// outermost first. The rule applies only where every one matches.
    pub media: Vec<String>,
}

/// What an at-rule with a `{ ... }` block contributes to the stylesheet.
enum AtBlock {
    /// Its body is a list of rules (`@media`, `@supports`, `@layer`); an
    /// `@media` query list is recorded on each of them.
    Rules(Option<String>),
    /// Its body is declarations (`@font-face`, `@page`): kept as one rule
    /// whose selector is the at-rule prelude, as before.
    Declarations,
    /// Nothing that styles an element in the static frame (`@keyframes`,
    /// `@container`, `@supports not (...)`, unknown at-rules): skipped whole.
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
        // Only a negated condition is decided here: every positive feature
        // query on the board's sites names something Chrome supports, and a
        // property RustKit lacks is ignored at apply time anyway.
        "supports" if condition.to_ascii_lowercase().starts_with("not") => AtBlock::Skip,
        "supports" | "layer" => AtBlock::Rules(None),
        "font-face" | "page" | "property" | "counter-style" | "font-palette-values" => {
            AtBlock::Declarations
        }
        _ => AtBlock::Skip,
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
#[derive(Debug, Clone)]
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
/// - It does not support CSS nesting or complex tokenization.
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
                // A statement at-rule (`@charset "UTF-8";`, `@import ...;`,
                // `@layer a, b;`) ends here and styles nothing.
                current_selector.clear();
                continue;
            }
            if c == '{' && at_rule {
                let prelude = current_selector.trim().to_string();
                match at_block_kind(&prelude) {
                    AtBlock::Declarations => {}
                    kind => {
                        let body = take_block(&mut chars);
                        if let AtBlock::Rules(media) = kind {
                            for mut rule in parse_stylesheet(&body)?.rules {
                                if let Some(m) = &media {
                                    rule.media.insert(0, m.clone());
                                }
                                out.rules.push(rule);
                            }
                        }
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
            );
            let selector = current_selector.trim().to_string();
            if !selector.is_empty() && !current_decls.is_empty() {
                out.rules.push(RuleAst {
                    selector,
                    declarations: current_decls.clone(),
                    media: Vec::new(),
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

        if !in_value {
            // NOTE: no `structural` check here, deliberately. It looks like it
            // belongs -- a colon inside url(data:...) is part of the scheme --
            // but falsification proved it dead: with the `;` fix below, a value
            // is never re-entered as a property, so a URL's colon is always
            // read in value position. A guard whose removal turns nothing red
            // is decoration, and decoration in a parser reads as intent.
            if c == ':' {
                in_value = true;
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
        );
        let selector = current_selector.trim().to_string();
        if !selector.is_empty() && !current_decls.is_empty() {
            out.rules.push(RuleAst {
                selector,
                declarations: current_decls,
                media: Vec::new(),
            });
        }
    }

    Ok(out)
}

fn flush_decl(
    current_property: &mut String,
    current_value: &mut String,
    decls: &mut Vec<DeclarationAst>,
) {
    let property = current_property.trim();
    let value_raw = current_value.trim();
    if property.is_empty() || value_raw.is_empty() {
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
                rule("@font-face", &[]),
                rule(".after", &[]),
            ]
        );
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
}
