//! The computed values `getComputedStyle` serializes beyond the box model, from
//! what `ComputedStyle` stores (Z plan C9 item 0). Nothing here is read by layout
//! or paint, and nothing is stored for it.
//!
//! A row is published only when the style differs from the initial value: the
//! initial values are one table, `COMPUTED_INITIAL` in rustkit-bindings' script,
//! so a plain element costs no text. A value the engine cannot know (a used grid
//! track, a viewport unit here with no viewport) publishes an empty row: script
//! reads `''`, not a guess. `computed_read_cases.json` holds Chromium's answers.

use rustkit_css::{
    self as css, BackgroundImage, Gradient, GradientDirection, Length, TimingFunction, TransformOp,
};

use crate::{css_color, css_keyword, css_px, LayoutBox};

/// A number the way Blink prints one in a computed value: six significant digits.
fn num(v: f64) -> String {
    // Trigonometry leaves noise: cos(90deg) is 6e-17, not 0. Blink rounds it.
    let v = if (v - v.round()).abs() < 1e-6 { v.round() } else { v };
    if v == 0.0 || !v.is_finite() {
        return "0".into();
    }
    let digits = (5 - v.abs().log10().floor() as i32).max(0) as usize;
    let mut s = format!("{v:.digits$}");
    if s.contains('.') {
        s = s.trim_end_matches('0').trim_end_matches('.').to_string();
    }
    if s == "-0" {
        "0".into()
    } else {
        s
    }
}

fn numf(v: f32) -> String {
    // Through the shortest decimal: 0.1f32 as an f64 is 0.100000001.
    num(v.to_string().parse().unwrap_or(0.0))
}

thread_local! {
    static ROOT_FONT_PX: std::cell::Cell<f32> = const { std::cell::Cell::new(16.0) };
}

/// The root element's font size, for `rem`, set before a snapshot is written.
pub(crate) fn set_root_font(style: &css::ComputedStyle) {
    let px = match style.font_size {
        Length::Px(px) => px,
        Length::Em(em) | Length::Rem(em) => em * 16.0,
        // The root's own style keeps `62.5%` as written: against the 16px default.
        Length::Percent(p) => p / 100.0 * 16.0,
        _ => 16.0,
    };
    ROOT_FONT_PX.with(|r| r.set(px));
}

pub(crate) fn root_font() -> f32 {
    ROOT_FONT_PX.with(|r| r.get())
}

fn keyword(debug: impl std::fmt::Debug) -> String {
    css_keyword(&format!("{debug:?}"))
}

/// A computed `<length-percentage>`; `None` where this box has no answer.
pub(crate) fn length(l: &Length, font_px: f32) -> Option<String> {
    Some(match l {
        Length::Px(p) => css_px(*p),
        Length::Em(e) => css_px(e * font_px),
        Length::Rem(r) => css_px(r * root_font()),
        Length::Percent(p) => format!("{}%", numf(*p)),
        Length::Zero => "0px".into(),
        Length::Auto => "auto".into(),
        Length::FitContent => "fit-content".into(),
        _ => return None,
    })
}

fn pair(a: &str, b: &str) -> String {
    if a == b {
        a.to_string()
    } else {
        format!("{a} {b}")
    }
}

/// The 1 to 4 value form: a side drops when the expansion would refill it.
fn sides(top: &str, right: &str, bottom: &str, left: &str) -> String {
    if top == right && right == bottom && bottom == left {
        top.to_string()
    } else if top == bottom && right == left {
        format!("{top} {right}")
    } else if right == left {
        format!("{top} {right} {bottom}")
    } else {
        format!("{top} {right} {bottom} {left}")
    }
}

fn transform_matrix(ops: &[TransformOp], font_px: f32, w: f32, h: f32) -> String {
    let mut m = [1.0f64, 0.0, 0.0, 1.0, 0.0, 0.0];
    let px = |l: &Length, size: f32| f64::from(l.to_px(font_px, root_font(), size));
    let tan = |d: f32| f64::from(d).to_radians().tan();
    for op in ops {
        let n = match op {
            TransformOp::Translate(x, y) => [1.0, 0.0, 0.0, 1.0, px(x, w), px(y, h)],
            TransformOp::TranslateX(x) => [1.0, 0.0, 0.0, 1.0, px(x, w), 0.0],
            TransformOp::TranslateY(y) => [1.0, 0.0, 0.0, 1.0, 0.0, px(y, h)],
            TransformOp::Scale(x, y) => [f64::from(*x), 0.0, 0.0, f64::from(*y), 0.0, 0.0],
            TransformOp::ScaleX(x) => [f64::from(*x), 0.0, 0.0, 1.0, 0.0, 0.0],
            TransformOp::ScaleY(y) => [1.0, 0.0, 0.0, f64::from(*y), 0.0, 0.0],
            TransformOp::Rotate(d) => {
                let (s, c) = f64::from(*d).to_radians().sin_cos();
                [c, s, -s, c, 0.0, 0.0]
            }
            TransformOp::SkewX(d) => [1.0, 0.0, tan(*d), 1.0, 0.0, 0.0],
            TransformOp::SkewY(d) => [1.0, tan(*d), 0.0, 1.0, 0.0, 0.0],
            TransformOp::Skew(x, y) => [1.0, tan(*y), tan(*x), 1.0, 0.0, 0.0],
            TransformOp::Matrix(a, b, c, d, e, f) => [*a, *b, *c, *d, *e, *f].map(f64::from),
        };
        m = [
            m[0] * n[0] + m[2] * n[1],
            m[1] * n[0] + m[3] * n[1],
            m[0] * n[2] + m[2] * n[3],
            m[1] * n[2] + m[3] * n[3],
            m[0] * n[4] + m[2] * n[5] + m[4],
            m[1] * n[4] + m[3] * n[5] + m[5],
        ];
    }
    format!("matrix({})", m.map(num).join(", "))
}

/// `name(args)` at the top level of a value, in order; `None` for anything else.
fn functions(value: &str) -> Option<Vec<(String, String)>> {
    let mut out = Vec::new();
    let mut rest = value.trim();
    while !rest.is_empty() {
        let open = rest.find('(')?;
        let (mut depth, mut close) = (0, None);
        for (i, c) in rest.char_indices().skip(open) {
            match c {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        close = Some(i);
                        break;
                    }
                }
                _ => {}
            }
        }
        let close = close?;
        out.push((
            rest[..open].trim().to_ascii_lowercase(),
            rest[open + 1..close].trim().to_string(),
        ));
        rest = rest[close + 1..].trim_start();
    }
    Some(out)
}

