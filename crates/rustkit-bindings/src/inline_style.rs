//! NATIVE-SETPROPERTY: an element's inline style (`el.style`, CSSStyleDeclaration over the `style`
//! attribute) read and written in Rust, with the same results as the bindings' JavaScript copy in
//! dom.rs (`splitDecls`, `parseDecls`, `setProperty`, `removeProperty`, `serializeDecls`): the same
//! declarations read from the same text, the same text written back. That copy interprets ~6,800 VM
//! opcodes per write (`style_set_cost_tests`); here a write is one host call.
//!
//! The text is read the way the JS reads it: as UTF-16 code units in JS and as chars here, which give
//! the same splits (every character the rules look at is in the Basic Multilingual Plane), with JS's
//! own whitespace set (`String.prototype.trim`, `\s`), not Rust's (they differ at U+0085 and U+FEFF).
//!
//! One rule is stricter than the JS copy on purpose: a value holding a bad url is refused by the
//! tokenizer's rule (CSS Syntax 3 §4.3.6), with escapes in the function name decoded
//! (`has_bad_url`), and nothing here reads anything page script can change.

/// One declaration: a lowercase name, its value without `!important`, and whether it had one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Decl {
    pub(crate) name: String,
    pub(crate) value: String,
    pub(crate) important: bool,
}

/// JavaScript's WhiteSpace and LineTerminator code points (ECMA-262 §12.2, §12.3): what `trim()` strips
/// and `/\s/` matches.
pub(crate) fn js_space(c: char) -> bool {
    matches!(
        c,
        '\u{9}' | '\u{a}' | '\u{b}' | '\u{c}' | '\u{d}' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}'
            | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}'
    )
}

/// `String.prototype.trim`.
pub(crate) fn js_trim(s: &str) -> &str {
    s.trim_matches(js_space)
}

/// A value without its leading and trailing CSS whitespace (space, tab, newlines): NBSP, U+FEFF and the
/// other characters `trim()` takes are name characters to CSS and stay (#1266's `cssTrimValue`).
pub(crate) fn css_trim_value(s: &str) -> &str {
    s.trim_matches(css_space)
}

/// Whether `c` is in the JS name-character class `[\w\-\\\u0080-￿]`.
fn js_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '\\' || c as u32 >= 0x80
}

/// `/(^|[^\w\-\\\u0080-￿])url$/i`: `text` ends with `url` (any ASCII case) as its own name.
fn ends_with_url_name(text: &[char]) -> bool {
    let n = text.len();
    if n < 3 || !text[n - 3..].iter().zip("url".chars()).all(|(a, b)| a.eq_ignore_ascii_case(&b)) {
        return false;
    }
    n == 3 || !js_name_char(text[n - 4])
}

