//! Layout constraints and fragments: the L0 slice of
//! `docs/LAYOUT_CONSTRAINTS_FRAGMENTS_2026-09-30.md`.
//!
//! A layout query takes a typed [`Constraint`] and returns an explicit
//! [`Fragment`]. The one query this slice moves is the intrinsic size of a
//! nested flex or grid item that holds text or a form control
//! ([`intrinsic_fragment`]): the subtree is laid out under the constraint
//! by the same flex and grid layout final layout uses, and the result is
//! the contribution. The estimators it replaces at that call site stay for
//! every item outside the class, and the [`differential`] keeps both
//! numbers visible.

use crate::{BoxType, LayoutBox, Position};
use rustkit_css::{FlexDirection, Length, WritingMode};
use std::cell::{Cell, RefCell};

/// A box length in fixed point, 1/64 CSS px (§5; Chromium's `LayoutUnit` is
/// the precedent for the split). Box and fragment sizes are written in this
/// unit. Glyph advances and text baselines are not: they stay `f32`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct LayoutUnit(i32);

impl LayoutUnit {
    /// Units per CSS px.
    pub const PER_PX: i32 = 64;
    pub const ZERO: LayoutUnit = LayoutUnit(0);

    /// The nearest unit to `px`. Not a number (and anything a cast would
    /// saturate) clamps instead of wrapping.
    pub fn from_px(px: f32) -> Self {
        let raw = (px * Self::PER_PX as f32).round();
        if raw.is_nan() {
            return LayoutUnit(0);
        }
        LayoutUnit(raw.clamp(i32::MIN as f32, i32::MAX as f32) as i32)
    }

    /// The smallest unit not below `px`. An intrinsic inline size is
    /// written this way, so a box sized to its content is never a
    /// fraction of a unit narrower than what it holds.
    pub fn from_px_ceil(px: f32) -> Self {
        let raw = (px * Self::PER_PX as f32).ceil();
        if raw.is_nan() {
            return LayoutUnit(0);
        }
        LayoutUnit(raw.clamp(i32::MIN as f32, i32::MAX as f32) as i32)
    }

    pub fn to_px(self) -> f32 {
        self.0 as f32 / Self::PER_PX as f32
    }

    /// The count of 1/64 px.
    pub fn raw(self) -> i32 {
        self.0
    }
}

/// One axis of a size: a number, or the statement that there is none.
/// `Definite(0)` is a definite zero. It is not a synonym for `Indefinite`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AxisSize {
    Definite(LayoutUnit),
    Indefinite,
}

impl AxisSize {
    pub fn definite_px(px: f32) -> Self {
        AxisSize::Definite(LayoutUnit::from_px(px))
    }

    pub fn px(self) -> Option<f32> {
        match self {
            AxisSize::Definite(u) => Some(u.to_px()),
            AxisSize::Indefinite => None,
        }
    }
}

/// What the query asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SizeQuery {
    MinContent,
    MaxContent,
    FitContent {
        available: LayoutUnit,
    },
    /// Lay out under the constraint's definite sizes.
    Definite,
}

/// The input of a layout query (§1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Constraint {
    /// The box's own border-box inline size, where the caller has fixed it.
    pub inline_size: AxisSize,
    /// The box's own border-box block size, where the caller has fixed it.
    pub block_size: AxisSize,
    pub query: SizeQuery,
    /// The space the query may use, per axis.
    pub available_inline: AxisSize,
    pub available_block: AxisSize,
    /// What a percentage resolves against, per axis. A percentage against
    /// an indefinite basis stays a percentage.
    pub percentage_inline: AxisSize,
    pub percentage_block: AxisSize,
    pub writing_mode: WritingMode,
    /// Which box supplies the percentage basis and the available size
    /// (`LayoutBox::element_id`), where it has an element.
    pub containing_block: Option<usize>,
}

