//! Registered custom properties (CSS Properties and Values API Level 1):
//! `@property` rules and `CSS.registerProperty()`.
//!
//! A registration gives a custom property a syntax, an inheritance flag and
//! an initial value. Its computed value is then typed: a `<color>` computes
//! to `rgb()`, a `<length>` to px, and so on, and a transition between two
//! typed values interpolates (§6 Animation behavior) where an unregistered
//! property's value is a token stream that only flips.
//!
//! What this module covers:
//! - the syntax strings of §5.1: `*`, the data types `<length>`,
//!   `<number>`, `<percentage>`, `<length-percentage>`, `<color>`,
//!   `<integer>`, `<angle>`, `<time>`, `<custom-ident>`, a keyword, each
//!   with an optional `+` (space list) or `#` (comma list), and `|`
//!   alternatives;
//! - the computed value of a declared value, serialised as Chrome 148
//!   serialises it (`TypedValue::to_css`);
//! - a registry built from a document's `@property` rules (and the script
//!   ones, which come in as `@property` text after them).
//!
//! What it does not: `<image>`, `<url>`, `<transform-function>`,
//! `<transform-list>`, `<resolution>` and `<string>` syntaxes (a rule that
//! names one is dropped, as a registration the engine cannot honour), and
//! `currentcolor` inside a registered `<color>` interpolates as discrete.

use crate::{parse_color, parse_length, Color, Length, Stylesheet};
use std::collections::HashMap;
use std::sync::Arc;

/// A data type a syntax component names.
#[derive(Clone, Debug, PartialEq)]
pub enum Component {
    Length,
    Number,
    Percentage,
    LengthPercentage,
    Color,
    Integer,
    Angle,
    Time,
    CustomIdent,
    /// A literal keyword (`auto` in `<length> | auto`).
    Keyword(String),
}

/// How a component repeats.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Multiplier {
    One,
    /// `+`: a space-separated list.
    Space,
    /// `#`: a comma-separated list.
    Comma,
}

/// A parsed `syntax` descriptor.
#[derive(Clone, Debug, PartialEq)]
pub enum Syntax {
    /// `*`: any token stream; computed and animated as unregistered.
    Universal,
    /// `|` alternatives, tried in order.
    Terms(Vec<(Component, Multiplier)>),
}

/// Parse a `syntax` string (the unquoted content).
pub fn parse_syntax(s: &str) -> Option<Syntax> {
    let s = s.trim();
    if s == "*" {
        return Some(Syntax::Universal);
    }
    if s.is_empty() {
        return None;
    }
    let mut terms = Vec::new();
    for part in s.split('|') {
        let part = part.trim();
        let (body, mult) = if let Some(b) = part.strip_suffix('+') {
            (b.trim_end(), Multiplier::Space)
        } else if let Some(b) = part.strip_suffix('#') {
            (b.trim_end(), Multiplier::Comma)
        } else {
            (part, Multiplier::One)
        };
        let component = if let Some(name) = body.strip_prefix('<').and_then(|b| b.strip_suffix('>'))
        {
            match name {
                "length" => Component::Length,
                "number" => Component::Number,
                "percentage" => Component::Percentage,
                "length-percentage" => Component::LengthPercentage,
                "color" => Component::Color,
                "integer" => Component::Integer,
                "angle" => Component::Angle,
                "time" => Component::Time,
                "custom-ident" => Component::CustomIdent,
                _ => return None,
            }
        } else if is_ident(body)
            && !matches!(
                body.to_ascii_lowercase().as_str(),
                "inherit" | "initial" | "unset" | "revert" | "revert-layer" | "default"
            )
        {
            Component::Keyword(body.to_string())
        } else {
            return None;
        };
        terms.push((component, mult));
    }
    Some(Syntax::Terms(terms))
}

fn is_ident(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' || c == '-' || !c.is_ascii() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || !c.is_ascii())
}