/// `splitDecls`: the style attribute's text cut at its top-level `;` (CH2.E27), comments dropped (each
/// one a space), and at the end of a declaration other than a custom property its open url, string and
/// blocks closed. Each piece as written: untrimmed, empty pieces kept.
pub(crate) fn split_decls(text: &str) -> Vec<String> {
    let t: Vec<char> = text.chars().collect();
    let n = t.len();
    let mut out = Vec::new();
    let mut cur: Vec<char> = Vec::new();
    let mut closers: Vec<char> = Vec::new();
    let mut quote: Option<char> = None;
    let mut open_url = false;
    let mut i = 0;
    while i < n {
        let c = t[i];
        i += 1;
        if c == '\\' {
            cur.push(c);
            if i < n {
                cur.push(t[i]);
            }
            i += 1;
            continue;
        }
        if let Some(q) = quote {
            if c == q {
                quote = None;
            }
            cur.push(c);
            continue;
        }
        if c == '/' && t.get(i) == Some(&'*') {
            // `text.indexOf('*/', i + 1)`: the `*` at `i` does not start the closer.
            let end = (i + 1..n.saturating_sub(1)).find(|&k| t[k] == '*' && t[k + 1] == '/');
            i = end.map_or(n, |e| e + 2);
            cur.push(' ');
            continue;
        }
        if c == '(' && ends_with_url_name(&cur) {
            cur.push(c);
            let mut j = i;
            while j < n && js_space(t[j]) {
                j += 1;
            }
            if matches!(t.get(j), Some('"' | '\'')) {
                closers.push(')');
                continue;
            }
            open_url = true;
            while i < n {
                let u = t[i];
                i += 1;
                cur.push(u);
                if u == '\\' {
                    if i < n {
                        cur.push(t[i]);
                    }
                    i += 1;
                } else if u == ')' {
                    open_url = false;
                    break;
                }
            }
            continue;
        }
        match c {
            '"' | '\'' => quote = Some(c),
            '(' => closers.push(')'),
            '[' => closers.push(']'),
            '{' => closers.push('}'),
            _ if closers.last() == Some(&c) => {
                closers.pop();
            }
            ';' if closers.is_empty() => {
                out.push(cur.drain(..).collect());
                continue;
            }
            _ => {}
        }
        cur.push(c);
    }
    let custom = {
        let mut k = 0;
        while k < cur.len() && js_space(cur[k]) {
            k += 1;
        }
        cur.get(k) == Some(&'-') && cur.get(k + 1) == Some(&'-')
    };
    if !custom {
        if open_url {
            cur.push(')');
        }
        if let Some(q) = quote {
            cur.push(q);
        }
        while let Some(c) = closers.pop() {
            cur.push(c);
        }
    }
    out.push(cur.into_iter().collect());
    out
}

/// `/!\s*important$/i` on `value`: where the `!` is, when the value ends with one.
fn important_bang(value: &[char]) -> Option<usize> {
    let word = "important";
    let n = value.len();
    if n < word.len() + 1 {
        return None;
    }
    let tail = &value[n - word.len()..];
    if !tail.iter().zip(word.chars()).all(|(a, b)| a.eq_ignore_ascii_case(&b)) {
        return None;
    }
    let mut k = n - word.len();
    while k > 0 && js_space(value[k - 1]) {
        k -= 1;
    }
    (k > 0 && value[k - 1] == '!').then_some(k - 1)
}

/// The most declarations one style attribute is read for: the pieces after it are not read. Far past
/// any page's (an attribute this long is megabytes of text); it bounds what one host call does, which
/// the script budget cannot interrupt.
pub(crate) const MAX_DECLARATIONS: usize = 100_000;

/// `parseDecls`: the declarations of a style attribute's text, the last of each name kept (moved to the
/// end, as the JS filter-and-push does), with any declaration whose value holds a bad url left out.
/// Linear in the text: each name's last position is looked up, not searched for (#1264 R1).
pub(crate) fn parse_decls(text: &str) -> Vec<Decl> {
    let mut all: Vec<Decl> = Vec::new();
    for piece in split_decls(text).into_iter().take(MAX_DECLARATIONS) {
        let Some(colon) = piece.find(':') else {
            continue;
        };
        let name = js_trim(&piece[..colon]).to_lowercase();
        let value: Vec<char> = css_trim_value(&piece[colon + 1..]).chars().collect();
        let (value, important) = match important_bang(&value) {
            // `value.replace(/\s*!\s*important$/i, '')`: the whitespace before the `!` goes too.
            Some(bang) => {
                let mut end = bang;
                while end > 0 && js_space(value[end - 1]) {
                    end -= 1;
                }
                (value[..end].iter().collect::<String>(), true)
            }
            None => (value.into_iter().collect(), false),
        };
        if name.is_empty() || value.is_empty() || has_bad_url(&value) {
            continue;
        }
        all.push(Decl { name, value, important });
    }
    // Where each name is last: the declarations kept, in the order of their last occurrence.
    let mut last: std::collections::HashMap<&str, usize> = std::collections::HashMap::with_capacity(all.len());
    for (i, d) in all.iter().enumerate() {
        #[cfg(test)]
        DEDUP_COMPARISONS.with(|c| c.set(c.get() + 1));
        last.insert(d.name.as_str(), i);
    }
    let keep: Vec<bool> = all
        .iter()
        .enumerate()
        .map(|(i, d)| {
            #[cfg(test)]
            DEDUP_COMPARISONS.with(|c| c.set(c.get() + 1));
            last.get(d.name.as_str()) == Some(&i)
        })
        .collect();
    drop(last);
    all.into_iter().zip(keep).filter_map(|(d, k)| k.then_some(d)).collect()
}

