//! CSS transitions (css-transitions-1), run by the engine (S13a).
//!
//! Until now the cascade parsed `transition` and nothing ticked it: a
//! `transition: opacity 100ms` class toggle jumped to its end value and no
//! `transitionend` ever fired. Now every build hands each element's
//! cascaded style to [`Animations::on_style`], which keeps the values of
//! the properties the element's `transition` names as of the last build
//! (the before-change style), starts a transition for each one that
//! changed (from the value the element shows now, so a toggle back midway
//! reverses from where it got to, with the spec's reversing shortening),
//! and overlays the running transitions' values on the style. The layout
//! box, paint and `getComputedStyle` all read the overlaid value; the
//! cascade's own result (and the style memo) stay untouched.
//!
//! The clock is the page's timer clock, read after each live turn's timers
//! (`DomBindings::clock_now_ms`), so transitions move with
//! `requestAnimationFrame` and the tests' `pump`. While one runs, the live
//! loop lays the view out every frame (`Engine::settle_animations`).
//!
//! Events (`transitionrun`, `transitionstart`, `transitionend`,
//! `transitioncancel`) are queued here as the phases change and fired at
//! the element after the build, in queue order, as Chrome orders them.
//!
//! What is interpolated: numbers (`opacity`), lengths (one unit with
//! itself, two units as a `calc()` mix of them; `auto` never transitions,
//! as in Chrome),
//! colours (premultiplied sRGB; `outline-color` and
//! `text-decoration-color` keep `currentcolor` as a value of its own,
//! `AnimValue::CurrentColor`), and transform lists (function by function
//! when the lists match, `none` as the identity of the other list, else
//! the two matrices component-wise, which is not the spec's decomposition
//! and is a stated gap), and the individual `translate`/`rotate`/`scale`
//! properties (`none` as the identity), `line-height`, the four
//! `border-*-radius` corners, `background-size`, `visibility` (its
//! discrete rule: `visible` in between when either end is), `box-shadow`
//! and `filter`/`backdrop-filter` lists. Not here yet: `background-position`,
//! pseudo-element transitions, and `transition-behavior: allow-discrete`.

use crate::svg_computed::{same_paint, Paint};
use rustkit_bindings::{CssAnimationControl, CssAnimationInfo};
use rustkit_css::{
    AnimationDirection, AnimationFillMode, AnimationItem, AnimationIterationCount,
    AnimationPlayState, BackgroundSize, BoxShadow, Color, ComputedStyle, CornerRadius, Display,
    GridTemplate, Length, LineHeight, TimingFunction, TrackDefinition, TrackRepeat, TrackSize,
    TransformList, TransformOp, TransitionItem, TypedValue, Visibility,
};
use rustkit_css::registered_property::{LengthPercentage, Typed};
use std::collections::HashMap;
use std::sync::Arc;

/// A value one of the transitionable properties takes.
#[derive(Clone, Debug)]
pub(crate) enum AnimValue {
    Number(f32),
    Length(Length),
    Color(Color),
    /// `currentcolor` of a colour property that keeps it as a keyword
    /// (`outline-color`, `text-decoration-color`), with the element's
    /// `color` when it was read. Two of them are equal whatever the colour:
    /// `currentcolor` before and after is no change (Chrome 148 starts no
    /// transition when only `color` jumps). Against a colour it
    /// interpolates as that `color`, which is held where Chrome tracks a
    /// `color` that is itself animating (a stated gap).
    CurrentColor(Color),
    Transform(TransformList),
    /// One of the individual transform properties (css-transforms-2 §5:
    /// `translate`, `rotate`, `scale`); `None` is `none`.
    Individual(Option<TransformOp>),
    /// `line-height`: a number with a number, a length with a length;
    /// `normal` and a number against a length are discrete (Chrome 148
    /// starts no transition).
    LineHeight(LineHeight),
    /// A `border-*-radius` corner: both radii, each a length-percentage.
    Radius(CornerRadius),
    /// `background-size`, one per layer (bottom layer first, as stored).
    BackgroundSizes(Vec<BackgroundSize>),
    /// `visibility`: discrete, but `visible` between the end points when
    /// either end is `visible` (`interpolate_visibility`).
    Visibility(Visibility),
    /// `box-shadow`, in the declaration's order (the first is on top).
    Shadows(Vec<BoxShadow>),
    /// `filter` / `backdrop-filter`: the declared text (what the style
    /// keeps; empty is `none`) and its functions as read for
    /// interpolation (`None` where a function is not one of the
    /// interpolable ones, a `url()` say: such a list does not transition).
    Filter(String, Option<Vec<FilterFn>>),
    /// `grid-template-columns` / `grid-template-rows`: the track list as
    /// declared (`repeat()` kept, which is what decides the shape).
    Grid(GridTemplate),
    /// TRANS-F: a registered custom property's typed computed value.
    Custom(TypedValue),
}

/// One `filter` function as a transition reads it (filter-effects-1 §8):
/// lengths in px, angles in degrees, amounts as numbers (`50%` is 0.5).
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum FilterFn {
    Blur(f32),
    /// `brightness`, `contrast`, `grayscale`, `invert`, `opacity`,
    /// `saturate` or `sepia`.
    Amount(&'static str, f32),
    HueRotate(f32),
    DropShadow {
        x: f32,
        y: f32,
        blur: f32,
        color: Color,
    },
}

impl PartialEq for AnimValue {
    fn eq(&self, other: &Self) -> bool {
        use AnimValue::*;
        match (self, other) {
            (Number(a), Number(b)) => a == b,
            (Length(a), Length(b)) => a == b,
            (Color(a), Color(b)) => a == b,
            (CurrentColor(_), CurrentColor(_)) => true,
            (Transform(a), Transform(b)) => a == b,
            (Individual(a), Individual(b)) => a == b,
            (LineHeight(a), LineHeight(b)) => a == b,
            (Radius(a), Radius(b)) => a == b,
            (BackgroundSizes(a), BackgroundSizes(b)) => a == b,
            (Visibility(a), Visibility(b)) => a == b,
            (Shadows(a), Shadows(b)) => a == b,
            (Filter(a, _), Filter(b, _)) => a == b,
            (Grid(a), Grid(b)) => a == b,
            (Custom(a), Custom(b)) => a == b,
            // Every variant by name: a new one is a compile error here
            // until its equality is decided.
            (
                Number(_) | Length(_) | Color(_) | CurrentColor(_) | Transform(_) | Individual(_)
                | LineHeight(_) | Radius(_) | BackgroundSizes(_) | Visibility(_) | Shadows(_)
                | Filter(..) | Grid(_) | Custom(_),
                _,
            ) => false,
        }
    }
}

struct Prop {
    name: &'static str,
    get: fn(&ComputedStyle) -> AnimValue,
    set: fn(&mut ComputedStyle, &AnimValue),
}

macro_rules! num_prop {
    ($name:literal, $field:ident) => {
        Prop {
            name: $name,
            get: |s| AnimValue::Number(s.$field),
            set: |s, v| {
                if let AnimValue::Number(n) = v {
                    s.$field = *n;
                }
            },
        }
    };
}
macro_rules! len_prop {
    ($name:literal, $field:ident) => {
        Prop {
            name: $name,
            get: |s| AnimValue::Length(s.$field.clone()),
            set: |s, v| {
                if let AnimValue::Length(l) = v {
                    s.$field = l.clone();
                }
            },
        }
    };
}
macro_rules! inset_prop {
    ($name:literal, $field:ident) => {
        Prop {
            name: $name,
            get: |s| AnimValue::Length(s.$field.clone().unwrap_or(Length::Auto)),
            set: |s, v| {
                if let AnimValue::Length(l) = v {
                    s.$field = Some(l.clone());
                }
            },
        }
    };
}
macro_rules! individual_prop {
    ($name:literal, $field:ident) => {
        Prop {
            name: $name,
            get: |s| AnimValue::Individual(s.$field.clone()),
            set: |s, v| {
                if let AnimValue::Individual(op) = v {
                    s.$field = op.clone();
                }
            },
        }
    };
}
macro_rules! radius_prop {
    ($name:literal, $field:ident) => {
        Prop {
            name: $name,
            get: |s| AnimValue::Radius(s.$field.clone()),
            set: |s, v| {
                if let AnimValue::Radius(r) = v {
                    s.$field = r.clone();
                }
            },
        }
    };
}
macro_rules! color_prop {
    ($name:literal, $field:ident) => {
        Prop {
            name: $name,
            get: |s| AnimValue::Color(s.$field),
            set: |s, v| {
                if let AnimValue::Color(c) = v {
                    s.$field = *c;
                }
            },
        }
    };
}

/// A colour property whose `None` is `currentcolor`.
macro_rules! current_color_prop {
    ($name:literal, $($field:ident).+) => {
        Prop {
            name: $name,
            get: |s| match s.$($field).+ {
                Some(c) => AnimValue::Color(c),
                None => AnimValue::CurrentColor(s.color),
            },
            set: |s, v| match v {
                AnimValue::Color(c) => s.$($field).+ = Some(*c),
                AnimValue::CurrentColor(_) => s.$($field).+ = None,
                _ => {}
            },
        }
    };
}

/// The properties a transition can run on, by their CSS names.
const PROPS: &[Prop] = &[
    // Its range is [0, 1]: a `composite: add` past it (`.75 + .5`) clamps,
    // as Chrome's computed value does.
    Prop {
        name: "opacity",
        get: |s| AnimValue::Number(s.opacity),
        set: |s, v| {
            if let AnimValue::Number(n) = v {
                s.opacity = n.clamp(0.0, 1.0);
            }
        },
    },
    Prop {
        name: "transform",
        get: |s| AnimValue::Transform(s.transform.clone()),
        set: |s, v| {
            if let AnimValue::Transform(t) = v {
                s.transform = t.clone();
            }
        },
    },
    individual_prop!("translate", translate),
    individual_prop!("rotate", rotate),
    individual_prop!("scale", scale),
    color_prop!("color", color),
    color_prop!("background-color", background_color),
    color_prop!("border-top-color", border_top_color),
    color_prop!("border-right-color", border_right_color),
    color_prop!("border-bottom-color", border_bottom_color),
    color_prop!("border-left-color", border_left_color),
    current_color_prop!("outline-color", outline.color),
    current_color_prop!("text-decoration-color", text_decoration_color),
    len_prop!("width", width),
    len_prop!("height", height),
    len_prop!("min-width", min_width),
    len_prop!("min-height", min_height),
    len_prop!("max-width", max_width),
    len_prop!("max-height", max_height),
    len_prop!("margin-top", margin_top),
    len_prop!("margin-right", margin_right),
    len_prop!("margin-bottom", margin_bottom),
    len_prop!("margin-left", margin_left),
    len_prop!("padding-top", padding_top),
    len_prop!("padding-right", padding_right),
    len_prop!("padding-bottom", padding_bottom),
    len_prop!("padding-left", padding_left),
    len_prop!("border-top-width", border_top_width),
    len_prop!("border-right-width", border_right_width),
    len_prop!("border-bottom-width", border_bottom_width),
    len_prop!("border-left-width", border_left_width),
    inset_prop!("top", top),
    inset_prop!("right", right),
    inset_prop!("bottom", bottom),
    inset_prop!("left", left),
    len_prop!("font-size", font_size),
    Prop {
        name: "line-height",
        get: |s| AnimValue::LineHeight(s.line_height),
        set: |s, v| {
            if let AnimValue::LineHeight(l) = v {
                s.line_height = *l;
            }
        },
    },
    Prop {
        name: "box-shadow",
        get: |s| AnimValue::Shadows(s.box_shadows.clone()),
        set: |s, v| {
            if let AnimValue::Shadows(list) = v {
                s.box_shadows = list.clone();
            }
        },
    },
    Prop {
        name: "filter",
        get: |s| AnimValue::Filter(s.filter.clone(), parse_filter_list(&s.filter, s.color)),
        set: |s, v| {
            if let AnimValue::Filter(text, _) = v {
                s.filter = text.clone();
            }
        },
    },
    // A `backdrop-filter` stacks only while it is not `none`; a value on
    // the way between two lists is never `none`.
    Prop {
        name: "backdrop-filter",
        get: |s| {
            AnimValue::Filter(
                s.backdrop_filter.clone(),
                parse_filter_list(&s.backdrop_filter, s.color),
            )
        },
        set: |s, v| {
            if let AnimValue::Filter(text, _) = v {
                s.backdrop_filter = text.clone();
                crate::set_stacks(s, rustkit_css::STACKS_BACKDROP, !text.is_empty());
            }
        },
    },
    Prop {
        name: "visibility",
        get: |s| AnimValue::Visibility(s.visibility),
        set: |s, v| {
            if let AnimValue::Visibility(x) = v {
                s.visibility = *x;
            }
        },
    },
    radius_prop!("border-top-left-radius", border_top_left_radius),
    radius_prop!("border-top-right-radius", border_top_right_radius),
    radius_prop!("border-bottom-right-radius", border_bottom_right_radius),
    radius_prop!("border-bottom-left-radius", border_bottom_left_radius),
    // The layer list is the background-image list's: a size written for
    // a different number of layers (a transition overlapping a change of
    // images) is left alone.
    Prop {
        name: "background-size",
        get: |s| {
            AnimValue::BackgroundSizes(s.background_layers.iter().map(|l| l.size.clone()).collect())
        },
        set: |s, v| {
            if let AnimValue::BackgroundSizes(sizes) = v {
                if sizes.len() == s.background_layers.len() {
                    for (layer, size) in s.background_layers.iter_mut().zip(sizes) {
                        layer.size = size.clone();
                    }
                }
            }
        },
    },
    len_prop!("letter-spacing", letter_spacing),
    len_prop!("word-spacing", word_spacing),
    len_prop!("text-indent", text_indent),
    Prop {
        name: "grid-template-columns",
        get: |s| AnimValue::Grid(s.grid_template_columns.clone()),
        set: |s, v| {
            if let AnimValue::Grid(g) = v {
                s.grid_template_columns = g.clone();
            }
        },
    },
    Prop {
        name: "grid-template-rows",
        get: |s| AnimValue::Grid(s.grid_template_rows.clone()),
        set: |s, v| {
            if let AnimValue::Grid(g) = v {
                s.grid_template_rows = g.clone();
            }
        },
    },
];

/// The shorthands a `transition-property` name expands to.
fn expands_to(name: &str) -> &'static [&'static str] {
    match name {
        "all" => &[],
        "margin" => &["margin-top", "margin-right", "margin-bottom", "margin-left"],
        "padding" => &[
            "padding-top",
            "padding-right",
            "padding-bottom",
            "padding-left",
        ],
        "border-color" => &[
            "border-top-color",
            "border-right-color",
            "border-bottom-color",
            "border-left-color",
        ],
        "border-width" => &[
            "border-top-width",
            "border-right-width",
            "border-bottom-width",
            "border-left-width",
        ],
        "border" => &[
            "border-top-color",
            "border-right-color",
            "border-bottom-color",
            "border-left-color",
            "border-top-width",
            "border-right-width",
            "border-bottom-width",
            "border-left-width",
        ],
        "inset" => &["top", "right", "bottom", "left"],
        "outline" => &["outline-color"],
        "text-decoration" => &["text-decoration-color"],
        "background" => &["background-color", "background-size"],
        "font" => &["font-size", "line-height"],
        "grid-template" | "grid" => &["grid-template-columns", "grid-template-rows"],
        "border-radius" => &[
            "border-top-left-radius",
            "border-top-right-radius",
            "border-bottom-right-radius",
            "border-bottom-left-radius",
        ],
        _ => &[],
    }
}

