//! The colour-matrix functions of CSS `filter` (Filter Effects 1 §13):
//! `grayscale()`, `sepia()`, `saturate()`, `hue-rotate()`, `invert()`,
//! `opacity()`, `brightness()` and `contrast()`, each the 4x5 matrix of its
//! equivalent `feColorMatrix` / `feComponentTransfer`, composed in order.
//!
//! `blur()`, `drop-shadow()` and `url()` are not colour matrices; they are
//! skipped, so the rest of the list still applies. For `url()` that is what
//! Chrome 148 paints when the reference is missing, external or not a
//! `<filter>`; a list mixing in `blur()` or `drop-shadow()` paints only its
//! colour functions.
//!
//! Invalid values give `None`, as the cascade would drop the declaration:
//! a non-finite or negative amount, an unknown function, or whitespace
//! between a function's name and its `(`.

/// A 4x5 row-major colour matrix: `[r' g' b' a']` from `[r g b a 1]`.
pub type ColorMatrix = [f32; 20];

const IDENTITY: ColorMatrix = [
    1.0, 0.0, 0.0, 0.0, 0.0, //
    0.0, 1.0, 0.0, 0.0, 0.0, //
    0.0, 0.0, 1.0, 0.0, 0.0, //
    0.0, 0.0, 0.0, 1.0, 0.0,
];

/// The colour matrix of a `filter` value, or `None` when it has none to
/// apply (empty, `none`, only skipped functions, identity) or is invalid
/// (a declaration the cascade would have dropped).
pub fn filter_color_matrix(value: &str) -> Option<ColorMatrix> {
    let mut total = IDENTITY;
    let mut rest = value.trim();
    if rest.is_empty() || rest.eq_ignore_ascii_case("none") {
        return None;
    }
    while !rest.is_empty() {
        let open = rest.find('(')?;
        // A function token is `name(` (CSS Syntax 3 §4.3.4): an empty name
        // or whitespace before the paren is not a filter function.
        let name = &rest[..open];
        if name.is_empty() || name.contains(|c: char| c.is_whitespace()) {
            return None;
        }
        let name = name.to_ascii_lowercase();
        let close = matching_paren(rest, open)?;
        let arg = rest[open + 1..close].trim();
        rest = rest[close + 1..].trim_start();
        if let Some(m) = function_matrix(&name, arg)? {
            total = compose(&m, &total);
        }
    }
    (total != IDENTITY).then_some(total)
}

/// One step of a filter list painted on pixels (`backdrop-filter`): a
/// Gaussian blur of that standard deviation in CSS px (Filter Effects 1
/// §13 `blur()`), or one colour function's matrix. Each colour function is
/// its own step because Chromium clamps to [0, 1] after each one:
/// `brightness(2) saturate(0)` greys a clamped colour.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FilterStep {
    Blur(f32),
    ColorMatrix(ColorMatrix),
}

/// The steps of a filter list in order, for `backdrop-filter`. Empty for
/// `none` and for a valid list with nothing to paint (`blur(0)`, identity
/// colour functions, `drop-shadow()` and `url()`, which are not painted);
/// `None` when the value is invalid (an unknown function, a negative
/// amount or blur, a percentage blur), which the cascade drops whole.
/// `font_px` and `root_px` resolve `em` and `rem` blur lengths.
pub fn filter_steps(value: &str, font_px: f32, root_px: f32) -> Option<Vec<FilterStep>> {
    let mut steps = Vec::new();
    let mut rest = value.trim();
    if rest.is_empty() || rest.eq_ignore_ascii_case("none") {
        return Some(steps);
    }
    while !rest.is_empty() {
        let open = rest.find('(')?;
        let name = &rest[..open];
        if name.is_empty() || name.contains(|c: char| c.is_whitespace() || c == ',') {
            return None;
        }
        let name = name.to_ascii_lowercase();
        let close = matching_paren(rest, open)?;
        let arg = rest[open + 1..close].trim();
        rest = rest[close + 1..].trim_start();
        if name == "blur" {
            let sigma = if arg.is_empty() { 0.0 } else { blur_length(arg, font_px, root_px)? };
            if sigma > 0.0 {
                steps.push(FilterStep::Blur(sigma));
            }
            continue;
        }
        if let Some(m) = function_matrix(&name, arg)? {
            if m != IDENTITY {
                steps.push(FilterStep::ColorMatrix(m));
            }
        }
    }
    Some(steps)
}

