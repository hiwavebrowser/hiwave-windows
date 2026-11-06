//! `@container` size queries (css-contain-3 §6) at style time.
//!
//! A rule inside `@container` applies to an element only where its
//! condition holds for the element's query container: the nearest ancestor
//! with a `container-type` other than `normal` (and the queried
//! `container-name`, when one is given). Chrome answers the query with the
//! container's laid-out size. The cascade here runs before layout, so the
//! walk carries an estimate of each box's content width instead.
//!
//! The estimate is an allowlist (#782 HOLD 3): a width is known only when
//! the container and every ancestor up to the root are plain in-flow
//! blocks, each with an auto width (its parent's width less its margins,
//! borders and padding) or a length or percentage of its parent's width.
//! A plain block here is `display: block` (which includes `list-item` and
//! `flow-root`), horizontal-tb, not floated or absolutely positioned, not a
//! `-webkit-box`, not a form control, replaced element or `dialog`, with no
//! sizing keyword (`min-content`, `fit-content`, ...) in `width`,
//! `min-width` or `max-width`, no `aspect-ratio` on an auto width, and not
//! a child of a multi-column box. After a float anywhere earlier in the
//! same formatting context (#782 HOLD 4: however tall its ancestors are,
//! since only layout knows where it ends), an auto-width box that starts a
//! formatting context is narrowed by it, and so is one the walk cannot see
//! (`flow-root`, `contain`, `overflow-y`), so any width, `min-width` or
//! `max-width` there other than a length is unknown too. A float counts
//! whether it is an element or a floated `::before`, `::after` or
//! `::first-letter`, and inside an inline box or `display: contents`
//! element, which never start a context (#782 HOLD 5). An absolute or
//! fixed `border-box` container with percentage padding is unknown: the
//! padding resolves against its containing block, not its parent.
//!
//! Everything else is unknown, and a query on an unknown container does not
//! hold, which is how the engine treated every `@container` rule before:
//! flex, grid and table descendants, inline and `display: contents`
//! containers, floats, absolute boxes, vertical writing modes. Some of
//! those have a width Chrome knows (a `flex: none` item, a table with a
//! fixed width); they stay false rather than risk a guess wider than the
//! laid-out box.
//!
//! Not covered: container-relative units (`cq*`), style queries, and sizes
//! that only layout knows (a block's auto height).

use std::cell::RefCell;

use rustkit_css::{
    BoxSizing, ComputedStyle, ContainerType, Display, Float, Length, Overflow, Position,
    WritingMode,
};

/// A query container, as an `@container` condition sees it.
#[derive(Debug, Clone)]
struct Container {
    names: Vec<String>,
    /// Content-box inline size, CSS px.
    inline: f32,
    /// Content-box block size; `None` unless `container-type: size` with a
    /// definite height.
    block: Option<f32>,
    /// Computed font size, CSS px: what `em` in its conditions is.
    em: f32,
}

/// One element of the walk.
#[derive(Debug, Clone)]
struct Frame {
    /// Content-box width estimate of the element; `None` when unknown.
    width: Option<f32>,
    /// The element is a size container; `None` when it is not one, or when
    /// its size is unknown (`Some(None)` keeps the name so a named query
    /// still stops there and fails rather than reaching further out).
    container: Option<Option<Container>>,
    /// Names of an element that is a container of unknown size.
    unknown_names: Vec<String>,
    /// The element starts a block formatting context, which floats inside
    /// it do not leave.
    new_context: bool,
    /// A float has been entered in this box's formatting context, here or
    /// in an earlier descendant that does not start its own.
    floats: bool,
}