/// The transition item that applies to property `prop` (the last one that
/// names it, or `all`), with a combined duration above zero.
fn spec_for<'a>(items: &'a [TransitionItem], prop: &str) -> Option<&'a TransitionItem> {
    items.iter().rev().find(|item| {
        let name = item.property.trim();
        name.eq_ignore_ascii_case("all")
            || name.eq_ignore_ascii_case(prop)
            || expands_to(&name.to_ascii_lowercase()).contains(&prop)
    })
}

/// Split `value` at `sep` outside parentheses (`cubic-bezier(0, 0, 1, 1)`
/// stays whole).
pub(crate) fn split_top_level(value: &str, sep: char) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut start = 0;
    for (i, c) in value.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth = (depth - 1).max(0),
            c if c == sep && depth == 0 => {
                out.push(value[start..i].trim());
                start = i + c.len_utf8();
            }
            _ => {}
        }
    }
    out.push(value[start..].trim());
    out.retain(|s| !s.is_empty());
    out
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// `a` and `b` interpolated at `t`; `None` when they are not interpolable
/// (css-transitions-1 §3: no transition then starts).
fn interpolate_length(a: &Length, b: &Length, t: f32) -> Option<Length> {
    use Length::*;
    Some(match (a, b) {
        (Px(x), Px(y)) => Px(lerp(*x, *y, t)),
        (Zero, Px(y)) => Px(lerp(0.0, *y, t)),
        (Px(x), Zero) => Px(lerp(*x, 0.0, t)),
        (Percent(x), Percent(y)) => Percent(lerp(*x, *y, t)),
        (Zero, Percent(y)) => Percent(lerp(0.0, *y, t)),
        (Percent(x), Zero) => Percent(lerp(*x, 0.0, t)),
        (Em(x), Em(y)) => Em(lerp(*x, *y, t)),
        (Rem(x), Rem(y)) => Rem(lerp(*x, *y, t)),
        (Vw(x), Vw(y)) => Vw(lerp(*x, *y, t)),
        (Vh(x), Vh(y)) => Vh(lerp(*x, *y, t)),
        (Vmin(x), Vmin(y)) => Vmin(lerp(*x, *y, t)),
        (Vmax(x), Vmax(y)) => Vmax(lerp(*x, *y, t)),
        (Zero, Zero) => Zero,
        // Two units: a `calc()` mix of them (css-values-4 §3.2; Chrome 148
        // reads `10px` to `50%` at the midpoint as `calc(25% + 5px)`).
        _ => return Length::interpolate_as_calc(a, b, t),
    })
}

/// Premultiplied sRGB interpolation (css-color-4 §12, what Chrome does for
/// `color` and the other legacy-colour properties).
fn interpolate_color(a: &Color, b: &Color, t: f32) -> Color {
    let alpha = lerp(a.a, b.a, t);
    if alpha <= 0.0 {
        return Color {
            r: 0,
            g: 0,
            b: 0,
            a: 0.0,
        };
    }
    let channel = |x: u8, y: u8| {
        let v = lerp(x as f32 * a.a, y as f32 * b.a, t) / alpha;
        v.round().clamp(0.0, 255.0) as u8
    };
    Color {
        r: channel(a.r, b.r),
        g: channel(a.g, b.g),
        b: channel(a.b, b.b),
        a: alpha,
    }
}

fn identity_of(op: &TransformOp) -> TransformOp {
    use TransformOp::*;
    match op {
        Translate(..) => Translate(Length::Zero, Length::Zero),
        TranslateX(_) => TranslateX(Length::Zero),
        TranslateY(_) => TranslateY(Length::Zero),
        Scale(..) => Scale(1.0, 1.0),
        ScaleX(_) => ScaleX(1.0),
        ScaleY(_) => ScaleY(1.0),
        Rotate(_) => Rotate(0.0),
        SkewX(_) => SkewX(0.0),
        SkewY(_) => SkewY(0.0),
        Skew(..) => Skew(0.0, 0.0),
        Matrix(..) => Matrix(1.0, 0.0, 0.0, 1.0, 0.0, 0.0),
    }
}

fn interpolate_op(a: &TransformOp, b: &TransformOp, t: f32) -> Option<TransformOp> {
    use TransformOp::*;
    Some(match (a, b) {
        (Translate(ax, ay), Translate(bx, by)) => Translate(
            interpolate_length(ax, bx, t)?,
            interpolate_length(ay, by, t)?,
        ),
        (TranslateX(ax), TranslateX(bx)) => TranslateX(interpolate_length(ax, bx, t)?),
        (TranslateY(ay), TranslateY(by)) => TranslateY(interpolate_length(ay, by, t)?),
        (Scale(ax, ay), Scale(bx, by)) => Scale(lerp(*ax, *bx, t), lerp(*ay, *by, t)),
        (ScaleX(x), ScaleX(y)) => ScaleX(lerp(*x, *y, t)),
        (ScaleY(x), ScaleY(y)) => ScaleY(lerp(*x, *y, t)),
        (Rotate(x), Rotate(y)) => Rotate(lerp(*x, *y, t)),
        (SkewX(x), SkewX(y)) => SkewX(lerp(*x, *y, t)),
        (SkewY(x), SkewY(y)) => SkewY(lerp(*x, *y, t)),
        (Skew(ax, ay), Skew(bx, by)) => Skew(lerp(*ax, *bx, t), lerp(*ay, *by, t)),
        (Matrix(a0, a1, a2, a3, a4, a5), Matrix(b0, b1, b2, b3, b4, b5)) => Matrix(
            lerp(*a0, *b0, t),
            lerp(*a1, *b1, t),
            lerp(*a2, *b2, t),
            lerp(*a3, *b3, t),
            lerp(*a4, *b4, t),
            lerp(*a5, *b5, t),
        ),
        _ => return None,
    })
}

/// css-transforms-1 §9: matching lists interpolate function by function,
/// `none` is the identity of the other list, and anything else falls back
/// to the two matrices (interpolated component-wise here, not decomposed).
fn interpolate_transform(a: &TransformList, b: &TransformList, t: f32) -> TransformList {
    let (a_ops, b_ops): (Vec<TransformOp>, Vec<TransformOp>) =
        match (a.ops.is_empty(), b.ops.is_empty()) {
            (true, true) => return TransformList::none(),
            (true, false) => (b.ops.iter().map(identity_of).collect(), b.ops.clone()),
            (false, true) => (a.ops.clone(), a.ops.iter().map(identity_of).collect()),
            (false, false) => (a.ops.clone(), b.ops.clone()),
        };
    if a_ops.len() == b_ops.len() {
        let ops: Option<Vec<TransformOp>> = a_ops
            .iter()
            .zip(&b_ops)
            .map(|(x, y)| interpolate_op(x, y, t))
            .collect();
        if let Some(ops) = ops {
            return TransformList { ops };
        }
    }
    let ma = a.to_matrix(0.0, 0.0);
    let mb = b.to_matrix(0.0, 0.0);
    TransformList {
        ops: vec![TransformOp::Matrix(
            lerp(ma[0], mb[0], t),
            lerp(ma[1], mb[1], t),
            lerp(ma[2], mb[2], t),
            lerp(ma[3], mb[3], t),
            lerp(ma[4], mb[4], t),
            lerp(ma[5], mb[5], t),
        )],
    }
}

/// css-transforms-2 §5 (individual transforms): `none` is the identity of
/// the other value (`0px`, `0deg`, `1`), and a value interpolates
/// component by component: `translate` per length (one unit each, as
/// `interpolate_length` takes them), `rotate` as a number of degrees with
/// no turn taken off (45deg to 405deg passes 225deg, Chrome 148), and
/// `scale` per factor. The engine keeps only the 2D forms (`rotate`'s z
/// axis), so there is no axis to match.
fn interpolate_individual(
    a: &Option<TransformOp>,
    b: &Option<TransformOp>,
    t: f32,
) -> Option<Option<TransformOp>> {
    // The end points are the values themselves: a transition to `none`
    // ends on `none`, not on the identity it ran to (Chrome 148 reads
    // `none` once it is over).
    if t == 0.0 {
        return Some(a.clone());
    }
    if t == 1.0 {
        return Some(b.clone());
    }
    let (a, b) = match (a, b) {
        (None, None) => return Some(None),
        (None, Some(b)) => (identity_of(b), b.clone()),
        (Some(a), None) => (a.clone(), identity_of(a)),
        (Some(a), Some(b)) => (a.clone(), b.clone()),
    };
    interpolate_op(&a, &b, t).map(Some)
}

/// `box-shadow` (css-backgrounds-3 §7.2): shadow by shadow, the shorter
/// list padded with transparent zero shadows of the other's kind; a pair
/// whose `inset` differs is not interpolable. The end points are the
/// lists themselves (a transition to `none` ends on no shadow).
fn interpolate_shadows(a: &[BoxShadow], b: &[BoxShadow], t: f32) -> Option<Vec<BoxShadow>> {
    if t == 0.0 {
        return Some(a.to_vec());
    }
    if t == 1.0 {
        return Some(b.to_vec());
    }
    let none = |other: &BoxShadow| BoxShadow {
        color: Color {
            r: 0,
            g: 0,
            b: 0,
            a: 0.0,
        },
        inset: other.inset,
        ..BoxShadow::default()
    };
    (0..a.len().max(b.len()))
        .map(|i| {
            let (x, y) = match (a.get(i), b.get(i)) {
                (Some(x), Some(y)) => (*x, *y),
                (Some(x), None) => (*x, none(x)),
                (None, Some(y)) => (none(y), *y),
                (None, None) => unreachable!("i is below the longer length"),
            };
            (x.inset == y.inset).then(|| BoxShadow {
                offset_x: lerp(x.offset_x, y.offset_x, t),
                offset_y: lerp(x.offset_y, y.offset_y, t),
                blur_radius: lerp(x.blur_radius, y.blur_radius, t).max(0.0),
                spread_radius: lerp(x.spread_radius, y.spread_radius, t),
                color: interpolate_color(&x.color, &y.color, t),
                inset: x.inset,
            })
        })
        .collect()
}

/// A `filter` list's functions (`None` where one is not interpolable, or
/// the text does not read). Empty text is `none`, no functions.
/// `currentcolor` is the colour of a `drop-shadow()` that names none.
pub(crate) fn parse_filter_list(text: &str, current: Color) -> Option<Vec<FilterFn>> {
    let amount = |args: &str| -> Option<f32> {
        let args = args.trim();
        if args.is_empty() {
            return Some(1.0);
        }
        match args.strip_suffix('%') {
            Some(p) => p.trim().parse::<f32>().ok().map(|p| p / 100.0),
            None => args.parse::<f32>().ok(),
        }
    };
    let px = |token: &str| -> Option<f32> {
        match crate::parse_length(token)? {
            Length::Percent(_) | Length::Auto | Length::FitContent => None,
            l => Some(l.to_px(16.0, 16.0, 0.0)),
        }
    };
    crate::computed_read::functions(text)?
        .into_iter()
        .map(|(name, args)| {
            Some(match name.as_str() {
                "blur" if args.trim().is_empty() => FilterFn::Blur(0.0),
                "blur" => FilterFn::Blur(px(&args)?),
                "hue-rotate" if args.trim().is_empty() => FilterFn::HueRotate(0.0),
                "hue-rotate" => FilterFn::HueRotate(crate::parse_angle(args.trim())?),
                "brightness" => FilterFn::Amount("brightness", amount(&args)?),
                "contrast" => FilterFn::Amount("contrast", amount(&args)?),
                "grayscale" => FilterFn::Amount("grayscale", amount(&args)?),
                "invert" => FilterFn::Amount("invert", amount(&args)?),
                "opacity" => FilterFn::Amount("opacity", amount(&args)?),
                "saturate" => FilterFn::Amount("saturate", amount(&args)?),
                "sepia" => FilterFn::Amount("sepia", amount(&args)?),
                "drop-shadow" => {
                    let mut lengths = Vec::new();
                    let mut color = None;
                    for token in split_top_level(&args, ' ') {
                        match crate::parse_color(token) {
                            Some(c) => color = Some(c),
                            None => lengths.push(px(token)?),
                        }
                    }
                    if !(2..=3).contains(&lengths.len()) {
                        return None;
                    }
                    FilterFn::DropShadow {
                        x: lengths[0],
                        y: lengths[1],
                        blur: lengths.get(2).copied().unwrap_or(0.0),
                        color: color.unwrap_or(current),
                    }
                }
                _ => return None,
            })
        })
        .collect()
}

/// The value a missing function takes in a filter interpolation (its
/// initial value, filter-effects-1 §8).
fn filter_initial(f: &FilterFn) -> FilterFn {
    match f {
        FilterFn::Blur(_) => FilterFn::Blur(0.0),
        FilterFn::HueRotate(_) => FilterFn::HueRotate(0.0),
        FilterFn::Amount(name, _) => FilterFn::Amount(
            name,
            match *name {
                "grayscale" | "invert" | "sepia" => 0.0,
                _ => 1.0,
            },
        ),
        FilterFn::DropShadow { .. } => FilterFn::DropShadow {
            x: 0.0,
            y: 0.0,
            blur: 0.0,
            color: Color {
                r: 0,
                g: 0,
                b: 0,
                a: 0.0,
            },
        },
    }
}

/// filter-effects-1 §8: function by function when one list's functions
/// are the other's first ones (a missing function at its initial value;
/// `none` is the empty list), else not interpolable.
fn interpolate_filter_fns(a: &[FilterFn], b: &[FilterFn], t: f32) -> Option<Vec<FilterFn>> {
    (0..a.len().max(b.len()))
        .map(|i| {
            let (x, y) = match (a.get(i), b.get(i)) {
                (Some(x), Some(y)) => (x.clone(), y.clone()),
                (Some(x), None) => (x.clone(), filter_initial(x)),
                (None, Some(y)) => (filter_initial(y), y.clone()),
                (None, None) => unreachable!("i is below the longer length"),
            };
            Some(match (x, y) {
                (FilterFn::Blur(p), FilterFn::Blur(q)) => FilterFn::Blur(lerp(p, q, t).max(0.0)),
                (FilterFn::HueRotate(p), FilterFn::HueRotate(q)) => {
                    FilterFn::HueRotate(lerp(p, q, t))
                }
                (FilterFn::Amount(n, p), FilterFn::Amount(m, q)) if n == m => {
                    FilterFn::Amount(n, lerp(p, q, t).max(0.0))
                }
                (
                    FilterFn::DropShadow {
                        x: x0,
                        y: y0,
                        blur: b0,
                        color: c0,
                    },
                    FilterFn::DropShadow {
                        x: x1,
                        y: y1,
                        blur: b1,
                        color: c1,
                    },
                ) => FilterFn::DropShadow {
                    x: lerp(x0, x1, t),
                    y: lerp(y0, y1, t),
                    blur: lerp(b0, b1, t).max(0.0),
                    color: interpolate_color(&c0, &c1, t),
                },
                _ => return None,
            })
        })
        .collect()
}