/// One length in a raw value: `0` and `em` come out as px, the rest as written.
fn raw_length(token: &str, font_px: f32) -> String {
    let t = token.to_ascii_lowercase();
    if let Some(e) = t.strip_suffix("rem").and_then(|n| n.parse::<f32>().ok()) {
        return css_px(e * root_font());
    }
    if let Some(e) = t.strip_suffix("em").and_then(|n| n.parse::<f32>().ok()) {
        return css_px(e * font_px);
    }
    if let Some(p) = t.strip_suffix("px").and_then(|n| n.parse::<f32>().ok()) {
        return css_px(p);
    }
    if let Some(p) = t.strip_suffix('%').and_then(|n| n.parse::<f32>().ok()) {
        return format!("{}%", numf(p));
    }
    match t.parse::<f32>() {
        Ok(n) => css_px(n),
        Err(_) => t,
    }
}

/// `at <position>`: keywords come out as percentages, in x y order.
fn raw_position(tokens: &[&str], font_px: f32) -> String {
    let horizontal = |t: &str| matches!(t, "left" | "right");
    let vertical = |t: &str| matches!(t, "top" | "bottom");
    let word = |t: &str| match t {
        "left" | "top" => "0%".to_string(),
        "right" | "bottom" => "100%".to_string(),
        "center" => "50%".to_string(),
        t => raw_length(t, font_px),
    };
    match tokens {
        [a] if vertical(a) => format!("50% {}", word(a)),
        [a] => format!("{} 50%", word(a)),
        [a, b] if vertical(a) || horizontal(b) => format!("{} {}", word(b), word(a)),
        [a, b] => format!("{} {}", word(a), word(b)),
        _ => tokens.join(" "),
    }
}

fn css_url(args: &str) -> String {
    format!(
        "url(\"{}\")",
        args.trim().trim_matches(|c| c == '"' || c == '\'')
    )
}

/// `clip-path` as Chrome serializes it, from the text the engine keeps.
fn clip_path(raw: &str, font_px: f32) -> String {
    let Some(list) = functions(raw) else {
        // A bare `<geometry-box>`.
        return raw.trim().to_ascii_lowercase();
    };
    let serialized: Vec<String> = list
        .iter()
        .map(|(name, args)| {
            let split: Vec<&str> = args.split_whitespace().collect();
            let until_at = |tokens: &[&str]| -> (Vec<String>, Option<String>) {
                match tokens.iter().position(|t| t.eq_ignore_ascii_case("at")) {
                    Some(i) => (
                        tokens[..i].iter().map(|t| raw_length(t, font_px)).collect(),
                        Some(raw_position(&tokens[i + 1..], font_px)),
                    ),
                    None => (
                        tokens.iter().map(|t| raw_length(t, font_px)).collect(),
                        None,
                    ),
                }
            };
            match name.as_str() {
                "circle" | "ellipse" => {
                    let (radii, at) = until_at(&split);
                    let mut parts = radii;
                    if let Some(at) = at {
                        parts.push(format!("at {at}"));
                    }
                    format!("{name}({})", parts.join(" "))
                }
                "inset" => {
                    let round = split.iter().position(|t| t.eq_ignore_ascii_case("round"));
                    let edges = &split[..round.unwrap_or(split.len())];
                    let e: Vec<String> = edges.iter().map(|t| raw_length(t, font_px)).collect();
                    let edge = match e.as_slice() {
                        [a] => sides(a, a, a, a),
                        [a, b] => sides(a, b, a, b),
                        [a, b, c] => sides(a, b, c, b),
                        [a, b, c, d] => sides(a, b, c, d),
                        _ => e.join(" "),
                    };
                    match round {
                        Some(i) => {
                            let r: Vec<String> = split[i + 1..]
                                .iter()
                                .map(|t| raw_length(t, font_px))
                                .collect();
                            format!("inset({edge} round {})", r.join(" "))
                        }
                        None => format!("inset({edge})"),
                    }
                }
                "polygon" => {
                    let points: Vec<String> = args
                        .split(',')
                        .map(|point| {
                            let t: Vec<&str> = point.split_whitespace().collect();
                            if t.len() == 1 {
                                t[0].to_string()
                            } else {
                                t.iter()
                                    .map(|v| raw_length(v, font_px))
                                    .collect::<Vec<_>>()
                                    .join(" ")
                            }
                        })
                        .collect();
                    format!("polygon({})", points.join(", "))
                }
                "path" => format!(
                    "path(\"{}\")",
                    path_data(args.trim().trim_matches(|c| c == '"' || c == '\''))
                ),
                "url" => css_url(args),
                _ => format!("{name}({args})"),
            }
        })
        .collect();
    serialized.join(" ")
}

/// Path data with one space between a command and each number.
fn path_data(d: &str) -> String {
    let mut out = String::new();
    let mut number = String::new();
    let flush = |number: &mut String, out: &mut String| {
        if !number.is_empty() {
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str(&numf(number.parse().unwrap_or(0.0)));
            number.clear();
        }
    };
    for c in d.chars() {
        match c {
            c if c.is_ascii_alphabetic() && c != 'e' => {
                flush(&mut number, &mut out);
                if !out.is_empty() {
                    out.push(' ');
                }
                out.push(c);
            }
            '-' if number.is_empty() || number.ends_with('e') => number.push(c),
            '-' => {
                flush(&mut number, &mut out);
                number.push(c);
            }
            c if c.is_ascii_digit() || c == '.' || c == 'e' => number.push(c),
            _ => flush(&mut number, &mut out),
        }
    }
    flush(&mut number, &mut out);
    out
}