#[cfg(test)]
thread_local! {
    /// Test probe: name comparisons `parse_decls` made to keep the last declaration of each name.
    pub(crate) static DEDUP_COMPARISONS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// `serializeDecls`.
pub(crate) fn serialize_decls(decls: &[Decl]) -> String {
    let mut text = String::new();
    for (i, d) in decls.iter().enumerate() {
        if i > 0 {
            text.push(' ');
        }
        text.push_str(&d.name);
        text.push_str(": ");
        text.push_str(&d.value);
        if d.important {
            text.push_str(" !important");
        }
        text.push(';');
    }
    text
}

/// `nameRoundTrips`: a name the serialization reads back as itself (an identifier, or a custom property
/// of lowercase letters, digits, `-` and `_`).
fn name_round_trips(name: &str) -> bool {
    let b = name.as_bytes();
    let lower_or_digit = |c: u8| c.is_ascii_lowercase() || c.is_ascii_digit();
    if b.len() >= 2 && b[0] == b'-' && b[1] == b'-' {
        return b.len() > 2 && b[2..].iter().all(|&c| lower_or_digit(c) || c == b'-' || c == b'_');
    }
    let start = usize::from(b.first() == Some(&b'-'));
    b.get(start).is_some_and(|c| c.is_ascii_lowercase()) && b[start + 1..].iter().all(|&c| lower_or_digit(c) || c == b'-')
}

/// `roundTrips`: a value with no quote, escape, `;`, `!`, brace, comment or control character whose
/// parentheses and brackets nest and close in order.
fn value_round_trips(value: &str) -> bool {
    let mut open: Vec<char> = Vec::new();
    let mut prev = '\0';
    for c in value.chars() {
        if matches!(c, '"' | '\'' | '\\' | ';' | '!' | '{' | '}') || (c as u32) < 32 || c == '\u{7f}' {
            return false;
        }
        if (prev == '/' && c == '*') || (prev == '*' && c == '/') {
            return false;
        }
        match c {
            '(' => open.push(')'),
            '[' => open.push(']'),
            ')' | ']' if open.pop() != Some(c) => return false,
            _ => {}
        }
        prev = c;
    }
    open.is_empty()
}

/// `exactOf`: whether every declaration of `decls` reads back as itself from `serialize_decls(decls)`,
/// so the list can be kept for that text without parsing it.
pub(crate) fn reads_back_as_itself(decls: &[Decl]) -> bool {
    decls.iter().all(|d| name_round_trips(&d.name) && value_round_trips(&d.value))
}

/// `setProperty(name, value, priority)` on declarations `decls`, with `name`, `value` and `priority`
/// already converted to strings (that conversion runs page code, so it stays in JS): the declarations
/// to write (`serialize_decls`), or `None` when the attribute is not written. An empty value is a
/// `removeProperty`; a value holding a bad url changes nothing; so does the declaration it already has.
pub(crate) fn set_property(decls: &[Decl], name: &str, value: &str, priority: &str) -> Option<Vec<Decl>> {
    let name = js_trim(name).to_lowercase();
    let value = css_trim_value(value);
    if value.is_empty() {
        return remove_property(decls, &name).1;
    }
    if has_bad_url(value) {
        return None;
    }
    let important = priority.to_lowercase() == "important";
    if decls.iter().any(|d| d.name == name && d.value == value && d.important == important) {
        return None;
    }
    let mut out = decls.to_vec();
    match out.iter_mut().find(|d| d.name == name) {
        Some(d) => {
            d.value = value.to_string();
            d.important = important;
        }
        None => out.push(Decl { name, value: value.to_string(), important }),
    }
    Some(out)
}

/// `removeProperty(name)`: the value it had (empty when none), and the declarations to write when it
/// had one.
pub(crate) fn remove_property(decls: &[Decl], name: &str) -> (String, Option<Vec<Decl>>) {
    let name = js_trim(name).to_lowercase();
    let old = decls.iter().find(|d| d.name == name).map(|d| d.value.clone()).unwrap_or_default();
    let kept: Vec<Decl> = decls.iter().filter(|d| d.name != name).cloned().collect();
    if kept.len() == decls.len() {
        return (old, None);
    }
    (old, Some(kept))
}

// ---- The bad-url rule: CSS Syntax 3 §4.3, the tokenizer as far as url tokens need it. ----

fn css_space(c: char) -> bool {
    matches!(c, '\n' | '\t' | ' ' | '\r' | '\u{c}')
}

fn newline(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\u{c}')
}

fn name_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_' || c as u32 >= 0x80
}

