//! CSS margin collapsing implementation per CSS 2.1 §8.3.1.
//!
//! This module provides comprehensive margin collapsing support including:
//! - Adjacent sibling margin collapsing
//! - Parent-child margin collapsing (first/last child)
//! - Through-flow collapsing for empty elements
//! - Block formatting context (BFC) establishment detection
//!
//! # Margin Collapsing Rules (CSS 2.1 §8.3.1)
//!
//! Margins collapse when:
//! 1. Both belong to in-flow block-level boxes in the same BFC
//! 2. No line boxes, clearance, padding, or border separate them
//! 3. Both belong to vertically-adjacent box edges (top-bottom or bottom-top)
//!
//! When margins collapse:
//! - Two positive margins: use the larger
//! - Two negative margins: use the more negative (smaller absolute value)
//! - One positive, one negative: algebraic sum (positive + negative)
//!
//! # Block Formatting Context
//!
//! A new BFC is established by:
//! - The root element
//! - Floats (float != none)
//! - Absolutely positioned elements (position: absolute/fixed)
//! - Inline-blocks (display: inline-block)
//! - Table cells, captions (display: table-cell, table-caption)
//! - Elements with overflow != visible
//! - Flex/grid items
//! - Elements with contain: layout, content, or paint

use rustkit_css::{ComputedStyle, Length, Overflow, Position};

use crate::Float;

/// Collapsible margin representation per CSS 2.1 §8.3.1.
///
/// Tracks positive and negative margin components separately to correctly
/// implement margin collapsing rules:
/// - Positive margins: take the maximum
/// - Negative margins: take the minimum (most negative)
/// - Final result: sum of max positive and min negative
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct CollapsibleMargin {
    /// Largest positive margin component (always >= 0).
    pub positive: f32,
    /// Most negative margin component stored as absolute value (always >= 0).
    /// The actual negative margin is `-negative`.
    pub negative: f32,
}

impl CollapsibleMargin {
    /// Create a new zero collapsible margin.
    pub fn zero() -> Self {
        Self::default()
    }

    /// Create a collapsible margin from a single margin value.
    pub fn from_margin(value: f32) -> Self {
        if value >= 0.0 {
            Self {
                positive: value,
                negative: 0.0,
            }
        } else {
            Self {
                positive: 0.0,
                negative: -value, // Store as positive absolute value
            }
        }
    }

    /// Collapse this margin with another, returning the combined result.
    ///
    /// Per CSS 2.1:
    /// - Takes maximum of positive components
    /// - Takes maximum of negative components (most negative)
    /// - Final value = positive - negative
    pub fn collapse_with(self, other: Self) -> Self {
        Self {
            positive: self.positive.max(other.positive),
            negative: self.negative.max(other.negative),
        }
    }

    /// Adjoin a margin value to this collapsible margin.
    ///
    /// Equivalent to `collapse_with(CollapsibleMargin::from_margin(value))`.
    pub fn adjoin(&mut self, margin: f32) {
        if margin >= 0.0 {
            self.positive = self.positive.max(margin);
        } else {
            self.negative = self.negative.max(-margin);
        }
    }

    /// Resolve the collapsed margin to a single value.
    ///
    /// Returns `positive - negative`, which handles all three cases:
    /// - Both positive: returns max positive (negative is 0)
    /// - Both negative: returns most negative (positive is 0)
    /// - Mixed: returns algebraic sum
    pub fn resolve(self) -> f32 {
        self.positive - self.negative
    }

    /// Check if this margin is effectively zero.
    pub fn is_zero(&self) -> bool {
        self.positive == 0.0 && self.negative == 0.0
    }
}

