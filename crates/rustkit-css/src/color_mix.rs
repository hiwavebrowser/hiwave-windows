//! `color-mix()` (css-color-5 §2).
//!
//! `color-mix(in <space> [<hue> hue]?, <color> <pct>?, <color> <pct>?)`
//! mixes two colours in the named space with premultiplied alpha. Before
//! this it did not parse: `background: red; background: color-mix(...)`
//! painted nothing (the shorthand kept the value but had no colour).
//!
//! The spaces here are `srgb`, `srgb-linear`, `oklab`, `oklch`, `xyz`,
//! `xyz-d65` and `hsl`. Any other space does not parse, as before.

use crate::{parse_color, Color};

#[derive(Clone, Copy, PartialEq)]
enum Space {
    Srgb,
    SrgbLinear,
    Oklab,
    Oklch,
    Xyz,
    Hsl,
}

#[derive(Clone, Copy, PartialEq)]
enum HueMethod {
    Shorter,
    Longer,
    Increasing,
    Decreasing,
}

impl Space {
    fn parse(name: &str) -> Option<Space> {
        Some(match name {
            "srgb" => Space::Srgb,
            "srgb-linear" => Space::SrgbLinear,
            "oklab" => Space::Oklab,
            "oklch" => Space::Oklch,
            "xyz" | "xyz-d65" => Space::Xyz,
            "hsl" => Space::Hsl,
            _ => return None,
        })
    }

    /// The index of the hue among the three components, for a polar space.
    fn hue_index(self) -> Option<usize> {
        match self {
            Space::Oklch => Some(2),
            Space::Hsl => Some(0),
            _ => None,
        }
    }
}

/// Parse `color-mix(...)`. `value` is trimmed and lowercase.
pub(crate) fn parse_color_mix(value: &str) -> Option<Color> {
    let inner = value.strip_prefix("color-mix(")?.strip_suffix(')')?;
    let args = crate::split_css_function_args(inner);
    let [interp, first, second] = args[..] else {
        return None;
    };

    let mut words = interp.split_whitespace();
    if words.next()? != "in" {
        return None;
    }
    let space = Space::parse(words.next()?)?;
    let method = match (words.next(), words.next(), words.next()) {
        (None, _, _) => HueMethod::Shorter,
        (Some(m), Some("hue"), None) if space.hue_index().is_some() => match m {
            "shorter" => HueMethod::Shorter,
            "longer" => HueMethod::Longer,
            "increasing" => HueMethod::Increasing,
            "decreasing" => HueMethod::Decreasing,
            _ => return None,
        },
        _ => return None,
    };

    let (c1, p1) = color_and_percentage(first)?;
    let (c2, p2) = color_and_percentage(second)?;
    let (p1, p2) = match (p1, p2) {
        (None, None) => (0.5, 0.5),
        (Some(p), None) => (p, 1.0 - p),
        (None, Some(p)) => (1.0 - p, p),
        (Some(a), Some(b)) => (a, b),
    };
    let sum = p1 + p2;
    if sum <= 0.0 {
        return None;
    }
    // Below 100% the result is that much more transparent.
    let multiplier = sum.min(1.0);
    Some(mix(space, method, c1, c2, p2 / sum, multiplier))
}

/// `<color> <percentage>?` or `<percentage> <color>`.
fn color_and_percentage(arg: &str) -> Option<(Color, Option<f64>)> {
    let arg = arg.trim();
    let percentage = |token: &str| -> Option<f64> {
        let p = token.strip_suffix('%')?.parse::<f64>().ok()?;
        (0.0..=100.0).contains(&p).then_some(p / 100.0)
    };
    // The percentage is the last or the first top-level token.
    if let Some(at) = arg.rfind(|c: char| c.is_whitespace()) {
        let (head, tail) = (arg[..at].trim(), arg[at..].trim());
        if !tail.contains(')') {
            if let Some(p) = percentage(tail) {
                return Some((parse_color(head)?, Some(p)));
            }
        }
    }
    if let Some(at) = arg.find(|c: char| c.is_whitespace()) {
        let (head, tail) = (arg[..at].trim(), arg[at..].trim());
        if let Some(p) = percentage(head) {
            return Some((parse_color(tail)?, Some(p)));
        }
    }
    Some((parse_color(arg)?, None))
}

/// Components in `space`; `None` is a missing (powerless) hue.
type Components = [Option<f64>; 3];