/// A `blur()` `<length>` in px: non-negative, no percentage; a bare `0`.
fn blur_length(arg: &str, font_px: f32, root_px: f32) -> Option<f32> {
    let split = arg
        .find(|c: char| !(c.is_ascii_digit() || matches!(c, '.' | '-' | '+' | 'e' | 'E')))
        .unwrap_or(arg.len());
    // `e` belongs to the number only when a digit or sign follows it.
    let (num, unit) = match arg[..split].rfind(['e', 'E']) {
        Some(i) if !arg[i + 1..split].starts_with(|c: char| c.is_ascii_digit() || c == '-' || c == '+') => {
            arg.split_at(i)
        }
        _ => arg.split_at(split),
    };
    let v = number(num)?;
    let per = match unit.to_ascii_lowercase().as_str() {
        "" if v == 0.0 => 0.0,
        "px" => 1.0,
        "em" => font_px,
        "rem" => root_px,
        "pt" => 96.0 / 72.0,
        "pc" => 16.0,
        "in" => 96.0,
        "cm" => 96.0 / 2.54,
        "mm" => 96.0 / 25.4,
        "q" => 96.0 / 101.6,
        _ => return None,
    };
    (v >= 0.0).then_some(v * per)
}

fn matching_paren(s: &str, open: usize) -> Option<usize> {
    let mut depth = 0;
    for (i, c) in s.char_indices().skip_while(|&(i, _)| i < open) {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

/// `Some(Some(m))` a matrix, `Some(None)` a valid function that is not a
/// colour matrix, `None` invalid.
fn function_matrix(name: &str, arg: &str) -> Option<Option<ColorMatrix>> {
    if matches!(name, "blur" | "drop-shadow" | "url") {
        return Some(None);
    }
    if name == "hue-rotate" {
        let deg = if arg.is_empty() { 0.0 } else { angle_deg(arg)? };
        return Some(Some(hue_rotate(deg)));
    }
    let amount = if arg.is_empty() { 1.0 } else { amount(arg)? };
    if amount < 0.0 {
        return None;
    }
    // These four clamp to 1 (§13: "values of amount over 100% are allowed
    // but UAs must clamp the values to 1").
    let unit = amount.min(1.0);
    let m = match name {
        "grayscale" => {
            let s = 1.0 - unit;
            [
                0.2126 + 0.7874 * s, 0.7152 - 0.7152 * s, 0.0722 - 0.0722 * s, 0.0, 0.0,
                0.2126 - 0.2126 * s, 0.7152 + 0.2848 * s, 0.0722 - 0.0722 * s, 0.0, 0.0,
                0.2126 - 0.2126 * s, 0.7152 - 0.7152 * s, 0.0722 + 0.9278 * s, 0.0, 0.0,
                0.0, 0.0, 0.0, 1.0, 0.0,
            ]
        }
        "sepia" => {
            let s = 1.0 - unit;
            [
                0.393 + 0.607 * s, 0.769 - 0.769 * s, 0.189 - 0.189 * s, 0.0, 0.0,
                0.349 - 0.349 * s, 0.686 + 0.314 * s, 0.168 - 0.168 * s, 0.0, 0.0,
                0.272 - 0.272 * s, 0.534 - 0.534 * s, 0.131 + 0.869 * s, 0.0, 0.0,
                0.0, 0.0, 0.0, 1.0, 0.0,
            ]
        }
        "saturate" => {
            let s = amount;
            [
                0.213 + 0.787 * s, 0.715 - 0.715 * s, 0.072 - 0.072 * s, 0.0, 0.0,
                0.213 - 0.213 * s, 0.715 + 0.285 * s, 0.072 - 0.072 * s, 0.0, 0.0,
                0.213 - 0.213 * s, 0.715 - 0.715 * s, 0.072 + 0.928 * s, 0.0, 0.0,
                0.0, 0.0, 0.0, 1.0, 0.0,
            ]
        }
        "invert" => {
            let d = 1.0 - 2.0 * unit;
            [
                d, 0.0, 0.0, 0.0, unit, //
                0.0, d, 0.0, 0.0, unit, //
                0.0, 0.0, d, 0.0, unit, //
                0.0, 0.0, 0.0, 1.0, 0.0,
            ]
        }
        "opacity" => {
            let mut m = IDENTITY;
            m[18] = unit;
            m
        }
        "brightness" => {
            let mut m = IDENTITY;
            m[0] = amount;
            m[6] = amount;
            m[12] = amount;
            m
        }
        "contrast" => {
            let o = 0.5 - 0.5 * amount;
            [
                amount, 0.0, 0.0, 0.0, o, //
                0.0, amount, 0.0, 0.0, o, //
                0.0, 0.0, amount, 0.0, o, //
                0.0, 0.0, 0.0, 1.0, 0.0,
            ]
        }
        _ => return None,
    };
    Some(Some(m))
}

/// `<number> | <percentage>`.
fn amount(arg: &str) -> Option<f32> {
    match arg.strip_suffix('%') {
        Some(pct) => number(pct.trim()).map(|v| v / 100.0),
        None => number(arg),
    }
}

/// A CSS `<number>`. `f32::from_str` also takes `inf`, `infinity` and
/// `NaN`, which in CSS are identifiers, so they void the declaration. A
/// finite value past `f32`'s range saturates rather than becoming infinite.
fn number(s: &str) -> Option<f32> {
    let v = s.parse::<f64>().ok().filter(|v| v.is_finite())?;
    Some(v.clamp(f32::MIN as f64, f32::MAX as f64) as f32)
}

/// `<angle> | <zero>` in degrees.
fn angle_deg(arg: &str) -> Option<f32> {
    for (unit, per) in [("deg", 1.0), ("grad", 0.9), ("rad", 180.0 / std::f32::consts::PI), ("turn", 360.0)] {
        if let Some(v) = arg.strip_suffix(unit) {
            return number(v.trim()).map(|v| v * per);
        }
    }
    (number(arg)? == 0.0).then_some(0.0)
}

fn hue_rotate(deg: f32) -> ColorMatrix {
    let (sin, cos) = deg.to_radians().sin_cos();
    [
        0.213 + cos * 0.787 - sin * 0.213,
        0.715 - cos * 0.715 - sin * 0.715,
        0.072 - cos * 0.072 + sin * 0.928,
        0.0,
        0.0,
        0.213 - cos * 0.213 + sin * 0.143,
        0.715 + cos * 0.285 + sin * 0.140,
        0.072 - cos * 0.072 - sin * 0.283,
        0.0,
        0.0,
        0.213 - cos * 0.213 - sin * 0.787,
        0.715 - cos * 0.715 + sin * 0.715,
        0.072 + cos * 0.928 + sin * 0.072,
        0.0,
        0.0,
        0.0, 0.0, 0.0, 1.0, 0.0,
    ]
}

/// `a` after `b`: the matrix of applying `b`, then `a`.
fn compose(a: &ColorMatrix, b: &ColorMatrix) -> ColorMatrix {
    let mut out = [0.0; 20];
    for r in 0..4 {
        for c in 0..5 {
            let mut v: f32 = (0..4).map(|k| a[r * 5 + k] * b[k * 5 + c]).sum();
            if c == 4 {
                v += a[r * 5 + 4];
            }
            out[r * 5 + c] = v;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Apply to an opaque or translucent colour the way the painter does.
    fn apply(m: &ColorMatrix, c: [f32; 4]) -> [u8; 4] {
        let mut o = [0u8; 4];
        for r in 0..4 {
            let v: f32 = (0..4).map(|k| m[r * 5 + k] * c[k]).sum::<f32>() + m[r * 5 + 4];
            o[r] = (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        }
        o
    }

    const RED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];

    #[test]
    fn functions_match_chrome_on_red() {
        // Chrome 148 renders of docs/diagnostics/2026-10-23/css_filter.html.
        let cases = [
            ("hue-rotate(120deg)", [0, 113, 0, 255]),
            ("grayscale(1)", [54, 54, 54, 255]),
            ("sepia(1)", [100, 89, 69, 255]),
            ("saturate(0) contrast(0)", [128, 128, 128, 255]),
            ("opacity(0)", [255, 0, 0, 0]),
            ("brightness(50%)", [128, 0, 0, 255]),
        ];
        for (value, want) in cases {
            let m = filter_color_matrix(value).unwrap_or_else(|| panic!("{value}: no matrix"));
            assert_eq!(apply(&m, RED), want, "{value}");
        }
        let m = filter_color_matrix("invert(1)").unwrap();
        assert_eq!(apply(&m, [1.0, 0.0, 1.0, 1.0]), [0, 255, 0, 255]);
    }

    #[test]
    fn functions_compose_in_order() {
        // invert then brightness(.5): magenta -> green -> half green.
        let m = filter_color_matrix("invert(100%) brightness(.5)").unwrap();
        assert_eq!(apply(&m, [1.0, 0.0, 1.0, 1.0]), [0, 128, 0, 255]);
        // brightness(.5) then invert: magenta -> half magenta -> (127,255,127).
        let m = filter_color_matrix("brightness(.5) invert(1)").unwrap();
        assert_eq!(apply(&m, [1.0, 0.0, 1.0, 1.0]), [128, 255, 128, 255]);
    }

    #[test]
    fn nothing_to_apply_or_invalid_is_none() {
        for value in [
            "", "none", "blur(4px)", "grayscale(0)", "hue-rotate(0)", "hue-rotate(1turn)",
            "grayscale(-1)", "frobnicate(1)", "grayscale(1", "hue-rotate(10)",
        ] {
            let got = filter_color_matrix(value);
            let identity_ish = got.map(|m| m.iter().zip(IDENTITY).all(|(a, b)| (a - b).abs() < 1e-4));
            assert!(got.is_none() || identity_ish == Some(true), "{value}: {got:?}");
        }
        // A skipped function does not void the rest.
        assert!(filter_color_matrix("blur(2px) grayscale(1)").is_some());
    }

    #[test]
    fn follow_up_nits_from_790_void_the_whole_list() {
        // #790 R1 nits 1-3: a value the cascade would drop gives no filter.
        let applied = [
            // 1. Not a CSS <number>: `inf`, `infinity` and `NaN` are idents.
            "grayscale(inf)", "brightness(nan)", "brightness(NaN)", "contrast(infinity)",
            "saturate(-inf)", "invert(inf%)", "hue-rotate(infdeg)", "hue-rotate(NaNdeg)",
            "grayscale(1) brightness(nan)",
            // 3. A function token is `name(`: whitespace before the paren is
            //    an ident and a block, not a filter function.
            "grayscale (1)", "invert\t(1)", "sepia(1) contrast (2)", "(1)",
        ]
        .into_iter()
        .filter(|value| filter_color_matrix(value).is_some())
        .collect::<Vec<_>>();
        assert!(applied.is_empty(), "applied: {applied:?}");
        // Still valid: adjacent functions, exponents, signs, case.
        for value in ["grayscale(1)sepia(1)", "brightness(1e-1)", "brightness(+.5)", "brightness(1e39)", "INVERT(1)", " invert(1) "] {
            assert!(filter_color_matrix(value).is_some(), "{value}");
        }

        // Nit 2 asked for url() to void the list, as Filter Effects 1 §5
        // reads. Chrome 148 does not: a url() naming a missing element, an
        // external document or a non-<filter> is passed through and the rest
        // of the list applies (docs/diagnostics/2026-10-27/
        // css_filter_clip_path.html, box d: rgb(51,51,51) for
        // `url(#missing) grayscale(1)` on #e00). That is what skipping does,
        // so url() stays skipped; only a url() to a real <filter> differs.
        let gray = filter_color_matrix("grayscale(1)");
        for value in ["url(#missing) grayscale(1)", "grayscale(1) url(x.svg#f)"] {
            assert_eq!(filter_color_matrix(value), gray, "{value}");
        }
        assert_eq!(filter_color_matrix("url(#missing)"), None);
    }

    #[test]
    fn filter_steps_keep_the_order_and_void_unknown_functions() {
        let steps = filter_steps("saturate(180%) blur(20px)", 16.0, 16.0).unwrap();
        assert!(matches!(steps[..], [FilterStep::ColorMatrix(_), FilterStep::Blur(b)] if b == 20.0), "{steps:?}");
        assert_eq!(filter_steps("blur(0.5em)", 16.0, 10.0), Some(vec![FilterStep::Blur(8.0)]));
        assert_eq!(filter_steps("blur(2rem)", 16.0, 10.0), Some(vec![FilterStep::Blur(20.0)]));
        assert_eq!(filter_steps("blur(2px)blur(3px)", 16.0, 16.0).map(|s| s.len()), Some(2));
        for empty in ["none", "", "blur()", "blur(0)", "blur(0px)", "saturate(1)", "drop-shadow(1px 1px red)", "url(#x)"] {
            assert_eq!(filter_steps(empty, 16.0, 16.0), Some(vec![]), "{empty}");
        }
        for invalid in ["blur(2px) foo(1)", "blur(-1px)", "blur(10%)", "blur(2)", "saturate(-1)", "blur (2px)", "blur(2px", "inherit"] {
            assert_eq!(filter_steps(invalid, 16.0, 16.0), None, "{invalid}");
        }
    }
}