impl Constraint {
    /// A box whose inline size is fixed at `inline_px` (border box) and
    /// whose block size is to be found: the question a grid row asks of an
    /// item once the columns are sized.
    pub fn definite_inline(
        inline_px: f32,
        writing_mode: WritingMode,
        containing_block: Option<usize>,
    ) -> Self {
        let inline = AxisSize::definite_px(inline_px);
        Constraint {
            inline_size: inline,
            block_size: AxisSize::Indefinite,
            query: SizeQuery::Definite,
            available_inline: inline,
            available_block: AxisSize::Indefinite,
            percentage_inline: inline,
            percentage_block: AxisSize::Indefinite,
            writing_mode,
            containing_block,
        }
    }
}

impl Constraint {
    /// The fit-content inline size of a box in `available_px` of inline
    /// space (border box), neither axis fixed: the question a column flex
    /// container asks of an item it will not stretch.
    pub fn fit_content_inline(
        available_px: f32,
        writing_mode: WritingMode,
        containing_block: Option<usize>,
    ) -> Self {
        let available = LayoutUnit::from_px(available_px);
        Constraint {
            inline_size: AxisSize::Indefinite,
            block_size: AxisSize::Indefinite,
            query: SizeQuery::FitContent { available },
            available_inline: AxisSize::Definite(available),
            available_block: AxisSize::Indefinite,
            percentage_inline: AxisSize::Indefinite,
            percentage_block: AxisSize::Indefinite,
            writing_mode,
            containing_block,
        }
    }
}

/// An inline and a block size, in the box unit. The block size is
/// `Indefinite` in the answer to a query that asks for an inline size
/// alone: the box was not laid out, so there is no block size to report,
/// and 0 would be a number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FragmentSize {
    pub inline: LayoutUnit,
    pub block: AxisSize,
}

/// The result of one layout query (§2).
#[derive(Debug, Clone, PartialEq)]
pub struct Fragment {
    pub border_box: FragmentSize,
    pub content_box: FragmentSize,
    /// The last-line baseline as a distance below the border-box top, in
    /// text precision. `None` when no in-flow descendant carries one.
    pub baseline: Option<f32>,
    /// The extent of the content measured from the padding-box origin: the
    /// padding box itself, or further where a descendant's border box
    /// leaves it. Content overflow only; ink overflow is a later field.
    pub content_overflow: FragmentSize,
    /// The constraint this fragment answers.
    pub constraint: Constraint,
    /// Child fragments. Empty in L0; inline fragments, floats and
    /// fragmentation are later slices and use this slot.
    pub children: Vec<Fragment>,
    /// The border-box block size before it was written in the box unit, so
    /// a 1/64 snap is not mistaken for a wrap.
    pub unsnapped_block_px: f32,
    /// The border-box inline size before it was written in the box unit.
    pub unsnapped_inline_px: f32,
}

impl Fragment {
    /// The fragment of a box that has just been laid out under `constraint`.
    fn of_laid_out(b: &LayoutBox, constraint: Constraint) -> Fragment {
        let border = b.dimensions.border_box();
        let padding = b.dimensions.padding_box();
        let content = &b.dimensions.content;
        let mut right = padding.x + padding.width;
        let mut bottom = padding.y + padding.height;
        for child in &b.children {
            descendant_extent(child, &mut right, &mut bottom);
        }
        Fragment {
            border_box: FragmentSize {
                inline: LayoutUnit::from_px(border.width),
                block: AxisSize::definite_px(border.height),
            },
            content_box: FragmentSize {
                inline: LayoutUnit::from_px(content.width),
                block: AxisSize::definite_px(content.height),
            },
            baseline: b.inline_block_baseline_y().map(|y| y - border.y),
            content_overflow: FragmentSize {
                inline: LayoutUnit::from_px(right - padding.x),
                block: AxisSize::definite_px(bottom - padding.y),
            },
            constraint,
            children: Vec::new(),
            unsnapped_block_px: border.height,
            unsnapped_inline_px: border.width,
        }
    }