fn mix(space: Space, method: HueMethod, c1: Color, c2: Color, t: f64, multiplier: f64) -> Color {
    let (a1, a2) = (c1.a as f64, c2.a as f64);
    let mut x = to_space(space, c1);
    let mut y = to_space(space, c2);
    let hue = space.hue_index();

    // A missing component takes the other colour's (css-color-4 §12.2).
    for i in 0..3 {
        match (x[i], y[i]) {
            (None, Some(v)) => x[i] = Some(v),
            (Some(v), None) => y[i] = Some(v),
            (None, None) => {
                x[i] = Some(0.0);
                y[i] = Some(0.0);
            }
            _ => {}
        }
    }
    let x = x.map(|v| v.unwrap_or(0.0));
    let y = y.map(|v| v.unwrap_or(0.0));

    let alpha = a1 + (a2 - a1) * t;
    let mut out = [0.0; 3];
    for i in 0..3 {
        if Some(i) == hue {
            let (h1, h2) = fix_hues(x[i], y[i], method);
            out[i] = (h1 + (h2 - h1) * t).rem_euclid(360.0);
        } else if alpha > 0.0 {
            // Premultiplied interpolation (css-color-4 §12.3).
            out[i] = (x[i] * a1 + (y[i] * a2 - x[i] * a1) * t) / alpha;
        } else {
            out[i] = x[i] + (y[i] - x[i]) * t;
        }
    }

    let [r, g, b] = from_space(space, out);
    let byte = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    Color::new(
        byte(r),
        byte(g),
        byte(b),
        (alpha * multiplier).clamp(0.0, 1.0) as f32,
    )
}

fn fix_hues(h1: f64, h2: f64, method: HueMethod) -> (f64, f64) {
    let (h1, mut h2) = (h1.rem_euclid(360.0), h2.rem_euclid(360.0));
    let d = h2 - h1;
    match method {
        HueMethod::Shorter => {
            if d > 180.0 {
                h2 -= 360.0;
            } else if d < -180.0 {
                h2 += 360.0;
            }
        }
        HueMethod::Longer => {
            if 0.0 < d && d < 180.0 {
                h2 -= 360.0;
            } else if -180.0 < d && d <= 0.0 {
                h2 += 360.0;
            }
        }
        HueMethod::Increasing => {
            if d < 0.0 {
                h2 += 360.0;
            }
        }
        HueMethod::Decreasing => {
            if d > 0.0 {
                h2 -= 360.0;
            }
        }
    }
    (h1, h2)
}

fn to_linear(c: f64) -> f64 {
    if c.abs() <= 0.04045 {
        c / 12.92
    } else {
        c.signum() * ((c.abs() + 0.055) / 1.055).powf(2.4)
    }
}

fn from_linear(c: f64) -> f64 {
    if c.abs() <= 0.0031308 {
        c * 12.92
    } else {
        c.signum() * (1.055 * c.abs().powf(1.0 / 2.4) - 0.055)
    }
}

/// The epsilon below which a polar colour's hue is powerless.
const ACHROMATIC: f64 = 1e-6;

fn to_space(space: Space, c: Color) -> Components {
    let srgb = [c.r as f64 / 255.0, c.g as f64 / 255.0, c.b as f64 / 255.0];
    let lin = srgb.map(to_linear);
    match space {
        Space::Srgb => srgb.map(Some),
        Space::SrgbLinear => lin.map(Some),
        Space::Xyz => lin_to_xyz(lin).map(Some),
        Space::Oklab => lin_to_oklab(lin).map(Some),
        Space::Oklch => {
            let [l, a, b] = lin_to_oklab(lin);
            let chroma = (a * a + b * b).sqrt();
            let hue =
                (chroma > ACHROMATIC * 100.0).then(|| b.atan2(a).to_degrees().rem_euclid(360.0));
            [Some(l), Some(chroma), hue]
        }
        Space::Hsl => srgb_to_hsl(srgb),
    }
}

fn from_space(space: Space, v: [f64; 3]) -> [f64; 3] {
    match space {
        Space::Srgb => v,
        Space::SrgbLinear => v.map(from_linear),
        Space::Xyz => xyz_to_lin(v).map(from_linear),
        Space::Oklab => oklab_to_lin(v).map(from_linear),
        Space::Oklch => {
            let [l, chroma, hue] = v;
            let (s, c) = hue.to_radians().sin_cos();
            oklab_to_lin([l, chroma * c, chroma * s]).map(from_linear)
        }
        Space::Hsl => hsl_to_srgb(v),
    }
}

fn lin_to_xyz([r, g, b]: [f64; 3]) -> [f64; 3] {
    [
        0.41239079926595934 * r + 0.357584339383878 * g + 0.1804807884018343 * b,
        0.21263900587151027 * r + 0.715168678767756 * g + 0.07219231536073371 * b,
        0.01933081871559182 * r + 0.11919477979462598 * g + 0.9505321522496607 * b,
    ]
}

fn xyz_to_lin([x, y, z]: [f64; 3]) -> [f64; 3] {
    [
        3.2409699419045226 * x - 1.537383177570094 * y - 0.4986107602930034 * z,
        -0.9692436362808796 * x + 1.8759675015077202 * y + 0.04155505740717559 * z,
        0.05563007969699366 * x - 0.20397695888897652 * y + 1.0569715142428786 * z,
    ]
}