/// Check if an element establishes a new block formatting context (BFC).
///
/// Per CSS 2.1 §9.4.1, a new BFC is established by:
/// - Floats (`float` is not `none`)
/// - Absolutely positioned elements (`position` is `absolute` or `fixed`)
/// - Inline-blocks (`display: inline-block`)
/// - Table cells (`display: table-cell`)
/// - Table captions (`display: table-caption`)
/// - Elements with `overflow` other than `visible`
/// - Flex/grid containers
/// - Elements with `display: flow-root`
///
/// Note: The root element also establishes a BFC but that's handled at the
/// layout tree level, not per-element style.
///
/// # Arguments
/// * `style` - The element's computed style
/// * `float` - The element's float value (from LayoutBox, not style)
pub fn establishes_bfc(style: &ComputedStyle, float: Float) -> bool {
    // Float creates BFC
    if float != Float::None {
        return true;
    }

    // Absolute/fixed positioning creates BFC
    if matches!(style.position, Position::Absolute | Position::Fixed) {
        return true;
    }

    // Overflow other than visible creates BFC
    if style.overflow_x != Overflow::Visible || style.overflow_y != Overflow::Visible {
        return true;
    }

    // display: inline-block creates BFC
    if style.display.is_inline_block() {
        return true;
    }

    // display: flex/grid containers create BFC for their contents
    if style.display.is_flex() || style.display.is_grid() {
        return true;
    }

    false
}

/// Check if a length value is auto.
#[inline]
fn is_length_auto(length: &Length) -> bool {
    matches!(length, Length::Auto)
}

/// Get the pixel value of a length, treating auto as 0.
#[inline]
fn length_to_px_or_zero(length: &Length) -> f32 {
    match length {
        Length::Auto => 0.0,
        Length::Zero => 0.0,
        Length::Px(px) => *px,
        // For relative units, we can't resolve them without context.
        // In margin collapse checks, we only need to know if it's non-zero
        // and for percentage/em/rem units, we treat them as potentially non-zero.
        _ => 1.0, // Non-zero sentinel
    }
}

/// Check if margins can collapse through an element (through-flow).
///
/// An element is "collapsible through" if:
/// 1. It has no in-flow content (no line boxes, no block children that aren't also collapsible through)
/// 2. It has no border or padding
/// 3. It has no clearance
/// 4. It does not establish a new BFC
/// 5. min-height is not set (or is 0)
/// 6. height is auto (or explicitly 0)
///
/// This allows an empty block's top and bottom margins to collapse together,
/// and potentially collapse with adjacent siblings.
///
/// # Arguments
/// * `style` - The element's computed style
/// * `float` - The element's float value
/// * `has_in_flow_content` - Whether the element has any in-flow content
/// * `border_top` - Resolved top border width in pixels
/// * `border_bottom` - Resolved bottom border width in pixels
/// * `padding_top` - Resolved top padding in pixels
/// * `padding_bottom` - Resolved bottom padding in pixels
/// * `has_clearance` - Whether the element has clearance (clear != none and affects layout)
pub fn is_margin_collapsible_through(
    style: &ComputedStyle,
    float: Float,
    has_in_flow_content: bool,
    border_top: f32,
    border_bottom: f32,
    padding_top: f32,
    padding_bottom: f32,
    has_clearance: bool,
) -> bool {
    // Must not have in-flow content
    if has_in_flow_content {
        return false;
    }

    // Must not have any border
    if border_top > 0.0 || border_bottom > 0.0 {
        return false;
    }

    // Must not have any padding
    if padding_top > 0.0 || padding_bottom > 0.0 {
        return false;
    }

    // Must not have clearance
    if has_clearance {
        return false;
    }

    // Must not establish a BFC
    if establishes_bfc(style, float) {
        return false;
    }

    // Height must be auto or 0
    if !is_length_auto(&style.height) && length_to_px_or_zero(&style.height) != 0.0 {
        return false;
    }

    // min-height must be 0 or auto
    if !is_length_auto(&style.min_height) && length_to_px_or_zero(&style.min_height) > 0.0 {
        return false;
    }

    true
}

/// Check if a parent's top margin should collapse with its first child's top margin.
///
/// This occurs when:
/// 1. Parent does not establish a new BFC
/// 2. Parent has no top border
/// 3. Parent has no top padding
/// 4. No clearance separates the parent's top margin from the child's top margin
///
/// # Arguments
/// * `parent_style` - The parent element's computed style
/// * `float` - The parent's float value
/// * `border_top` - Parent's resolved top border width in pixels
/// * `padding_top` - Parent's resolved top padding in pixels
pub fn should_collapse_with_first_child(
    parent_style: &ComputedStyle,
    float: Float,
    border_top: f32,
    padding_top: f32,
) -> bool {
    // BFC blocks collapse
    if establishes_bfc(parent_style, float) {
        return false;
    }

    // Border blocks collapse
    if border_top > 0.0 {
        return false;
    }

    // Padding blocks collapse
    if padding_top > 0.0 {
        return false;
    }

    true
}