/// `filter` as Chrome serializes it, from the text the engine keeps.
fn filter(raw: &str, color: css::Color, font_px: f32) -> Option<String> {
    let parts: Vec<String> = functions(raw)?
        .iter()
        .map(|(name, args)| match name.as_str() {
            "blur" if args.trim().is_empty() => "blur(0px)".into(),
            "blur" => format!("blur({})", raw_length(args, font_px)),
            "hue-rotate" => {
                let degrees = crate::parse_angle(args.trim()).unwrap_or(0.0);
                format!("hue-rotate({}deg)", numf(degrees))
            }
            "brightness" | "contrast" | "grayscale" | "invert" | "opacity" | "saturate"
            | "sepia" => {
                let amount = match args
                    .strip_suffix('%')
                    .filter(|_| !args.trim().is_empty())
                    .and_then(|p| p.trim().parse::<f64>().ok())
                {
                    Some(p) => num(p / 100.0),
                    None if args.trim().is_empty() => "1".into(),
                    None => args
                        .parse::<f64>()
                        .map(num)
                        .unwrap_or_else(|_| args.clone()),
                };
                format!("{name}({amount})")
            }
            "drop-shadow" => {
                // Lengths in order, a colour anywhere: split at spaces outside `()`.
                let (mut lengths, mut shade, mut depth, mut token) =
                    (Vec::new(), None, 0, String::new());
                let mut take = |token: &mut String| {
                    if token.is_empty() {
                        return;
                    }
                    match crate::parse_color(token) {
                        Some(c) => shade = Some(c),
                        None => lengths.push(raw_length(token, font_px)),
                    }
                    token.clear();
                };
                for c in args.chars() {
                    match c {
                        '(' => depth += 1,
                        ')' => depth -= 1,
                        _ => {}
                    }
                    if c.is_whitespace() && depth == 0 {
                        take(&mut token);
                    } else {
                        token.push(c);
                    }
                }
                take(&mut token);
                if lengths.len() == 2 {
                    lengths.push("0px".into());
                }
                format!(
                    "drop-shadow({} {})",
                    css_color(shade.unwrap_or(color)),
                    lengths.join(" ")
                )
            }
            "url" => css_url(args),
            _ => format!("{name}({args})"),
        })
        .collect();
    Some(parts.join(" "))
}