/// A filter list as the style keeps it (the text paint and
/// `getComputedStyle` read).
fn filter_text(fns: &[FilterFn]) -> String {
    let n = |v: f32| {
        let r = (v * 1000.0).round() / 1000.0;
        format!("{r}")
    };
    fns.iter()
        .map(|f| match f {
            FilterFn::Blur(px) => format!("blur({}px)", n(*px)),
            FilterFn::HueRotate(deg) => format!("hue-rotate({}deg)", n(*deg)),
            FilterFn::Amount(name, v) => format!("{name}({})", n(*v)),
            FilterFn::DropShadow { x, y, blur, color } => format!(
                "drop-shadow({}px {}px {}px rgba({}, {}, {}, {}))",
                n(*x),
                n(*y),
                n(*blur),
                color.r,
                color.g,
                color.b,
                n(color.a)
            ),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// `visibility` (web-animations-1 §5.3, css-transitions-1): discrete,
/// except that with `visible` at either end the value is `visible` for
/// every progress strictly between 0 and 1. With neither end `visible`
/// it is not interpolable, so no transition starts (Chrome 148: `hidden`
/// to `collapse`).
fn interpolate_visibility(a: Visibility, b: Visibility, t: f32) -> Option<Visibility> {
    if a != Visibility::Visible && b != Visibility::Visible {
        return (a == b).then_some(a);
    }
    Some(if t <= 0.0 {
        a
    } else if t >= 1.0 {
        b
    } else {
        Visibility::Visible
    })
}

/// `line-height`: numbers and lengths each with their own kind.
fn interpolate_line_height(a: &LineHeight, b: &LineHeight, t: f32) -> Option<LineHeight> {
    match (a, b) {
        (LineHeight::Number(x), LineHeight::Number(y)) => Some(LineHeight::Number(lerp(*x, *y, t))),
        (LineHeight::Px(x), LineHeight::Px(y)) => Some(LineHeight::Px(lerp(*x, *y, t))),
        _ => None,
    }
}

/// One `background-size` dimension: `None` is `auto` (only with `auto`),
/// a negative number a percentage (`parse_background_size_dimension`'s
/// encoding, `-0.0` for `0%`), else px. A px against a percentage is a
/// `calc()` Chrome interpolates and this encoding cannot hold: not
/// interpolable here (a stated gap).
fn interpolate_size_dimension(a: Option<f32>, b: Option<f32>, t: f32) -> Option<Option<f32>> {
    match (a, b) {
        (None, None) => Some(None),
        (Some(x), Some(y)) if x.is_sign_negative() == y.is_sign_negative() => {
            let v = lerp(x, y, t);
            // Keep `0%` a percentage on the way.
            Some(Some(if x.is_sign_negative() { -v.abs() } else { v }))
        }
        _ => None,
    }
}

/// `background-size`, layer by layer: explicit sizes per dimension;
/// `cover`, `contain` and `auto` are discrete (Chrome 148: `auto` or
/// `cover` to `30px 40px` starts no transition).
fn interpolate_background_sizes(
    a: &[BackgroundSize],
    b: &[BackgroundSize],
    t: f32,
) -> Option<Vec<BackgroundSize>> {
    if a.len() != b.len() {
        return None;
    }
    a.iter()
        .zip(b)
        .map(|(x, y)| match (x, y) {
            (
                BackgroundSize::Explicit {
                    width: aw,
                    height: ah,
                },
                BackgroundSize::Explicit {
                    width: bw,
                    height: bh,
                },
            ) => Some(BackgroundSize::Explicit {
                width: interpolate_size_dimension(*aw, *bw, t)?,
                height: interpolate_size_dimension(*ah, *bh, t)?,
            }),
            (x, y) if x == y => Some(x.clone()),
            _ => None,
        })
        .collect()
}

/// A discrete pair: the first value before the midpoint, the second from
/// it on (web-animations-1 §5.3, the 50% flip).
fn flip<T: Clone>(a: &T, b: &T, t: f32) -> T {
    if t < 0.5 {
        a.clone()
    } else {
        b.clone()
    }
}

/// One grid track size (css-grid-2 §7.2.5): two of one kind interpolate
/// (px, %, fr, the `fit-content()` limit, a `minmax()` end by end), equal
/// keywords stay; `None` is a pair of different kinds, which flips at the
/// midpoint while the rest of the list moves (Chrome 148: `1fr` against
/// `100px`, `100px` against `auto`).
fn interpolate_track_size(a: &TrackSize, b: &TrackSize, t: f32) -> Option<TrackSize> {
    use TrackSize::*;
    Some(match (a, b) {
        (Px(x), Px(y)) => Px(lerp(*x, *y, t).max(0.0)),
        (Percent(x), Percent(y)) => Percent(lerp(*x, *y, t).max(0.0)),
        (Fr(x), Fr(y)) => Fr(lerp(*x, *y, t).max(0.0)),
        (FitContent(x), FitContent(y)) => FitContent(lerp(*x, *y, t).max(0.0)),
        (MinMax(a0, a1), MinMax(b0, b1)) => MinMax(
            Box::new(interpolate_track_size(a0, b0, t)?),
            Box::new(interpolate_track_size(a1, b1, t)?),
        ),
        (x, y) if x == y => x.clone(),
        _ => return None,
    })
}

/// Two track runs of one length, track by track; line names flip.
fn interpolate_tracks(
    a: &[TrackDefinition],
    b: &[TrackDefinition],
    t: f32,
) -> Option<Vec<TrackDefinition>> {
    (a.len() == b.len()).then(|| {
        a.iter()
            .zip(b)
            .map(|(x, y)| TrackDefinition {
                size: interpolate_track_size(&x.size, &y.size, t)
                    .unwrap_or_else(|| flip(&x.size, &y.size, t)),
                line_names: flip(&x.line_names, &y.line_names, t),
            })
            .collect()
    })
}

/// `grid-template-columns` / `-rows` (css-grid-2 §7.2.5): two lists of
/// the same shape (as many tracks, the same `repeat()`s at the same
/// places with the same counts and lengths) interpolate track by track;
/// anything else (`none` against a list, a `repeat()` against the plain
/// list it expands to, different lengths) is not interpolable, so no
/// transition starts (Chrome 148).
fn interpolate_grid_template(a: &GridTemplate, b: &GridTemplate, t: f32) -> Option<GridTemplate> {
    let tracks = interpolate_tracks(&a.tracks, &b.tracks, t)?;
    if a.repeats.len() != b.repeats.len() {
        return None;
    }
    let repeats = a
        .repeats
        .iter()
        .zip(&b.repeats)
        .map(|((at, x), (bt, y))| {
            if at != bt {
                return None;
            }
            let repeat = match (x, y) {
                (TrackRepeat::Count(n, x), TrackRepeat::Count(m, y)) if n == m => {
                    TrackRepeat::Count(*n, interpolate_tracks(x, y, t)?)
                }
                (TrackRepeat::AutoFill(x), TrackRepeat::AutoFill(y)) => {
                    TrackRepeat::AutoFill(interpolate_tracks(x, y, t)?)
                }
                (TrackRepeat::AutoFit(x), TrackRepeat::AutoFit(y)) => {
                    TrackRepeat::AutoFit(interpolate_tracks(x, y, t)?)
                }
                _ => return None,
            };
            Some((*at, repeat))
        })
        .collect::<Option<Vec<_>>>()?;
    // The end points are the lists themselves.
    if t == 0.0 {
        return Some(a.clone());
    }
    if t == 1.0 {
        return Some(b.clone());
    }
    Some(GridTemplate {
        tracks,
        repeats,
        final_line_names: flip(&a.final_line_names, &b.final_line_names, t),
    })
}

/// TRANS-F: two computed values of one registered property, item by item
/// (css-properties-values-api-1 §6: a list interpolates when the lengths
/// match; colours premultiplied, as `color`; a length with a percentage as
/// a `calc()` mix; an `<integer>` rounds). `None` (discrete, no
/// transition) where the shapes differ or an item is a keyword.
fn interpolate_typed(a: &TypedValue, b: &TypedValue, t: f32) -> Option<TypedValue> {
    if a.multiplier != b.multiplier || a.items.len() != b.items.len() {
        return None;
    }
    // The end is the end value itself (`50%`, where the way there was a
    // `calc()` mix).
    if t >= 1.0 {
        return Some(b.clone());
    }
    let items = a
        .items
        .iter()
        .zip(&b.items)
        .map(|(x, y)| {
            Some(match (x, y) {
                (Typed::Color(x), Typed::Color(y)) => Typed::Color(interpolate_color(x, y, t)),
                (Typed::Length(x), Typed::Length(y)) => Typed::Length(lerp(*x, *y, t)),
                (Typed::Number(x), Typed::Number(y)) => Typed::Number(lerp(*x, *y, t)),
                (Typed::Integer(x), Typed::Integer(y)) => Typed::Integer(lerp(*x, *y, t).round()),
                (Typed::Percentage(x), Typed::Percentage(y)) => Typed::Percentage(lerp(*x, *y, t)),
                (Typed::Angle(x), Typed::Angle(y)) => Typed::Angle(lerp(*x, *y, t)),
                (Typed::Time(x), Typed::Time(y)) => Typed::Time(lerp(*x, *y, t)),
                (Typed::LengthPercentage(x), Typed::LengthPercentage(y)) => {
                    use LengthPercentage::*;
                    Typed::LengthPercentage(match (x, y) {
                        (Px(x), Px(y)) => Px(lerp(*x, *y, t)),
                        (Percent(x), Percent(y)) => Percent(lerp(*x, *y, t)),
                        _ => {
                            let parts = |v: &LengthPercentage| match *v {
                                Px(px) => (0.0, px),
                                Percent(p) => (p, 0.0),
                                Calc { percent, px } => (percent, px),
                            };
                            let ((pa, xa), (pb, xb)) = (parts(x), parts(y));
                            Calc {
                                percent: lerp(pa, pb, t),
                                px: lerp(xa, xb, t),
                            }
                        }
                    })
                }
                _ => return None,
            })
        })
        .collect::<Option<Vec<Typed>>>()?;
    Some(TypedValue {
        items,
        multiplier: a.multiplier,
    })
}

fn interpolate(a: &AnimValue, b: &AnimValue, t: f32) -> Option<AnimValue> {
    Some(match (a, b) {
        (AnimValue::Number(x), AnimValue::Number(y)) => AnimValue::Number(lerp(*x, *y, t)),
        (AnimValue::Length(x), AnimValue::Length(y)) => {
            AnimValue::Length(interpolate_length(x, y, t)?)
        }
        (AnimValue::Color(x), AnimValue::Color(y))
        | (AnimValue::CurrentColor(x), AnimValue::Color(y))
        | (AnimValue::Color(x), AnimValue::CurrentColor(y)) => {
            AnimValue::Color(interpolate_color(x, y, t))
        }
        (AnimValue::CurrentColor(_), AnimValue::CurrentColor(y)) => AnimValue::CurrentColor(*y),
        (AnimValue::Transform(x), AnimValue::Transform(y)) => {
            AnimValue::Transform(interpolate_transform(x, y, t))
        }
        (AnimValue::Individual(x), AnimValue::Individual(y)) => {
            AnimValue::Individual(interpolate_individual(x, y, t)?)
        }
        (AnimValue::LineHeight(x), AnimValue::LineHeight(y)) => {
            AnimValue::LineHeight(interpolate_line_height(x, y, t)?)
        }
        (AnimValue::Radius(x), AnimValue::Radius(y)) => AnimValue::Radius(CornerRadius {
            horizontal: interpolate_length(&x.horizontal, &y.horizontal, t)?,
            vertical: interpolate_length(&x.vertical, &y.vertical, t)?,
        }),
        (AnimValue::BackgroundSizes(x), AnimValue::BackgroundSizes(y)) => {
            AnimValue::BackgroundSizes(interpolate_background_sizes(x, y, t)?)
        }
        (AnimValue::Visibility(x), AnimValue::Visibility(y)) => {
            AnimValue::Visibility(interpolate_visibility(*x, *y, t)?)
        }
        (AnimValue::Shadows(x), AnimValue::Shadows(y)) => {
            AnimValue::Shadows(interpolate_shadows(x, y, t)?)
        }
        // The end points keep the declared text.
        (AnimValue::Filter(xt, x), AnimValue::Filter(yt, y)) => {
            let fns = interpolate_filter_fns(x.as_ref()?, y.as_ref()?, t)?;
            match t {
                t if t == 0.0 => AnimValue::Filter(xt.clone(), Some(fns)),
                t if t == 1.0 => AnimValue::Filter(yt.clone(), Some(fns)),
                _ => AnimValue::Filter(filter_text(&fns), Some(fns)),
            }
        }
        (AnimValue::Grid(x), AnimValue::Grid(y)) => {
            AnimValue::Grid(interpolate_grid_template(x, y, t)?)
        }
        (AnimValue::Custom(x), AnimValue::Custom(y)) => AnimValue::Custom(interpolate_typed(x, y, t)?),
        _ => return None,
    })
}

/// CH2.E16: a `::before` or `::after` animates under its own key, the
/// originating element's raw NodeId with the pseudo-element in the top
/// two bits (raw ids never reach them). Every map of this module is keyed
/// by it, so a pseudo-element's transitions and animations live, end and
/// are cancelled on their own, as the element's do.
const PSEUDO_SHIFT: u32 = usize::BITS - 2;

/// The animation key of `node`'s `pseudo` (`::before` or `::after`).
pub(crate) fn pseudo_key(node: usize, pseudo: &str) -> usize {
    let tag: usize = match pseudo {
        "::before" => 1,
        "::after" => 2,
        _ => 0,
    };
    debug_assert!(tag != 0 && node >> PSEUDO_SHIFT == 0, "{pseudo} of {node}");
    node | (tag << PSEUDO_SHIFT)
}

/// The originating element's raw NodeId and the pseudo-element (`None`
/// for the element itself) of an animation key.
pub(crate) fn split_key(key: usize) -> (usize, Option<&'static str>) {
    let node = key & ((1usize << PSEUDO_SHIFT) - 1);
    match key >> PSEUDO_SHIFT {
        1 => (node, Some("::before")),
        2 => (node, Some("::after")),
        _ => (node, None),
    }
}

/// One running transition (css-transitions-1 §3).
#[derive(Clone, Debug)]
struct Transition {
    /// Its identity for script (`CSSTransition`), unique in the view.
    id: u64,
    node: usize,
    prop: usize,
    from: AnimValue,
    to: AnimValue,
    /// When the delay was counted from.
    created_ms: f64,
    delay_ms: f64,
    duration_ms: f64,
    timing: TimingFunction,
    reversing_adjusted_start: AnimValue,
    reversing_shortening: f32,
    /// `transitionstart` has fired (the active phase began).
    started: bool,
    done: bool,
}

impl Transition {
    fn start_ms(&self) -> f64 {
        self.created_ms + self.delay_ms
    }
    fn end_ms(&self) -> f64 {
        self.start_ms() + self.duration_ms
    }
    /// Input progress at `now`, 0 before the active phase.
    fn progress(&self, now: f64) -> f32 {
        if self.duration_ms <= 0.0 {
            return if now >= self.start_ms() { 1.0 } else { 0.0 };
        }
        (((now - self.start_ms()) / self.duration_ms) as f32).clamp(0.0, 1.0)
    }
    fn value_at(&self, now: f64) -> AnimValue {
        let eased = self.timing.evaluate(self.progress(now));
        interpolate(&self.from, &self.to, eased).unwrap_or_else(|| self.to.clone())
    }
    /// `elapsedTime` of `transitionrun`/`transitionstart`.
    fn elapsed_at_start(&self) -> f32 {
        ((-self.delay_ms).max(0.0).min(self.duration_ms) / 1000.0) as f32
    }
    /// `elapsedTime` of `transitioncancel`: the active time at `now`.
    fn elapsed_at(&self, now: f64) -> f32 {
        ((now - self.start_ms()).clamp(0.0, self.duration_ms) / 1000.0) as f32
    }
}

/// A transition or animation event to fire at an element once the build
/// is over: `property` for a `transition*` event, `name` for an
/// `animation*` one.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct AnimationEvent {
    pub(crate) node: usize,
    pub(crate) kind: &'static str,
    pub(crate) property: String,
    pub(crate) name: String,
    pub(crate) elapsed_s: f32,
}

/// The view's running transitions and what they need between builds.
#[derive(Default)]
pub(crate) struct Animations {
    /// The animation clock, ms on the page's timer clock.
    pub(crate) clock_ms: f64,
    /// The clock the last build ran at (`None` before any).
    pub(crate) last_build_clock: Option<f64>,
    /// Transitions do not start (parity captures): values jump.
    pub(crate) disabled: bool,
    /// The transitionable values each element with a `transition` cascaded
    /// to at the last build, by raw NodeId and property index.
    base: HashMap<usize, Vec<(usize, AnimValue)>>,
    transitions: Vec<Transition>,
    events: Vec<AnimationEvent>,
    /// The full build in progress (or the last one), and the build each
    /// element was last handed to `on_style` in. An element a full build
    /// did not visit (under a `display: none` ancestor, or gone) has no
    /// computed style any more: its transitions are cancelled at the end
    /// of that build and its base dropped.
    build_serial: u64,
    seen: HashMap<usize, u64>,
    /// Property names a `transition` named that the engine cannot run,
    /// once each per document, for the Inspector's unsupported APIs.
    reported: std::collections::BTreeSet<String>,
    pending_unsupported: Vec<String>,
    /// The running `@keyframes` animations, one per element and
    /// `animation` list index.
    css: Vec<CssAnimation>,
    /// The last id handed to a transition or animation (one sequence
    /// for both, so an id names one record of either kind).
    last_id: u64,
    /// LAT2: each animated element's style as the last build cascaded it,
    /// and what the build painted of it, so that a frame can be restyled
    /// without a build (`LayoutCtx::animation_frame`).
    pub(crate) captured: HashMap<usize, Captured>,
    /// A2.2: the svg paint lane (`svg_update`). `fill` / `stroke` of the
    /// elements of inline svgs, which have no `ComputedStyle`: each one a
    /// transition covers, with its resolved value as of the last pass, by
    /// raw NodeId and property name.
    svg_base: HashMap<(usize, &'static str), Paint>,
    svg_transitions: Vec<SvgTransition>,
    /// What the last lane pass resolved: the author paint map and the DOM
    /// generation. While neither changed and nothing moves, a pass would
    /// find what the last one found (`LayoutCtx::svg_paint_transitions`).
    pub(crate) svg_last: Option<(std::rc::Weak<HashMap<usize, crate::SvgPaintStyle>>, u64)>,
    /// TRANS-F: the document's registered custom properties, as the last
    /// build installed them.
    pub(crate) registry: Option<Arc<rustkit_css::PropertyRegistry>>,
    /// The registered custom properties a transition ran on: a transition's
    /// `prop` of `CUSTOM_BASE + k` is `custom_names[k]`.
    custom_names: Vec<String>,
    /// The registered values the last overlay set, by element: the build
    /// cascades the element again with them (`take_custom_overlay`).
    custom_overlay: HashMap<usize, HashMap<String, String>>,
}

/// TRANS-F: a transition's `prop` at or past this is a registered custom
/// property (`Animations::custom_names`), not a `PROPS` row.
const CUSTOM_BASE: usize = 1 << 20;

/// A2.2: a `fill` or `stroke` transition of an element of an inline svg.
/// Its timing is a `Transition`'s (its `prop` unused: svg paint is no
/// `PROPS` row), between two colours.
#[derive(Clone, Debug)]
struct SvgTransition {
    t: Transition,
    prop: &'static str,
}

/// An animated element as the last build (or animation frame) left it.
#[derive(Clone, Debug)]
pub(crate) struct Captured {
    /// Its cascaded style, before transitions, animations and script's
    /// animated-style layer.
    pub(crate) cascaded: ComputedStyle,
    /// The animatable properties' values after them (`all_values`).
    pub(crate) values: Vec<AnimValue>,
    /// `animation_settled_opacity` after them (paint reads it for a box
    /// that names an animation).
    pub(crate) settled_opacity: Option<f32>,
    /// Script's animated-style entries the build applied.
    pub(crate) script: Option<Vec<(String, String, String)>>,
}

impl Animations {
    /// A new document: nothing of the old one's transitions carries over.
    /// Ids keep counting: a wrapper of the old document never names a
    /// record of the new one.
    pub(crate) fn reset(&mut self) {
        *self = Animations {
            disabled: self.disabled,
            last_id: self.last_id,
            ..Animations::default()
        };
    }

    /// A fresh id for a transition or animation.
    fn next_id(&mut self) -> u64 {
        self.last_id += 1;
        self.last_id
    }

    /// The CSS name of transition property `prop`: a `PROPS` row or a
    /// registered custom property.
    fn prop_label(&self, prop: usize) -> &str {
        match prop.checked_sub(CUSTOM_BASE) {
            Some(k) => self.custom_names.get(k).map_or("", String::as_str),
            None => PROPS[prop].name,
        }
    }

    /// The `prop` index of registered custom property `name`.
    fn custom_index(&mut self, name: &str) -> usize {
        let k = match self.custom_names.iter().position(|n| n == name) {
            Some(k) => k,
            None => {
                self.custom_names.push(name.to_string());
                self.custom_names.len() - 1
            }
        };
        CUSTOM_BASE + k
    }

    /// Whether a registered custom property's transition runs: what reads it
    /// (`var()`) is known only to a cascade, so a frame takes a build.
    pub(crate) fn custom_running(&self) -> bool {
        self.transitions.iter().any(|t| t.prop >= CUSTOM_BASE)
    }

    /// The registered values `node`'s overlay set in this build, if any.
    pub(crate) fn take_custom_overlay(&mut self, node: usize) -> Option<HashMap<String, String>> {
        self.custom_overlay.remove(&node)
    }

    /// The registered custom properties `specs` names (by name: `all`
    /// does not cover them, as in Chrome 148) with a typed syntax, and
    /// their values in `style`.
    fn custom_values(&mut self, specs: &[TransitionItem], style: &ComputedStyle) -> Vec<(usize, AnimValue)> {
        let Some(registry) = self.registry.clone() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for item in specs {
            let name = item.property.trim();
            if !name.starts_with("--") {
                continue;
            }
            let Some(reg) = registry.get(name) else {
                continue;
            };
            let value = style
                .custom_properties
                .get(name)
                .and_then(|v| reg.syntax.compute(v, &Default::default()));
            let Some(value) = value else {
                continue;
            };
            let i = self.custom_index(name);
            if !out.iter().any(|(j, _)| *j == i) {
                out.push((i, AnimValue::Custom(value)));
            }
        }
        out
    }

    /// The transition property names reported since the last call
    /// (`CSSTransition.<name>` in the Inspector's unsupported APIs).
    pub(crate) fn take_unsupported(&mut self) -> Vec<String> {
        std::mem::take(&mut self.pending_unsupported)
    }

    fn note_unsupported(&mut self, specs: &[TransitionItem]) {
        for item in specs {
            let name = item.property.trim().to_ascii_lowercase();
            // A custom property is no gap: an unregistered one is discrete
            // in Chrome too, and a registered one runs (TRANS-F).
            if name == "all"
                || name == "none"
                || name.starts_with("--")
                || PROPS.iter().any(|p| p.name == name)
                || SVG_PAINT_PROPS.contains(&name.as_str())
                || !expands_to(&name).is_empty()
                || self.reported.contains(&name)
            {
                continue;
            }
            self.reported.insert(name.clone());
            self.pending_unsupported.push(name);
        }
    }

    /// Whether the live loop has to keep turning for this view: a
    /// transition is running or an event waits to be fired.
    pub(crate) fn active(&self) -> bool {
        !self.transitions.is_empty()
            || !self.events.is_empty()
            || self.css.iter().any(CssAnimation::moving)
            || !self.svg_transitions.is_empty()
    }

    pub(crate) fn take_events(&mut self) -> Vec<AnimationEvent> {
        std::mem::take(&mut self.events)
    }

    fn push_event(&mut self, t: &Transition, kind: &'static str, elapsed_s: f32) {
        self.events.push(AnimationEvent {
            node: t.node,
            kind,
            property: self.prop_label(t.prop).to_string(),
            name: String::new(),
            elapsed_s,
        });
    }

    /// Whether a transition or an animation is moving: a build must then
    /// walk the DOM, since a kept box tree would carry the overlaid value
    /// of one frame. (A paused or filled animation holds its value.)
    pub(crate) fn running(&self) -> bool {
        !self.transitions.is_empty() || self.css.iter().any(CssAnimation::moving)
    }

    /// A full build of the view starts: what it visits is noted.
    pub(crate) fn begin_build(&mut self) {
        self.build_serial += 1;
    }

    /// The full build is over: elements it did not visit have no computed
    /// style (css-transitions-1 §3: a transition on an element that no
    /// longer has one is cancelled). A build that took the kept box tree
    /// (`reused_tree`) visited no element at all, and a pointer restyle's
    /// or a kept-tree splice's partial build (`partial_build`) visited only
    /// what is under the changed elements; the rest are all still there
    /// with their styles:
    /// nothing is cancelled and the before-change values stay for the
    /// next change. (The restyle itself misses to a full build when a
    /// changed element left the document.)
    pub(crate) fn end_build(&mut self, reused_tree: bool, partial_build: bool) {
        if reused_tree || partial_build {
            return;
        }
        let serial = self.build_serial;
        let seen = |node: usize, seen: &HashMap<usize, u64>| seen.get(&node) == Some(&serial);
        let mut gone = Vec::new();
        self.transitions.retain(|t| {
            if seen(t.node, &self.seen) {
                true
            } else {
                gone.push(t.clone());
                false
            }
        });
        let now = self.clock_ms;
        for t in gone {
            self.push_event(&t, "transitioncancel", t.elapsed_at(now));
        }
        self.base.retain(|node, _| seen(*node, &self.seen));
        let mut gone = Vec::new();
        self.css.retain(|a| {
            if seen(a.node, &self.seen) {
                true
            } else {
                gone.push(a.clone());
                false
            }
        });
        for a in gone {
            self.cancel_animation_event(&a);
        }
        self.captured.retain(|node, _| seen(*node, &self.seen));
        self.seen.retain(|_, s| *s == serial);
    }

    /// The elements whose before-change values `end_build` would drop if the build so far ended
    /// now as `partial_build` says (none for a partial build): those the build has not visited.
    /// `boxes-verify` reads it after a splice, before its comparison build visits the rest.
    pub(crate) fn would_sweep(&self, partial_build: bool) -> Vec<usize> {
        if partial_build {
            return Vec::new();
        }
        let serial = self.build_serial;
        self.base
            .keys()
            .copied()
            .filter(|node| self.seen.get(node) != Some(&serial))
            .collect()
    }

    /// Whether the build in progress has handed `node` to `on_style`.
    pub(crate) fn visited_this_build(&self, node: usize) -> bool {
        self.seen.get(&node) == Some(&self.build_serial)
    }

    /// Whether a build of this document has handed `node` to `on_style`:
    /// it has a computed style and may have a box. A node no build visited
    /// (detached, or under a `display: none` ancestor) paints nothing, so
    /// an animation on it changes nothing a frame has to place (LAT14).
    pub(crate) fn visited(&self, node: usize) -> bool {
        self.seen.contains_key(&node)
    }

    /// Whether `node` has a transition or a CSS animation record.
    pub(crate) fn has_records(&self, node: usize) -> bool {
        self.transitions.iter().any(|t| t.node == node) || self.css.iter().any(|a| a.node == node)
    }

    /// The elements with a transition or a CSS animation record.
    pub(crate) fn record_nodes(&self) -> Vec<usize> {
        let mut nodes: Vec<usize> = self
            .transitions
            .iter()
            .map(|t| t.node)
            .chain(self.css.iter().map(|a| a.node))
            .collect();
        nodes.sort_unstable();
        nodes.dedup();
        nodes
    }

    /// Cancel this element's transitions (it lost its computed style).
    fn cancel_node(&mut self, node: usize) {
        let now = self.clock_ms;
        let mut gone = Vec::new();
        self.transitions.retain(|t| {
            if t.node == node {
                gone.push(t.clone());
                false
            } else {
                true
            }
        });
        for t in gone {
            self.push_event(&t, "transitioncancel", t.elapsed_at(now));
        }
        self.base.remove(&node);
        let mut gone = Vec::new();
        self.css.retain(|a| {
            if a.node == node {
                gone.push(a.clone());
                false
            } else {
                true
            }
        });
        for a in gone {
            self.cancel_animation_event(&a);
        }
    }

    /// `animationcancel` for an animation taken away while in its active
    /// phase: none for one still in its delay, none after its
    /// `animationend` (css-animations-2 event dispatch; Chrome).
    fn cancel_animation_event(&mut self, a: &CssAnimation) {
        if a.started && !a.ended {
            let elapsed = a.elapsed_at(self.clock_ms);
            self.push_animation_event(a, "animationcancel", elapsed);
        }
    }

    /// Cancel the transitions of elements that `alive` no longer finds in
    /// the document (css-transitions-1 §3: an element removed from the
    /// document ends its transitions with `transitioncancel`).
    pub(crate) fn prune(&mut self, alive: impl Fn(usize) -> bool) {
        // A pseudo-element's records live with its element.
        let alive = |key: usize| alive(split_key(key).0);
        let now = self.clock_ms;
        let mut gone = Vec::new();
        self.transitions.retain(|t| {
            if alive(t.node) {
                true
            } else {
                gone.push(t.clone());
                false
            }
        });
        for t in gone {
            self.push_event(&t, "transitioncancel", t.elapsed_at(now));
            self.base.remove(&t.node);
        }
        let mut gone = Vec::new();
        self.css.retain(|a| {
            if alive(a.node) {
                true
            } else {
                gone.push(a.clone());
                false
            }
        });
        for a in gone {
            self.cancel_animation_event(&a);
        }
        let mut gone = Vec::new();
        self.svg_transitions.retain(|t| {
            if alive(t.t.node) {
                true
            } else {
                gone.push(t.clone());
                false
            }
        });
        for t in gone {
            self.push_svg_event(&t, "transitioncancel", t.t.elapsed_at(now));
        }
        self.svg_base.retain(|(node, _), _| alive(*node));
    }

    /// The build's hand-off for one element: start the transitions its
    /// changed values call for, run its `@keyframes` animations, and
    /// overlay both on `style` (animations under transitions, the
    /// cascade's order). `resolve` reads a `@keyframes` by name against
    /// the element's cascaded style.
    pub(crate) fn on_style(
        &mut self,
        node: usize,
        style: &mut ComputedStyle,
        resolve: &dyn Fn(&str, &ComputedStyle) -> Option<Vec<Frame>>,
    ) {
        self.seen.insert(node, self.build_serial);
        // `display: none`: no computed style to animate (its subtree is not
        // visited at all); nothing to transition from when shown, and an
        // animation starts over then (Chrome).
        if style.display == Display::None {
            self.cancel_node(node);
            return;
        }
        self.transitions_update(node, style);
        self.animations_update(node, style, resolve);
        self.transitions_overlay(node, style);
        // CH2.B18b: an animated or transitioning width on a `none` side
        // (no style, or `none`/`hidden`) is still a 0 used width (Chromium
        // 143: `@keyframes { border-width: 5px }` on an unstyled div is 0).
        crate::zero_width_of_borderless_sides(style);
    }

    /// Start the transitions this element's changed values call for.
    fn transitions_update(&mut self, node: usize, style: &mut ComputedStyle) {
        let now = self.clock_ms;
        let has_running = self.transitions.iter().any(|t| t.node == node);
        if style.transitions.is_empty() && !has_running {
            self.base.remove(&node);
            return;
        }
        let specs = style.transitions.clone();
        self.note_unsupported(&specs);
        let mut covered: Vec<usize> = (0..PROPS.len())
            .filter(|&i| spec_for(&specs, PROPS[i].name).is_some())
            .collect();
        let mut new_vals: Vec<(usize, AnimValue)> = covered
            .iter()
            .map(|&i| (i, (PROPS[i].get)(style)))
            .collect();
        for (i, v) in self.custom_values(&specs, style) {
            covered.push(i);
            new_vals.push((i, v));
        }
        let prev = self.base.insert(node, new_vals.clone());

        // The transition's spec went away (`transition: none`, or the
        // property is no longer named): the running ones are cancelled.
        let mut cancelled = Vec::new();
        self.transitions.retain(|t| {
            if t.node == node && !covered.contains(&t.prop) {
                cancelled.push(t.clone());
                false
            } else {
                true
            }
        });
        for t in cancelled {
            self.push_event(&t, "transitioncancel", t.elapsed_at(now));
        }

        if let Some(prev) = prev.filter(|_| !self.disabled) {
            for (i, new) in &new_vals {
                let Some(old) = prev.iter().find(|(j, _)| j == i).map(|(_, v)| v) else {
                    continue;
                };
                let spec = match i.checked_sub(CUSTOM_BASE) {
                    Some(_) => {
                        let name = self.prop_label(*i);
                        specs.iter().rev().find(|item| item.property.trim() == name)
                    }
                    None => spec_for(&specs, PROPS[*i].name),
                };
                let Some(spec) = spec else {
                    continue;
                };
                let running = self
                    .transitions
                    .iter()
                    .position(|t| t.node == node && t.prop == *i);
                // Already heading there: the running one keeps going.
                if let Some(at) = running {
                    if self.transitions[at].to == *new {
                        continue;
                    }
                } else if old == new {
                    continue;
                }
                // The before-change value: what the element shows now.
                let (from, reversing) = match running {
                    Some(at) => {
                        let t = self.transitions.remove(at);
                        let current = t.value_at(now);
                        self.push_event(&t, "transitioncancel", t.elapsed_at(now));
                        (current, Some(t))
                    }
                    None => (old.clone(), None),
                };
                let Some(t) = self.new_transition(node, *i, from, new, spec, reversing) else {
                    continue;
                };
                self.push_event(&t, "transitionrun", t.elapsed_at_start());
                self.transitions.push(t);
            }
        }
    }

    /// A transition of `node`'s property `prop` from `from` to `new` under
    /// `spec`, starting at the clock; `reversing` is the running one it
    /// replaces. `None` when there is nothing to run: no duration and no
    /// delay, or values that do not interpolate.
    fn new_transition(
        &mut self,
        node: usize,
        prop: usize,
        from: AnimValue,
        new: &AnimValue,
        spec: &TransitionItem,
        reversing: Option<Transition>,
    ) -> Option<Transition> {
        let now = self.clock_ms;
        // To the microsecond: `0.1` seconds as an f32 is 100.0000015
        // ms as an f64, and an end at exactly 1100 would be missed.
        let us = |ms: f64| (ms * 1000.0).round() / 1000.0;
        let duration_ms = us((spec.duration.max(0.0) as f64) * 1000.0);
        let delay_ms = us(spec.delay as f64 * 1000.0);
        if duration_ms + delay_ms.max(0.0) <= 0.0 || interpolate(&from, new, 0.5).is_none() {
            return None;
        }
        // css-transitions-1 §3, the reversing shortening factor:
        // a transition back to where the cancelled one started
        // takes as long as that one had got.
        let (reversing_adjusted_start, factor) = match reversing {
            Some(t) if t.reversing_adjusted_start == *new => {
                let p = t.timing.evaluate(t.progress(now));
                let f = (p * t.reversing_shortening + (1.0 - t.reversing_shortening))
                    .abs()
                    .clamp(0.0, 1.0);
                (t.to.clone(), f)
            }
            _ => (from.clone(), 1.0),
        };
        let delay_ms = if delay_ms < 0.0 {
            us(delay_ms * factor as f64)
        } else {
            delay_ms
        };
        let id = self.next_id();
        Some(Transition {
            id,
            node,
            prop,
            from,
            to: new.clone(),
            created_ms: now,
            delay_ms,
            duration_ms: us(duration_ms * factor as f64),
            timing: spec.timing.clone(),
            reversing_adjusted_start,
            reversing_shortening: factor,
            started: false,
            done: false,
        })
    }

    /// Overlay the running transitions of this element.
    fn transitions_overlay(&mut self, node: usize, style: &mut ComputedStyle) {
        let now = self.clock_ms;
        let mut custom: HashMap<String, String> = HashMap::new();
        let mut i = 0;
        while i < self.transitions.len() {
            if self.transitions[i].node != node {
                i += 1;
                continue;
            }
            let t = &mut self.transitions[i];
            if !t.started && now >= t.start_ms() {
                t.started = true;
                let (t, elapsed) = (t.clone(), t.elapsed_at_start());
                self.push_event(&t, "transitionstart", elapsed);
            }
            let t = &mut self.transitions[i];
            let value = t.value_at(now);
            match t.prop.checked_sub(CUSTOM_BASE) {
                Some(k) => {
                    if let (Some(name), AnimValue::Custom(v)) = (self.custom_names.get(k), &value) {
                        custom.insert(name.clone(), v.to_css());
                    }
                }
                None => (PROPS[t.prop].set)(style, &value),
            }
            let t = &mut self.transitions[i];
            if now >= t.end_ms() {
                t.done = true;
                let (t, elapsed) = (t.clone(), (t.duration_ms / 1000.0) as f32);
                self.push_event(&t, "transitionend", elapsed);
                self.transitions.remove(i);
            } else {
                i += 1;
            }
        }
        if !custom.is_empty() {
            let layer = custom.iter().map(|(k, v)| (k.clone(), Some(v.clone()))).collect();
            style.custom_properties =
                Arc::new(rustkit_css::CustomProperties::over(&style.custom_properties, layer));
            self.custom_overlay.insert(node, custom);
        } else {
            self.custom_overlay.remove(&node);
        }
    }
}

/// `transition` shorthand: a comma-separated list of
/// `[property] || duration || timing-function || delay` items; the first
/// time is the duration, the second the delay. `none` is the empty list.
pub(crate) fn parse_transition_shorthand(
    value: &str,
    parse_time: fn(&str) -> Option<f32>,
    parse_timing: fn(&str) -> TimingFunction,
) -> Vec<TransitionItem> {
    let value = value.trim();
    if value.eq_ignore_ascii_case("none") || value.is_empty() {
        return Vec::new();
    }
    split_top_level(value, ',')
        .into_iter()
        .map(|item| {
            let mut out = TransitionItem {
                property: "all".to_string(),
                ..Default::default()
            };
            let mut times = 0;
            for token in split_top_level(item, ' ') {
                let lower = token.to_ascii_lowercase();
                if let Some(t) = parse_time(&lower) {
                    if times == 0 {
                        out.duration = t;
                    } else {
                        out.delay = t;
                    }
                    times += 1;
                } else if is_timing_keyword(&lower) {
                    out.timing = parse_timing(&lower);
                } else if token.starts_with("--") {
                    // A custom property's name is case-sensitive.
                    out.property = token.to_string();
                } else if !lower.is_empty() {
                    out.property = lower;
                }
            }
            out
        })
        .collect()
}

fn is_timing_keyword(token: &str) -> bool {
    matches!(
        token,
        "ease" | "linear" | "ease-in" | "ease-out" | "ease-in-out" | "step-start" | "step-end"
    ) || token.starts_with("cubic-bezier(")
        || token.starts_with("steps(")
}

/// `transition-property`: the list sets how many items there are; the
/// other longhands' values repeat to fill them.
pub(crate) fn set_transition_properties(style: &mut ComputedStyle, value: &str) {
    let value = value.trim();
    if value.eq_ignore_ascii_case("none") {
        style.transitions.clear();
        return;
    }
    let names = split_top_level(value, ',');
    let old = std::mem::take(&mut style.transitions);
    style.transitions = names
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let mut item = if old.is_empty() {
                TransitionItem::default()
            } else {
                old[i % old.len()].clone()
            };
            // A custom property's name is case-sensitive.
            item.property = if name.starts_with("--") { name.to_string() } else { name.to_ascii_lowercase() };
            item
        })
        .collect();
}

/// A `transition-duration`/`-delay`/`-timing-function` list over the
/// items there are (`all` items when none was named yet).
pub(crate) fn set_transition_component<T: Clone>(
    style: &mut ComputedStyle,
    values: Vec<T>,
    set: fn(&mut TransitionItem, T),
) {
    if values.is_empty() {
        return;
    }
    if style.transitions.is_empty() {
        style.transitions = values
            .iter()
            .map(|_| TransitionItem {
                property: "all".to_string(),
                ..Default::default()
            })
            .collect();
    }
    let n = values.len();
    for (i, item) in style.transitions.iter_mut().enumerate() {
        set(item, values[i % n].clone());
    }
}

/// The scalar transition fields follow the first item (what
/// `getComputedStyle` serializes).
pub(crate) fn sync_transition_scalars(style: &mut ComputedStyle, none: bool) {
    match style.transitions.first() {
        Some(item) => {
            style.transition_property = style
                .transitions
                .iter()
                .map(|item| item.property.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            style.transition_duration = item.duration;
            style.transition_delay = item.delay;
            style.transition_timing_function = item.timing.clone();
        }
        None => {
            style.transition_property = if none {
                "none".to_string()
            } else {
                String::new()
            };
            style.transition_duration = 0.0;
            style.transition_delay = 0.0;
            style.transition_timing_function = TimingFunction::Ease;
        }
    }
}

// ------------------------------------------------------------ @keyframes

/// A keyframe of a resolved `@keyframes`: its offset, the values it sets
/// and its own timing function, when it declares one.
#[derive(Clone, Debug)]
pub(crate) struct Frame {
    pub(crate) offset: f32,
    pub(crate) values: Vec<(usize, AnimValue)>,
    pub(crate) timing: Option<TimingFunction>,
}

/// The indices of the animatable properties `name` sets: its own, or the
/// longhands a shorthand expands to.
pub(crate) fn prop_indices(name: &str) -> Vec<usize> {
    let name = name.to_ascii_lowercase();
    if let Some(i) = PROPS.iter().position(|p| p.name == name) {
        return vec![i];
    }
    expands_to(&name)
        .iter()
        .filter_map(|n| PROPS.iter().position(|p| p.name == *n))
        .collect()
}

/// The value of animatable property `index` in `style`.
pub(crate) fn prop_value(style: &ComputedStyle, index: usize) -> AnimValue {
    (PROPS[index].get)(style)
}

/// TRANS-F: onto `fresh` (the element cascaded again with its moving
/// registered values), the values its animations set over `cascaded` (its
/// first cascade) as `animated` shows them.
pub(crate) fn carry_animated(cascaded: &ComputedStyle, animated: &ComputedStyle, fresh: &mut ComputedStyle) {
    for p in PROPS {
        let now = (p.get)(animated);
        if now != (p.get)(cascaded) {
            (p.set)(fresh, &now);
        }
    }
    fresh.animation_settled_opacity = animated.animation_settled_opacity;
}

/// The name of animatable property `index`.
pub(crate) fn prop_name(index: usize) -> &'static str {
    PROPS[index].name
}

/// Every animatable property's value in `style`, by index.
pub(crate) fn all_values(style: &ComputedStyle) -> Vec<AnimValue> {
    PROPS.iter().map(|p| (p.get)(style)).collect()
}

/// Whether animatable property `index` changes what is painted and
/// nothing layout reads (LAT2): a frame that moves only these keeps the
/// laid-out boxes. `color` inherits, so the frame repaints the text runs
/// that take it too (`Engine::paint_animation_frame`).
pub(crate) fn is_paint_prop(index: usize) -> bool {
    matches!(
        PROPS[index].name,
        "opacity"
            | "transform"
            | "translate"
            | "rotate"
            | "scale"
            | "color"
            | "background-color"
            | "border-top-color"
            | "border-right-color"
            | "border-bottom-color"
            | "border-left-color"
            | "outline-color"
            // Ink only: a shadow adds to no box and no overflow layout reads.
            | "box-shadow"
    )
}

/// One running CSS animation (css-animations-1 §4, web-animations-1 §4.8
/// for the timing).
#[derive(Clone, Debug)]
struct CssAnimation {
    /// Its identity for script (`CSSAnimation`), unique in the view: a
    /// restart or a same-name replacement is a new animation.
    id: u64,
    node: usize,
    /// Its position in the element's `animation` list.
    index: usize,
    name: String,
    frames: Vec<Frame>,
    /// When its delay was counted from (moved on when it is resumed).
    created_ms: f64,
    delay_ms: f64,
    duration_ms: f64,
    iterations: f32,
    direction: AnimationDirection,
    fill: AnimationFillMode,
    timing: TimingFunction,
    /// The local time it was paused at.
    paused_local: Option<f64>,
    /// `animationstart` has fired.
    started: bool,
    /// The iteration `animationiteration` last fired for.
    iterations_seen: u32,
    /// `animationend` has fired: the active phase is over.
    ended: bool,
    /// Script cancelled it: no effect and not listed, and not started
    /// again while the CSS still names it (as in Chrome).
    cancelled: bool,
    /// Script's `pause()`/`play()` overrides `animation-play-state` until
    /// the CSS value changes.
    script_paused: Option<bool>,
    /// The `animation-play-state` last cascaded, to see it change.
    css_play_state: AnimationPlayState,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Before,
    Active,
    After,
}

impl CssAnimation {
    fn moving(&self) -> bool {
        !self.ended && !self.cancelled && self.paused_local.is_none()
    }
    fn local(&self, now: f64) -> f64 {
        self.paused_local.unwrap_or(now - self.created_ms)
    }
    /// web-animations-1 §4.8.2: zero whenever the duration is zero (so
    /// `0s infinite` is not 0 × ∞ = NaN), else the duration times the
    /// iteration count.
    fn active_duration(&self) -> f64 {
        if self.duration_ms <= 0.0 || self.iterations <= 0.0 {
            0.0
        } else if self.iterations.is_infinite() {
            f64::INFINITY
        } else {
            self.duration_ms * self.iterations as f64
        }
    }
    fn phase(&self, local: f64) -> Phase {
        if local < self.delay_ms {
            Phase::Before
        } else if local - self.delay_ms < self.active_duration() {
            Phase::Active
        } else {
            Phase::After
        }
    }
    /// The iteration and the directed progress at local time `local`, or
    /// `None` when the animation is not in effect (outside the active
    /// phase with no fill that way).
    fn progress(&self, local: f64) -> Option<(u32, f32)> {
        let phase = self.phase(local);
        let active = match phase {
            Phase::Before => {
                if matches!(
                    self.fill,
                    AnimationFillMode::Backwards | AnimationFillMode::Both
                ) {
                    0.0
                } else {
                    return None;
                }
            }
            Phase::Active => local - self.delay_ms,
            Phase::After => {
                if matches!(
                    self.fill,
                    AnimationFillMode::Forwards | AnimationFillMode::Both
                ) {
                    self.active_duration()
                } else {
                    return None;
                }
            }
        };
        let (iteration, p) = if self.duration_ms <= 0.0 {
            match phase {
                Phase::Before => (0.0, 0.0),
                _ => ((self.iterations.ceil().max(1.0) - 1.0) as f64, 1.0),
            }
        } else {
            let overall = active / self.duration_ms;
            let mut iteration = overall.floor();
            let mut p = overall - iteration;
            // The end of the active phase is the last iteration's end.
            if phase == Phase::After && p == 0.0 && overall > 0.0 {
                iteration -= 1.0;
                p = 1.0;
            }
            (iteration.max(0.0), p)
        };
        let iteration = iteration as u32;
        let p = p as f32;
        let forwards = match self.direction {
            AnimationDirection::Normal => true,
            AnimationDirection::Reverse => false,
            AnimationDirection::Alternate => iteration % 2 == 0,
            AnimationDirection::AlternateReverse => iteration % 2 == 1,
        };
        Some((iteration, if forwards { p } else { 1.0 - p }))
    }
    /// The animatable properties its keyframes set.
    fn props(&self) -> Vec<usize> {
        let mut out: Vec<usize> = self
            .frames
            .iter()
            .flat_map(|f| f.values.iter().map(|(i, _)| *i))
            .collect();
        out.sort_unstable();
        out.dedup();
        out
    }
    /// The value of `prop` at directed progress `p`: between the keyframes
    /// around it, eased by the first one's timing function (the
    /// animation's own when it has none), with the underlying value as an
    /// implicit 0% and 100% keyframe (css-animations-1 §4.1).
    fn value_of(&self, prop: usize, p: f32, underlying: &AnimValue) -> AnimValue {
        let mut list: Vec<(f32, &AnimValue, Option<&TimingFunction>)> = self
            .frames
            .iter()
            .filter_map(|f| {
                f.values
                    .iter()
                    .find(|(i, _)| *i == prop)
                    .map(|(_, v)| (f.offset, v, f.timing.as_ref()))
            })
            .collect();
        list.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        if list.first().is_none_or(|f| f.0 > 0.0) {
            list.insert(0, (0.0, underlying, None));
        }
        if list.last().is_none_or(|f| f.0 < 1.0) {
            list.push((1.0, underlying, None));
        }
        if p >= 1.0 {
            return list[list.len() - 1].1.clone();
        }
        let a = list.iter().rposition(|f| f.0 <= p).unwrap_or(0);
        let b = (a + 1).min(list.len() - 1);
        let (ao, av, at) = list[a];
        let (bo, bv, _) = list[b];
        let span = bo - ao;
        let local = if span <= 0.0 { 0.0 } else { (p - ao) / span };
        let eased = at.unwrap_or(&self.timing).evaluate(local);
        interpolate(av, bv, eased)
            .unwrap_or_else(|| if eased < 0.5 { av.clone() } else { bv.clone() })
    }
    /// `elapsedTime` of `animationstart`.
    fn elapsed_at_start(&self) -> f32 {
        ((-self.delay_ms).max(0.0).min(self.active_duration()) / 1000.0) as f32
    }
    /// `elapsedTime` of `animationcancel`: the active time at `now`.
    fn elapsed_at(&self, now: f64) -> f32 {
        ((self.local(now) - self.delay_ms).clamp(0.0, self.active_duration()) / 1000.0) as f32
    }
}

impl Animations {
    fn push_animation_event(&mut self, a: &CssAnimation, kind: &'static str, elapsed_s: f32) {
        self.events.push(AnimationEvent {
            node: a.node,
            kind,
            property: String::new(),
            name: a.name.clone(),
            elapsed_s,
        });
    }

    /// Match this element's `animation` list to its running animations
    /// (by list index and name: a renamed or dropped entry is cancelled,
    /// a new one started, a kept one has its timing refreshed), apply the
    /// play state, and overlay their values in list order (a later
    /// animation wins a property both set).
    fn animations_update(
        &mut self,
        node: usize,
        style: &mut ComputedStyle,
        resolve: &dyn Fn(&str, &ComputedStyle) -> Option<Vec<Frame>>,
    ) {
        if self.disabled {
            return;
        }
        let now = self.clock_ms;
        let items = style.animations.clone();
        let named = |item: &AnimationItem| {
            let name = item.name.trim();
            !name.is_empty() && !name.eq_ignore_ascii_case("none")
        };
        let mut cancelled = Vec::new();
        self.css.retain(|a| {
            let kept = a.node != node
                || items
                    .get(a.index)
                    .is_some_and(|item| named(item) && item.name.trim() == a.name);
            if !kept {
                cancelled.push(a.clone());
            }
            kept
        });
        for a in cancelled {
            self.cancel_animation_event(&a);
        }
        let us = |ms: f64| (ms * 1000.0).round() / 1000.0;
        for (index, item) in items.iter().enumerate() {
            if !named(item) {
                continue;
            }
            let name = item.name.trim();
            let at = match self
                .css
                .iter()
                .position(|a| a.node == node && a.index == index)
            {
                Some(at) => at,
                None => {
                    // No `@keyframes` of that name: no animation (Chrome
                    // lists none and fires nothing).
                    let Some(frames) = resolve(name, style) else {
                        continue;
                    };
                    let id = self.next_id();
                    self.css.push(CssAnimation {
                        id,
                        node,
                        index,
                        name: name.to_string(),
                        frames,
                        created_ms: now,
                        delay_ms: 0.0,
                        duration_ms: 0.0,
                        iterations: 1.0,
                        direction: AnimationDirection::Normal,
                        fill: AnimationFillMode::None,
                        timing: TimingFunction::Ease,
                        paused_local: None,
                        started: false,
                        iterations_seen: 0,
                        ended: false,
                        cancelled: false,
                        script_paused: None,
                        css_play_state: item.play_state,
                    });
                    self.css.len() - 1
                }
            };
            let a = &mut self.css[at];
            a.duration_ms = us(item.duration.max(0.0) as f64 * 1000.0);
            a.delay_ms = us(item.delay as f64 * 1000.0);
            a.iterations = match item.iteration_count {
                AnimationIterationCount::Infinite => f32::INFINITY,
                AnimationIterationCount::One => 1.0,
                AnimationIterationCount::Count(n) => n.max(0.0),
            };
            a.direction = item.direction;
            a.fill = item.fill_mode;
            a.timing = item.timing.clone();
            if item.play_state != a.css_play_state {
                a.css_play_state = item.play_state;
                a.script_paused = None;
            }
            let paused = a
                .script_paused
                .unwrap_or(item.play_state == AnimationPlayState::Paused);
            match (paused, a.paused_local) {
                (true, None) => a.paused_local = Some(now - a.created_ms),
                (false, Some(local)) => {
                    a.created_ms = now - local;
                    a.paused_local = None;
                }
                _ => {}
            }
        }
        let mut events = Vec::new();
        for a in self
            .css
            .iter_mut()
            .filter(|a| a.node == node && !a.cancelled)
        {
            let local = a.local(now);
            let phase = a.phase(local);
            if !a.started && phase != Phase::Before {
                a.started = true;
                events.push((a.clone(), "animationstart", a.elapsed_at_start()));
            }
            if let Some((iteration, p)) = a.progress(local) {
                if phase == Phase::Active && iteration > a.iterations_seen {
                    a.iterations_seen = iteration;
                    let elapsed = (us(iteration as f64 * a.duration_ms) / 1000.0) as f32;
                    events.push((a.clone(), "animationiteration", elapsed));
                }
                for prop in a.props() {
                    let underlying = (PROPS[prop].get)(style);
                    let value = a.value_of(prop, p, &underlying);
                    (PROPS[prop].set)(style, &value);
                }
            }
            if phase == Phase::After && !a.ended {
                a.ended = true;
                let elapsed = (a.active_duration() / 1000.0) as f32;
                events.push((a.clone(), "animationend", elapsed));
            }
        }
        for (a, kind, elapsed) in events {
            self.push_animation_event(&a, kind, elapsed);
        }
        // Paint reads the animated opacity (layout's `group_opacity` takes
        // the settled frame for a box that names an animation).
        if items.iter().any(named) {
            style.animation_settled_opacity = Some(style.opacity);
        }
    }
}

/// `animation` shorthand: a comma-separated list of
/// `[name] || duration || timing-function || delay || iteration-count ||
/// direction || fill-mode || play-state` items; the first time is the
/// duration, the second the delay, a bare number the iteration count.
pub(crate) fn parse_animation_shorthand(
    value: &str,
    parse_time: fn(&str) -> Option<f32>,
    parse_timing: fn(&str) -> TimingFunction,
) -> Vec<AnimationItem> {
    let value = value.trim();
    if value.is_empty() {
        return Vec::new();
    }
    split_top_level(value, ',')
        .into_iter()
        .map(|item| {
            let mut out = AnimationItem::default();
            let mut times = 0;
            for token in split_top_level(item, ' ') {
                let lower = token.to_ascii_lowercase();
                if let Some(t) = parse_time(&lower) {
                    if times == 0 {
                        out.duration = t;
                    } else {
                        out.delay = t;
                    }
                    times += 1;
                } else if is_timing_keyword(&lower) {
                    out.timing = parse_timing(&lower);
                } else if let Some(count) = parse_iteration_count(&lower) {
                    out.iteration_count = count;
                } else if let Some(direction) = parse_direction(&lower) {
                    out.direction = direction;
                } else if let Some(fill) = parse_fill_mode(&lower) {
                    out.fill_mode = fill;
                } else if let Some(state) = parse_play_state(&lower) {
                    out.play_state = state;
                } else if !lower.is_empty() {
                    // Case matters in a `@keyframes` name.
                    out.name = unquoted(token).to_string();
                }
            }
            out
        })
        .collect()
}

pub(crate) fn parse_iteration_count(token: &str) -> Option<AnimationIterationCount> {
    let token = token.trim();
    if token.eq_ignore_ascii_case("infinite") {
        return Some(AnimationIterationCount::Infinite);
    }
    token
        .parse::<f32>()
        .ok()
        .filter(|n| *n >= 0.0)
        .map(AnimationIterationCount::Count)
}

pub(crate) fn parse_direction(token: &str) -> Option<AnimationDirection> {
    Some(match token.trim().to_ascii_lowercase().as_str() {
        "normal" => AnimationDirection::Normal,
        "reverse" => AnimationDirection::Reverse,
        "alternate" => AnimationDirection::Alternate,
        "alternate-reverse" => AnimationDirection::AlternateReverse,
        _ => return None,
    })
}

pub(crate) fn parse_fill_mode(token: &str) -> Option<AnimationFillMode> {
    Some(match token.trim().to_ascii_lowercase().as_str() {
        "none" => AnimationFillMode::None,
        "forwards" => AnimationFillMode::Forwards,
        "backwards" => AnimationFillMode::Backwards,
        "both" => AnimationFillMode::Both,
        _ => return None,
    })
}

pub(crate) fn parse_play_state(token: &str) -> Option<AnimationPlayState> {
    Some(match token.trim().to_ascii_lowercase().as_str() {
        "running" => AnimationPlayState::Running,
        "paused" => AnimationPlayState::Paused,
        _ => return None,
    })
}

/// `animation-name`: the list sets how many items there are; the other
/// longhands' values repeat to fill them.
pub(crate) fn set_animation_names(style: &mut ComputedStyle, value: &str) {
    let names = split_top_level(value, ',');
    let old = std::mem::take(&mut style.animations);
    style.animations = names
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let mut item = if old.is_empty() {
                AnimationItem::default()
            } else {
                old[i % old.len()].clone()
            };
            item.name = unquoted(name).to_string();
            item
        })
        .collect();
}

