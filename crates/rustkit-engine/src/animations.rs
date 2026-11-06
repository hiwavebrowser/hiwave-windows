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
//! What is interpolated: numbers (`opacity`), lengths of one unit (`px`
//! with `px`, `%` with `%`; `auto` never transitions, as in Chrome),
//! colours (premultiplied sRGB), and transform lists (function by function
//! when the lists match, `none` as the identity of the other list, else
//! the two matrices component-wise, which is not the spec's decomposition
//! and is a stated gap). Not here yet: the individual `translate`/
//! `rotate`/`scale` properties, `border-radius`, `box-shadow`,
//! `background-position`, `line-height`, `visibility`'s discrete rule,
//! pseudo-element transitions, and `transition-behavior: allow-discrete`.

use rustkit_bindings::{CssAnimationControl, CssAnimationInfo};
use rustkit_css::{
    AnimationDirection, AnimationFillMode, AnimationItem, AnimationIterationCount,
    AnimationPlayState, Color, ComputedStyle, Display, Length, TimingFunction, TransformList,
    TransformOp, TransitionItem,
};
use std::collections::HashMap;

/// A value one of the transitionable properties takes.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum AnimValue {
    Number(f32),
    Length(Length),
    Color(Color),
    Transform(TransformList),
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
    color_prop!("color", color),
    color_prop!("background-color", background_color),
    color_prop!("border-top-color", border_top_color),
    color_prop!("border-right-color", border_right_color),
    color_prop!("border-bottom-color", border_bottom_color),
    color_prop!("border-left-color", border_left_color),
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
    len_prop!("letter-spacing", letter_spacing),
    len_prop!("word-spacing", word_spacing),
    len_prop!("text-indent", text_indent),
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
        "background" => &["background-color"],
        "font" => &["font-size"],
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
        _ => return None,
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

fn interpolate(a: &AnimValue, b: &AnimValue, t: f32) -> Option<AnimValue> {
    Some(match (a, b) {
        (AnimValue::Number(x), AnimValue::Number(y)) => AnimValue::Number(lerp(*x, *y, t)),
        (AnimValue::Length(x), AnimValue::Length(y)) => {
            AnimValue::Length(interpolate_length(x, y, t)?)
        }
        (AnimValue::Color(x), AnimValue::Color(y)) => AnimValue::Color(interpolate_color(x, y, t)),
        (AnimValue::Transform(x), AnimValue::Transform(y)) => {
            AnimValue::Transform(interpolate_transform(x, y, t))
        }
        _ => return None,
    })
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

    /// The transition property names reported since the last call
    /// (`CSSTransition.<name>` in the Inspector's unsupported APIs).
    pub(crate) fn take_unsupported(&mut self) -> Vec<String> {
        std::mem::take(&mut self.pending_unsupported)
    }

    fn note_unsupported(&mut self, specs: &[TransitionItem]) {
        for item in specs {
            let name = item.property.trim().to_ascii_lowercase();
            if name == "all"
                || name == "none"
                || PROPS.iter().any(|p| p.name == name)
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
    }

    pub(crate) fn take_events(&mut self) -> Vec<AnimationEvent> {
        std::mem::take(&mut self.events)
    }

    fn push_event(&mut self, t: &Transition, kind: &'static str, elapsed_s: f32) {
        self.events.push(AnimationEvent {
            node: t.node,
            kind,
            property: PROPS[t.prop].name.to_string(),
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
    /// partial build (`partial_build`) visited only what is under the
    /// changed elements; the rest are all still there with their styles:
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
        let covered: Vec<usize> = (0..PROPS.len())
            .filter(|&i| spec_for(&specs, PROPS[i].name).is_some())
            .collect();
        let new_vals: Vec<(usize, AnimValue)> = covered
            .iter()
            .map(|&i| (i, (PROPS[i].get)(style)))
            .collect();
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
                let Some(spec) = spec_for(&specs, PROPS[*i].name) else {
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
                // To the microsecond: `0.1` seconds as an f32 is 100.0000015
                // ms as an f64, and an end at exactly 1100 would be missed.
                let us = |ms: f64| (ms * 1000.0).round() / 1000.0;
                let duration_ms = us((spec.duration.max(0.0) as f64) * 1000.0);
                let delay_ms = us(spec.delay as f64 * 1000.0);
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
                if duration_ms + delay_ms.max(0.0) <= 0.0 || interpolate(&from, new, 0.5).is_none()
                {
                    continue;
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
                let t = Transition {
                    id,
                    node,
                    prop: *i,
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
                };
                self.push_event(&t, "transitionrun", t.elapsed_at_start());
                self.transitions.push(t);
            }
        }
    }

    /// Overlay the running transitions of this element.
    fn transitions_overlay(&mut self, node: usize, style: &mut ComputedStyle) {
        let now = self.clock_ms;
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
            (PROPS[t.prop].set)(style, &value);
            if now >= t.end_ms() {
                t.done = true;
                let (t, elapsed) = (t.clone(), (t.duration_ms / 1000.0) as f32);
                self.push_event(&t, "transitionend", elapsed);
                self.transitions.remove(i);
            } else {
                i += 1;
            }
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
            item.property = name.to_ascii_lowercase();
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
            | "color"
            | "background-color"
            | "border-top-color"
            | "border-right-color"
            | "border-bottom-color"
            | "border-left-color"
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
                node: t.node,
                kind: "transition",
                index: t.prop,
                name: PROPS[t.prop].name.to_string(),
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
                node: a.node,
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
        for t in &self.transitions {
            out.entry(t.node)
                .or_default()
                .push(PROPS[t.prop].name.to_string());
        }
        for a in self.css.iter().filter(|a| !a.cancelled) {
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
    fn lengths_interpolate_only_within_a_unit() {
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
        assert_eq!(
            interpolate_length(&Length::Percent(50.0), &Length::Px(100.0), 0.5),
            None
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
}