/// What a computed value is resolved against.
#[derive(Clone, Copy, Debug)]
pub struct ComputeContext {
    /// The element's font size (`em`).
    pub font_px: f32,
    /// The root's font size (`rem`).
    pub root_font_px: f32,
    /// The viewport, for `vw` / `vh`.
    pub viewport: (f32, f32),
    /// An initial value: it must be computationally independent (§3.1),
    /// so relative units are invalid.
    pub independent: bool,
}

impl Default for ComputeContext {
    fn default() -> Self {
        ComputeContext {
            font_px: 16.0,
            root_font_px: 16.0,
            viewport: (0.0, 0.0),
            independent: false,
        }
    }
}

/// One `<length-percentage>`: which form it takes decides how it prints.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LengthPercentage {
    Px(f32),
    Percent(f32),
    /// `calc(<percent>% + <px>px)`: a mix of the two (or the result of
    /// interpolating a length with a percentage, which Chrome prints as a
    /// `calc()` at every point, its end points too).
    Calc {
        percent: f32,
        px: f32,
    },
}

/// One component of a typed computed value.
#[derive(Clone, Debug, PartialEq)]
pub enum Typed {
    Color(Color),
    /// `currentcolor` in a `<color>`: kept as the keyword; discrete.
    CurrentColor,
    Length(f32),
    LengthPercentage(LengthPercentage),
    Number(f32),
    Integer(f32),
    Percentage(f32),
    /// Degrees.
    Angle(f32),
    /// Seconds.
    Time(f32),
    /// A keyword or `<custom-ident>`: discrete.
    Ident(String),
}

/// A registered property's computed value.
#[derive(Clone, Debug, PartialEq)]
pub struct TypedValue {
    pub items: Vec<Typed>,
    pub multiplier: Multiplier,
}

impl TypedValue {
    /// The computed value as Chrome serialises it.
    pub fn to_css(&self) -> String {
        let sep = if self.multiplier == Multiplier::Comma {
            ", "
        } else {
            " "
        };
        self.items
            .iter()
            .map(typed_css)
            .collect::<Vec<_>>()
            .join(sep)
    }
}

fn typed_css(t: &Typed) -> String {
    match t {
        Typed::Color(c) => color_css(*c),
        Typed::CurrentColor => "currentcolor".into(),
        Typed::Length(px) => format!("{}px", num(*px)),
        Typed::LengthPercentage(LengthPercentage::Px(px)) => format!("{}px", num(*px)),
        Typed::LengthPercentage(LengthPercentage::Percent(p)) => format!("{}%", num(*p)),
        Typed::LengthPercentage(LengthPercentage::Calc { percent, px }) => {
            if *px < 0.0 {
                format!("calc({}% - {}px)", num(*percent), num(-*px))
            } else {
                format!("calc({}% + {}px)", num(*percent), num(*px))
            }
        }
        Typed::Number(n) => num(*n),
        Typed::Integer(n) => num(n.round()),
        Typed::Percentage(p) => format!("{}%", num(*p)),
        Typed::Angle(d) => format!("{}deg", num(*d)),
        Typed::Time(s) => format!("{}s", num(*s)),
        Typed::Ident(s) => s.clone(),
    }
}

/// A colour as `getComputedStyle` prints it.
pub fn color_css(c: Color) -> String {
    if c.a >= 1.0 {
        format!("rgb({}, {}, {})", c.r, c.g, c.b)
    } else {
        format!("rgba({}, {}, {}, {})", c.r, c.g, c.b, num(c.a))
    }
}

/// A number the way Blink prints one in a computed value: six significant
/// digits, no trailing zeros.
pub fn num(v: f32) -> String {
    let v: f64 = v.to_string().parse().unwrap_or(0.0);
    let v = if (v - v.round()).abs() < 1e-6 {
        v.round()
    } else {
        v
    };
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

/// Split at top-level whitespace (or commas), outside parentheses.
fn split_list(value: &str, comma: bool) -> Vec<&str> {
    let mut out = Vec::new();
    let (mut depth, mut start) = (0i32, 0usize);
    for (i, c) in value.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth = (depth - 1).max(0),
            c if depth == 0 && ((comma && c == ',') || (!comma && c.is_whitespace())) => {
                out.push(value[start..i].trim());
                start = i + c.len_utf8();
            }
            _ => {}
        }
    }
    out.push(value[start..].trim());
    if comma {
        // An empty entry in a comma list (`a,,b`) is invalid.
        if out.iter().any(|s| s.is_empty()) {
            return Vec::new();
        }
        out
    } else {
        out.into_iter().filter(|s| !s.is_empty()).collect()
    }
}

