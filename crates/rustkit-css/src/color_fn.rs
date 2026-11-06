//! `rgb()` / `rgba()` / `hsl()` / `hsla()` in both CSS Color 4 forms:
//!
//! - modern: `rgb(70 31 140 / 60%)` — space-separated channels, an optional
//!   `/ alpha`, numbers and percentages mixed freely, `none` for 0;
//! - legacy: `rgba(70, 31, 140, 0.6)` — commas, no `/`, no `none`, the
//!   three rgb channels all numbers or all percentages, and hsl's
//!   saturation and lightness percentages.
//!
//! As in Chromium 143, the first separator that is a comma makes the whole
//! function legacy (`rgb(1 2, 3)` is legacy; `rgb(1, 2 3)` is invalid).
//! Channels clamp to 0..=255 and round; alpha clamps to 0..=1; numbers out
//! of range clamp rather than refuse. `calc()`, `min()`, `max()`, `clamp()`,
//! `abs()`, `sign()`, `round()` and the constants `pi`, `e`, `infinity`,
//! `nan` work in any slot. Comments are skipped.
//!
//! A refusal is either a grammar error this parser is sure of, or something
//! valid CSS it does not read yet (relative colour `rgb(from …)`, `mod()`,
//! trig, ...). [`is_unreadable_color_function`] tells the two apart, so a
//! shorthand is dropped only for the first.
//! The checked answers are Chromium 143's (see `color_syntax_tests.rs`).

use crate::Color;

/// One argument slot's value.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Arg {
    Number(f32),
    Percent(f32),
    /// An angle in degrees (hue only).
    Degrees(f32),
    None,
}

/// `rgb(`/`rgba(`/`hsl(`/`hsla(` and what is inside the parentheses.
fn split_function(value: &str) -> Option<(bool, &str)> {
    let (is_hsl, rest) = if let Some(r) = value.strip_prefix("rgba(") {
        (false, r)
    } else if let Some(r) = value.strip_prefix("rgb(") {
        (false, r)
    } else if let Some(r) = value.strip_prefix("hsla(") {
        (true, r)
    } else if let Some(r) = value.strip_prefix("hsl(") {
        (true, r)
    } else {
        return None;
    };
    // An unclosed function at the end of a value closes itself.
    Some((is_hsl, rest.strip_suffix(')').unwrap_or(rest)))
}

/// Whether a lowercase `rgb()`/`hsl()` this parser refuses may still be a
/// valid colour it does not read yet: relative colour (`rgb(from red r g
/// b)`) or a nested function it does not evaluate (`mod()`, `sin()`, ...).
/// Such a value must not drop a shorthand; a plain grammar error must.
pub fn is_unreadable_color_function(value: &str) -> bool {
    let Some((_, inner)) = split_function(value) else {
        return false;
    };
    if parse_color_function(value).is_some() {
        return false;
    }
    let inner = strip_comments(inner);
    inner.trim_start().starts_with("from") || inner.contains('(')
}

fn strip_comments(s: &str) -> std::borrow::Cow<'_, str> {
    if !s.contains("/*") {
        return s.into();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(at) = rest.find("/*") {
        out.push_str(&rest[..at]);
        out.push(' ');
        rest = match rest[at + 2..].find("*/") {
            Some(end) => &rest[at + 2 + end + 2..],
            None => "",
        };
    }
    out.push_str(rest);
    out.into()
}