/// Check if a parent's bottom margin should collapse with its last child's bottom margin.
///
/// This occurs when:
/// 1. Parent does not establish a new BFC
/// 2. Parent has no bottom border
/// 3. Parent has no bottom padding
/// 4. Parent has `height: auto` (no fixed height separating margins)
/// 5. Parent has `min-height` of 0 or auto
///
/// # Arguments
/// * `parent_style` - The parent element's computed style
/// * `float` - The parent's float value
/// * `border_bottom` - Parent's resolved bottom border width in pixels
/// * `padding_bottom` - Parent's resolved bottom padding in pixels
pub fn should_collapse_with_last_child(
    parent_style: &ComputedStyle,
    float: Float,
    border_bottom: f32,
    padding_bottom: f32,
) -> bool {
    // BFC blocks collapse
    if establishes_bfc(parent_style, float) {
        return false;
    }

    // Border blocks collapse
    if border_bottom > 0.0 {
        return false;
    }

    // Padding blocks collapse
    if padding_bottom > 0.0 {
        return false;
    }

    // Height must be auto for bottom margin to collapse
    if !is_length_auto(&parent_style.height) {
        return false;
    }

    // min-height must be 0 or auto
    if !is_length_auto(&parent_style.min_height) && length_to_px_or_zero(&parent_style.min_height) > 0.0
    {
        return false;
    }

    true
}

/// Collapse two margin values directly, returning the collapsed result.
///
/// This is a convenience function for simple cases where you just have
/// two margin values rather than `CollapsibleMargin` structs.
///
/// # Rules
/// - Two positive: max(m1, m2)
/// - Two negative: min(m1, m2) (most negative)
/// - Mixed: m1 + m2 (algebraic sum)
pub fn collapse_margins(margin1: f32, margin2: f32) -> f32 {
    CollapsibleMargin::from_margin(margin1)
        .collapse_with(CollapsibleMargin::from_margin(margin2))
        .resolve()
}

#[cfg(test)]
mod tests {
    use super::*;

    // ========================================================================
    // CollapsibleMargin struct tests
    // ========================================================================

    #[test]
    fn test_collapsible_margin_from_positive() {
        let m = CollapsibleMargin::from_margin(20.0);
        assert_eq!(m.positive, 20.0);
        assert_eq!(m.negative, 0.0);
        assert_eq!(m.resolve(), 20.0);
    }

    #[test]
    fn test_collapsible_margin_from_negative() {
        let m = CollapsibleMargin::from_margin(-15.0);
        assert_eq!(m.positive, 0.0);
        assert_eq!(m.negative, 15.0);
        assert_eq!(m.resolve(), -15.0);
    }

    #[test]
    fn test_collapsible_margin_from_zero() {
        let m = CollapsibleMargin::from_margin(0.0);
        assert!(m.is_zero());
        assert_eq!(m.resolve(), 0.0);
    }

    #[test]
    fn test_collapse_two_positive_margins() {
        let m1 = CollapsibleMargin::from_margin(20.0);
        let m2 = CollapsibleMargin::from_margin(30.0);
        let collapsed = m1.collapse_with(m2);
        assert_eq!(collapsed.resolve(), 30.0); // Max of positive
    }

    #[test]
    fn test_collapse_two_negative_margins() {
        let m1 = CollapsibleMargin::from_margin(-10.0);
        let m2 = CollapsibleMargin::from_margin(-25.0);
        let collapsed = m1.collapse_with(m2);
        assert_eq!(collapsed.resolve(), -25.0); // Most negative
    }

    #[test]
    fn test_collapse_mixed_margins_positive_wins() {
        let m1 = CollapsibleMargin::from_margin(30.0);
        let m2 = CollapsibleMargin::from_margin(-10.0);
        let collapsed = m1.collapse_with(m2);
        assert_eq!(collapsed.resolve(), 20.0); // 30 + (-10) = 20
    }

    #[test]
    fn test_collapse_mixed_margins_negative_wins() {
        let m1 = CollapsibleMargin::from_margin(10.0);
        let m2 = CollapsibleMargin::from_margin(-30.0);
        let collapsed = m1.collapse_with(m2);
        assert_eq!(collapsed.resolve(), -20.0); // 10 + (-30) = -20
    }