fn relative(l: &Length) -> bool {
    match l.to_calc_sum() {
        Some(s) => {
            s.em != 0.0
                || s.rem != 0.0
                || s.vw != 0.0
                || s.vh != 0.0
                || s.vmin != 0.0
                || s.vmax != 0.0
        }
        None => !matches!(l, Length::Px(_) | Length::Zero),
    }
}

/// `value` as a length: (px, percent), `None` when it is not one. `Auto`
/// and the like are not lengths.
fn length_parts(value: &str, ctx: &ComputeContext) -> Option<(f32, f32, bool)> {
    let v = value.trim();
    // A bare number other than 0 is not a length.
    if v.parse::<f32>().is_ok() && v.parse::<f32>().ok() != Some(0.0) {
        return None;
    }
    let l = parse_length(v)?;
    if matches!(l, Length::Auto | Length::FitContent) {
        return None;
    }
    if ctx.independent && relative(&l) {
        return None;
    }
    let is_percent_only = matches!(l, Length::Percent(_));
    match l.to_calc_sum() {
        Some(s) => {
            let (vw, vh) = ctx.viewport;
            let px = s.px
                + s.em * ctx.font_px
                + s.rem * ctx.root_font_px
                + s.vw / 100.0 * vw
                + s.vh / 100.0 * vh
                + s.vmin / 100.0 * vw.min(vh)
                + s.vmax / 100.0 * vw.max(vh);
            Some((px, s.percent, is_percent_only))
        }
        // min()/max()/clamp() without a percentage.
        None => Some((
            l.to_px_with_viewport(
                ctx.font_px,
                ctx.root_font_px,
                0.0,
                ctx.viewport.0,
                ctx.viewport.1,
            ),
            0.0,
            false,
        )),
    }
}

fn parse_angle(v: &str) -> Option<f32> {
    let v = v.trim().to_ascii_lowercase();
    let (n, k) = if let Some(n) = v.strip_suffix("deg") {
        (n, 1.0)
    } else if let Some(n) = v.strip_suffix("grad") {
        (n, 0.9)
    } else if let Some(n) = v.strip_suffix("rad") {
        (n, 180.0 / std::f32::consts::PI)
    } else if let Some(n) = v.strip_suffix("turn") {
        (n, 360.0)
    } else if v == "0" {
        return Some(0.0);
    } else {
        return None;
    };
    Some(n.trim().parse::<f32>().ok()? * k)
}

fn parse_time(v: &str) -> Option<f32> {
    let v = v.trim().to_ascii_lowercase();
    if let Some(n) = v.strip_suffix("ms") {
        return Some(n.parse::<f32>().ok()? / 1000.0);
    }
    v.strip_suffix('s')?.parse::<f32>().ok()
}