fn name_char(c: char) -> bool {
    name_start(c) || c.is_ascii_digit() || c == '-'
}

fn non_printable(c: char) -> bool {
    matches!(c, '\u{0}'..='\u{8}' | '\u{b}' | '\u{e}'..='\u{1f}' | '\u{7f}')
}

/// Characters the engine's url reader (`rustkit_css::mask::parse_css_url`: Rust's `char::is_whitespace`
/// and `is_control`) refuses in an unquoted url although CSS does not: refused here while the engine does,
/// so a value kept here is one the engine reads alike (#1266's `engineRefusesInUrl`).
fn engine_refuses_in_url(c: char) -> bool {
    matches!(
        c,
        '\u{0}' | '\u{7f}'..='\u{9f}' | '\u{a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}'
            | '\u{202f}' | '\u{205f}' | '\u{3000}'
    )
}

/// §4.3.8: a `\` that starts an escape: any but one before a newline. One at the end of the value is an
/// escape too (§4.3.7 reads it as U+FFFD), as #1266's check and Chrome read it.
fn valid_escape(t: &[char], i: usize) -> bool {
    t.get(i) == Some(&'\\') && t.get(i + 1).is_none_or(|&c| !newline(c))
}

/// §4.3.9: whether the code points at `i` would start an identifier.
fn starts_ident(t: &[char], i: usize) -> bool {
    match t.get(i) {
        Some('-') => t.get(i + 1).is_some_and(|&c| name_start(c) || c == '-') || valid_escape(t, i + 1),
        Some(&c) if name_start(c) => true,
        Some('\\') => valid_escape(t, i),
        _ => false,
    }
}

/// §4.3.10: whether the code points at `i` would start a number.
fn starts_number(t: &[char], i: usize) -> bool {
    let digit = |k: usize| t.get(k).is_some_and(|c| c.is_ascii_digit());
    match t.get(i) {
        Some('+' | '-') => digit(i + 1) || (t.get(i + 1) == Some(&'.') && digit(i + 2)),
        Some('.') => digit(i + 1),
        Some(c) => c.is_ascii_digit(),
        None => false,
    }
}

/// §4.3.7: consume an escape after its `\` at `i`; the decoded code point and where it ends.
fn consume_escape(t: &[char], mut i: usize) -> (char, usize) {
    let Some(&c) = t.get(i) else {
        return ('\u{fffd}', i);
    };
    if c.is_ascii_hexdigit() {
        let mut v: u32 = 0;
        let mut k = 0;
        while k < 6 && t.get(i).is_some_and(|c| c.is_ascii_hexdigit()) {
            v = v * 16 + t[i].to_digit(16).unwrap();
            i += 1;
            k += 1;
        }
        if t.get(i).is_some_and(|&c| css_space(c)) {
            // `\r\n` is one newline after preprocessing.
            if t[i] == '\r' && t.get(i + 1) == Some(&'\n') {
                i += 1;
            }
            i += 1;
        }
        let decoded = match v {
            0 | 0xd800..=0xdfff => '\u{fffd}',
            v => char::from_u32(v).unwrap_or('\u{fffd}'),
        };
        return (decoded, i);
    }
    (c, i + 1)
}