    #[test]
    fn test_adjoin_positive_to_positive() {
        let mut m = CollapsibleMargin::from_margin(10.0);
        m.adjoin(20.0);
        assert_eq!(m.resolve(), 20.0);
    }

    #[test]
    fn test_adjoin_negative_to_positive() {
        let mut m = CollapsibleMargin::from_margin(30.0);
        m.adjoin(-10.0);
        assert_eq!(m.resolve(), 20.0);
    }

    #[test]
    fn test_adjoin_multiple() {
        let mut m = CollapsibleMargin::zero();
        m.adjoin(10.0);
        m.adjoin(20.0);
        m.adjoin(-5.0);
        m.adjoin(-15.0);
        // positive: max(10, 20) = 20
        // negative: max(5, 15) = 15
        // result: 20 - 15 = 5
        assert_eq!(m.resolve(), 5.0);
    }

    #[test]
    fn test_collapse_commutative() {
        let m1 = CollapsibleMargin::from_margin(20.0);
        let m2 = CollapsibleMargin::from_margin(-10.0);
        assert_eq!(
            m1.collapse_with(m2).resolve(),
            m2.collapse_with(m1).resolve()
        );
    }

    // ========================================================================
    // collapse_margins helper function tests
    // ========================================================================

    #[test]
    fn test_collapse_margins_two_positive() {
        assert_eq!(collapse_margins(10.0, 20.0), 20.0);
    }

    #[test]
    fn test_collapse_margins_two_negative() {
        assert_eq!(collapse_margins(-10.0, -20.0), -20.0);
    }

    #[test]
    fn test_collapse_margins_mixed() {
        assert_eq!(collapse_margins(20.0, -10.0), 10.0);
        assert_eq!(collapse_margins(-10.0, 20.0), 10.0);
    }

    #[test]
    fn test_collapse_margins_zero() {
        assert_eq!(collapse_margins(0.0, 20.0), 20.0);
        assert_eq!(collapse_margins(20.0, 0.0), 20.0);
        assert_eq!(collapse_margins(0.0, -10.0), -10.0);
        assert_eq!(collapse_margins(0.0, 0.0), 0.0);
    }

    // ========================================================================
    // BFC establishment tests
    // ========================================================================

    #[test]
    fn test_bfc_default_style() {
        let style = ComputedStyle::new();
        // Default style with no float should not establish BFC
        assert!(!establishes_bfc(&style, Float::None));
    }

    #[test]
    fn test_bfc_float_left() {
        let style = ComputedStyle::new();
        assert!(establishes_bfc(&style, Float::Left));
    }

    #[test]
    fn test_bfc_float_right() {
        let style = ComputedStyle::new();
        assert!(establishes_bfc(&style, Float::Right));
    }

    #[test]
    fn test_bfc_absolute_position() {
        let mut style = ComputedStyle::new();
        style.position = Position::Absolute;
        assert!(establishes_bfc(&style, Float::None));
    }

    #[test]
    fn test_bfc_fixed_position() {
        let mut style = ComputedStyle::new();
        style.position = Position::Fixed;
        assert!(establishes_bfc(&style, Float::None));
    }

    #[test]
    fn test_bfc_relative_position_no_bfc() {
        let mut style = ComputedStyle::new();
        style.position = Position::Relative;
        assert!(!establishes_bfc(&style, Float::None));
    }

    #[test]
    fn test_bfc_overflow_hidden() {
        let mut style = ComputedStyle::new();
        style.overflow_x = Overflow::Hidden;
        assert!(establishes_bfc(&style, Float::None));
    }

    #[test]
    fn test_bfc_overflow_auto() {
        let mut style = ComputedStyle::new();
        style.overflow_y = Overflow::Auto;
        assert!(establishes_bfc(&style, Float::None));
    }

    #[test]
    fn test_bfc_overflow_scroll() {
        let mut style = ComputedStyle::new();
        style.overflow_x = Overflow::Scroll;
        assert!(establishes_bfc(&style, Float::None));
    }

    // ========================================================================
    // Through-flow collapse tests
    // ========================================================================