    /// The answer to an inline-size query: an inline size and no layout.
    fn of_inline_size(
        border_box_px: f32,
        padding_border_px: f32,
        constraint: Constraint,
    ) -> Fragment {
        let inline = LayoutUnit::from_px_ceil(border_box_px);
        let content = LayoutUnit::from_px((inline.to_px() - padding_border_px).max(0.0));
        Fragment {
            border_box: FragmentSize {
                inline,
                block: AxisSize::Indefinite,
            },
            content_box: FragmentSize {
                inline: content,
                block: AxisSize::Indefinite,
            },
            baseline: None,
            content_overflow: FragmentSize {
                inline: LayoutUnit::from_px((inline.to_px() - padding_border_px).max(0.0)),
                block: AxisSize::Indefinite,
            },
            constraint,
            children: Vec::new(),
            unsnapped_block_px: 0.0,
            unsnapped_inline_px: border_box_px,
        }
    }
}

/// Grow `right`/`bottom` to cover the border boxes of an in-flow subtree.
fn descendant_extent(b: &LayoutBox, right: &mut f32, bottom: &mut f32) {
    if b.style.display == rustkit_css::Display::None
        || matches!(b.position, Position::Absolute | Position::Fixed)
    {
        return;
    }
    let bb = b.dimensions.border_box();
    *right = right.max(bb.x + bb.width);
    *bottom = bottom.max(bb.y + bb.height);
    for child in &b.children {
        descendant_extent(child, right, bottom);
    }
}

fn holds_text_or_control(b: &LayoutBox) -> bool {
    match &b.box_type {
        BoxType::Text(t) => !t.trim().is_empty(),
        BoxType::FormControl(_) => true,
        _ => b
            .children
            .iter()
            .any(|c| c.style.display != rustkit_css::Display::None && holds_text_or_control(c)),
    }
}

/// Whether `item` is in the L0 class: a row flex container or a grid
/// container, block size `auto`, holding text or a form control.
///
/// Out of the class, and so still on the estimators and the repair passes:
/// a percentage or other relative block size, an `aspect-ratio` item, an
/// out-of-flow item, a plain block, a replaced element, any writing mode
/// but `horizontal-tb`. A COLUMN flex container is out as well, for a
/// reason that is this engine's and not the design's: `layout_flex_container`
/// reads a column's main size from the height its caller left in the box,
/// so an indefinite block size cannot be put to it yet.
pub(crate) fn in_l0_class(item: &LayoutBox) -> bool {
    let s = &item.style;
    let container = if s.display.is_flex() {
        matches!(
            s.flex_direction,
            FlexDirection::Row | FlexDirection::RowReverse
        )
    } else {
        s.display.is_grid()
    };
    container
        && s.writing_mode == WritingMode::HorizontalTb
        && matches!(s.height, Length::Auto)
        && matches!(s.min_height, Length::Auto | Length::Px(_))
        && matches!(s.max_height, Length::Auto | Length::Px(_))
        && s.aspect_ratio.is_none()
        && !matches!(
            s.position,
            rustkit_css::Position::Absolute | rustkit_css::Position::Fixed
        )
        && holds_text_or_control(item)
}