fn parse_one(component: &Component, v: &str, ctx: &ComputeContext) -> Option<Typed> {
    let v = v.trim();
    if v.is_empty() || v.contains("var(") {
        return None;
    }
    let lower = v.to_ascii_lowercase();
    Some(match component {
        Component::Color => {
            if lower == "currentcolor" {
                if ctx.independent {
                    return None;
                }
                Typed::CurrentColor
            } else {
                Typed::Color(parse_color(v)?)
            }
        }
        Component::Length => {
            let (px, percent, percent_only) = length_parts(v, ctx)?;
            if percent != 0.0 || percent_only {
                return None;
            }
            Typed::Length(px)
        }
        Component::LengthPercentage => {
            let (px, percent, percent_only) = length_parts(v, ctx)?;
            Typed::LengthPercentage(if percent_only {
                LengthPercentage::Percent(percent)
            } else if percent == 0.0 {
                LengthPercentage::Px(px)
            } else if px == 0.0 && !v.contains("px") {
                LengthPercentage::Percent(percent)
            } else {
                LengthPercentage::Calc { percent, px }
            })
        }
        Component::Number => Typed::Number(v.parse::<f32>().ok().filter(|n| n.is_finite())?),
        Component::Integer => {
            if v.contains('.') || v.contains('e') || v.contains('E') {
                return None;
            }
            Typed::Integer(v.parse::<i64>().ok()? as f32)
        }
        Component::Percentage => Typed::Percentage(
            v.strip_suffix('%')?
                .trim()
                .parse::<f32>()
                .ok()
                .filter(|n| n.is_finite())?,
        ),
        Component::Angle => Typed::Angle(parse_angle(v)?),
        Component::Time => Typed::Time(parse_time(v)?),
        Component::CustomIdent => {
            if !is_ident(v)
                || matches!(
                    lower.as_str(),
                    "inherit" | "initial" | "unset" | "revert" | "revert-layer" | "default"
                )
            {
                return None;
            }
            Typed::Ident(v.to_string())
        }
        Component::Keyword(k) => {
            if v != k {
                return None;
            }
            Typed::Ident(k.clone())
        }
    })
}

impl Syntax {
    /// The computed value of `value` under this syntax, `None` when the
    /// value does not match it (the declaration is then invalid at
    /// computed-value time). `Syntax::Universal` has no typed value: its
    /// value is the token stream as declared.
    pub fn compute(&self, value: &str, ctx: &ComputeContext) -> Option<TypedValue> {
        let Syntax::Terms(terms) = self else {
            return None;
        };
        for (component, mult) in terms {
            let parts = match mult {
                Multiplier::One => vec![value.trim()],
                Multiplier::Space => split_list(value, false),
                Multiplier::Comma => split_list(value, true),
            };
            if parts.is_empty() {
                continue;
            }
            let items: Option<Vec<Typed>> =
                parts.iter().map(|p| parse_one(component, p, ctx)).collect();
            if let Some(items) = items {
                return Some(TypedValue {
                    items,
                    multiplier: *mult,
                });
            }
        }
        None
    }

    /// Whether `value` is valid for this syntax.
    pub fn matches(&self, value: &str) -> bool {
        match self {
            Syntax::Universal => true,
            Syntax::Terms(_) => self.compute(value, &ComputeContext::default()).is_some(),
        }
    }
}

/// One registration.
#[derive(Clone, Debug, PartialEq)]
pub struct Registration {
    pub name: String,
    pub syntax: Syntax,
    pub inherits: bool,
    /// The initial value, computed (`None` for a `*` syntax with no
    /// `initial-value`: the guaranteed-invalid value).
    pub initial: Option<String>,
}