/// Parse a lowercase `rgb(`/`rgba(`/`hsl(`/`hsla(` function, or `None`.
pub(crate) fn parse_color_function(value: &str) -> Option<Color> {
    let (is_hsl, inner) = split_function(value)?;
    let inner = strip_comments(inner);
    let tokens = tokenize(&inner)?;

    // channel [sep] channel [sep] channel [, alpha | / alpha]: the first
    // comma makes it legacy, after which every separator must be a comma.
    let mut legacy = false;
    let mut args = Vec::new();
    let mut alpha = None;
    let mut i = 0;
    let arg_at = |i: usize| match tokens.get(i) {
        Some(Tok::Arg(s)) => Some(*s),
        _ => None,
    };
    for n in 0..3 {
        args.push(arg_at(i)?);
        i += 1;
        if n < 2 {
            match tokens.get(i) {
                Some(Tok::Comma) => {
                    legacy = true;
                    i += 1;
                }
                Some(Tok::Arg(_)) if !legacy => {}
                _ => return None,
            }
        }
    }
    match tokens.get(i) {
        None => {}
        Some(Tok::Comma) if legacy => {
            alpha = Some(arg_at(i + 1)?);
            i += 2;
        }
        Some(Tok::Slash) if !legacy => {
            alpha = Some(arg_at(i + 1)?);
            i += 2;
        }
        _ => return None,
    }
    if i != tokens.len() {
        return None;
    }

    let arg = |s: &str| {
        let a = parse_arg(s)?;
        // `none` is modern-syntax only.
        if legacy && a == Arg::None {
            return None;
        }
        Some(a)
    };
    let alpha = match alpha {
        None => 1.0,
        Some(s) => {
            let a = match arg(s)? {
                Arg::Number(n) => n,
                Arg::Percent(p) => p / 100.0,
                Arg::None => 0.0,
                Arg::Degrees(_) => return None,
            };
            if a.is_nan() {
                0.0
            } else {
                a.clamp(0.0, 1.0)
            }
        }
    };

    if is_hsl {
        let hue = match arg(args[0])? {
            Arg::Number(n) | Arg::Degrees(n) => n as f64,
            Arg::None => 0.0,
            Arg::Percent(_) => return None,
        };
        let hue = if hue.is_finite() { hue } else { 0.0 };
        // Saturation and lightness: percentages, or (modern only) bare
        // numbers meaning the same.
        let pct = |s: &str| match arg(s)? {
            // In f64: `90%` as f32 / 100 is 0.89999998, a hair under the .5
            // that Chromium rounds up (230 for hsl(0 0% 90%)).
            Arg::Percent(p) => Some(p as f64 / 100.0),
            Arg::Number(p) if !legacy => Some(p as f64 / 100.0),
            Arg::None => Some(0.0),
            _ => None,
        };
        let (s, l) = (pct(args[1])?, pct(args[2])?);
        let finite = |v: f64| if v.is_nan() { 0.0 } else { v };
        let (r, g, b) = crate::hsl_to_rgb(hue, finite(s), finite(l));
        return Some(Color::new(r, g, b, alpha));
    }

    let channels = [arg(args[0])?, arg(args[1])?, arg(args[2])?];
    if legacy {
        let all_pct = channels.iter().all(|c| matches!(c, Arg::Percent(_)));
        let all_num = channels.iter().all(|c| matches!(c, Arg::Number(_)));
        if !all_pct && !all_num {
            return None;
        }
    }
    let mut rgb = [0u8; 3];
    for (out, c) in rgb.iter_mut().zip(channels) {
        let v = match c {
            Arg::Number(n) => n,
            Arg::Percent(p) => p * 255.0 / 100.0,
            Arg::None => 0.0,
            Arg::Degrees(_) => return None,
        };
        // NaN (`calc(NaN)`) is 0, as in Chromium.
        *out = if v.is_nan() {
            0
        } else {
            v.clamp(0.0, 255.0).round() as u8
        };
    }
    Some(Color::new(rgb[0], rgb[1], rgb[2], alpha))
}

#[derive(Debug)]
enum Tok<'a> {
    Arg(&'a str),
    Comma,
    Slash,
}

/// Split the function's inside into arguments and `,` / `/` separators,
/// keeping nested `( … )` (a `calc()`) whole.
fn tokenize(inner: &str) -> Option<Vec<Tok<'_>>> {
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut start: Option<usize> = None;
    for (i, ch) in inner.char_indices() {
        if depth > 0 {
            match ch {
                '(' => depth += 1,
                ')' => depth -= 1,
                _ => {}
            }
            continue;
        }
        let sep = match ch {
            ',' => Some(Tok::Comma),
            '/' => Some(Tok::Slash),
            c if c.is_whitespace() => None,
            '(' => {
                depth += 1;
                start.get_or_insert(i);
                continue;
            }
            ')' => return None,
            _ => {
                start.get_or_insert(i);
                continue;
            }
        };
        if let Some(s) = start.take() {
            out.push(Tok::Arg(&inner[s..i]));
        }
        if let Some(sep) = sep {
            out.push(sep);
        }
    }
    if depth != 0 {
        return None;
    }
    if let Some(s) = start {
        out.push(Tok::Arg(&inner[s..]));
    }
    Some(out)
}

fn parse_arg(s: &str) -> Option<Arg> {
    if s == "none" {
        return Some(Arg::None);
    }
    if s.contains('(') {
        // calc(), min(), clamp(), ...
        return calc::eval(s);
    }
    dimension(s)
}