/// (an unsized form control somewhere in the subtree, something in the
/// subtree the width estimators do not measure). The first is what they
/// could not see until they grew a control arm (`own_max_content_width`,
/// `form_control_min_content_width`). The second is any of:
///
/// - an unsized image;
/// - a grid container: `own_max_content_width` has no grid arm, so a grid
///   answers its widest child and not the sum of its columns;
/// - below the item itself, a box whose width is a definite length that is
///   not written in px (`em`, `rem`, viewport units, `calc()`), or that has
///   a `min-width` floor or a `max-width` cap that is not a percentage. The
///   estimators read `width: <px>` and nothing else of the three. The
///   item's own `min-width` and `max-width` are applied by the caller.
fn unsized_control_and_unmeasured(b: &LayoutBox, is_item: bool) -> (bool, bool) {
    let s = &b.style;
    if s.display == rustkit_css::Display::None {
        return (false, false);
    }
    let unsized_box = !matches!(s.width, Length::Px(_));
    let mut control = unsized_box && matches!(b.box_type, BoxType::FormControl(_));
    let mut unmeasured =
        unsized_box && (matches!(b.box_type, BoxType::Image { .. }) || s.display.is_grid());
    if !is_item && !matches!(b.box_type, BoxType::Text(_)) {
        let floor = match s.min_width {
            Length::Auto | Length::Percent(_) => false,
            Length::Px(v) => v > 0.0,
            _ => true,
        };
        unmeasured |= floor
            || !matches!(s.width, Length::Auto | Length::Px(_) | Length::Percent(_))
            || !matches!(s.max_width, Length::Auto | Length::Percent(_));
    }
    for child in &b.children {
        let (c, u) = unsized_control_and_unmeasured(child, false);
        control |= c;
        unmeasured |= u;
    }
    (control, unmeasured)
}

/// Whether `item` is in the L0 class for an inline-size query: a flex
/// container, inline size `auto`, holding a form control with no
/// specified width, and nothing the width estimators do not measure.
///
/// The design puts a nested grid item in the slice, and does not make the
/// class depend on what the estimators can see. Both narrowings are this
/// engine's: the answer is the estimators', and where they are blind the
/// item keeps the width it had. Measured, not assumed: on the fixture a
/// grid of two `auto` columns answered 67.80 where Chromium gives 115.73,
/// and on facebook.com the login column answered 197.63 where Chromium's
/// max-content is 536, because a box inside it is
/// `width: calc(-104px + 50vw)`; the form was pushed off the right edge.
pub(crate) fn in_l0_inline_class(item: &LayoutBox) -> bool {
    let s = &item.style;
    s.display.is_flex()
        && s.writing_mode == WritingMode::HorizontalTb
        && matches!(s.width, Length::Auto)
        && unsized_control_and_unmeasured(item, true) == (true, false)
}

/// The fit-content arm of the query (§4, call site 2): the inline size of
/// an item that holds a control, with the control's label inside the
/// number. `min(max(min-content, available), max-content)` over the
/// border-box estimators, which measure a control with
/// `form_control_min_content_width` and `form_control_intrinsic_size`,
/// the control's own measurement. Nothing is laid out, so the fragment
/// has no block size, and no query depth is spent: the answer inside a
/// probe is the answer outside it.
fn fit_content_fragment(item: &LayoutBox, constraint: &Constraint) -> Option<Fragment> {
    let SizeQuery::FitContent { available } = constraint.query else {
        return None;
    };
    if !in_l0_inline_class(item) {
        return None;
    }
    let max_content = crate::grid::estimate_max_content_width(item);
    if max_content <= 0.0 {
        return None;
    }
    let min_content = crate::grid::estimate_min_content_width(item);
    let border_box = min_content.max(available.to_px().min(max_content));
    Some(Fragment::of_inline_size(
        border_box,
        crate::grid::horizontal_padding_border(&item.style),
        *constraint,
    ))
}

thread_local! {
    /// Depth of `intrinsic_fragment` calls on this thread.
    static QUERY_DEPTH: Cell<u32> = const { Cell::new(0) };
}

/// `RUSTKIT_L0=0` keeps every call site on the estimators (the A arm of an
/// A/B on one binary).
fn enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("RUSTKIT_L0").map_or(true, |v| v != "0"))
}