impl Registration {
    /// A registration from its descriptors, as `@property` and
    /// `CSS.registerProperty()` validate them (§3.1, §3.2). `Err` is the
    /// DOMException name `registerProperty` throws.
    pub fn new(
        name: &str,
        syntax: &str,
        inherits: Option<bool>,
        initial: Option<&str>,
    ) -> Result<Registration, &'static str> {
        if !name.starts_with("--") || name.len() < 3 {
            return Err("SyntaxError");
        }
        let syntax = parse_syntax(syntax).ok_or("SyntaxError")?;
        let inherits = inherits.ok_or("SyntaxError")?;
        let initial = match (&syntax, initial.map(str::trim)) {
            (Syntax::Universal, None) => None,
            (Syntax::Universal, Some(v)) => {
                if v.contains("var(") {
                    return Err("SyntaxError");
                }
                Some(v.to_string())
            }
            (Syntax::Terms(_), None) => return Err("SyntaxError"),
            (Syntax::Terms(_), Some(v)) => {
                let ctx = ComputeContext {
                    independent: true,
                    ..ComputeContext::default()
                };
                Some(syntax.compute(v, &ctx).ok_or("SyntaxError")?.to_css())
            }
        };
        Ok(Registration {
            name: name.to_string(),
            syntax,
            inherits,
            initial,
        })
    }

    /// The registration an `@property` rule makes, `None` when the rule is
    /// invalid (and ignored). `prelude` is the rule's selector
    /// (`@property --x`).
    pub fn from_rule(prelude: &str, declarations: &[(String, String)]) -> Option<Registration> {
        let rest = prelude.trim().strip_prefix('@')?;
        if !rest.get(..8)?.eq_ignore_ascii_case("property") {
            return None;
        }
        let name = rest[8..].trim();
        let mut syntax = None;
        let mut inherits = None;
        let mut initial = None;
        for (property, value) in declarations {
            let value = value.trim();
            match property.trim().to_ascii_lowercase().as_str() {
                "syntax" => {
                    let unquoted = value
                        .strip_prefix('"')
                        .and_then(|v| v.strip_suffix('"'))
                        .or_else(|| value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))?;
                    syntax = Some(unquoted.to_string());
                }
                "inherits" => {
                    inherits = match value.to_ascii_lowercase().as_str() {
                        "true" => Some(true),
                        "false" => Some(false),
                        _ => return None,
                    }
                }
                "initial-value" => initial = Some(value.to_string()),
                _ => {}
            }
        }
        Registration::new(name, &syntax?, inherits, initial.as_deref()).ok()
    }
}

/// The document's registered custom properties, by name.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PropertyRegistry {
    map: HashMap<String, Arc<Registration>>,
}