/// A number, percentage or angle token.
fn dimension(s: &str) -> Option<Arg> {
    let split = s
        .find(|c: char| c.is_ascii_alphabetic() && c != 'e' || c == '%')
        .unwrap_or(s.len());
    // `1e2` is a number but `1em` is not; a trailing `e` belongs to a unit.
    let (num, unit) = s.split_at(split);
    let (num, unit) = match num.strip_suffix('e') {
        Some(n) if !unit.is_empty() => (n, &s[n.len()..]),
        _ => (num, unit),
    };
    let n = number(num)?;
    Some(match unit {
        "" => Arg::Number(n),
        "%" => Arg::Percent(n),
        "deg" => Arg::Degrees(n),
        "rad" => Arg::Degrees(n.to_degrees()),
        "grad" => Arg::Degrees(n * 9.0 / 10.0),
        "turn" => Arg::Degrees(n * 360.0),
        _ => return None,
    })
}

/// A CSS `<number>`: `[+-]? (digits [. digits] | . digits) [e [+-] digits]`.
/// `5.` and `1e` are not numbers (Chromium refuses `rgb(5. 0 0)`); one out
/// of f32 range clamps to it, as Chromium clamps `rgb(1e40 0 0)` to 255.
fn number(s: &str) -> Option<f32> {
    let b = s.as_bytes();
    let mut i = usize::from(matches!(b.first(), Some(b'+' | b'-')));
    let digits = |i: &mut usize| {
        let start = *i;
        while b.get(*i).is_some_and(u8::is_ascii_digit) {
            *i += 1;
        }
        *i > start
    };
    let int = digits(&mut i);
    if b.get(i) == Some(&b'.') {
        i += 1;
        if !digits(&mut i) {
            return None;
        }
    } else if !int {
        return None;
    }
    if matches!(b.get(i), Some(b'e' | b'E')) {
        i += 1;
        if matches!(b.get(i), Some(b'+' | b'-')) {
            i += 1;
        }
        if !digits(&mut i) {
            return None;
        }
    }
    if i != b.len() {
        return None;
    }
    let n = s.parse::<f64>().ok()?;
    Some(n.clamp(-(f32::MAX as f64), f32::MAX as f64) as f32)
}

/// `calc()` and the math functions Chromium takes in a colour channel:
/// `+ - * /`, parentheses, `calc() min() max() clamp() abs() sign()
/// round(x[, step])` and `pi e infinity nan`. Sums need one kind; products
/// need a plain number on one side; division needs a number on the right.
/// Anything else (`mod()`, trig, `round(up, …)`) is refused.
mod calc {
    use super::{dimension, Arg};

    pub(super) fn eval(expr: &str) -> Option<Arg> {
        let toks = lex(expr)?;
        let mut pos = 0;
        let v = sum(&toks, &mut pos)?;
        (pos == toks.len()).then_some(v)
    }

    #[derive(Clone, Copy, Debug, PartialEq)]
    enum F {
        Calc,
        Min,
        Max,
        Clamp,
        Abs,
        Sign,
        Round,
    }

    #[derive(Clone, Copy, Debug, PartialEq)]
    enum T {
        V(Arg),
        Op(u8),
        Open,
        Func(F),
        Comma,
        Close,
    }