/// The L0 query: the fragment of `item` under `constraint`.
///
/// `item` is not modified. A copy of its subtree is given its box by
/// `place` (the caller's own placement step, the one its final layout
/// runs: margins, alignment, padding and border for a definite inline
/// size) and is then laid out by `layout_flex_container` or
/// `layout_grid_container`, the functions final layout calls.
///
/// A `FitContent` query is answered without a layout and without `place`
/// (see `fit_content_fragment`).
///
/// `None` means "not this slice" and the caller keeps its present path:
/// the item is outside the class ([`in_l0_class`], or
/// [`in_l0_inline_class`] for an inline-size query), the constraint is one
/// L0 does not execute (it executes a definite inline size with an
/// indefinite block size, and fit-content, in `horizontal-tb`), or a
/// layout query is already inside a query. That last rule bounds the
/// cost: a container nested in a queried subtree is laid out once more by
/// the query and not once more per level.
pub(crate) fn intrinsic_fragment(
    item: &LayoutBox,
    constraint: &Constraint,
    place: impl FnOnce(&mut LayoutBox),
) -> Option<Fragment> {
    if !enabled() || constraint.writing_mode != WritingMode::HorizontalTb {
        return None;
    }
    if matches!(constraint.query, SizeQuery::FitContent { .. }) {
        return fit_content_fragment(item, constraint);
    }
    if constraint.query != SizeQuery::Definite
        || constraint.block_size != AxisSize::Indefinite
        || !matches!(constraint.inline_size, AxisSize::Definite(_))
        || !in_l0_class(item)
        || QUERY_DEPTH.with(|d| d.get()) > 0
    {
        return None;
    }

    let mut probe = item.clone();
    place(&mut probe);
    QUERY_DEPTH.with(|d| d.set(d.get() + 1));
    if probe.style.display.is_flex() {
        let own_box = probe.dimensions.clone();
        crate::flex::layout_flex_container(&mut probe, &own_box);
    } else {
        let (w, h) = (
            probe.dimensions.content.width,
            probe.dimensions.content.height,
        );
        crate::grid::layout_grid_container(&mut probe, w, h);
    }
    QUERY_DEPTH.with(|d| d.set(d.get() - 1));

    Some(Fragment::of_laid_out(&probe, *constraint))
}

/// The four numbers of §4 for one in-slice item, less Chrome's: what the
/// estimate said, what the fragment says, and what the repair pass then
/// did to the item's row.
#[derive(Debug, Clone, PartialEq)]
pub struct Differential {
    /// The item's oracle selector, where it has an element.
    pub selector: Option<String>,
    /// `get_height_contribution`: outer height from `estimate_content_height`.
    pub old_estimate_px: f32,
    /// The fragment's border-box block size plus the item's block margins:
    /// the contribution track sizing used.
    pub fragment_outer_px: f32,
    /// The fragment's border-box block size, in the box unit.
    pub fragment_block: LayoutUnit,
    /// The same before the 1/64 snap.
    pub unsnapped_block_px: f32,
    /// What Phase 9.5 added to the item's row. `None` where the pass does
    /// not run (a grid with a definite height).
    pub phase_9_5_delta_px: Option<f32>,
}

/// The same for an inline-size query (call site 2): the width the item
/// kept before, and the fragment that replaces it.
#[derive(Debug, Clone, PartialEq)]
pub struct InlineDifferential {
    pub selector: Option<String>,
    /// What `fit_content_cross_width` answered before: the width a prior
    /// pass left on the box, as a border box.
    pub old_width_px: f32,
    /// The fragment's border-box inline size, in the box unit.
    pub fragment_inline: LayoutUnit,
    /// The same before the snap.
    pub unsnapped_inline_px: f32,
    pub available_px: f32,
}

thread_local! {
    static RECORD: RefCell<Option<Vec<Differential>>> = const { RefCell::new(None) };
    static INLINE_RECORD: RefCell<Option<Vec<InlineDifferential>>> = const { RefCell::new(None) };
}

fn log_to_stderr() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("RUSTKIT_L0_DIFF").is_some())
}

/// The differential of §4: collected on this thread between [`start`] and
/// [`take`], and printed one line per item when `RUSTKIT_L0_DIFF` is set.
///
/// [`start`]: differential::start
/// [`take`]: differential::take
pub mod differential {
    use super::{log_to_stderr, Differential, InlineDifferential, INLINE_RECORD, RECORD};