/// §4.3.12: consume a name from `i`, escapes decoded.
fn consume_name(t: &[char], mut i: usize) -> (String, usize) {
    let mut name = String::new();
    loop {
        match t.get(i) {
            Some(&c) if name_char(c) => {
                name.push(c);
                i += 1;
            }
            Some('\\') if valid_escape(t, i) => {
                let (c, next) = consume_escape(t, i + 1);
                name.push(c);
                i = next;
            }
            _ => return (name, i),
        }
    }
}

/// §4.3.12 (numbers): where a number from `i` ends.
fn skip_number(t: &[char], mut i: usize) -> usize {
    let digits = |mut i: usize| {
        while t.get(i).is_some_and(|c| c.is_ascii_digit()) {
            i += 1;
        }
        i
    };
    if matches!(t.get(i), Some('+' | '-')) {
        i += 1;
    }
    i = digits(i);
    if t.get(i) == Some(&'.') && t.get(i + 1).is_some_and(|c| c.is_ascii_digit()) {
        i = digits(i + 1);
    }
    if matches!(t.get(i), Some('e' | 'E')) {
        let k = if matches!(t.get(i + 1), Some('+' | '-')) { i + 2 } else { i + 1 };
        if t.get(k).is_some_and(|c| c.is_ascii_digit()) {
            i = digits(k);
        }
    }
    i
}

/// §4.3.6 from just after `url(` (whitespace already consumed): whether the url token is a bad url, and
/// where the token ends.
fn url_token(t: &[char], mut i: usize) -> (bool, usize) {
    loop {
        let Some(&c) = t.get(i) else {
            return (false, i);
        };
        i += 1;
        match c {
            ')' => return (false, i),
            c if css_space(c) => {
                while t.get(i).is_some_and(|&c| css_space(c)) {
                    i += 1;
                }
                match t.get(i) {
                    None => return (false, i),
                    Some(')') => return (false, i + 1),
                    Some(_) => return (true, i),
                }
            }
            '"' | '\'' | '(' => return (true, i),
            c if non_printable(c) || engine_refuses_in_url(c) => return (true, i),
            '\\' if valid_escape(t, i - 1) => i = consume_escape(t, i).1,
            '\\' => return (true, i),
            _ => {}
        }
    }
}