fn stops(list: &[css::ColorStop]) -> String {
    list.iter()
        .map(|stop| {
            let color = css_color(stop.color);
            match stop.position {
                Some(css::StopPosition::Percent(p)) => {
                    format!("{color} {}%", num(f64::from(p) * 100.0))
                }
                Some(css::StopPosition::Pixels(p)) => format!("{color} {}", css_px(p)),
                None => color,
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn center(c: (f32, f32)) -> Option<String> {
    (c != (0.5, 0.5)).then(|| {
        format!(
            "at {}% {}%",
            num(f64::from(c.0) * 100.0),
            num(f64::from(c.1) * 100.0)
        )
    })
}

fn gradient(g: &Gradient) -> String {
    let named = |base: &str, repeating: bool, lead: Vec<String>, list: &[css::ColorStop]| {
        let prefix = if repeating { "repeating-" } else { "" };
        let lead = lead.join(" ");
        if lead.is_empty() {
            format!("{prefix}{base}({})", stops(list))
        } else {
            format!("{prefix}{base}({lead}, {})", stops(list))
        }
    };
    match g {
        Gradient::Linear(l) => {
            let lead = match l.direction {
                GradientDirection::ToBottom => None,
                GradientDirection::Angle(a) if a == 180.0 => None,
                GradientDirection::Angle(a) => Some(format!("{}deg", numf(a))),
                GradientDirection::ToTop => Some("to top".into()),
                GradientDirection::ToRight => Some("to right".into()),
                GradientDirection::ToLeft => Some("to left".into()),
                GradientDirection::ToTopRight => Some("to right top".into()),
                GradientDirection::ToTopLeft => Some("to left top".into()),
                GradientDirection::ToBottomRight => Some("to right bottom".into()),
                GradientDirection::ToBottomLeft => Some("to left bottom".into()),
            };
            named(
                "linear-gradient",
                l.repeating,
                lead.into_iter().collect(),
                &l.stops,
            )
        }
        Gradient::Radial(r) => {
            let mut lead = Vec::new();
            if r.shape == css::RadialShape::Circle {
                lead.push("circle".to_string());
            }
            match r.size {
                css::RadialSize::FarthestCorner => {}
                css::RadialSize::Explicit(x, y) => lead.push(pair(&css_px(x), &css_px(y))),
                other => lead.push(keyword(other)),
            }
            lead.extend(center(r.center));
            named("radial-gradient", r.repeating, lead, &r.stops)
        }
        Gradient::Conic(c) => {
            let mut lead = Vec::new();
            if c.from_angle != 0.0 {
                lead.push(format!("from {}deg", numf(c.from_angle)));
            }
            lead.extend(center(c.center));
            named("conic-gradient", c.repeating, lead, &c.stops)
        }
    }
}

fn image(i: &BackgroundImage) -> String {
    match i {
        BackgroundImage::None => "none".into(),
        BackgroundImage::Gradient(g) => gradient(g),
        BackgroundImage::Url(u) => format!("url(\"{u}\")"),
    }
}

fn bg_size(s: &css::BackgroundSize) -> String {
    let dim = |d: Option<f32>| match d {
        None => "auto".to_string(),
        // Percentages are stored negative (`parse_background_size_dimension`).
        Some(p) if p < 0.0 => format!("{}%", numf(-p)),
        Some(p) => css_px(p),
    };
    match s {
        css::BackgroundSize::Cover => "cover".into(),
        css::BackgroundSize::Contain => "contain".into(),
        css::BackgroundSize::Auto => "auto".into(),
        css::BackgroundSize::Explicit {
            width,
            height: None,
        } => dim(*width),
        css::BackgroundSize::Explicit { width, height } => {
            format!("{} {}", dim(*width), dim(*height))
        }
    }
}

fn bg_axis(v: &css::BackgroundPositionValue) -> String {
    use css::BackgroundPositionValue as P;
    match v {
        P::Percent(p) => format!("{}%", num(f64::from(*p) * 100.0)),
        P::Px(p) => css_px(*p),
        P::Calc { percent, px } => {
            format!(
                "calc({}% + {})",
                num(f64::from(*percent) * 100.0),
                css_px(*px)
            )
        }
    }
}

fn timing(t: &TimingFunction) -> String {
    match t {
        TimingFunction::Ease => "ease".into(),
        TimingFunction::Linear => "linear".into(),
        TimingFunction::EaseIn => "ease-in".into(),
        TimingFunction::EaseOut => "ease-out".into(),
        TimingFunction::EaseInOut => "ease-in-out".into(),
        TimingFunction::StepStart => "steps(1, start)".into(),
        TimingFunction::StepEnd => "steps(1)".into(),
        TimingFunction::Steps(n, true) => format!("steps({n}, start)"),
        TimingFunction::Steps(n, false) => format!("steps({n})"),
        TimingFunction::CubicBezier(a, b, c, d) => {
            format!(
                "cubic-bezier({}, {}, {}, {})",
                numf(*a),
                numf(*b),
                numf(*c),
                numf(*d)
            )
        }
    }
}

fn seconds(s: f32) -> String {
    format!("{}s", numf(s))
}

/// `aspect-ratio` keeps one number. A small whole ratio is what a page wrote
/// (`16 / 9`); a `1.5` a page wrote as a number reads `3 / 2`, Chrome `1.5 / 1`.
fn ratio(r: f32) -> String {
    for d in 1..=64u32 {
        let n = r * d as f32;
        if (n - n.round()).abs() < 1e-4 {
            return format!("{} / {d}", n.round() as i64);
        }
    }
    format!("{} / 1", numf(r))
}

fn track(t: &css::TrackSize) -> String {
    use css::TrackSize as T;
    match t {
        T::Px(p) => css_px(*p),
        T::Percent(p) => format!("{}%", numf(*p)),
        T::Fr(f) => format!("{}fr", numf(*f)),
        T::MinContent => "min-content".into(),
        T::MaxContent => "max-content".into(),
        T::Auto => "auto".into(),
        T::MinMax(a, b) => format!("minmax({}, {})", track(a), track(b)),
        T::FitContent(p) => format!("fit-content({})", css_px(*p)),
    }
}

/// A track list: Chrome reads declared tracks off a box that is not a grid and
/// used pixels off a grid. The engine keeps no used tracks, so a grid's list is
/// known only when every track is a length already. `''` otherwise.
fn track_list(t: &css::GridTemplate, is_grid: bool) -> String {
    let mut tracks: Vec<&css::TrackDefinition> = t.tracks.iter().collect();
    for (at, repeat) in t.repeats.iter().rev() {
        let css::TrackRepeat::Count(n, list) = repeat else {
            return String::new();
        };
        let at = (*at).min(tracks.len());
        let expanded: Vec<&css::TrackDefinition> = (0..*n).flat_map(|_| list.iter()).collect();
        tracks.splice(at..at, expanded);
    }
    let named = tracks.iter().any(|d| !d.line_names.is_empty()) || !t.final_line_names.is_empty();
    let pixels = tracks
        .iter()
        .all(|d| matches!(d.size, css::TrackSize::Px(_)));
    if named || (is_grid && !pixels) {
        return String::new();
    }
    tracks
        .iter()
        .map(|d| track(&d.size))
        .collect::<Vec<_>>()
        .join(" ")
}

fn grid_line(l: &css::GridLine) -> String {
    use css::GridLine as G;
    match l {
        G::Auto => "auto".into(),
        G::Number(n) => n.to_string(),
        G::Name(n) => n.clone(),
        G::Span(n) => format!("span {n}"),
        G::SpanName(n) => format!("span {n}"),
    }
}

fn grid_lines(lines: &[String]) -> String {
    let mut n = lines.len();
    while n > 1 && lines[n - 1] == "auto" {
        n -= 1;
    }
    lines[..n].join(" / ")
}

fn font_shorthand(s: &css::ComputedStyle, font_px: f32, line_height: &str) -> String {
    let mut parts = Vec::new();
    if s.font_style != css::FontStyle::default() {
        parts.push(keyword(s.font_style));
    }
    if s.font_weight.0 != 400 {
        parts.push(s.font_weight.0.to_string());
    }
    parts.push(if line_height == "normal" {
        css_px(font_px)
    } else {
        format!("{} / {line_height}", css_px(font_px))
    });
    parts.push(s.font_family.clone());
    parts.join(" ")
}

/// The rows for one box, each `\nname\tvalue`, appended to `computed_style_lines`.
pub(crate) fn rows(b: &LayoutBox, font_px: f32, line_height: &str) -> String {
    let s = &*b.style;
    let d = &b.dimensions;
    // The border widths as CSS reports them. A styled scrollbar's space
    // is its own gutter (`Dimensions::scrollbar`), not border (CH2.E7).
    let css_border = d.border;
    let bx = d.border_box();
    let black = css::Color::BLACK;
    let mut out = String::new();
    let mut row = |name: &str, value: &str| {
        out.push('\n');
        out.push_str(name);
        out.push('\t');
        out.push_str(value);
    };
    let len = |l: &Length| length(l, font_px).unwrap_or_default();

    // CH2.E4: the keyword; a `url()` image is not kept.
    if s.cursor != css::Cursor::Auto {
        row("cursor", s.cursor.as_str());
    }

    // Transforms.
    // `display: none` has no transform in Chromium: it reads `none`. A viewport
    // unit has no viewport here: say nothing rather than resolve it to 0.
    if !s.transform.ops.is_empty() && s.display != css::Display::None {
        let viewport = |l: &Length| {
            matches!(
                l,
                Length::Vw(_)
                    | Length::Vh(_)
                    | Length::Vmin(_)
                    | Length::Vmax(_)
                    | Length::Calc(_)
                    | Length::Min(_)
                    | Length::Max(_)
                    | Length::Clamp(_)
            )
        };
        let unknown = s.transform.ops.iter().any(|op| match op {
            TransformOp::Translate(x, y) => viewport(x) || viewport(y),
            TransformOp::TranslateX(x) | TransformOp::TranslateY(x) => viewport(x),
            _ => false,
        });
        let value = if unknown {
            String::new()
        } else {
            transform_matrix(&s.transform.ops, font_px, bx.width, bx.height)
        };
        row("transform", &value);
    }
    if let Some(TransformOp::Translate(x, y)) = &s.translate {
        let (x, y) = (len(x), len(y));
        let value = if y == "0px" { x } else { format!("{x} {y}") };
        row("translate", &value);
    }
    if let Some(TransformOp::Rotate(a)) = &s.rotate {
        row("rotate", &format!("{}deg", numf(*a)));
    }
    if let Some(TransformOp::Scale(x, y)) = &s.scale {
        row("scale", &pair(&numf(*x), &numf(*y)));
    }
    // An inline box has no transform box in Chromium: `0px 0px`, unset or set.
    let (origin_w, origin_h) = if s.display == css::Display::Inline {
        (0.0, 0.0)
    } else {
        (bx.width, bx.height)
    };
    row(
        "transform-origin",
        &format!(
            "{} {}",
            css_px(s.transform_origin.x.to_px(font_px, root_font(), origin_w)),
            css_px(s.transform_origin.y.to_px(font_px, root_font(), origin_h))
        ),
    );

    // Clip and filter: the engine keeps the declaration's text.
    if !s.clip_path.is_empty() {
        let value = clip_path(&s.clip_path, font_px);
        row("clip-path", &value);
        row("-webkit-clip-path", &value);
    }
    if !s.filter.is_empty() {
        if let Some(value) = filter(&s.filter, s.color, font_px) {
            row("filter", &value);
            row("-webkit-filter", &value);
        }
    }
    if !s.backdrop_filter.is_empty() {
        if let Some(value) = filter(&s.backdrop_filter, s.color, font_px) {
            row("backdrop-filter", &value);
        }
    }
    if !s.box_shadows.is_empty() {
        let value: Vec<String> = s
            .box_shadows
            .iter()
            .map(|sh| {
                format!(
                    "{} {} {} {} {}{}",
                    css_color(sh.color),
                    css_px(sh.offset_x),
                    css_px(sh.offset_y),
                    css_px(sh.blur_radius),
                    css_px(sh.spread_radius),
                    if sh.inset { " inset" } else { "" }
                )
            })
            .collect();
        row("box-shadow", &value.join(", "));
    }

    // Text decoration.
    let line = {
        let l = &s.text_decoration_line;
        let names: Vec<&str> = [
            (l.underline, "underline"),
            (l.overline, "overline"),
            (l.line_through, "line-through"),
        ]
        .iter()
        .filter(|(on, _)| *on)
        .map(|(_, n)| *n)
        .collect();
        names.join(" ")
    };
    let decoration_color = s.text_decoration_color.unwrap_or(s.color);
    if !line.is_empty() {
        row("text-decoration-line", &line);
    }
    let styled = s.text_decoration_style != css::TextDecorationStyle::Solid;
    if styled {
        row("text-decoration-style", &keyword(s.text_decoration_style));
    }
    if decoration_color != black {
        row("text-decoration-color", &css_color(decoration_color));
    }
    let colored = s.text_decoration_color.filter(|c| *c != s.color);
    if !line.is_empty() || styled || colored.is_some() {
        let mut parts = vec![if line.is_empty() {
            "none".to_string()
        } else {
            line.clone()
        }];
        if styled {
            parts.push(keyword(s.text_decoration_style));
        }
        parts.extend(colored.map(css_color));
        row("text-decoration", &parts.join(" "));
    }

    // Text.
    if !matches!(s.word_spacing, Length::Zero) {
        row("word-spacing", &len(&s.word_spacing));
    }
    match s.vertical_align {
        css::VerticalAlign::Baseline => {}
        css::VerticalAlign::Length(p) => row("vertical-align", &css_px(p)),
        other => row("vertical-align", &keyword(other)),
    }
    if s.text_overflow != css::TextOverflow::Clip {
        row("text-overflow", &keyword(s.text_overflow));
    }
    if s.word_break != css::WordBreak::Normal {
        row("word-break", &keyword(s.word_break));
    }
    if s.overflow_wrap != css::OverflowWrap::Normal {
        let value = keyword(s.overflow_wrap);
        row("overflow-wrap", &value);
        row("word-wrap", &value);
    }
    if s.line_break != css::LineBreak::Auto {
        row("line-break", &keyword(s.line_break));
    }
    let fill = s.webkit_text_fill_color.unwrap_or(s.color);
    if fill != black {
        row("-webkit-text-fill-color", &css_color(fill));
    }
    if let Some(n) = s.webkit_line_clamp {
        row("-webkit-line-clamp", &n.to_string());
    }
    if s.webkit_box_orient_vertical {
        row("-webkit-box-orient", "vertical");
    }

    // Background. Layers are stored bottommost first; CSS lists the top first.
    let layers: Vec<&css::BackgroundLayer> = s.background_layers.iter().rev().collect();
    let each = |f: &dyn Fn(&css::BackgroundLayer) -> String| {
        layers.iter().map(|l| f(l)).collect::<Vec<_>>().join(", ")
    };
    let clip_of = |l: &css::BackgroundLayer| {
        let clip = if s.background_clip != css::BackgroundClip::BorderBox {
            s.background_clip
        } else {
            l.clip
        };
        keyword(clip)
    };
    if !layers.is_empty() {
        row("background-image", &each(&|l| image(&l.image)));
        row("background-size", &each(&|l| bg_size(&l.size)));
        row(
            "background-position",
            &each(&|l| format!("{} {}", bg_axis(&l.position.x), bg_axis(&l.position.y))),
        );
        row("background-position-x", &each(&|l| bg_axis(&l.position.x)));
        row("background-position-y", &each(&|l| bg_axis(&l.position.y)));
        row("background-repeat", &each(&|l| keyword(l.repeat)));
        row("background-origin", &each(&|l| keyword(l.origin)));
        row("background-clip", &each(&clip_of));
    } else if s.background_clip != css::BackgroundClip::BorderBox {
        row("background-clip", &keyword(s.background_clip));
    }
    let clip_declared = s.background_clip != css::BackgroundClip::BorderBox;
    if !layers.is_empty() || clip_declared || s.background_color != css::Color::TRANSPARENT {
        let layer_text = |image_text: String,
                          repeat: String,
                          position: String,
                          size: String,
                          origin: String,
                          clip: String| {
            format!("{image_text} {repeat} scroll {position} / {size} {origin} {clip}")
        };
        let mut texts: Vec<String> = layers
            .iter()
            .map(|l| {
                layer_text(
                    image(&l.image),
                    keyword(l.repeat),
                    format!("{} {}", bg_axis(&l.position.x), bg_axis(&l.position.y)),
                    bg_size(&l.size),
                    keyword(l.origin),
                    clip_of(l),
                )
            })
            .collect();
        if texts.is_empty() {
            texts.push(layer_text(
                "none".into(),
                "repeat".into(),
                "0% 0%".into(),
                "auto".into(),
                "padding-box".into(),
                keyword(s.background_clip),
            ));
        }
        let last = texts.len() - 1;
        texts[last] = format!("{} {}", css_color(s.background_color), texts[last]);
        row("background", &texts.join(", "));
    }

    // Replaced content.
    if !s.object_fit.is_empty() && s.object_fit != "fill" {
        row("object-fit", &s.object_fit);
    }
    if s.object_position != (0.5, 0.5) || s.object_position_offset != (0.0, 0.0) {
        let axis = |f: f32, o: f32| {
            use css::BackgroundPositionValue as P;
            bg_axis(&match (f, o) {
                (f, 0.0) => P::Percent(f),
                (0.0, o) => P::Px(o),
                (percent, px) => P::Calc { percent, px },
            })
        };
        row(
            "object-position",
            &format!(
                "{} {}",
                axis(s.object_position.0, s.object_position_offset.0),
                axis(s.object_position.1, s.object_position_offset.1)
            ),
        );
    }

    // Transitions and animations: one of each is what the engine keeps.
    let property = s
        .transition_property
        .split(',')
        .map(str::trim)
        .collect::<Vec<_>>()
        .join(", ");
    let property = if property.is_empty() {
        "all".to_string()
    } else {
        property
    };
    let (duration, delay) = (s.transition_duration, s.transition_delay);
    let eased = s.transition_timing_function == TimingFunction::Ease;
    if property != "all" {
        row("transition-property", &property);
    }
    if duration != 0.0 {
        row("transition-duration", &seconds(duration));
    }
    if delay != 0.0 {
        row("transition-delay", &seconds(delay));
    }
    if !eased {
        row(
            "transition-timing-function",
            &timing(&s.transition_timing_function),
        );
    }
    if property != "all" || duration != 0.0 || delay != 0.0 || !eased {
        let mut parts = Vec::new();
        if property != "all" {
            parts.push(property.clone());
        }
        if duration != 0.0 || delay != 0.0 {
            parts.push(seconds(duration));
        }
        if !eased {
            parts.push(timing(&s.transition_timing_function));
        }
        if delay != 0.0 {
            parts.push(seconds(delay));
        }
        row("transition", &parts.join(" "));
    }
    let name = if s.animation_name.is_empty() {
        "none"
    } else {
        s.animation_name.as_str()
    };
    let count = match s.animation_iteration_count {
        css::AnimationIterationCount::One => "1".to_string(),
        css::AnimationIterationCount::Infinite => "infinite".to_string(),
        css::AnimationIterationCount::Count(n) => numf(n),
    };
    let tail = format!(
        "{} {} {} {count} {} {} {}",
        seconds(s.animation_duration),
        timing(&s.animation_timing_function),
        seconds(s.animation_delay),
        keyword(s.animation_direction),
        keyword(s.animation_fill_mode),
        keyword(s.animation_play_state),
    );
    if name != "none" {
        row("animation-name", name);
    }
    if s.animation_duration != 0.0 {
        row("animation-duration", &seconds(s.animation_duration));
    }
    if s.animation_delay != 0.0 {
        row("animation-delay", &seconds(s.animation_delay));
    }
    if s.animation_timing_function != TimingFunction::Ease {
        row(
            "animation-timing-function",
            &timing(&s.animation_timing_function),
        );
    }
    if count != "1" {
        row("animation-iteration-count", &count);
    }
    if s.animation_direction != css::AnimationDirection::Normal {
        row("animation-direction", &keyword(s.animation_direction));
    }
    if s.animation_fill_mode != css::AnimationFillMode::None {
        row("animation-fill-mode", &keyword(s.animation_fill_mode));
    }
    if s.animation_play_state != css::AnimationPlayState::Running {
        row("animation-play-state", &keyword(s.animation_play_state));
    }
    if name != "none" || tail != "0s ease 0s 1 normal none running" {
        row("animation", &format!("{tail} {name}"));
    }

    // Container queries.
    let names = s.container_name.join(" ");
    if !names.is_empty() {
        row("container-name", &names);
    }
    if s.container_type != css::ContainerType::Normal {
        row("container-type", &keyword(s.container_type));
    }
    if !names.is_empty() || s.container_type != css::ContainerType::Normal {
        let shown = if names.is_empty() {
            "none"
        } else {
            names.as_str()
        };
        let value = if s.container_type == css::ContainerType::Normal {
            shown.to_string()
        } else {
            format!("{shown} / {}", keyword(s.container_type))
        };
        row("container", &value);
    }

    // Sizing. Auto is what an unset min and max hold.
    if !matches!(s.min_width, Length::Auto) {
        row("min-width", &len(&s.min_width));
    }
    if !matches!(s.min_height, Length::Auto) {
        row("min-height", &len(&s.min_height));
    }
    if !matches!(s.max_width, Length::Auto) {
        row("max-width", &len(&s.max_width));
    }
    if !matches!(s.max_height, Length::Auto) {
        row("max-height", &len(&s.max_height));
    }
    if let Some(r) = s.aspect_ratio {
        row("aspect-ratio", &ratio(r));
    }
    if s.clear != css::Clear::None {
        row("clear", &keyword(s.clear));
    }
    if s.order != 0 {
        row("order", &s.order.to_string());
    }
    if let Some(n) = s.column_count {
        row("column-count", &n.to_string());
    }
    if s.border_collapse != css::BorderCollapse::Separate {
        row("border-collapse", &keyword(s.border_collapse));
    }
    if s.border_spacing != (0.0, 0.0) {
        row(
            "border-spacing",
            &pair(&css_px(s.border_spacing.0), &css_px(s.border_spacing.1)),
        );
    }

    // Box edges: shorthands and logical names read the used physical values.
    let edge4 = |e: &rustkit_layout::EdgeSizes| {
        (
            css_px(e.top),
            css_px(e.right),
            css_px(e.bottom),
            css_px(e.left),
        )
    };
    for (name, e) in [("margin", &d.margin), ("padding", &d.padding)] {
        if (e.top, e.right, e.bottom, e.left) == (0.0, 0.0, 0.0, 0.0) {
            continue;
        }
        let (t, r, bt, l) = edge4(e);
        row(name, &sides(&t, &r, &bt, &l));
        row(&format!("{name}-inline"), &pair(&l, &r));
        row(&format!("{name}-block"), &pair(&t, &bt));
        row(&format!("{name}-inline-start"), &l);
        row(&format!("{name}-inline-end"), &r);
        row(&format!("{name}-block-start"), &t);
        row(&format!("{name}-block-end"), &bt);
    }
    let offset = |l: &Option<Length>, size: f32| match l {
        None | Some(Length::Auto) => "auto".to_string(),
        Some(l) => css_px(l.to_px(font_px, root_font(), size)),
    };
    if s.top.is_some() || s.right.is_some() || s.bottom.is_some() || s.left.is_some() {
        let (t, r, bt, l) = (
            offset(&s.top, d.content.height),
            offset(&s.right, d.content.width),
            offset(&s.bottom, d.content.height),
            offset(&s.left, d.content.width),
        );
        row("inset", &sides(&t, &r, &bt, &l));
        row("inset-inline", &pair(&l, &r));
        row("inset-block", &pair(&t, &bt));
        row("inset-inline-start", &l);
        row("inset-inline-end", &r);
        row("inset-block-start", &t);
        row("inset-block-end", &bt);
    }
    if s.overflow_x != css::Overflow::Visible || s.overflow_y != css::Overflow::Visible {
        row(
            "overflow",
            &pair(&keyword(s.overflow_x), &keyword(s.overflow_y)),
        );
    }

    // Borders.
    let border_side = |w: f32, style: css::BorderStyle, color: css::Color| {
        format!("{} {} {}", css_px(w), keyword(style), css_color(color))
    };
    // An unset style is stored as solid; only its zero width tells it apart.
    let shown = |style: css::BorderStyle, w: f32| {
        if style == css::BorderStyle::Solid && w == 0.0 {
            css::BorderStyle::None
        } else {
            style
        }
    };
    let (style_t, style_r, style_b, style_l) = (
        shown(s.border_top_style, css_border.top),
        shown(s.border_right_style, css_border.right),
        shown(s.border_bottom_style, css_border.bottom),
        shown(s.border_left_style, css_border.left),
    );
    // The engine stores an unset or `currentcolor` border colour as black (its
    // initial); Chromium's initial is currentcolor. Black is read as the text
    // colour, which makes an explicit black border on coloured text read wrong.
    let current = |c: css::Color| if c == black { s.color } else { c };
    let (ct, cr, cb, cl) = (
        current(s.border_top_color),
        current(s.border_right_color),
        current(s.border_bottom_color),
        current(s.border_left_color),
    );
    let top = border_side(css_border.top, style_t, ct);
    let right = border_side(css_border.right, style_r, cr);
    let bottom = border_side(css_border.bottom, style_b, cb);
    let left = border_side(css_border.left, style_l, cl);
    const UNSET: &str = "0px none rgb(0, 0, 0)";
    let mut border_named = |prefix: &str, value: &str| {
        if value != UNSET {
            row(prefix, value);
        }
    };
    border_named("border-top", &top);
    border_named("border-right", &right);
    border_named("border-bottom", &bottom);
    border_named("border-left", &left);
    border_named("border-block-start", &top);
    border_named("border-block-end", &bottom);
    border_named("border-inline-start", &left);
    border_named("border-inline-end", &right);
    let same = |a: &str, b: &str| if a == b { a.to_string() } else { String::new() };
    border_named("border-block", &same(&top, &bottom));
    border_named("border-inline", &same(&left, &right));
    border_named(
        "border",
        &if top == right && right == bottom && bottom == left {
            top.clone()
        } else {
            String::new()
        },
    );
    let k = |st: css::BorderStyle| keyword(st);
    let (st, sr, sb, sl) = (k(style_t), k(style_r), k(style_b), k(style_l));
    if (st.as_str(), sr.as_str(), sb.as_str(), sl.as_str()) != ("none", "none", "none", "none") {
        row("border-style", &sides(&st, &sr, &sb, &sl));
        row("border-top-style", &st);
        row("border-right-style", &sr);
        row("border-bottom-style", &sb);
        row("border-left-style", &sl);
    }
    if (ct, cr, cb) != (black, black, black) || cl != black {
        let (t, r, bt, l) = (css_color(ct), css_color(cr), css_color(cb), css_color(cl));
        row("border-color", &sides(&t, &r, &bt, &l));
        row("border-top-color", &t);
        row("border-right-color", &r);
        row("border-bottom-color", &bt);
        row("border-left-color", &l);
    }
    if (css_border.top, css_border.right, css_border.bottom, css_border.left) != (0.0, 0.0, 0.0, 0.0) {
        let (t, r, bt, l) = edge4(&d.border);
        row("border-width", &sides(&t, &r, &bt, &l));
    }
    let radius = |c: &css::CornerRadius| {
        pair(
            &length(&c.horizontal, font_px).unwrap_or_default(),
            &length(&c.vertical, font_px).unwrap_or_default(),
        )
    };
    let is_zero = |c: &css::CornerRadius| {
        matches!(c.horizontal, Length::Zero) && matches!(c.vertical, Length::Zero)
    };
    for (name, corner) in [
        ("border-start-start-radius", &s.border_top_left_radius),
        ("border-start-end-radius", &s.border_top_right_radius),
        ("border-end-end-radius", &s.border_bottom_right_radius),
        ("border-end-start-radius", &s.border_bottom_left_radius),
    ] {
        if !is_zero(corner) {
            row(name, &radius(corner));
        }
    }

    // Flexbox and alignment. The default variant of each alignment enum is
    // also what an unset property holds, so it reads as Chromium's `normal`.
    let basis = match s.flex_basis {
        css::FlexBasis::Auto => "auto".to_string(),
        css::FlexBasis::Content => "content".to_string(),
        css::FlexBasis::Length(p) => css_px(p),
        css::FlexBasis::Percent(p) => format!("{}%", numf(p)),
    };
    if s.flex_grow != 0.0 {
        row("flex-grow", &numf(s.flex_grow));
    }
    if s.flex_shrink != 1.0 {
        row("flex-shrink", &numf(s.flex_shrink));
    }
    if basis != "auto" {
        row("flex-basis", &basis);
    }
    if s.flex_grow != 0.0 || s.flex_shrink != 1.0 || basis != "auto" {
        row(
            "flex",
            &format!("{} {} {basis}", numf(s.flex_grow), numf(s.flex_shrink)),
        );
    }
    if s.flex_wrap != css::FlexWrap::NoWrap {
        row("flex-wrap", &keyword(s.flex_wrap));
    }
    if s.flex_wrap != css::FlexWrap::NoWrap || s.flex_direction != css::FlexDirection::Row {
        row(
            "flex-flow",
            &format!("{} {}", keyword(s.flex_direction), keyword(s.flex_wrap)),
        );
    }
    let justify_content = if s.justify_content == css::JustifyContent::FlexStart {
        "normal".into()
    } else {
        keyword(s.justify_content)
    };
    let align_content = if s.align_content == css::AlignContent::Stretch {
        "normal".into()
    } else {
        keyword(s.align_content)
    };
    let align_items = if s.align_items == css::AlignItems::Stretch {
        "normal".into()
    } else {
        keyword(s.align_items)
    };
    let justify_items = if s.justify_items == css::JustifyItems::Stretch {
        "normal".into()
    } else {
        keyword(s.justify_items)
    };
    let (align_self, justify_self) = (keyword(s.align_self), keyword(s.justify_self));
    for (name, value, initial) in [
        ("justify-content", &justify_content, "normal"),
        ("align-content", &align_content, "normal"),
        ("align-items", &align_items, "normal"),
        ("justify-items", &justify_items, "normal"),
        ("align-self", &align_self, "auto"),
        ("justify-self", &justify_self, "auto"),
    ] {
        if value != initial {
            row(name, value);
        }
    }
    if justify_content != "normal" || align_content != "normal" {
        row("place-content", &pair(&align_content, &justify_content));
    }
    if align_items != "normal" || justify_items != "normal" {
        row("place-items", &pair(&align_items, &justify_items));
    }
    if align_self != "auto" || justify_self != "auto" {
        row("place-self", &pair(&align_self, &justify_self));
    }
    let gap = |l: &Length| {
        if matches!(l, Length::Zero) {
            "normal".to_string()
        } else {
            len(l)
        }
    };
    let (row_gap, column_gap) = (gap(&s.row_gap), gap(&s.column_gap));
    if row_gap != "normal" {
        row("row-gap", &row_gap);
    }
    if column_gap != "normal" {
        row("column-gap", &column_gap);
    }
    if row_gap != "normal" || column_gap != "normal" {
        row("gap", &pair(&row_gap, &column_gap));
    }

    // Grid.
    let is_grid = matches!(s.display, css::Display::Grid | css::Display::InlineGrid);
    let blank = |t: &css::GridTemplate| t.tracks.is_empty() && t.repeats.is_empty();
    let (columns, rows_list) = (
        track_list(&s.grid_template_columns, is_grid),
        track_list(&s.grid_template_rows, is_grid),
    );
    if !blank(&s.grid_template_columns) {
        row("grid-template-columns", &columns);
    }
    if !blank(&s.grid_template_rows) {
        row("grid-template-rows", &rows_list);
    }
    if let Some(areas) = &s.grid_template_areas {
        let value: Vec<String> = areas
            .rows
            .iter()
            .map(|cells| {
                let names: Vec<&str> = cells.iter().map(|c| c.as_deref().unwrap_or(".")).collect();
                format!("\"{}\"", names.join(" "))
            })
            .collect();
        row("grid-template-areas", &value.join(" "));
    }
    if !blank(&s.grid_template_columns) || !blank(&s.grid_template_rows) {
        let known = !columns.is_empty() && !rows_list.is_empty() && s.grid_template_areas.is_none();
        let value = if known {
            format!("{rows_list} / {columns}")
        } else {
            String::new()
        };
        row("grid-template", &value);
    }
    let (cs, ce, rs, re) = (
        grid_line(&s.grid_column_start),
        grid_line(&s.grid_column_end),
        grid_line(&s.grid_row_start),
        grid_line(&s.grid_row_end),
    );
    if cs != "auto" {
        row("grid-column-start", &cs);
    }
    if ce != "auto" {
        row("grid-column-end", &ce);
    }
    if rs != "auto" {
        row("grid-row-start", &rs);
    }
    if re != "auto" {
        row("grid-row-end", &re);
    }
    if cs != "auto" || ce != "auto" {
        row("grid-column", &grid_lines(&[cs.clone(), ce.clone()]));
    }
    if rs != "auto" || re != "auto" {
        row("grid-row", &grid_lines(&[rs.clone(), re.clone()]));
    }
    if cs != "auto" || ce != "auto" || rs != "auto" || re != "auto" {
        row("grid-area", &grid_lines(&[rs, cs, re, ce]));
    }
    if s.grid_auto_columns != css::TrackSize::Auto {
        row("grid-auto-columns", &track(&s.grid_auto_columns));
    }
    if s.grid_auto_rows != css::TrackSize::Auto {
        row("grid-auto-rows", &track(&s.grid_auto_rows));
    }
    match s.grid_auto_flow {
        css::GridAutoFlow::Row => {}
        css::GridAutoFlow::Column => row("grid-auto-flow", "column"),
        css::GridAutoFlow::RowDense => row("grid-auto-flow", "dense"),
        css::GridAutoFlow::ColumnDense => row("grid-auto-flow", "column dense"),
    }

    // Masks. Each longhand keeps its own list, in CSS order.
    let m = &s.mask;
    let mut mask_row = |name: &str, value: String| {
        row(name, &value);
        row(&format!("-webkit-{name}"), &value);
    };
    let join = |v: Vec<String>| v.join(", ");
    if !m.images.is_empty() {
        mask_row("mask-image", join(m.images.iter().map(image).collect()));
    }
    if !m.sizes.is_empty() {
        mask_row("mask-size", join(m.sizes.iter().map(bg_size).collect()));
    }
    if !m.positions.is_empty() {
        mask_row(
            "mask-position",
            join(
                m.positions
                    .iter()
                    .map(|p| format!("{} {}", bg_axis(&p.x), bg_axis(&p.y)))
                    .collect(),
            ),
        );
    }
    if !m.repeats.is_empty() {
        mask_row(
            "mask-repeat",
            join(m.repeats.iter().map(|r| keyword(*r)).collect()),
        );
    }
    if !m.origins.is_empty() {
        mask_row(
            "mask-origin",
            join(m.origins.iter().map(|o| keyword(*o)).collect()),
        );
    }
    if !m.clips.is_empty() {
        mask_row(
            "mask-clip",
            join(m.clips.iter().map(|c| keyword(*c)).collect()),
        );
    }
    if !m.images.is_empty() {
        // Only an image alone has a form the probes pinned; the rest stay `''`.
        let alone = m.sizes.iter().all(|v| *v == css::BackgroundSize::Auto)
            && m.positions
                .iter()
                .all(|v| *v == css::BackgroundPosition::default())
            && m.repeats
                .iter()
                .all(|v| *v == css::BackgroundRepeat::Repeat)
            && m.origins
                .iter()
                .all(|v| *v == css::BackgroundOrigin::BorderBox)
            && m.clips.iter().all(|v| *v == css::BackgroundClip::BorderBox);
        mask_row(
            "mask",
            if alone {
                join(m.images.iter().map(image).collect())
            } else {
                String::new()
            },
        );
    }

    // Font.
    let font = font_shorthand(s, font_px, line_height);
    if font != "16px Times" {
        row("font", &font);
    }
    out
}