    /// Begin collecting on this thread.
    pub fn start() {
        RECORD.with(|r| *r.borrow_mut() = Some(Vec::new()));
        INLINE_RECORD.with(|r| *r.borrow_mut() = Some(Vec::new()));
    }

    /// Stop collecting inline-size records and hand back what was recorded.
    pub fn take_inline() -> Vec<InlineDifferential> {
        INLINE_RECORD
            .with(|r| r.borrow_mut().take())
            .unwrap_or_default()
    }

    pub(crate) fn record_inline(d: InlineDifferential) {
        if log_to_stderr() {
            eprintln!(
                "L0-inline {} old_width={:.3} fragment_inline={}/64 unsnapped={:.5} available={:.3}",
                d.selector.as_deref().unwrap_or("(anonymous)"),
                d.old_width_px,
                d.fragment_inline.raw(),
                d.unsnapped_inline_px,
                d.available_px,
            );
        }
        INLINE_RECORD.with(|r| {
            if let Some(v) = r.borrow_mut().as_mut() {
                v.push(d);
            }
        });
    }

    /// Stop collecting and hand back what was recorded.
    pub fn take() -> Vec<Differential> {
        RECORD.with(|r| r.borrow_mut().take()).unwrap_or_default()
    }

    pub(crate) fn wanted() -> bool {
        log_to_stderr() || RECORD.with(|r| r.borrow().is_some())
    }