/// Whether `value`, tokenized as CSS Syntax 3 does (§4.3), holds a bad-url token anywhere, nested
/// blocks included: a value that does is invalid for every property (the declaration is not taken).
/// The function name is matched after its escapes are decoded (`u\72l(`, `\75 rl(`), as the tokenizer
/// matches it; strings and comments are skipped, and so are numbers, dimensions and hashes, so the
/// `url` in `10url(` or `#url(` is not a function name.
pub(crate) fn has_bad_url(value: &str) -> bool {
    // §3.3 preprocessing: CR, FF and CR LF are newlines.
    let t: Vec<char> = value.chars().collect();
    let n = t.len();
    let mut i = 0;
    while i < n {
        let c = t[i];
        if c == '/' && t.get(i + 1) == Some(&'*') {
            i = (i + 2..n.saturating_sub(1)).find(|&k| t[k] == '*' && t[k + 1] == '/').map_or(n, |e| e + 2);
            continue;
        }
        if c == '"' || c == '\'' {
            i += 1;
            while i < n {
                let s = t[i];
                if s == c {
                    i += 1;
                    break;
                }
                if newline(s) {
                    // A bad string ends at the newline, which is not part of it.
                    break;
                }
                if s == '\\' {
                    // An escaped newline continues the string; any other escape is one code point.
                    i += if t.get(i + 1).is_some() { 2 } else { 1 };
                    continue;
                }
                i += 1;
            }
            continue;
        }
        if starts_number(&t, i) {
            i = skip_number(&t, i);
            if starts_ident(&t, i) {
                i = consume_name(&t, i).1;
            } else if t.get(i) == Some(&'%') {
                i += 1;
            }
            continue;
        }
        if (c == '#' || c == '@') && (t.get(i + 1).is_some_and(|&c| name_char(c)) || valid_escape(&t, i + 1)) {
            i = consume_name(&t, i + 1).1;
            continue;
        }
        if starts_ident(&t, i) {
            let (name, next) = consume_name(&t, i);
            i = next;
            if t.get(i) != Some(&'(') {
                continue;
            }
            i += 1;
            if !name.eq_ignore_ascii_case("url") {
                // An ordinary function: its contents are tokenized as they come.
                continue;
            }
            while t.get(i).is_some_and(|&c| css_space(c)) {
                i += 1;
            }
            if matches!(t.get(i), Some('"' | '\'')) {
                // `url("…")` is a function holding a string.
                continue;
            }
            let (bad, next) = url_token(&t, i);
            if bad {
                return true;
            }
            i = next;
            continue;
        }
        if c == '\\' {
            // A `\` that starts no escape (before a newline) is a delimiter.
            i += 1;
            continue;
        }
        i += 1;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_declaration_list_corpus_splits_as_both_copies_do() {
        let cases: serde_json::Value =
            serde_json::from_str(include_str!("../../rustkit-cssparser/src/declaration_list_cases.json")).unwrap();
        for case in cases["cases"].as_array().unwrap() {
            let input = case["input"].as_str().unwrap();
            // As `style_splits_the_shared_declaration_list_cases` (lib.rs) reads the list: a declaration
            // whose value is a bad url is split the same way but is not in the block (CH2.E29).
            let want: Vec<(String, String)> = case["decls"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|p| {
                    let v = p[1].as_str().unwrap();
                    let is_url = v.get(..4).is_some_and(|h| h.eq_ignore_ascii_case("url("));
                    let closed = if v.ends_with(')') { v.to_string() } else { format!("{v})") };
                    !is_url || rustkit_css::mask::parse_css_url(&closed).is_some()
                })
                .map(|p| (p[0].as_str().unwrap().to_string(), p[1].as_str().unwrap().to_string()))
                .collect();
            let got: Vec<(String, String)> = parse_decls(input).into_iter().map(|d| (d.name, d.value)).collect();
            assert_eq!(got, want, "{input:?}");
        }
    }

    #[test]
    fn bad_urls_are_found_as_the_tokenizer_finds_them() {
        for bad in [
            "url(a b)",
            "url(a\"b)",
            "url(a'b)",
            "url(a(b)",
            "url(a\u{1}b)",
            "u\\72l(a b)",
            "\\75rl(a b)",
            "\\55 RL(a b)",
            "\\75 rl(a b)",
            "U\\52L(a b)",
            "ur\\l(a b)",
            "url(a\\\nb)",
            "image-set(url(a b) 1x)",
            "linear-gradient(red, blue), url(a b)",
        ] {
            assert!(has_bad_url(bad), "{bad:?} is a bad url");
        }
        for fine in [
            "url(a.png)",
            "url( a.png )",
            "url(\"a b\")",
            "u\\72l(a.png)",
            "\"url(a b)\"",
            "'url(a b)'",
            "url\\(a b)",
            "10url(a b)",
            "#url(a b)",
            "foourl(a b)",
            "/* url(a b) */ red",
            "url(a\\ b)",
            "rotate(9deg)",
        ] {
            assert!(!has_bad_url(fine), "{fine:?} holds no bad url");
        }
    }

    /// #1264 R1 blocker: a host call cannot be stopped by the script budget, so reading a style attribute
    /// must cost time linear in its text. 4,000 distinct names, each then repeated: keeping the last of
    /// each name may compare a few times per declaration, not once per pair.
    #[test]
    fn keeping_the_last_of_each_name_is_linear() {
        let n = 4_000u64;
        let mut text = String::new();
        for round in 0..2 {
            for i in 0..n {
                text.push_str(&format!("p{i}: {round}px;"));
            }
        }
        DEDUP_COMPARISONS.with(|c| c.set(0));
        let decls = parse_decls(&text);
        let comparisons = DEDUP_COMPARISONS.with(|c| c.get());
        assert_eq!(decls.len() as u64, n);
        assert_eq!(decls[0], Decl { name: "p0".into(), value: "1px".into(), important: false });
        assert!(comparisons <= 4 * 2 * n, "{comparisons} name comparisons for {} declarations", 2 * n);
    }

    /// CSS Syntax 3 §4.3.8: a backslash at the end of the value is an escape (of U+FFFD), so a url body
    /// that ends with one is a url, not a bad url (#1266 and Chrome keep it).
    #[test]
    fn a_backslash_at_the_end_is_an_escape() {
        assert!(!has_bad_url("url(a\\"), "url(a then a backslash at the end");
        assert!(!has_bad_url("u\\72l(a\\"), "the same with an escaped name");
        assert!(has_bad_url("url(a\\\nb)"), "a backslash before a newline is still no escape");
    }

    #[test]
    fn set_and_remove_write_what_the_js_copy_writes() {
        let decls = parse_decls("left: 1px; top: 2px");
        let text = |w: Option<Vec<Decl>>| w.map(|d| serialize_decls(&d));
        assert_eq!(text(set_property(&decls, "LEFT ", " 3px", "")), Some("left: 3px; top: 2px;".into()));
        assert_eq!(text(set_property(&decls, "left", "1px", "")), None);
        assert_eq!(
            text(set_property(&decls, "left", "1px", "IMPORTANT")),
            Some("left: 1px !important; top: 2px;".into())
        );
        assert_eq!(text(set_property(&decls, "color", "red", "")), Some("left: 1px; top: 2px; color: red;".into()));
        assert_eq!(text(set_property(&decls, "left", "  ", "")), Some("top: 2px;".into()));
        assert_eq!(text(set_property(&decls, "background-image", "url(a b)", "")), None);
        let (old, kept) = remove_property(&decls, "top");
        assert_eq!((old.as_str(), text(kept)), ("2px", Some("left: 1px;".into())));
        assert_eq!(remove_property(&decls, "width"), (String::new(), None));
    }

    #[test]
    fn a_written_list_is_kept_only_when_it_reads_back_as_itself() {
        for (text, exact) in [
            ("left: 1px; transform: rotate(9deg); --a-b_c: 2px; -webkit-x: hsla(1, 2%, 3%, 0.5);", true),
            ("content: 'a';", false),
            ("x: a\\;b;", false),
            ("transform: rotate(9deg];", false),
        ] {
            let decls = parse_decls(text);
            let exact_here = reads_back_as_itself(&decls);
            assert_eq!(exact_here, exact, "{text:?}");
            if exact_here {
                assert_eq!(parse_decls(&serialize_decls(&decls)), decls, "{text:?} reads back as itself");
            }
        }
        // Names only a write makes (setProperty takes any string): never kept.
        for name in ["a;b", "x:y", "--A", "--", "1a", "a b"] {
            let decls = vec![Decl { name: name.to_string(), value: "1px".into(), important: false }];
            assert!(!reads_back_as_itself(&decls), "{name:?}");
        }
        // Values only a write makes (the parse drops comments and closes what is open).
        for value in ["red /* c */", "rotate(9deg", "a)b", "x!y"] {
            let decls = vec![Decl { name: "color".into(), value: value.to_string(), important: false }];
            assert!(!reads_back_as_itself(&decls), "{value:?}");
        }
    }

    #[test]
    fn important_and_duplicates_read_as_the_js_copy_reads_them() {
        let d = parse_decls("color: red ! IMPORTANT; left: 1px; color: blue;--x:  y ;");
        assert_eq!(
            d,
            vec![
                Decl { name: "left".into(), value: "1px".into(), important: false },
                Decl { name: "color".into(), value: "blue".into(), important: false },
                Decl { name: "--x".into(), value: "y".into(), important: false },
            ]
        );
        assert_eq!(parse_decls("color: !important; top:"), vec![]);
        // JS whitespace, not Rust's: U+FEFF is trimmed, U+0085 is not.
        assert_eq!(parse_decls("\u{feff}left: 1px")[0].name, "left");
        assert_eq!(parse_decls("\u{85}left: 1px")[0].name, "\u{85}left");
    }
}