/// An animation name without its quotes: `"fade"` names the same
/// `@keyframes` as `fade` (the parser strips them from the at-rule too).
fn unquoted(name: &str) -> &str {
    name.trim().trim_matches(|c| c == '"' || c == '\'')
}

/// An `animation-*` longhand list over the items there are (`none` items
/// when no name was given yet).
pub(crate) fn set_animation_component<T: Clone>(
    style: &mut ComputedStyle,
    values: Vec<T>,
    set: fn(&mut AnimationItem, T),
) {
    if values.is_empty() {
        return;
    }
    if style.animations.is_empty() {
        style.animations = values.iter().map(|_| AnimationItem::default()).collect();
    }
    let n = values.len();
    for (i, item) in style.animations.iter_mut().enumerate() {
        set(item, values[i % n].clone());
    }
}

/// The scalar animation fields follow the first item, and `animation_name`
/// joins every name (what `getComputedStyle` serializes, and what the
/// parity capture's settle reads: it declines a list).
pub(crate) fn sync_animation_scalars(style: &mut ComputedStyle) {
    let initial = AnimationItem::default();
    let first = style.animations.first().unwrap_or(&initial);
    style.animation_name = style
        .animations
        .iter()
        .map(|item| item.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    style.animation_duration = first.duration;
    style.animation_delay = first.delay;
    style.animation_timing_function = first.timing.clone();
    style.animation_iteration_count = first.iteration_count;
    style.animation_direction = first.direction;
    style.animation_fill_mode = first.fill_mode;
    style.animation_play_state = first.play_state;
}

// ------------------------------------------------ script's view and layer

fn easing_text(t: &TimingFunction) -> String {
    match t {
        TimingFunction::Ease => "ease".to_string(),
        TimingFunction::Linear => "linear".to_string(),
        TimingFunction::EaseIn => "ease-in".to_string(),
        TimingFunction::EaseOut => "ease-out".to_string(),
        TimingFunction::EaseInOut => "ease-in-out".to_string(),
        TimingFunction::StepStart => "step-start".to_string(),
        TimingFunction::StepEnd => "step-end".to_string(),
        TimingFunction::Steps(n, true) => format!("steps({n}, start)"),
        TimingFunction::Steps(n, false) => format!("steps({n})"),
        TimingFunction::CubicBezier(a, b, c, d) => format!("cubic-bezier({a}, {b}, {c}, {d})"),
    }
}

impl Animations {
    /// The running transitions and animations as script sees them
    /// (`getAnimations()`'s `CSSTransition`/`CSSAnimation` objects).
    pub(crate) fn describe(&self) -> Vec<CssAnimationInfo> {
        let mut out = Vec::new();
        for t in &self.transitions {
            out.push(CssAnimationInfo {
                id: t.id,
                node: split_key(t.node).0,
                pseudo: split_key(t.node).1,
                kind: "transition",
                index: t.prop,
                name: self.prop_label(t.prop).to_string(),
                start_ms: t.created_ms,
                paused_ms: None,
                delay_ms: t.delay_ms,
                duration_ms: t.duration_ms,
                iterations: 1.0,
                direction: "normal",
                fill: "backwards",
                easing: easing_text(&t.timing),
                ended: false,
            });
        }
        for (k, svg) in self.svg_transitions.iter().enumerate() {
            let t = &svg.t;
            out.push(CssAnimationInfo {
                id: t.id,
                node: t.node,
                pseudo: None,
                kind: "transition",
                // After every `PROPS` row, in the lane's order.
                index: PROPS.len() + k,
                name: svg.prop.to_string(),
                start_ms: t.created_ms,
                paused_ms: None,
                delay_ms: t.delay_ms,
                duration_ms: t.duration_ms,
                iterations: 1.0,
                direction: "normal",
                fill: "backwards",
                easing: easing_text(&t.timing),
                ended: false,
            });
        }
        for a in self.css.iter().filter(|a| !a.cancelled) {
            out.push(CssAnimationInfo {
                id: a.id,
                node: split_key(a.node).0,
                pseudo: split_key(a.node).1,
                kind: "animation",
                index: a.index,
                name: a.name.clone(),
                start_ms: a.created_ms,
                paused_ms: a.paused_local,
                delay_ms: a.delay_ms,
                duration_ms: a.duration_ms,
                iterations: a.iterations,
                direction: match a.direction {
                    AnimationDirection::Normal => "normal",
                    AnimationDirection::Reverse => "reverse",
                    AnimationDirection::Alternate => "alternate",
                    AnimationDirection::AlternateReverse => "alternate-reverse",
                },
                fill: match a.fill {
                    AnimationFillMode::None => "none",
                    AnimationFillMode::Forwards => "forwards",
                    AnimationFillMode::Backwards => "backwards",
                    AnimationFillMode::Both => "both",
                },
                easing: easing_text(&a.timing),
                ended: a.ended,
            });
        }
        out
    }

    /// The properties the running transitions and animations set, per
    /// element (what a computed read keeps the published row for).
    pub(crate) fn animated_props(&self) -> HashMap<usize, Vec<String>> {
        let mut out: HashMap<usize, Vec<String>> = HashMap::new();
        // A pseudo-element's are not its element's (its reads come from
        // the pseudo styles, which the frames keep).
        for t in self.transitions.iter().filter(|t| split_key(t.node).1.is_none()) {
            out.entry(t.node)
                .or_default()
                .push(self.prop_label(t.prop).to_string());
        }
        for a in self.css.iter().filter(|a| !a.cancelled && split_key(a.node).1.is_none()) {
            let props = out.entry(a.node).or_default();
            for prop in a.props() {
                props.push(PROPS[prop].name.to_string());
            }
        }
        out
    }

    /// Script's `pause()`/`play()`/`cancel()`/`finish()` on one of them,
    /// by id: a control for a record that is gone (cancelled, ended and
    /// dropped, replaced by a same-name restart) does nothing, and never
    /// reaches the record that took its place.
    pub(crate) fn control(&mut self, c: &CssAnimationControl) {
        let now = self.clock_ms;
        if c.kind == "transition" {
            if let Some(at) = self.svg_transitions.iter().position(|t| t.t.id == c.id) {
                match c.op.as_str() {
                    "cancel" => {
                        let t = self.svg_transitions.remove(at);
                        self.svg_base.remove(&(t.t.node, t.prop));
                        self.push_svg_event(&t, "transitioncancel", t.t.elapsed_at(now));
                    }
                    "finish" => {
                        let t = &mut self.svg_transitions[at].t;
                        t.created_ms = now - t.delay_ms - t.duration_ms;
                    }
                    _ => {}
                }
                return;
            }
            let Some(at) = self.transitions.iter().position(|t| t.id == c.id) else {
                return;
            };
            match c.op.as_str() {
                "cancel" => {
                    let t = self.transitions.remove(at);
                    self.push_event(&t, "transitioncancel", t.elapsed_at(now));
                }
                "finish" => {
                    let t = &mut self.transitions[at];
                    t.created_ms = now - t.delay_ms - t.duration_ms;
                }
                _ => {}
            }
            return;
        }
        let Some(at) = self.css.iter().position(|a| a.id == c.id) else {
            return;
        };
        match c.op.as_str() {
            "pause" => {
                let a = &mut self.css[at];
                a.script_paused = Some(true);
                if a.paused_local.is_none() {
                    a.paused_local = Some(now - a.created_ms);
                }
            }
            "play" => {
                let a = &mut self.css[at];
                a.script_paused = Some(false);
                if let Some(local) = a.paused_local.take() {
                    a.created_ms = now - local;
                }
            }
            "cancel" => {
                let a = &mut self.css[at];
                if a.cancelled {
                    return;
                }
                a.cancelled = true;
                if !a.ended {
                    let (a, elapsed) = (a.clone(), a.elapsed_at(now));
                    self.push_animation_event(&a, "animationcancel", elapsed);
                }
            }
            "finish" => {
                let a = &mut self.css[at];
                if a.iterations.is_finite() {
                    a.paused_local = None;
                    a.script_paused = Some(false);
                    a.created_ms = now - a.delay_ms - a.active_duration();
                }
            }
            _ => {}
        }
    }
}

/// Set animatable property `index` of `style` to `value`.
pub(crate) fn set_prop_value(style: &mut ComputedStyle, index: usize, value: &AnimValue) {
    (PROPS[index].set)(style, value)
}

fn add_length(a: &Length, b: &Length) -> Option<Length> {
    use Length::*;
    Some(match (a, b) {
        (Px(x), Px(y)) => Px(x + y),
        (Zero, Px(y)) | (Px(y), Zero) => Px(*y),
        (Percent(x), Percent(y)) => Percent(x + y),
        (Zero, Percent(y)) | (Percent(y), Zero) => Percent(*y),
        (Em(x), Em(y)) => Em(x + y),
        (Rem(x), Rem(y)) => Rem(x + y),
        (Zero, Zero) => Zero,
        _ => return None,
    })
}

/// A script animation's `value` composed on the `underlying` value
/// (web-animations-1 §4.4.1, css-transforms-2 §13): `add` sums numbers
/// and lengths and post-multiplies a transform list, `accumulate` sums a
/// matching transform list function by function (`scale` as s1 + s2 - 1)
/// and otherwise adds; `replace` is the value.
pub(crate) fn compose(underlying: &AnimValue, value: &AnimValue, composite: &str) -> AnimValue {
    if composite != "add" && composite != "accumulate" {
        return value.clone();
    }
    match (underlying, value) {
        (AnimValue::Number(u), AnimValue::Number(v)) => AnimValue::Number(u + v),
        (AnimValue::Length(u), AnimValue::Length(v)) => {
            AnimValue::Length(add_length(u, v).unwrap_or_else(|| v.clone()))
        }
        (AnimValue::Transform(u), AnimValue::Transform(v)) => {
            if u.ops.is_empty() {
                return value.clone();
            }
            if composite == "accumulate" && u.ops.len() == v.ops.len() {
                use TransformOp::*;
                let ops: Option<Vec<TransformOp>> = u
                    .ops
                    .iter()
                    .zip(&v.ops)
                    .map(|(a, b)| {
                        Some(match (a, b) {
                            (Translate(ax, ay), Translate(bx, by)) => {
                                Translate(add_length(ax, bx)?, add_length(ay, by)?)
                            }
                            (TranslateX(ax), TranslateX(bx)) => TranslateX(add_length(ax, bx)?),
                            (TranslateY(ay), TranslateY(by)) => TranslateY(add_length(ay, by)?),
                            (Scale(ax, ay), Scale(bx, by)) => Scale(ax + bx - 1.0, ay + by - 1.0),
                            (ScaleX(x), ScaleX(y)) => ScaleX(x + y - 1.0),
                            (ScaleY(x), ScaleY(y)) => ScaleY(x + y - 1.0),
                            (Rotate(x), Rotate(y)) => Rotate(x + y),
                            (SkewX(x), SkewX(y)) => SkewX(x + y),
                            (SkewY(x), SkewY(y)) => SkewY(x + y),
                            (Skew(ax, ay), Skew(bx, by)) => Skew(ax + bx, ay + by),
                            _ => return None,
                        })
                    })
                    .collect();
                if let Some(ops) = ops {
                    return AnimValue::Transform(TransformList { ops });
                }
            }
            let mut ops = u.ops.clone();
            ops.extend(v.ops.iter().cloned());
            AnimValue::Transform(TransformList { ops })
        }
        _ => value.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SVG_BLUE: Color = Color {
        r: 0,
        g: 0,
        b: 255,
        a: 1.0,
    };
    const SVG_RED: Color = Color {
        r: 255,
        g: 0,
        b: 0,
        a: 1.0,
    };

    fn linear_100ms(property: &str) -> Option<TransitionItem> {
        Some(TransitionItem {
            property: property.to_string(),
            duration: 0.1,
            delay: 0.0,
            timing: TimingFunction::Linear,
        })
    }

    /// One lane pass at `at` ms with `fill` of node 7 as `paint`.
    fn svg_pass(
        a: &mut Animations,
        at: f64,
        paint: Paint,
        spec: Option<TransitionItem>,
    ) -> Vec<(usize, &'static str, Color)> {
        a.clock_ms = at;
        a.svg_update(&[(7, "fill", paint, spec)]);
        a.svg_overlay()
    }

    fn kinds(a: &mut Animations) -> Vec<&'static str> {
        a.take_events().iter().map(|e| e.kind).collect()
    }

    /// A2.2: a colour against a colour interpolates (premultiplied, as
    /// `color`), with run, start and end at the element.
    #[test]
    fn svg_fill_colours_interpolate() {
        let mut a = Animations::default();
        assert!(svg_pass(&mut a, 0.0, Paint::Color(SVG_BLUE), linear_100ms("all")).is_empty());
        let start = svg_pass(&mut a, 0.0, Paint::Color(SVG_RED), linear_100ms("all"));
        assert_eq!(start, vec![(7, "fill", SVG_BLUE)]);
        assert!(a.svg_moving() && a.active());
        assert!(!a.running(), "the svg lane is not a `running()` one");
        let mid = svg_pass(&mut a, 50.0, Paint::Color(SVG_RED), linear_100ms("all"));
        assert_eq!(mid.len(), 1);
        assert_eq!((mid[0].2.r, mid[0].2.g, mid[0].2.b), (128, 0, 128));
        assert!(svg_pass(&mut a, 100.0, Paint::Color(SVG_RED), linear_100ms("all")).is_empty());
        assert!(!a.svg_moving());
        let events = a.take_events();
        assert_eq!(
            events
                .iter()
                .map(|e| (e.node, e.kind, e.property.as_str()))
                .collect::<Vec<_>>(),
            vec![
                (7, "transitionrun", "fill"),
                (7, "transitionstart", "fill"),
                (7, "transitionend", "fill")
            ]
        );
    }

    /// `none` and `url()` do not interpolate (Chrome 148): no transition,
    /// no event; `currentcolor` both times is no change.
    #[test]
    fn svg_paint_keywords_are_discrete() {
        let url = Paint::Url("#g".to_string(), None);
        for (before, after) in [
            (Paint::None, Paint::Color(SVG_RED)),
            (Paint::Color(SVG_RED), Paint::None),
            (url.clone(), Paint::Color(SVG_RED)),
            (Paint::Color(SVG_RED), url),
            (Paint::CurrentColor(SVG_BLUE), Paint::CurrentColor(SVG_RED)),
        ] {
            let mut a = Animations::default();
            svg_pass(&mut a, 0.0, before.clone(), linear_100ms("fill"));
            let moving = svg_pass(&mut a, 0.0, after.clone(), linear_100ms("fill"));
            assert!(
                moving.is_empty() && !a.svg_moving(),
                "{before:?} to {after:?}"
            );
            assert!(
                kinds(&mut a).is_empty(),
                "{before:?} to {after:?}: no events"
            );
        }
    }

    /// A transition list that stops naming the property (no item covers
    /// it: `None`), or an element that left the svg, cancels the running one (the lane's own sweep).
    #[test]
    fn svg_transitions_are_swept() {
        let mut a = Animations::default();
        svg_pass(&mut a, 0.0, Paint::Color(SVG_BLUE), linear_100ms("fill"));
        svg_pass(&mut a, 0.0, Paint::Color(SVG_RED), linear_100ms("fill"));
        assert!(svg_pass(&mut a, 20.0, Paint::Color(SVG_RED), None).is_empty());
        assert_eq!(
            kinds(&mut a),
            vec!["transitionrun", "transitionstart", "transitioncancel"]
        );
        assert!(a.svg_lane_empty());

        let mut a = Animations::default();
        svg_pass(&mut a, 0.0, Paint::Color(SVG_BLUE), linear_100ms("fill"));
        svg_pass(&mut a, 0.0, Paint::Color(SVG_RED), linear_100ms("fill"));
        a.clock_ms = 20.0;
        a.svg_update(&[]);
        assert!(a.svg_lane_empty());
        assert_eq!(
            kinds(&mut a),
            vec!["transitionrun", "transitionstart", "transitioncancel"]
        );
    }

    /// Back to where it started midway: the running one is cancelled and
    /// the new one runs from the value shown, shortened (css-transitions-1
    /// §3), as the `PROPS` lane does.
    #[test]
    fn an_svg_transition_reverses_from_where_it_got_to() {
        let mut a = Animations::default();
        svg_pass(&mut a, 0.0, Paint::Color(SVG_BLUE), linear_100ms("fill"));
        svg_pass(&mut a, 0.0, Paint::Color(SVG_RED), linear_100ms("fill"));
        let back = svg_pass(&mut a, 40.0, Paint::Color(SVG_BLUE), linear_100ms("fill"));
        assert_eq!(back.len(), 1);
        assert_eq!(
            (back[0].2.r, back[0].2.b),
            (102, 153),
            "from the colour at 40 ms"
        );
        // 40 ms out, so 40 ms back.
        assert!(svg_pass(&mut a, 80.0, Paint::Color(SVG_BLUE), linear_100ms("fill")).is_empty());
        assert_eq!(
            kinds(&mut a),
            vec![
                "transitionrun",
                "transitionstart",
                "transitioncancel",
                "transitionrun",
                "transitionstart",
                "transitionend"
            ]
        );
    }

    #[test]
    fn colours_interpolate_premultiplied() {
        let blue = Color {
            r: 0,
            g: 0,
            b: 255,
            a: 1.0,
        };
        let red = Color {
            r: 255,
            g: 0,
            b: 0,
            a: 1.0,
        };
        let mid = interpolate_color(&blue, &red, 0.5);
        assert_eq!((mid.r, mid.g, mid.b), (128, 0, 128));
        assert_eq!(mid.a, 1.0);
        let clear = Color {
            r: 0,
            g: 0,
            b: 0,
            a: 0.0,
        };
        let mid = interpolate_color(&clear, &red, 0.5);
        assert_eq!(
            (mid.r, mid.g, mid.b),
            (255, 0, 0),
            "transparent black does not darken"
        );
        assert_eq!(mid.a, 0.5);
    }

    #[test]
    fn currentcolor_is_its_own_value_and_interpolates_as_the_colour() {
        let blue = Color {
            r: 0,
            g: 0,
            b: 255,
            a: 1.0,
        };
        let red = Color {
            r: 255,
            g: 0,
            b: 0,
            a: 1.0,
        };
        assert_eq!(
            AnimValue::CurrentColor(blue),
            AnimValue::CurrentColor(red),
            "currentcolor before and after is no change"
        );
        assert_ne!(AnimValue::CurrentColor(blue), AnimValue::Color(blue));
        assert_eq!(
            interpolate(&AnimValue::CurrentColor(blue), &AnimValue::Color(red), 0.5),
            Some(AnimValue::Color(Color {
                r: 128,
                g: 0,
                b: 128,
                a: 1.0
            }))
        );
        let mut style = ComputedStyle::new();
        style.color = blue;
        let i = prop_indices("outline-color")[0];
        assert_eq!(prop_value(&style, i), AnimValue::CurrentColor(blue));
        set_prop_value(&mut style, i, &AnimValue::Color(red));
        assert_eq!(style.outline.color, Some(red));
        set_prop_value(&mut style, i, &AnimValue::CurrentColor(blue));
        assert_eq!(style.outline.color, None);
        assert_eq!(
            prop_indices("text-decoration"),
            prop_indices("text-decoration-color")
        );
    }

    #[test]
    fn lengths_interpolate_within_a_unit_and_mix_two_units_as_calc() {
        assert_eq!(
            interpolate_length(&Length::Px(100.0), &Length::Px(200.0), 0.5),
            Some(Length::Px(150.0))
        );
        assert_eq!(
            interpolate_length(&Length::Zero, &Length::Px(100.0), 0.25),
            Some(Length::Px(25.0))
        );
        assert_eq!(
            interpolate_length(&Length::Auto, &Length::Px(100.0), 0.5),
            None
        );
        // Two units: a calc() mix (Chrome 148: `10px` to `50%` is
        // `calc(25% + 5px)` at the midpoint).
        assert_eq!(
            interpolate_length(&Length::Px(10.0), &Length::Percent(50.0), 0.5),
            Some(Length::Calc(Box::new(rustkit_css::CalcSum {
                px: 5.0,
                percent: 25.0,
                ..Default::default()
            })))
        );
        assert_eq!(
            interpolate_length(&Length::Px(10.0), &Length::Percent(50.0), 1.0),
            Some(Length::Percent(50.0)),
            "the end collapses to its one unit"
        );
    }

    #[test]
    fn transforms_interpolate_function_by_function_and_none_as_identity() {
        let a = TransformList {
            ops: vec![TransformOp::TranslateX(Length::Px(0.0))],
        };
        let b = TransformList {
            ops: vec![TransformOp::TranslateX(Length::Px(100.0))],
        };
        assert_eq!(
            interpolate_transform(&a, &b, 0.5).ops,
            vec![TransformOp::TranslateX(Length::Px(50.0))]
        );
        let scale = TransformList {
            ops: vec![TransformOp::Scale(2.0, 2.0)],
        };
        assert_eq!(
            interpolate_transform(&TransformList::none(), &scale, 0.5).ops,
            vec![TransformOp::Scale(1.5, 1.5)]
        );
    }

    #[test]
    fn the_shorthand_takes_a_list_with_a_timing_function_and_delay() {
        let items = parse_transition_shorthand(
            "opacity 100ms linear, transform .2s cubic-bezier(0.4, 0, 0.2, 1) 50ms, 1s",
            crate::parse_time,
            crate::parse_timing_function,
        );
        assert_eq!(items.len(), 3);
        assert_eq!(
            (
                items[0].property.as_str(),
                items[0].duration,
                items[0].delay
            ),
            ("opacity", 0.1, 0.0)
        );
        assert_eq!(items[0].timing, TimingFunction::Linear);
        assert_eq!(
            (
                items[1].property.as_str(),
                items[1].duration,
                items[1].delay
            ),
            ("transform", 0.2, 0.05)
        );
        assert_eq!(
            items[1].timing,
            TimingFunction::CubicBezier(0.4, 0.0, 0.2, 1.0)
        );
        assert_eq!(
            (items[2].property.as_str(), items[2].duration),
            ("all", 1.0)
        );
        assert!(parse_transition_shorthand(
            "none",
            crate::parse_time,
            crate::parse_timing_function
        )
        .is_empty());
    }

    #[test]
    fn the_later_item_naming_a_property_wins_and_shorthands_expand() {
        let items = parse_transition_shorthand(
            "all 1s, opacity 2s, margin 3s",
            crate::parse_time,
            crate::parse_timing_function,
        );
        assert_eq!(spec_for(&items, "opacity").unwrap().duration, 2.0);
        assert_eq!(spec_for(&items, "width").unwrap().duration, 1.0);
        assert_eq!(spec_for(&items, "margin-left").unwrap().duration, 3.0);
    }

    #[test]
    fn longhands_repeat_to_the_property_list() {
        let mut style = ComputedStyle::new();
        set_transition_properties(&mut style, "opacity, width, color");
        set_transition_component(&mut style, vec![0.1f32, 0.2], |item, d| item.duration = d);
        let d: Vec<f32> = style.transitions.iter().map(|i| i.duration).collect();
        assert_eq!(d, vec![0.1, 0.2, 0.1]);
        sync_transition_scalars(&mut style, false);
        assert_eq!(style.transition_property, "opacity, width, color");
        assert_eq!(style.transition_duration, 0.1);
    }

    #[test]
    fn a_change_starts_a_transition_and_the_overlay_follows_the_clock() {
        let mut anims = Animations::default();
        let no_frames = |_: &str, _: &ComputedStyle| None::<Vec<Frame>>;
        let mut style = ComputedStyle::new();
        style.opacity = 0.0;
        style.transitions = vec![TransitionItem {
            property: "opacity".into(),
            duration: 0.1,
            delay: 0.0,
            timing: TimingFunction::Linear,
        }];
        anims.clock_ms = 1000.0;
        anims.on_style(7, &mut style, &no_frames);
        assert!(!anims.active(), "the first build starts nothing");
        style.opacity = 1.0;
        anims.on_style(7, &mut style, &no_frames);
        assert_eq!(style.opacity, 0.0, "overlaid at the start value");
        let kinds: Vec<&str> = anims.events.iter().map(|e| e.kind).collect();
        assert_eq!(kinds, ["transitionrun", "transitionstart"]);
        anims.take_events();
        anims.clock_ms = 1050.0;
        style.opacity = 1.0;
        anims.on_style(7, &mut style, &no_frames);
        assert!(
            (style.opacity - 0.5).abs() < 1e-4,
            "midway: {}",
            style.opacity
        );
        anims.clock_ms = 1100.0;
        style.opacity = 1.0;
        anims.on_style(7, &mut style, &no_frames);
        assert_eq!(style.opacity, 1.0);
        let kinds: Vec<&str> = anims.events.iter().map(|e| e.kind).collect();
        assert_eq!(kinds, ["transitionend"]);
        assert_eq!(anims.events[0].elapsed_s, 0.1);
        assert!(!anims.active() || anims.events.len() == 1);
    }

    #[test]
    fn a_reversal_midway_is_shortened_to_the_distance_covered() {
        let mut anims = Animations::default();
        let no_frames = |_: &str, _: &ComputedStyle| None::<Vec<Frame>>;
        let mut style = ComputedStyle::new();
        style.opacity = 0.0;
        style.transitions = vec![TransitionItem {
            property: "opacity".into(),
            duration: 0.1,
            delay: 0.0,
            timing: TimingFunction::Linear,
        }];
        anims.on_style(1, &mut style, &no_frames);
        style.opacity = 1.0;
        anims.on_style(1, &mut style, &no_frames);
        anims.clock_ms = 40.0;
        style.opacity = 1.0;
        anims.on_style(1, &mut style, &no_frames);
        assert!((style.opacity - 0.4).abs() < 1e-4);
        // Back to 0 at 40 ms: from 0.4, over 40 ms.
        style.opacity = 0.0;
        anims.on_style(1, &mut style, &no_frames);
        assert!((style.opacity - 0.4).abs() < 1e-4);
        anims.clock_ms = 60.0;
        style.opacity = 0.0;
        anims.on_style(1, &mut style, &no_frames);
        assert!(
            (style.opacity - 0.2).abs() < 1e-4,
            "halfway back: {}",
            style.opacity
        );
        anims.clock_ms = 80.0;
        style.opacity = 0.0;
        anims.on_style(1, &mut style, &no_frames);
        assert_eq!(style.opacity, 0.0);
        let kinds: Vec<&str> = anims.take_events().iter().map(|e| e.kind).collect();
        assert_eq!(
            kinds,
            [
                "transitionrun",
                "transitionstart",
                "transitioncancel",
                "transitionrun",
                "transitionstart",
                "transitionend"
            ]
        );
    }

    /// TRANS-F: a transition naming a custom property is no engine gap
    /// (Chrome runs none for an unregistered one, and a registered one
    /// runs here), so it is not reported as an unsupported API. Tailwind
    /// v4's `--tw-gradient-from` reported one entry per name on x.com.
    #[test]
    fn a_custom_property_in_a_transition_is_not_reported_unsupported() {
        let mut anims = Animations::default();
        let no_frames = |_: &str, _: &ComputedStyle| None::<Vec<Frame>>;
        let mut style = ComputedStyle::new();
        style.transitions = ["--tw-gradient-from", "--u", "zoom"]
            .iter()
            .map(|p| linear_100ms(p).unwrap())
            .collect();
        anims.on_style(7, &mut style, &no_frames);
        assert_eq!(anims.take_unsupported(), ["zoom"]);
    }
}

// ------------------------------------------------------------ svg paint

/// The svg paint properties the svg lane transitions (A2.2).
pub(crate) const SVG_PAINT_PROPS: &[&str] = &["fill", "stroke"];

/// The transition item of `items` that covers svg paint property `prop`.
pub(crate) fn svg_spec_for<'a>(
    items: &'a [TransitionItem],
    prop: &str,
) -> Option<&'a TransitionItem> {
    spec_for(items, prop)
}