    pub(crate) fn record(d: Differential) {
        if log_to_stderr() {
            eprintln!(
                "L0 {} old_estimate={:.3} fragment_outer={:.3} fragment_block={}/64 unsnapped={:.5} phase_9_5_delta={}",
                d.selector.as_deref().unwrap_or("(anonymous)"),
                d.old_estimate_px,
                d.fragment_outer_px,
                d.fragment_block.raw(),
                d.unsnapped_block_px,
                d.phase_9_5_delta_px
                    .map_or("not-run".to_string(), |v| format!("{v:.3}")),
            );
        }
        RECORD.with(|r| {
            if let Some(v) = r.borrow_mut().as_mut() {
                v.push(d);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_layout_unit_is_a_sixty_fourth_of_a_pixel() {
        assert_eq!(LayoutUnit::from_px(1.0).raw(), 64);
        assert_eq!(LayoutUnit::from_px(43.0).to_px(), 43.0);
        // 0.01 px is 0.64 units: the nearest unit is 1.
        assert_eq!(LayoutUnit::from_px(0.01).raw(), 1);
        // Two ulps under a whole pixel is that pixel.
        assert_eq!(
            LayoutUnit::from_px(20.0 - 2.0 * f32::EPSILON * 20.0).raw(),
            20 * 64
        );
        assert_eq!(LayoutUnit::from_px(f32::NAN), LayoutUnit::ZERO);
        assert_eq!(LayoutUnit::from_px(f32::INFINITY).raw(), i32::MAX);
    }

    #[test]
    fn a_definite_zero_is_not_indefinite() {
        assert_ne!(AxisSize::definite_px(0.0), AxisSize::Indefinite);
        assert_eq!(AxisSize::definite_px(0.0).px(), Some(0.0));
        assert_eq!(AxisSize::Indefinite.px(), None);
    }

    #[test]
    fn a_query_l0_does_not_execute_is_not_this_slice() {
        let mut s = rustkit_css::ComputedStyle::new();
        s.display = rustkit_css::Display::Flex;
        let mut item = LayoutBox::new(BoxType::Block, s);
        item.children.push(LayoutBox::new(
            BoxType::Text("x".to_string()),
            rustkit_css::ComputedStyle::new(),
        ));
        assert!(in_l0_class(&item));

        let vertical = Constraint::definite_inline(100.0, WritingMode::VerticalRl, None);
        assert!(intrinsic_fragment(&item, &vertical, |_| {}).is_none());

        let mut min_content = Constraint::definite_inline(100.0, WritingMode::HorizontalTb, None);
        min_content.query = SizeQuery::MinContent;
        assert!(intrinsic_fragment(&item, &min_content, |_| {}).is_none());

        let mut column = item.clone();
        column.style.flex_direction = FlexDirection::Column;
        assert!(!in_l0_class(&column));

        let mut ratio = item.clone();
        ratio.style.aspect_ratio = Some(2.0);
        assert!(!in_l0_class(&ratio));

        let mut percent = item.clone();
        percent.style.height = Length::Percent(50.0);
        assert!(!in_l0_class(&percent));

        let mut empty = item.clone();
        empty.children.clear();
        assert!(!in_l0_class(&empty));
    }

    /// Membership that the first L0 slice pins in prose but not yet in a
    /// test: a form control counts; whitespace-only text, a `display:none`
    /// subtree, an image, and out-of-flow items do not. Nested queries also
    /// refuse to re-enter, so a queried subtree is not laid out once per level.
    #[test]
    fn l0_class_membership_and_nested_queries() {
        let flex = || {
            let mut s = rustkit_css::ComputedStyle::new();
            s.display = rustkit_css::Display::Flex;
            LayoutBox::new(BoxType::Block, s)
        };
        let mut with_control = flex();
        with_control.children.push(LayoutBox::new(
            BoxType::FormControl(crate::FormControlType::Button {
                label: "Go".into(),
                button_type: "button".into(),
            }),
            rustkit_css::ComputedStyle::new(),
        ));
        assert!(in_l0_class(&with_control), "a form control puts the item in L0");

        let mut grid = flex();
        grid.style.display = rustkit_css::Display::Grid;
        grid.children.push(LayoutBox::new(
            BoxType::Text("cell".into()),
            rustkit_css::ComputedStyle::new(),
        ));
        assert!(in_l0_class(&grid), "a grid holding text is in L0");

        let mut spaces = flex();
        spaces.children.push(LayoutBox::new(
            BoxType::Text(" \t\n".into()),
            rustkit_css::ComputedStyle::new(),
        ));
        assert!(!in_l0_class(&spaces), "whitespace-only text is not content");

        let mut hidden = flex();
        let mut none = rustkit_css::ComputedStyle::new();
        none.display = rustkit_css::Display::None;
        hidden.children.push(LayoutBox::new(BoxType::Text("x".into()), none));
        assert!(!in_l0_class(&hidden), "display:none children do not count");

        let mut image = flex();
        image.children.push(LayoutBox::new(
            BoxType::Image {
                url: "x.png".into(),
                natural_width: 10.0,
                natural_height: 10.0,
            },
            rustkit_css::ComputedStyle::new(),
        ));
        assert!(!in_l0_class(&image), "a replaced image alone is out of L0");

        let mut absolute = flex();
        absolute.style.position = rustkit_css::Position::Absolute;
        absolute.children.push(LayoutBox::new(
            BoxType::Text("x".into()),
            rustkit_css::ComputedStyle::new(),
        ));
        assert!(!in_l0_class(&absolute));
        absolute.style.position = rustkit_css::Position::Fixed;
        assert!(!in_l0_class(&absolute));

        let mut item = flex();
        item.children.push(LayoutBox::new(
            BoxType::Text("x".into()),
            rustkit_css::ComputedStyle::new(),
        ));
        let constraint = Constraint::definite_inline(100.0, WritingMode::HorizontalTb, None);
        QUERY_DEPTH.with(|d| d.set(1));
        assert!(
            intrinsic_fragment(&item, &constraint, |_| {}).is_none(),
            "a query already in flight must not re-enter"
        );
        QUERY_DEPTH.with(|d| d.set(0));
        assert!(
            intrinsic_fragment(&item, &constraint, |_| {}).is_some(),
            "precondition: the same item is queryable at depth 0"
        );
    }
}