    #[test]
    fn test_through_flow_empty_element() {
        let style = ComputedStyle::new();
        // Empty element with no border/padding should be collapsible through
        assert!(is_margin_collapsible_through(
            &style,
            Float::None,
            false, // no in-flow content
            0.0,   // no border top
            0.0,   // no border bottom
            0.0,   // no padding top
            0.0,   // no padding bottom
            false, // no clearance
        ));
    }

    #[test]
    fn test_through_flow_blocked_by_content() {
        let style = ComputedStyle::new();
        assert!(!is_margin_collapsible_through(
            &style,
            Float::None,
            true, // has in-flow content
            0.0,
            0.0,
            0.0,
            0.0,
            false,
        ));
    }

    #[test]
    fn test_through_flow_blocked_by_border() {
        let style = ComputedStyle::new();
        assert!(!is_margin_collapsible_through(
            &style,
            Float::None,
            false,
            1.0, // border top
            0.0,
            0.0,
            0.0,
            false,
        ));
    }

    #[test]
    fn test_through_flow_blocked_by_padding() {
        let style = ComputedStyle::new();
        assert!(!is_margin_collapsible_through(
            &style,
            Float::None,
            false,
            0.0,
            0.0,
            10.0, // padding top
            0.0,
            false,
        ));
    }

    #[test]
    fn test_through_flow_blocked_by_clearance() {
        let style = ComputedStyle::new();
        assert!(!is_margin_collapsible_through(
            &style,
            Float::None,
            false,
            0.0,
            0.0,
            0.0,
            0.0,
            true, // has clearance
        ));
    }

    #[test]
    fn test_through_flow_blocked_by_bfc() {
        let mut style = ComputedStyle::new();
        style.overflow_x = Overflow::Hidden; // Creates BFC
        assert!(!is_margin_collapsible_through(
            &style,
            Float::None,
            false,
            0.0,
            0.0,
            0.0,
            0.0,
            false,
        ));
    }

    #[test]
    fn test_through_flow_blocked_by_float() {
        let style = ComputedStyle::new();
        assert!(!is_margin_collapsible_through(
            &style,
            Float::Left, // Float creates BFC
            false,
            0.0,
            0.0,
            0.0,
            0.0,
            false,
        ));
    }

    // ========================================================================
    // Parent-child collapse tests
    // ========================================================================

    #[test]
    fn test_collapse_with_first_child_allowed() {
        let style = ComputedStyle::new();
        assert!(should_collapse_with_first_child(&style, Float::None, 0.0, 0.0));
    }

    #[test]
    fn test_collapse_with_first_child_blocked_by_border() {
        let style = ComputedStyle::new();
        assert!(!should_collapse_with_first_child(&style, Float::None, 1.0, 0.0));
    }

    #[test]
    fn test_collapse_with_first_child_blocked_by_padding() {
        let style = ComputedStyle::new();
        assert!(!should_collapse_with_first_child(&style, Float::None, 0.0, 10.0));
    }

    #[test]
    fn test_collapse_with_first_child_blocked_by_bfc() {
        let mut style = ComputedStyle::new();
        style.overflow_y = Overflow::Auto;
        assert!(!should_collapse_with_first_child(&style, Float::None, 0.0, 0.0));
    }

    #[test]
    fn test_collapse_with_first_child_blocked_by_float() {
        let style = ComputedStyle::new();
        assert!(!should_collapse_with_first_child(&style, Float::Right, 0.0, 0.0));
    }

    #[test]
    fn test_collapse_with_last_child_allowed() {
        let style = ComputedStyle::new();
        assert!(should_collapse_with_last_child(&style, Float::None, 0.0, 0.0));
    }

    #[test]
    fn test_collapse_with_last_child_blocked_by_border() {
        let style = ComputedStyle::new();
        assert!(!should_collapse_with_last_child(&style, Float::None, 1.0, 0.0));
    }

    #[test]
    fn test_collapse_with_last_child_blocked_by_padding() {
        let style = ComputedStyle::new();
        assert!(!should_collapse_with_last_child(&style, Float::None, 0.0, 5.0));
    }

    #[test]
    fn test_collapse_with_last_child_blocked_by_height() {
        let mut style = ComputedStyle::new();
        style.height = Length::Px(100.0);
        assert!(!should_collapse_with_last_child(&style, Float::None, 0.0, 0.0));
    }
}