impl PropertyRegistry {
    /// The registrations of `sheets`' `@property` rules, in order: a later
    /// valid rule for a name replaces an earlier one.
    pub fn from_sheets(sheets: &[Stylesheet]) -> Self {
        let mut map = HashMap::new();
        for sheet in sheets {
            for rule in &sheet.rules {
                let selector = rule.selector.trim_start();
                if !selector
                    .get(..9)
                    .is_some_and(|s| s.eq_ignore_ascii_case("@property"))
                {
                    continue;
                }
                let declarations: Vec<(String, String)> = rule
                    .declarations
                    .iter()
                    .filter_map(|d| match &d.value {
                        crate::PropertyValue::Specified(v) => Some((d.property.clone(), v.clone())),
                        _ => None,
                    })
                    .collect();
                if let Some(r) = Registration::from_rule(selector, &declarations) {
                    map.insert(r.name.clone(), Arc::new(r));
                }
            }
        }
        PropertyRegistry { map }
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn get(&self, name: &str) -> Option<&Arc<Registration>> {
        self.map.get(name)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Arc<Registration>> {
        self.map.values()
    }

    pub fn insert(&mut self, r: Registration) {
        self.map.insert(r.name.clone(), Arc::new(r));
    }
}

/// `@property` text for a registration from script, which the engine
/// appends after the document's sheets.
pub fn registration_rule(
    name: &str,
    syntax: &str,
    inherits: bool,
    initial: Option<&str>,
) -> String {
    let mut out = format!(
        "@property {name} {{ syntax: \"{}\"; inherits: {inherits};",
        syntax.replace('\\', "\\\\").replace('"', "\\\"")
    );
    if let Some(v) = initial {
        out.push_str(&format!(" initial-value: {v};"));
    }
    out.push_str(" }\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn computed(syntax: &str, v: &str) -> Option<String> {
        parse_syntax(syntax)?
            .compute(v, &ComputeContext::default())
            .map(|t| t.to_css())
    }

    #[test]
    fn syntax_strings_parse() {
        assert_eq!(parse_syntax("*"), Some(Syntax::Universal));
        assert_eq!(
            parse_syntax("<length> | auto"),
            Some(Syntax::Terms(vec![
                (Component::Length, Multiplier::One),
                (Component::Keyword("auto".into()), Multiplier::One)
            ]))
        );
        assert_eq!(
            parse_syntax("<color>#"),
            Some(Syntax::Terms(vec![(Component::Color, Multiplier::Comma)]))
        );
        assert_eq!(parse_syntax("<nonsense>"), None);
        assert_eq!(parse_syntax(""), None);
        assert_eq!(parse_syntax("inherit"), None);
    }

    /// Chrome 148 (receipt 2026-11-20/css_transitions_property_cft148.json).
    #[test]
    fn computed_values_serialise_as_chrome_does() {
        assert_eq!(
            computed("<color>", "red").as_deref(),
            Some("rgb(255, 0, 0)")
        );
        assert_eq!(
            computed("<color>", "#0000").as_deref(),
            Some("rgba(0, 0, 0, 0)")
        );
        assert_eq!(computed("<length>", "10px").as_deref(), Some("10px"));
        assert_eq!(computed("<length>", "1em").as_deref(), Some("16px"));
        assert_eq!(computed("<length>", "50%"), None);
        assert_eq!(computed("<length>", "red"), None);
        assert_eq!(
            computed("<length-percentage>", "50%").as_deref(),
            Some("50%")
        );
        assert_eq!(
            computed("<length-percentage>", "10px").as_deref(),
            Some("10px")
        );
        assert_eq!(computed("<number>", "10").as_deref(), Some("10"));
        assert_eq!(computed("<integer>", "10").as_deref(), Some("10"));
        assert_eq!(computed("<integer>", "1.5"), None);
        assert_eq!(computed("<percentage>", "100%").as_deref(), Some("100%"));
        assert_eq!(computed("<angle>", "1turn").as_deref(), Some("360deg"));
        assert_eq!(computed("<length>+", "0px 0px").as_deref(), Some("0px 0px"));
        assert_eq!(
            computed("<color>#", "blue, red").as_deref(),
            Some("rgb(0, 0, 255), rgb(255, 0, 0)")
        );
        assert_eq!(computed("<length> | auto", "auto").as_deref(), Some("auto"));
    }

    #[test]
    fn registrations_validate_as_register_property_does() {
        assert!(Registration::new("--x", "<length>", Some(false), Some("0px")).is_ok());
        assert_eq!(
            Registration::new("x", "<length>", Some(false), Some("0px")),
            Err("SyntaxError")
        );
        assert_eq!(
            Registration::new("--x", "<length>", Some(false), None),
            Err("SyntaxError")
        );
        assert_eq!(
            Registration::new("--x", "<length>", Some(false), Some("red")),
            Err("SyntaxError")
        );
        assert_eq!(
            Registration::new("--x", "<length>", Some(false), Some("1em")),
            Err("SyntaxError")
        );
        assert!(Registration::new("--x", "*", Some(false), None).is_ok());
        assert_eq!(
            Registration::new("--x", "<length>", None, Some("0px")),
            Err("SyntaxError")
        );
    }

    #[test]
    fn property_rules_register() {
        let sheet = Stylesheet::parse(
            "@property --a { syntax: '<color>'; inherits: false; initial-value: #0000 }\
             @property --b { syntax: \"<length>\"; inherits: true; initial-value: 0px }\
             @property --bad { syntax: '<length>'; inherits: false }\
             @property --noinherits { syntax: '<length>'; initial-value: 0px }\
             div { color: red }",
        )
        .unwrap();
        let reg = PropertyRegistry::from_sheets(&[sheet]);
        let a = reg.get("--a").expect("--a");
        assert_eq!(
            (a.inherits, a.initial.as_deref()),
            (false, Some("rgba(0, 0, 0, 0)"))
        );
        assert!(reg.get("--b").is_some_and(|b| b.inherits));
        assert!(reg.get("--bad").is_none());
        assert!(reg.get("--noinherits").is_none());
    }

    #[test]
    fn script_registrations_round_trip_through_rule_text() {
        let text = registration_rule("--r", "<color>", false, Some("black"));
        let reg = PropertyRegistry::from_sheets(&[Stylesheet::parse(&text).unwrap()]);
        assert_eq!(
            reg.get("--r").and_then(|r| r.initial.clone()).as_deref(),
            Some("rgb(0, 0, 0)")
        );
    }
}