    fn lex(s: &str) -> Option<Vec<T>> {
        let b = s.as_bytes();
        let mut out = Vec::new();
        let mut i = 0;
        while i < b.len() {
            let c = b[i];
            let operand_expected = matches!(
                out.last(),
                None | Some(T::Op(_)) | Some(T::Open) | Some(T::Func(_)) | Some(T::Comma)
            );
            if c.is_ascii_whitespace() {
                i += 1;
            } else if c == b'(' {
                out.push(T::Open);
                i += 1;
            } else if c == b')' {
                out.push(T::Close);
                i += 1;
            } else if c == b',' {
                out.push(T::Comma);
                i += 1;
            } else if c.is_ascii_alphabetic()
                || (operand_expected
                    && c == b'-'
                    && b.get(i + 1).is_some_and(u8::is_ascii_alphabetic))
            {
                let start = i;
                i += 1;
                while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'-') {
                    i += 1;
                }
                let word = &s[start..i];
                if b.get(i) == Some(&b'(') {
                    i += 1;
                    out.push(T::Func(match word {
                        "calc" => F::Calc,
                        "min" => F::Min,
                        "max" => F::Max,
                        "clamp" => F::Clamp,
                        "abs" => F::Abs,
                        "sign" => F::Sign,
                        "round" => F::Round,
                        _ => return None,
                    }));
                } else {
                    out.push(T::V(Arg::Number(match word {
                        "pi" => std::f32::consts::PI,
                        "e" => std::f32::consts::E,
                        "infinity" => f32::INFINITY,
                        "-infinity" => f32::NEG_INFINITY,
                        "nan" => f32::NAN,
                        _ => return None,
                    })));
                }
            } else if c.is_ascii_digit()
                || c == b'.'
                || (operand_expected && matches!(c, b'+' | b'-'))
            {
                let start = i;
                i += 1;
                while i < b.len() {
                    let d = b[i];
                    let exp_sign = matches!(d, b'+' | b'-') && matches!(b[i - 1], b'e' | b'E');
                    if d.is_ascii_alphanumeric() || d == b'.' || d == b'%' || exp_sign {
                        i += 1;
                    } else {
                        break;
                    }
                }
                out.push(T::V(dimension(&s[start..i])?));
            } else if matches!(c, b'+' | b'-' | b'*' | b'/') {
                out.push(T::Op(c));
                i += 1;
            } else {
                return None;
            }
        }
        Some(out)
    }

    fn same_kind(a: Arg, b: Arg) -> bool {
        std::mem::discriminant(&a) == std::mem::discriminant(&b)
    }

    fn value(a: Arg) -> f32 {
        match a {
            Arg::Number(n) | Arg::Percent(n) | Arg::Degrees(n) => n,
            Arg::None => 0.0,
        }
    }

    fn with(a: Arg, n: f32) -> Arg {
        match a {
            Arg::Number(_) => Arg::Number(n),
            Arg::Percent(_) => Arg::Percent(n),
            Arg::Degrees(_) => Arg::Degrees(n),
            Arg::None => Arg::None,
        }
    }

    fn sum(t: &[T], pos: &mut usize) -> Option<Arg> {
        let mut acc = product(t, pos)?;
        while let Some(T::Op(op @ (b'+' | b'-'))) = t.get(*pos).copied() {
            *pos += 1;
            let rhs = product(t, pos)?;
            if !same_kind(acc, rhs) {
                return None;
            }
            let sign = if op == b'+' { 1.0 } else { -1.0 };
            acc = with(acc, value(acc) + sign * value(rhs));
        }
        Some(acc)
    }

    fn product(t: &[T], pos: &mut usize) -> Option<Arg> {
        let mut acc = atom(t, pos)?;
        while let Some(T::Op(op @ (b'*' | b'/'))) = t.get(*pos).copied() {
            *pos += 1;
            let rhs = atom(t, pos)?;
            acc = match (op, acc, rhs) {
                (b'*', a, Arg::Number(k)) | (b'*', Arg::Number(k), a) => with(a, value(a) * k),
                (b'/', a, Arg::Number(k)) => with(a, value(a) / k),
                _ => return None,
            };
        }
        Some(acc)
    }

    fn atom(t: &[T], pos: &mut usize) -> Option<Arg> {
        match *t.get(*pos)? {
            T::V(v) => {
                *pos += 1;
                Some(v)
            }
            T::Open => {
                *pos += 1;
                let v = sum(t, pos)?;
                if t.get(*pos) != Some(&T::Close) {
                    return None;
                }
                *pos += 1;
                Some(v)
            }
            T::Func(f) => {
                *pos += 1;
                let mut args = vec![sum(t, pos)?];
                while t.get(*pos) == Some(&T::Comma) {
                    *pos += 1;
                    args.push(sum(t, pos)?);
                }
                if t.get(*pos) != Some(&T::Close) {
                    return None;
                }
                *pos += 1;
                apply(f, &args)
            }
            _ => None,
        }
    }

    fn apply(f: F, args: &[Arg]) -> Option<Arg> {
        let first = *args.first()?;
        if !args.iter().all(|a| same_kind(*a, first)) {
            return None;
        }
        let v: Vec<f32> = args.iter().map(|a| value(*a)).collect();
        Some(match (f, v.as_slice()) {
            (F::Calc, [_]) => first,
            (F::Min, _) => with(first, v.iter().copied().fold(f32::INFINITY, f32::min)),
            (F::Max, _) => with(first, v.iter().copied().fold(f32::NEG_INFINITY, f32::max)),
            (F::Clamp, [lo, x, hi]) => with(first, x.min(*hi).max(*lo)),
            (F::Abs, [x]) => with(first, x.abs()),
            (F::Sign, [x]) => Arg::Number(if *x == 0.0 { 0.0 } else { x.signum() }),
            // Nearest, a half rounding up (towards +infinity).
            (F::Round, [x]) => with(first, (x + 0.5).floor()),
            (F::Round, [x, step]) if *step != 0.0 => with(first, ((x / step) + 0.5).floor() * step),
            _ => return None,
        })
    }
}