fn lin_to_oklab(lin: [f64; 3]) -> [f64; 3] {
    let [x, y, z] = lin_to_xyz(lin);
    let l = (0.819022437996703 * x + 0.3619062600528904 * y - 0.1288737815209879 * z).cbrt();
    let m = (0.0329836539323885 * x + 0.9292868615863434 * y + 0.0361446663506424 * z).cbrt();
    let s = (0.0481771893596242 * x + 0.2642395317527308 * y + 0.6335478284694309 * z).cbrt();
    [
        0.210454268309314 * l + 0.7936177747023054 * m - 0.0040720430116193 * s,
        1.9779985324311684 * l - 2.42859224204858 * m + 0.450593709617411 * s,
        0.0259040424655478 * l + 0.7827717124575296 * m - 0.8086757549230774 * s,
    ]
}

fn oklab_to_lin([ll, a, b]: [f64; 3]) -> [f64; 3] {
    let l = (ll + 0.3963377773761749 * a + 0.2158037573099136 * b).powi(3);
    let m = (ll - 0.1055613458156586 * a - 0.0638541728258133 * b).powi(3);
    let s = (ll - 0.0894841775298119 * a - 1.2914855480194092 * b).powi(3);
    xyz_to_lin([
        1.2268798758459243 * l - 0.5578149944602171 * m + 0.2813910456659647 * s,
        -0.0405757452148008 * l + 1.112286803280317 * m - 0.0717110580655164 * s,
        -0.0763729366746601 * l - 0.4214933324022432 * m + 1.5869240198367816 * s,
    ])
}

/// sRGB to HSL with hue in degrees and saturation and lightness in 0..1.
fn srgb_to_hsl([r, g, b]: [f64; 3]) -> [Option<f64>; 3] {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    let d = max - min;
    if d < ACHROMATIC {
        return [None, Some(0.0), Some(l)];
    }
    let s = if l == 0.0 || l == 1.0 {
        0.0
    } else {
        (max - l) / l.min(1.0 - l)
    };
    let h = if max == r {
        (g - b) / d + if g < b { 6.0 } else { 0.0 }
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    [Some(h * 60.0), Some(s), Some(l)]
}

fn hsl_to_srgb([h, s, l]: [f64; 3]) -> [f64; 3] {
    let f = |n: f64| {
        let k = (n + h / 30.0).rem_euclid(12.0);
        let a = s * l.min(1.0 - l);
        l - a * (k - 3.0).min(9.0 - k).clamp(-1.0, 1.0)
    };
    [f(0.0), f(8.0), f(4.0)]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgba(c: Color) -> (u8, u8, u8, f32) {
        (c.r, c.g, c.b, (c.a * 1000.0).round() / 1000.0)
    }

    #[test]
    fn color_mix_mixes_in_the_named_space() {
        let mix = |v: &str| parse_color(v).map(rgba);
        // The reduced page's boxes.
        assert_eq!(
            mix("color-mix(in oklab, green, green)"),
            Some((0, 128, 0, 1.0))
        );
        assert_eq!(
            mix("color-mix(in oklab, green 50%, transparent)"),
            Some((0, 128, 0, 0.5)),
            "transparent mixes premultiplied"
        );
        // From the css-color-4 conversions (srgb, linear, oklab, oklch, hsl).
        assert_eq!(
            mix("color-mix(in srgb, red, blue)"),
            Some((128, 0, 128, 1.0))
        );
        assert_eq!(
            mix("color-mix(in srgb, red 25%, blue)"),
            Some((64, 0, 191, 1.0))
        );
        assert_eq!(
            mix("color-mix(in srgb, 25% red, blue)"),
            Some((64, 0, 191, 1.0))
        );
        assert_eq!(
            mix("color-mix(in oklab, red, blue)"),
            Some((140, 83, 162, 1.0))
        );
        assert_eq!(
            mix("color-mix(in srgb-linear, red, blue)"),
            Some((188, 0, 188, 1.0))
        );
        assert_eq!(
            mix("color-mix(in oklch, red, blue)"),
            Some((186, 0, 194, 1.0))
        );
        assert_eq!(
            mix("color-mix(in hsl, red, blue)"),
            Some((255, 0, 255, 1.0))
        );
        assert_eq!(
            mix("color-mix(in hsl longer hue, red, blue)"),
            Some((0, 255, 0, 1.0))
        );
        // Percentages under 100% scale the alpha.
        assert_eq!(
            mix("color-mix(in srgb, red 20%, blue 30%)"),
            Some((102, 0, 153, 0.5))
        );
        // A colour inside, and nested.
        assert_eq!(
            mix("color-mix(in srgb, color-mix(in srgb, red, red), blue)"),
            Some((128, 0, 128, 1.0))
        );
        for bad in [
            "color-mix(in srgb, red)",
            "color-mix(srgb, red, blue)",
            "color-mix(in nowhere, red, blue)",
            "color-mix(in srgb, red 0%, blue 0%)",
            "color-mix(in srgb, red 120%, blue)",
            "color-mix(in srgb longer hue, red, blue)",
            "color-mix(in srgb, currentcolor, blue)",
        ] {
            assert_eq!(mix(bad), None, "{bad}");
        }
    }
}