impl Animations {
    /// Whether an svg paint transition's value is moving at the clock: one
    /// in its delay holds its start value, so its frames need no build
    /// until the delay ends (`active()` keeps the frames turning).
    pub(crate) fn svg_moving(&self) -> bool {
        self.svg_transitions.iter().any(|t| self.clock_ms >= t.t.start_ms())
    }

    /// Whether the svg lane holds nothing: no before-change value and no
    /// transition.
    pub(crate) fn svg_lane_empty(&self) -> bool {
        self.svg_base.is_empty() && self.svg_transitions.is_empty()
    }

    /// Whether the svg lane holds anything for `node`'s `prop`.
    pub(crate) fn svg_holds(&self, node: usize, prop: &'static str) -> bool {
        self.svg_base.contains_key(&(node, prop))
            || self
                .svg_transitions
                .iter()
                .any(|t| t.t.node == node && t.prop == prop)
    }

    fn push_svg_event(&mut self, t: &SvgTransition, kind: &'static str, elapsed_s: f32) {
        self.events.push(AnimationEvent {
            node: t.t.node,
            kind,
            property: t.prop.to_string(),
            name: String::new(),
            elapsed_s,
        });
    }

    /// A2.2: one pass of the svg paint lane, given every svg element's
    /// `fill` and `stroke` as the author rules, presentation attributes and
    /// inheritance resolve them (`svg_computed::resolve_paints`, no
    /// transition applied), with the transition item that covers it.
    /// `paints` lists every element and property the lane holds
    /// (`svg_holds`) and every one a transition covers; anything left out
    /// is gone (its element left the svg, or the document).
    ///
    /// The rules are `transitions_update`'s: a covered value that changed
    /// since the last pass starts a transition from what the element
    /// showed (a running one is cancelled, with the reversing shortening);
    /// one heading where the value now is keeps going; a transition whose
    /// value is no longer covered is cancelled. Colours interpolate
    /// (premultiplied, as `color`); `none`, `url()` and `currentcolor` do
    /// not (Chrome 148 starts no transition for the first two; the third is
    /// a stated gap), so the value jumps.
    pub(crate) fn svg_update(
        &mut self,
        paints: &[(usize, &'static str, Paint, Option<TransitionItem>)],
    ) {
        let now = self.clock_ms;
        let covered: std::collections::HashSet<(usize, &str)> = paints
            .iter()
            .filter(|p| p.3.is_some())
            .map(|p| (p.0, p.1))
            .collect();
        // The cancel sweep: svg content gets no build visit, so the
        // end-of-build sweep never sees it.
        let mut gone = Vec::new();
        self.svg_transitions.retain(|t| {
            if covered.contains(&(t.t.node, t.prop)) {
                true
            } else {
                gone.push(t.clone());
                false
            }
        });
        for t in gone {
            self.push_svg_event(&t, "transitioncancel", t.t.elapsed_at(now));
        }
        self.svg_base.retain(|key, _| covered.contains(key));
        for (node, prop, new, spec) in paints {
            let Some(spec) = spec else { continue };
            let prev = self.svg_base.insert((*node, *prop), new.clone());
            let Some(old) = prev.filter(|_| !self.disabled) else {
                continue;
            };
            let running = self
                .svg_transitions
                .iter()
                .position(|t| t.t.node == *node && t.prop == *prop);
            match running {
                // Already heading there: the running one keeps going.
                Some(at) if matches!(new, Paint::Color(c) if self.svg_transitions[at].t.to == AnimValue::Color(*c)) => {
                    continue
                }
                None if same_paint(&old, new) => continue,
                _ => {}
            }
            let (from, reversing) = match running {
                Some(at) => {
                    let t = self.svg_transitions.remove(at);
                    let current = t.t.value_at(now);
                    self.push_svg_event(&t, "transitioncancel", t.t.elapsed_at(now));
                    (Some(current), Some(t.t))
                }
                None => (
                    match old {
                        Paint::Color(c) => Some(AnimValue::Color(c)),
                        _ => None,
                    },
                    None,
                ),
            };
            let (Some(from), Paint::Color(to)) = (from, new) else {
                continue;
            };
            let Some(t) = self.new_transition(
                *node,
                usize::MAX,
                from,
                &AnimValue::Color(*to),
                spec,
                reversing,
            ) else {
                continue;
            };
            let t = SvgTransition { t, prop: *prop };
            self.push_svg_event(&t, "transitionrun", t.t.elapsed_at_start());
            self.svg_transitions.push(t);
        }
    }

    /// The running svg paint transitions' values at the clock, as
    /// (raw NodeId, property, colour), with their `transitionstart` and
    /// `transitionend` queued as the phases change. One that ended is
    /// dropped: the resolved value is its end value.
    pub(crate) fn svg_overlay(&mut self) -> Vec<(usize, &'static str, Color)> {
        let now = self.clock_ms;
        let mut out = Vec::new();
        let mut i = 0;
        while i < self.svg_transitions.len() {
            let t = &mut self.svg_transitions[i];
            if !t.t.started && now >= t.t.start_ms() {
                t.t.started = true;
                let (t, elapsed) = (t.clone(), t.t.elapsed_at_start());
                self.push_svg_event(&t, "transitionstart", elapsed);
            }
            let t = &self.svg_transitions[i];
            if now >= t.t.end_ms() {
                let (t, elapsed) = (t.clone(), (t.t.duration_ms / 1000.0) as f32);
                self.push_svg_event(&t, "transitionend", elapsed);
                self.svg_transitions.remove(i);
                continue;
            }
            if let AnimValue::Color(c) = t.t.value_at(now) {
                out.push((t.t.node, t.prop, c));
            }
            i += 1;
        }
        out
    }
}