#[derive(Debug, Default)]
struct State {
    viewport: (f32, f32),
    /// The root element's font size, CSS px (`rem`); set by the first
    /// element entered.
    root_em: Option<f32>,
    frames: Vec<Frame>,
    /// Some rule floats a `::first-letter` (see `first_letter_floats`).
    first_letter_floats: bool,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

/// Active for one box-tree build; restores the previous state on drop.
pub(crate) struct DocumentScope {
    previous: Option<State>,
}

impl DocumentScope {
    /// Installs the walk's state when there is a viewport and some rule
    /// carries a container condition; otherwise every such rule stays out.
    /// `first_letter_floats`: some rule floats a `::first-letter`.
    pub(crate) fn install(
        viewport: Option<(f32, f32)>,
        any_container_rule: bool,
        first_letter_floats: bool,
    ) -> Self {
        let state = viewport.filter(|_| any_container_rule).map(|(w, h)| State {
            viewport: (w, h),
            root_em: None,
            first_letter_floats,
            frames: vec![Frame {
                width: Some(w),
                container: None,
                unknown_names: Vec::new(),
                new_context: true,
                floats: false,
            }],
        });
        Self {
            previous: STATE.with(|s| std::mem::replace(&mut *s.borrow_mut(), state)),
        }
    }
}

impl Drop for DocumentScope {
    fn drop(&mut self) {
        let previous = self.previous.take();
        STATE.with(|s| *s.borrow_mut() = previous);
    }
}

/// An element's frame; popped on drop. Its descendants (and its own
/// `::before`/`::after`) query through it.
pub(crate) struct ElementScope {
    pushed: bool,
}

impl ElementScope {
    pub(crate) fn enter(
        style: &ComputedStyle,
        parent: Option<&ComputedStyle>,
        tag: Option<&str>,
    ) -> Self {
        let pushed = STATE.with(|s| {
            let mut s = s.borrow_mut();
            let Some(state) = s.as_mut() else {
                return false;
            };
            state.root_em.get_or_insert_with(|| font_px(style));
            let parent_width = state.frames.last().and_then(|f| f.width);
            // An earlier float in the same formatting context may sit beside
            // this box; one that starts a new context is then narrowed by
            // it, which only layout knows. (`clear` is not modelled.)
            let mut float_seen = false;
            for f in state.frames.iter().rev() {
                float_seen |= f.floats;
                if float_seen || f.new_context {
                    break;
                }
            }
            let frame = frame_for(style, parent, tag, parent_width, state.viewport, float_seen);
            // A float overhangs a wrapper (block or inline) that does not
            // start a formatting context, so it still narrows a later box
            // there: record it up to the context's root. Whether it ends
            // inside a definite-height ancestor depends on where layout puts
            // it (text, padding, `::before`, `max-height`, negative margins
            // all move it), so there is no exception for that (#782 HOLD 4).
            if style.float != Float::None && style.display != Display::None {
                mark_float(&mut state.frames);
            }
            state.frames.push(frame);
            true
        });
        Self { pushed }
    }
}

/// Records a float in the innermost frame's formatting context, up to the
/// context's root.
fn mark_float(frames: &mut [Frame]) {
    for f in frames.iter_mut().rev() {
        f.floats = true;
        if f.new_context {
            break;
        }
    }
}

/// Whether the walk is running (a viewport and some `@container` rule), so
/// that callers can skip work done only for it.
pub(crate) fn active() -> bool {
    STATE.with(|s| s.borrow().is_some())
}

/// Whether a sibling built now, before the element whose frame comes next,
/// can still change what the walk answers under that element: the walk is
/// running and no float is on record yet in the innermost frame's
/// formatting context. A float only ever sets `floats` from the innermost
/// frame up to its context's root, and every later frame below it asks
/// that same chain, so once one is there another changes nothing.
pub(crate) fn earlier_siblings_matter() -> bool {
    STATE.with(|s| {
        s.borrow().as_ref().is_some_and(|state| {
            for f in state.frames.iter().rev() {
                if f.floats {
                    return false;
                }
                if f.new_context {
                    break;
                }
            }
            true
        })
    })
}

/// The walk is running and some rule floats a `::first-letter`, so an
/// element's `::first-letter` rules are worth matching for it.
pub(crate) fn wants_first_letter() -> bool {
    STATE.with(|s| s.borrow().as_ref().is_some_and(|st| st.first_letter_floats))
}

/// A rule that may float a `::first-letter` (or `:first-letter`): one of
/// its members names it and it declares a `float` other than `none`.
pub(crate) fn floats_first_letter(rule: &rustkit_css::Rule) -> bool {
    rule.selector.contains(":first-letter")
        && rule.declarations.iter().any(|d| {
            d.property == "float"
                && matches!(&d.value, rustkit_css::PropertyValue::Specified(v)
                    if !v.trim().eq_ignore_ascii_case("none"))
        })
}

/// A matched `::first-letter` rule floats the element's first letter: it is
/// recorded like a floated `::before` (#782 HOLD 5, q6). The engine does
/// not lay out `::first-letter`; whether the element has a first letter at
/// all is not checked (a false float only makes more widths unknown).
pub(crate) fn note_first_letter_float() {
    STATE.with(|s| {
        if let Some(state) = s.borrow_mut().as_mut() {
            mark_float(&mut state.frames);
        }
    });
}

/// A floated pseudo-element of the element whose frame is innermost
/// (#782 HOLD 5, q4-q6): `::before`, `::after` and `::first-letter` push no
/// frame, so their `float` is recorded here, from the element's own frame
/// up to its context's root, as an element child's would be. For `::before`
/// (and `::first-letter`, treated alike) this narrows the element's later
/// children; for `::after`, called after them, the element's later
/// siblings when the element does not start its own context.
pub(crate) fn note_pseudo(style: &ComputedStyle) {
    if style.float == Float::None || style.display == Display::None {
        return;
    }
    STATE.with(|s| {
        if let Some(state) = s.borrow_mut().as_mut() {
            mark_float(&mut state.frames);
        }
    });
}

impl Drop for ElementScope {
    fn drop(&mut self) {
        if self.pushed {
            STATE.with(|s| {
                if let Some(state) = s.borrow_mut().as_mut() {
                    state.frames.pop();
                }
            });
        }
    }
}

/// Whether a rule whose `@container` preludes (outermost first) are
/// `preludes` applies to the element now being styled.
pub(crate) fn applies(preludes: &[String]) -> bool {
    if preludes.is_empty() {
        return true;
    }
    STATE.with(|s| {
        let s = s.borrow();
        let Some(state) = s.as_ref() else {
            return false;
        };
        preludes.iter().all(|p| prelude_holds(p, state))
    })
}

fn prelude_holds(prelude: &str, state: &State) -> bool {
    let frames = &state.frames;
    let prelude = prelude.trim();
    // `<container-name>? <container-query>`: a leading word that is not
    // `not` and not a function (`foo(` is a function token, so
    // `foo(width > 1px)` has no name and is a general-enclosed condition,
    // as in Chromium) is the name.
    let (name, condition) = match prelude.split_once(char::is_whitespace) {
        Some((first, rest)) if !first.eq_ignore_ascii_case("not") && !first.contains('(') => {
            (Some(first), rest.trim())
        }
        _ => (None, prelude),
    };
    for frame in frames.iter().rev() {
        let names: &[String] = match &frame.container {
            Some(Some(c)) => &c.names,
            Some(None) => &frame.unknown_names,
            None => continue,
        };
        if name.is_some_and(|n| !names.iter().any(|m| m == n)) {
            continue;
        }
        return match &frame.container {
            Some(Some(c)) => {
                let units = rustkit_css::media::ContainerUnits {
                    em: c.em,
                    rem: state.root_em.unwrap_or(16.0),
                    vw: state.viewport.0,
                    vh: state.viewport.1,
                };
                rustkit_css::media::container_condition_matches(
                    condition, c.inline, c.block, units,
                )
            }
            _ => false,
        };
    }
    false
}

fn frame_for(
    style: &ComputedStyle,
    parent: Option<&ComputedStyle>,
    tag: Option<&str>,
    parent_width: Option<f32>,
    viewport: (f32, f32),
    float_seen: bool,
) -> Frame {
    let width = content_width(style, parent, tag, parent_width, viewport, float_seen);
    let container = match style.container_type {
        ContainerType::Normal => None,
        // An unknown width (every box that is not a plain block, among
        // them inline and `display: contents` ones) keeps the names, so a
        // named query stops here and fails.
        kind => Some(width.map(|inline| Container {
            names: style.container_name.clone(),
            inline,
            block: match kind {
                ContainerType::Size => definite_height(style, viewport),
                _ => None,
            },
            em: font_px(style),
        })),
    };
    Frame {
        width,
        container,
        unknown_names: style.container_name.clone(),
        new_context: starts_context(style),
        floats: false,
    }
}

/// The box starts a block formatting context that floats do not leave (a
/// subset; missing some only makes more widths unknown). `overflow` and
/// `container-type` do that only for a block-level or atomic box: on a
/// non-atomic inline box or a `display: contents` element a float inside
/// still belongs to the outer context (#782 HOLD 5, q1-q3). Floats and
/// absolute boxes are blockified, so they always do. The other displays
/// that are not block-level count too, rightly (#782 follow-up, Chromium
/// 148 on docs/diagnostics/2026-10-27/css_container_query_nits.html
/// k4-k9): in `ruby` and `ruby-text` (which keep the element's previous
/// display here) a float is inlinified and never floats, and in a table
/// row or group an anonymous cell holds it, so none of them lets a float
/// out. Without `overflow` they do not count, which is the safe side.
fn starts_context(style: &ComputedStyle) -> bool {
    let boxed = style.display != Display::Inline && !style.display_contents_declared;
    out_of_flow(style)
        || (boxed
            && (style.container_type != ContainerType::Normal
                || !matches!(style.overflow_x, Overflow::Visible | Overflow::Clip)))
}

fn font_px(style: &ComputedStyle) -> f32 {
    match style.font_size {
        Length::Px(px) => px,
        _ => 16.0,
    }
}

/// A length that does not need layout to resolve; percentages resolve
/// against `basis`, and fail without one.
fn resolve(len: &Length, style: &ComputedStyle, basis: Option<f32>, vp: (f32, f32)) -> Option<f32> {
    let needs_basis = |l: &Length| matches!(l, Length::Percent(_));
    match len {
        Length::Auto | Length::FitContent => None,
        Length::Calc(..) | Length::Min(..) | Length::Max(..) | Length::Clamp(..) => {
            // Mixed math may hold a percentage; resolve against the basis,
            // or not at all when there is none.
            let b = basis?;
            Some(len.to_px_with_viewport(font_px(style), 16.0, b, vp.0, vp.1))
        }
        l if needs_basis(l) => {
            Some(l.to_px_with_viewport(font_px(style), 16.0, basis?, vp.0, vp.1))
        }
        l => Some(l.to_px_with_viewport(font_px(style), 16.0, 0.0, vp.0, vp.1)),
    }
}

fn horizontal_extras(style: &ComputedStyle, basis: Option<f32>, vp: (f32, f32)) -> Option<f32> {
    let border = |w: &Length, s: rustkit_css::BorderStyle| match s {
        rustkit_css::BorderStyle::None => Some(0.0),
        _ => resolve(w, style, None, vp).or(Some(0.0)),
    };
    Some(
        resolve(&style.padding_left, style, basis, vp)?
            + resolve(&style.padding_right, style, basis, vp)?
            + border(&style.border_left_width, style.border_left_style)?
            + border(&style.border_right_width, style.border_right_style)?,
    )
}

/// Shrink-to-fit form controls and replaced elements: their width is not
/// the parent's even with `display: block`.
const NOT_PLAIN_TAGS: &[&str] = &[
    "button", "input", "select", "textarea", "fieldset", "legend", "meter", "progress", "img",
    "video", "audio", "canvas", "iframe", "embed", "object", "svg", "math",
    // Chromium's UA sheet makes an open dialog absolute and `fit-content`.
    "dialog",
];

/// Floated or absolutely positioned: out of flow, so the block rules give
/// an auto or percentage width no meaning here, but a length is exact.
fn out_of_flow(style: &ComputedStyle) -> bool {
    style.float != Float::None || matches!(style.position, Position::Absolute | Position::Fixed)
}

/// A length with no percentage in it (`px`, `em`, `vw`, ...).
fn absolute_length(len: &Length) -> bool {
    !matches!(
        len,
        Length::Auto
            | Length::FitContent
            | Length::Percent(_)
            | Length::Calc(..)
            | Length::Min(..)
            | Length::Max(..)
            | Length::Clamp(..)
    )
}

/// A box whose width follows from its parent's by the block rules alone,
/// or (out of flow) from its own length `width`.
fn plain_block(style: &ComputedStyle, tag: Option<&str>) -> bool {
    style.display == Display::Block
        && style.webkit_box.is_none()
        && !style.display_contents_declared
        && (!out_of_flow(style)
            || (absolute_length(&style.width)
                && [&style.min_width, &style.max_width]
                    .iter()
                    .all(|l| matches!(l, Length::Auto) || absolute_length(l))))
        && style.writing_mode == WritingMode::HorizontalTb
        && !style.writing_mode_declared
        && style.width_keywords == 0
        // An auto width with a definite height follows the height.
        && (style.aspect_ratio.is_none() || !matches!(style.width, Length::Auto))
        && ![&style.width, &style.min_width, &style.max_width]
            .iter()
            .any(|l| matches!(l, Length::FitContent))
        && !tag.is_some_and(|t| NOT_PLAIN_TAGS.iter().any(|n| t.eq_ignore_ascii_case(n)))
}

/// Records a `width` / `min-width` / `max-width` declaration for the walk:
/// `bit` is set while the winning value is one `Length` cannot carry (the
/// content keywords in any case or with a `-webkit-` prefix), and cleared
/// by one it can (or `none`).
pub(crate) fn note_width_value(style: &mut ComputedStyle, bit: u8, parsed: bool, value: &str) {
    if parsed || value.trim().eq_ignore_ascii_case("none") {
        style.width_keywords &= !bit;
    } else {
        style.width_keywords |= bit;
    }
}

fn content_width(
    style: &ComputedStyle,
    parent: Option<&ComputedStyle>,
    tag: Option<&str>,
    parent_width: Option<f32>,
    vp: (f32, f32),
    float_seen: bool,
) -> Option<f32> {
    // Unknown below any box that is not a plain block (the parent's width
    // is then `None`), and in a multi-column box, whose children sit in
    // columns multicol layout sizes (`columns`/`column-width` are not
    // parsed yet but still make one).
    let parent_width = parent_width?;
    if !plain_block(style, tag)
        || parent.is_some_and(|p| p.column_count.is_some() || p.columns_declared)
    {
        return None;
    }
    // Beside an earlier float, a box starting a context the walk does not
    // see (`flow-root`, `contain`, `overflow-y` alone) is narrower than its
    // parent, so a percentage of it is unknown (#782 HOLD 4).
    if float_seen
        && [&style.width, &style.min_width, &style.max_width]
            .iter()
            .any(|l| !matches!(l, Length::Auto) && !absolute_length(l))
    {
        return None;
    }
    // An absolute or fixed box's percentage padding resolves against its
    // containing block, which only layout knows, not against the parent:
    // with `border-box` its content width is then unknown (#782 HOLD 5, q7).
    // A float's containing block is its parent, so it is fine.
    if matches!(style.position, Position::Absolute | Position::Fixed)
        && style.box_sizing == BoxSizing::BorderBox
        && [&style.padding_left, &style.padding_right]
            .iter()
            .any(|l| !absolute_length(l))
    {
        return None;
    }
    let basis = Some(parent_width);
    let extras = horizontal_extras(style, basis, vp)?;
    let to_content = |border_box_or_content: f32| match style.box_sizing {
        BoxSizing::BorderBox => (border_box_or_content - extras).max(0.0),
        BoxSizing::ContentBox => border_box_or_content.max(0.0),
    };
    let width = match &style.width {
        // An earlier float in the same formatting context narrows a box
        // that starts its own; only layout knows by how much.
        Length::Auto if float_seen && starts_context(style) => return None,
        Length::Auto => {
            let margins = resolve(&style.margin_left, style, basis, vp).unwrap_or(0.0)
                + resolve(&style.margin_right, style, basis, vp).unwrap_or(0.0);
            (parent_width - margins - extras).max(0.0)
        }
        l => to_content(resolve(l, style, basis, vp)?),
    };
    // min-width wins over max-width (CSS 2.1 §10.4). A limit that does not
    // resolve here makes the width unknown rather than unlimited.
    let limit = |len: &Length| match len {
        Length::Auto => Some(None),
        l => resolve(l, style, basis, vp).map(|v| Some(to_content(v))),
    };
    let max = limit(&style.max_width)?;
    let min = limit(&style.min_width)?;
    let width = max.map_or(width, |m| width.min(m));
    Some(min.map_or(width, |m| width.max(m)))
}

/// A size container's content-box height when its style alone gives it:
/// `height` clamped by `max-height` and `min-height` (min wins, CSS 2.1
/// §10.7; #782 R2). A percentage, or a limit or border-box padding that
/// does not resolve here, makes it unknown.
fn definite_height(style: &ComputedStyle, vp: (f32, f32)) -> Option<f32> {
    let extras = match style.box_sizing {
        BoxSizing::BorderBox => {
            let border = |w: &Length, s: rustkit_css::BorderStyle| match s {
                rustkit_css::BorderStyle::None => Some(0.0),
                _ => resolve(w, style, None, vp).or(Some(0.0)),
            };
            resolve(&style.padding_top, style, None, vp)?
                + resolve(&style.padding_bottom, style, None, vp)?
                + border(&style.border_top_width, style.border_top_style)?
                + border(&style.border_bottom_width, style.border_bottom_style)?
        }
        BoxSizing::ContentBox => 0.0,
    };
    let to_content = |v: f32| (v - extras).max(0.0);
    let height = to_content(resolve(&style.height, style, None, vp)?);
    let limit = |len: &Length| match len {
        Length::Auto => Some(None),
        l => resolve(l, style, None, vp).map(|v| Some(to_content(v))),
    };
    let max = limit(&style.max_height)?;
    let min = limit(&style.min_height)?;
    let height = max.map_or(height, |m| height.min(m));
    Some(min.map_or(height, |m| height.max(m)))
}
