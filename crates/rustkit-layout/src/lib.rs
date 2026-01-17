//! # RustKit Layout
//!
//! Layout engine for the RustKit browser engine.
//! Implements block and inline layout algorithms.
//!
//! ## Design Goals
//!
//! 1. **Block layout**: Stack boxes vertically with margin collapse
//! 2. **Inline layout**: Flow text and inline elements horizontally with wrapping
//! 3. **Text shaping**: Use DirectWrite for accurate text measurement
//! 4. **Display list**: Generate paint commands with correct z-order
//! 5. **Positioned elements**: Support relative, absolute, fixed, sticky
//! 6. **Float layout**: Basic float behavior and clearance
//! 7. **Stacking contexts**: Z-index based paint ordering
//! 8. **Text rendering**: Font fallback, decorations, line height

pub mod flex;
pub mod forms;
pub mod grid;
pub mod images;
pub mod scroll;
pub mod text;

pub use grid::{layout_grid_container, GridItem, GridLayout, GridTrack};
pub use forms::{
    calculate_caret_position, calculate_selection_rects, render_button, render_checkbox,
    render_input, render_radio, CaretInfo, InputLayout, InputState, SelectionInfo,
};
pub use flex::{layout_flex_container, Axis, FlexItem, FlexLine};
pub use scroll::{
    calculate_scroll_into_view, handle_wheel_event, is_scroll_container, render_scrollbars,
    ScrollAlignment, Scrollbar, ScrollbarOrientation, ScrollMomentum, ScrollState, StickyOffsets,
    StickyState, WheelDeltaMode,
};
pub use images::{
    calculate_intrinsic_size, calculate_placeholder_size, render_background_image,
    render_broken_image, render_image, ImageLayoutInfo,
};
pub use text::{
    apply_text_transform, collapse_whitespace, FontCache, FontDisplay, FontFaceRule,
    FontFamilyChain, FontLoader, LineHeight, PositionedGlyph, ShapedRun, TextDecoration, TextError,
    TextMetrics, TextShaper,
};

use rustkit_css::{Color, ComputedStyle, Length};
use std::cmp::Ordering;
use thiserror::Error;

/// Errors that can occur in layout.
#[derive(Error, Debug)]
pub enum LayoutError {
    #[error("Layout failed: {0}")]
    LayoutFailed(String),

    #[error("Text shaping error: {0}")]
    TextShapingError(String),
}

/// CSS position property values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Position {
    #[default]
    Static,
    Relative,
    Absolute,
    Fixed,
    Sticky,
}

/// CSS float property values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Float {
    #[default]
    None,
    Left,
    Right,
}

/// CSS clear property values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Clear {
    #[default]
    None,
    Left,
    Right,
    Both,
}

/// Offset values for positioned elements.
#[derive(Debug, Clone, Copy, Default)]
pub struct PositionOffsets {
    pub top: Option<f32>,
    pub right: Option<f32>,
    pub bottom: Option<f32>,
    pub left: Option<f32>,
}

/// Float exclusion area.
#[derive(Debug, Clone, Copy)]
pub struct FloatExclusion {
    pub rect: Rect,
    pub float_type: Float,
}

/// Float context for tracking float exclusions.
#[derive(Debug, Clone, Default)]
pub struct FloatContext {
    pub left_floats: Vec<FloatExclusion>,
    pub right_floats: Vec<FloatExclusion>,
}

impl FloatContext {
    /// Create a new empty float context.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a left float.
    pub fn add_left(&mut self, rect: Rect) {
        self.left_floats.push(FloatExclusion {
            rect,
            float_type: Float::Left,
        });
    }

    /// Add a right float.
    pub fn add_right(&mut self, rect: Rect) {
        self.right_floats.push(FloatExclusion {
            rect,
            float_type: Float::Right,
        });
    }

    /// Get available width at a given y position.
    pub fn available_width(&self, y: f32, container_width: f32) -> (f32, f32) {
        let mut left_edge: f32 = 0.0;
        let mut right_edge: f32 = container_width;

        for float in &self.left_floats {
            if y >= float.rect.y && y < float.rect.bottom() {
                left_edge = left_edge.max(float.rect.right());
            }
        }

        for float in &self.right_floats {
            if y >= float.rect.y && y < float.rect.bottom() {
                right_edge = right_edge.min(float.rect.x);
            }
        }

        (left_edge, right_edge)
    }

    /// Clear floats up to a given y position.
    pub fn clear(&mut self, clear: Clear) -> f32 {
        let mut clear_y: f32 = 0.0;

        match clear {
            Clear::Left => {
                for float in &self.left_floats {
                    clear_y = clear_y.max(float.rect.bottom());
                }
            }
            Clear::Right => {
                for float in &self.right_floats {
                    clear_y = clear_y.max(float.rect.bottom());
                }
            }
            Clear::Both => {
                for float in &self.left_floats {
                    clear_y = clear_y.max(float.rect.bottom());
                }
                for float in &self.right_floats {
                    clear_y = clear_y.max(float.rect.bottom());
                }
            }
            Clear::None => {}
        }

        clear_y
    }
}

/// Margin collapse context.
#[derive(Debug, Clone, Default)]
pub struct MarginCollapseContext {
    /// Pending positive margin.
    pub positive_margin: f32,
    /// Pending negative margin.
    pub negative_margin: f32,
}

impl MarginCollapseContext {
    /// Create a new margin collapse context.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a margin to the collapse context.
    pub fn add_margin(&mut self, margin: f32) {
        if margin >= 0.0 {
            self.positive_margin = self.positive_margin.max(margin);
        } else {
            self.negative_margin = self.negative_margin.min(margin);
        }
    }

    /// Resolve the collapsed margin.
    pub fn resolve(&self) -> f32 {
        self.positive_margin + self.negative_margin
    }

    /// Reset the context.
    pub fn reset(&mut self) {
        self.positive_margin = 0.0;
        self.negative_margin = 0.0;
    }
}

/// A 2D rectangle.
#[derive(Debug, Clone, Copy, Default)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Rect {
    pub fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub fn zero() -> Self {
        Self::default()
    }

    pub fn right(&self) -> f32 {
        self.x + self.width
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.height
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }
}

/// Border radius for rounded corners.
#[derive(Debug, Clone, Copy, Default)]
pub struct BorderRadius {
    pub top_left: f32,
    pub top_right: f32,
    pub bottom_right: f32,
    pub bottom_left: f32,
}

impl BorderRadius {
    /// Create uniform border radius.
    pub fn uniform(radius: f32) -> Self {
        Self {
            top_left: radius,
            top_right: radius,
            bottom_right: radius,
            bottom_left: radius,
        }
    }

    /// Check if all radii are zero (no rounding).
    pub fn is_zero(&self) -> bool {
        self.top_left == 0.0 && self.top_right == 0.0
            && self.bottom_right == 0.0 && self.bottom_left == 0.0
    }
}

/// Edge sizes (margin, padding, border).
#[derive(Debug, Clone, Copy, Default)]
pub struct EdgeSizes {
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
}

impl EdgeSizes {
    pub fn horizontal(&self) -> f32 {
        self.left + self.right
    }

    pub fn vertical(&self) -> f32 {
        self.top + self.bottom
    }
}

/// Box dimensions including content, padding, border, and margin.
#[derive(Debug, Clone, Default)]
pub struct Dimensions {
    /// Content area.
    pub content: Rect,
    /// Padding.
    pub padding: EdgeSizes,
    /// Border.
    pub border: EdgeSizes,
    /// Margin.
    pub margin: EdgeSizes,
}

impl Dimensions {
    /// Get the padding box (content + padding).
    pub fn padding_box(&self) -> Rect {
        Rect {
            x: self.content.x - self.padding.left,
            y: self.content.y - self.padding.top,
            width: self.content.width + self.padding.horizontal(),
            height: self.content.height + self.padding.vertical(),
        }
    }

    /// Get the border box (content + padding + border).
    pub fn border_box(&self) -> Rect {
        let pb = self.padding_box();
        Rect {
            x: pb.x - self.border.left,
            y: pb.y - self.border.top,
            width: pb.width + self.border.horizontal(),
            height: pb.height + self.border.vertical(),
        }
    }

    /// Get the margin box (content + padding + border + margin).
    pub fn margin_box(&self) -> Rect {
        let bb = self.border_box();
        Rect {
            x: bb.x - self.margin.left,
            y: bb.y - self.margin.top,
            width: bb.width + self.margin.horizontal(),
            height: bb.height + self.margin.vertical(),
        }
    }
}

/// Type of layout box.
#[derive(Debug, Clone)]
pub enum BoxType {
    /// Block-level box.
    Block,
    /// Inline-level box.
    Inline,
    /// Anonymous block (for grouping inline content).
    AnonymousBlock,
    /// Text run.
    Text(String),
}

/// Stacking context for z-index ordering.
#[derive(Debug, Clone, Default)]
pub struct StackingContext {
    /// Z-index value (0 for auto).
    pub z_index: i32,
    /// Whether this creates a new stacking context.
    pub creates_context: bool,
    /// Positioned children in this stacking context.
    pub positioned_children: Vec<usize>,
}

/// A layout box in the layout tree.
#[derive(Debug)]
pub struct LayoutBox {
    /// Box type.
    pub box_type: BoxType,
    /// Computed dimensions.
    pub dimensions: Dimensions,
    /// Computed style.
    pub style: ComputedStyle,
    /// Child boxes.
    pub children: Vec<LayoutBox>,
    /// CSS position property.
    pub position: Position,
    /// Position offsets (top, right, bottom, left).
    pub offsets: PositionOffsets,
    /// Float property.
    pub float: Float,
    /// Clear property.
    pub clear: Clear,
    /// Z-index for stacking.
    pub z_index: i32,
    /// Whether this box creates a stacking context.
    pub stacking_context: Option<StackingContext>,
    /// Reference to containing block (for positioned elements).
    #[allow(dead_code)]
    pub containing_block_index: Option<usize>,
}

impl LayoutBox {
    /// Create a new layout box.
    pub fn new(box_type: BoxType, style: ComputedStyle) -> Self {
        Self {
            box_type,
            dimensions: Dimensions::default(),
            style,
            children: Vec::new(),
            position: Position::Static,
            offsets: PositionOffsets::default(),
            float: Float::None,
            clear: Clear::None,
            z_index: 0,
            stacking_context: None,
            containing_block_index: None,
        }
    }

    /// Create a new layout box with positioning.
    pub fn with_position(box_type: BoxType, style: ComputedStyle, position: Position) -> Self {
        let mut layout_box = Self::new(box_type, style);
        layout_box.position = position;

        // Create stacking context if positioned with z-index
        if position != Position::Static {
            layout_box.stacking_context = Some(StackingContext::default());
        }

        layout_box
    }

    /// Create a new layout box with float.
    pub fn with_float(box_type: BoxType, style: ComputedStyle, float: Float) -> Self {
        let mut layout_box = Self::new(box_type, style);
        layout_box.float = float;
        layout_box
    }

    /// Set z-index and create stacking context if needed.
    pub fn set_z_index(&mut self, z_index: i32) {
        self.z_index = z_index;
        if self.position != Position::Static {
            let mut ctx = self.stacking_context.take().unwrap_or_default();
            ctx.z_index = z_index;
            ctx.creates_context = true;
            self.stacking_context = Some(ctx);
        }
    }

    /// Set position offsets.
    pub fn set_offsets(
        &mut self,
        top: Option<f32>,
        right: Option<f32>,
        bottom: Option<f32>,
        left: Option<f32>,
    ) {
        self.offsets = PositionOffsets {
            top,
            right,
            bottom,
            left,
        };
    }

    /// Perform layout within the given containing block.
    pub fn layout(&mut self, containing_block: &Dimensions) {
        match &self.box_type {
            BoxType::Block | BoxType::AnonymousBlock => {
                // Check for flex or grid container
                if self.style.display.is_flex() {
                    self.layout_block(containing_block);
                    // Flex layout is applied to children
                    flex::layout_flex_container(
                        self,
                        &self.dimensions.clone(),
                    );
                } else if self.style.display.is_grid() {
                    self.layout_block(containing_block);
                    // Grid layout is applied to children
                    grid::layout_grid_container(
                        self,
                        self.dimensions.content.width,
                        self.dimensions.content.height,
                    );
                } else {
                    self.layout_block(containing_block);
                }
            }
            BoxType::Inline => {
                // Inline boxes: position at containing block's current content area
                self.layout_inline(containing_block);
            }
            BoxType::Text(text) => {
                // Text boxes: calculate dimensions based on text content
                self.layout_text(text.clone(), containing_block);
            }
        }

        // Apply positioning offsets after normal layout
        self.apply_position_offsets(containing_block);
    }

    /// Layout an inline box.
    fn layout_inline(&mut self, containing_block: &Dimensions) {
        // Position at containing block's content area
        self.dimensions.content.x = containing_block.content.x;
        self.dimensions.content.y = containing_block.content.y + containing_block.content.height;
        
        // Layout inline children sequentially
        let mut cursor_x = 0.0;
        let mut max_height = 0.0f32;
        
        for child in &mut self.children {
            let mut cb = self.dimensions.clone();
            cb.content.x = self.dimensions.content.x + cursor_x;
            cb.content.height = 0.0;
            
            child.layout(&cb);
            
            cursor_x += child.dimensions.margin_box().width;
            max_height = max_height.max(child.dimensions.margin_box().height);
        }
        
        self.dimensions.content.width = cursor_x;
        self.dimensions.content.height = max_height.max(self.get_line_height());
    }

    /// Layout a text box.
    fn layout_text(&mut self, text: String, containing_block: &Dimensions) {
        // Get font size
        let font_size = match self.style.font_size {
            Length::Px(px) => px,
            _ => 16.0,
        };
        
        // Estimate text width (rough approximation: 0.6 * font_size * char_count)
        // In a real implementation, this would use font metrics
        let char_count = text.chars().count() as f32;
        let avg_char_width = font_size * 0.5; // Approximate average character width
        let text_width = char_count * avg_char_width;
        
        // Position at containing block's content area
        self.dimensions.content.x = containing_block.content.x;
        self.dimensions.content.y = containing_block.content.y + containing_block.content.height;
        self.dimensions.content.width = text_width.min(containing_block.content.width);
        self.dimensions.content.height = self.get_line_height();
    }

    /// Get line height for text layout.
    fn get_line_height(&self) -> f32 {
        let font_size = match self.style.font_size {
            Length::Px(px) => px,
            _ => 16.0,
        };
        // Default line height is 1.2 * font_size
        font_size * 1.2
    }

    /// Perform layout with margin collapse context.
    pub fn layout_with_collapse(
        &mut self,
        containing_block: &Dimensions,
        margin_context: &mut MarginCollapseContext,
        float_context: &mut FloatContext,
    ) {
        // Handle clear property
        if self.clear != Clear::None {
            let clear_y = float_context.clear(self.clear);
            if clear_y > 0.0 {
                margin_context.reset();
            }
        }

        match self.box_type {
            BoxType::Block | BoxType::AnonymousBlock => {
                self.layout_block_with_collapse(containing_block, margin_context, float_context);
            }
            BoxType::Inline | BoxType::Text(_) => {
                // Inline layout handled by parent
            }
        }

        // Handle float
        if self.float != Float::None {
            self.layout_float(containing_block, float_context);
        }

        // Apply positioning offsets after normal layout
        self.apply_position_offsets(containing_block);
    }

    /// Layout a block-level box.
    fn layout_block(&mut self, containing_block: &Dimensions) {
        // Calculate width first (depends on containing block)
        self.calculate_block_width(containing_block);

        // Position the box
        self.calculate_block_position(containing_block);

        // Layout children
        self.layout_block_children();

        // Height depends on children
        self.calculate_block_height();
    }

    /// Layout a block-level box with margin collapse.
    fn layout_block_with_collapse(
        &mut self,
        containing_block: &Dimensions,
        margin_context: &mut MarginCollapseContext,
        float_context: &mut FloatContext,
    ) {
        // Calculate width first (depends on containing block)
        self.calculate_block_width(containing_block);

        // Calculate margin/padding/border
        self.calculate_block_vertical_box_model(containing_block);

        // Handle margin collapse with previous sibling
        margin_context.add_margin(self.dimensions.margin.top);
        let collapsed_margin = margin_context.resolve();

        // Position the box with collapsed margin
        self.dimensions.content.x = containing_block.content.x
            + self.dimensions.margin.left
            + self.dimensions.border.left
            + self.dimensions.padding.left;

        self.dimensions.content.y = containing_block.content.y
            + containing_block.content.height
            + collapsed_margin
            + self.dimensions.border.top
            + self.dimensions.padding.top;

        // If this box has border or padding, margins don't collapse through it
        let blocks_collapse = self.dimensions.border.top > 0.0
            || self.dimensions.padding.top > 0.0
            || self.dimensions.border.bottom > 0.0
            || self.dimensions.padding.bottom > 0.0;

        // Layout children with new margin context
        if blocks_collapse {
            let mut child_margin_context = MarginCollapseContext::new();
            self.layout_block_children_with_collapse(&mut child_margin_context, float_context);
        } else {
            // Margins can collapse through this box
            self.layout_block_children_with_collapse(margin_context, float_context);
        }

        // Height depends on children
        self.calculate_block_height();

        // Reset margin context for next sibling, add bottom margin
        margin_context.reset();
        margin_context.add_margin(self.dimensions.margin.bottom);
    }

    /// Calculate vertical box model values (margin, border, padding).
    fn calculate_block_vertical_box_model(&mut self, containing_block: &Dimensions) {
        let style = &self.style;

        self.dimensions.margin.top =
            self.length_to_px(style.margin_top, containing_block.content.width);
        self.dimensions.margin.bottom =
            self.length_to_px(style.margin_bottom, containing_block.content.width);
        self.dimensions.border.top =
            self.length_to_px(style.border_top_width, containing_block.content.width);
        self.dimensions.border.bottom =
            self.length_to_px(style.border_bottom_width, containing_block.content.width);
        self.dimensions.padding.top =
            self.length_to_px(style.padding_top, containing_block.content.width);
        self.dimensions.padding.bottom =
            self.length_to_px(style.padding_bottom, containing_block.content.width);
    }

    /// Layout a floated box.
    fn layout_float(&mut self, containing_block: &Dimensions, float_context: &mut FloatContext) {
        // Calculate dimensions
        self.calculate_block_width(containing_block);
        self.calculate_block_vertical_box_model(containing_block);

        // Find position based on float type
        let (left_edge, right_edge) = float_context.available_width(
            containing_block.content.y + containing_block.content.height,
            containing_block.content.width,
        );

        let box_width = self.dimensions.margin_box().width;

        match self.float {
            Float::Left => {
                self.dimensions.content.x = containing_block.content.x
                    + left_edge
                    + self.dimensions.margin.left
                    + self.dimensions.border.left
                    + self.dimensions.padding.left;

                float_context.add_left(self.dimensions.margin_box());
            }
            Float::Right => {
                self.dimensions.content.x = containing_block.content.x + right_edge - box_width
                    + self.dimensions.margin.left
                    + self.dimensions.border.left
                    + self.dimensions.padding.left;

                float_context.add_right(self.dimensions.margin_box());
            }
            Float::None => {}
        }

        self.dimensions.content.y = containing_block.content.y
            + containing_block.content.height
            + self.dimensions.margin.top
            + self.dimensions.border.top
            + self.dimensions.padding.top;

        // Layout children
        self.layout_block_children();
        self.calculate_block_height();
    }

    /// Apply position offsets for positioned elements.
    fn apply_position_offsets(&mut self, containing_block: &Dimensions) {
        match self.position {
            Position::Static => {
                // No offsets applied
            }
            Position::Relative => {
                // Offset from normal flow position
                if let Some(top) = self.offsets.top {
                    self.dimensions.content.y += top;
                } else if let Some(bottom) = self.offsets.bottom {
                    self.dimensions.content.y -= bottom;
                }

                if let Some(left) = self.offsets.left {
                    self.dimensions.content.x += left;
                } else if let Some(right) = self.offsets.right {
                    self.dimensions.content.x -= right;
                }
            }
            Position::Absolute => {
                // Position relative to containing block
                if let Some(left) = self.offsets.left {
                    self.dimensions.content.x = containing_block.content.x
                        + left
                        + self.dimensions.margin.left
                        + self.dimensions.border.left
                        + self.dimensions.padding.left;
                } else if let Some(right) = self.offsets.right {
                    self.dimensions.content.x = containing_block.content.right()
                        - right
                        - self.dimensions.margin.right
                        - self.dimensions.border.right
                        - self.dimensions.padding.right
                        - self.dimensions.content.width;
                }

                if let Some(top) = self.offsets.top {
                    self.dimensions.content.y = containing_block.content.y
                        + top
                        + self.dimensions.margin.top
                        + self.dimensions.border.top
                        + self.dimensions.padding.top;
                } else if let Some(bottom) = self.offsets.bottom {
                    self.dimensions.content.y = containing_block.content.bottom()
                        - bottom
                        - self.dimensions.margin.bottom
                        - self.dimensions.border.bottom
                        - self.dimensions.padding.bottom
                        - self.dimensions.content.height;
                }
            }
            Position::Fixed => {
                // Position relative to viewport (root containing block)
                // In a full implementation, this would use the viewport dimensions
                self.apply_position_offsets_absolute(containing_block);
            }
            Position::Sticky => {
                // Hybrid of relative and fixed
                // For now, treat like relative
                if let Some(top) = self.offsets.top {
                    self.dimensions.content.y += top;
                }
                if let Some(left) = self.offsets.left {
                    self.dimensions.content.x += left;
                }
            }
        }
    }

    /// Apply absolute positioning offsets.
    fn apply_position_offsets_absolute(&mut self, containing_block: &Dimensions) {
        if let Some(left) = self.offsets.left {
            self.dimensions.content.x = containing_block.content.x
                + left
                + self.dimensions.margin.left
                + self.dimensions.border.left
                + self.dimensions.padding.left;
        } else if let Some(right) = self.offsets.right {
            self.dimensions.content.x = containing_block.content.right()
                - right
                - self.dimensions.margin.right
                - self.dimensions.border.right
                - self.dimensions.padding.right
                - self.dimensions.content.width;
        }

        if let Some(top) = self.offsets.top {
            self.dimensions.content.y = containing_block.content.y
                + top
                + self.dimensions.margin.top
                + self.dimensions.border.top
                + self.dimensions.padding.top;
        } else if let Some(bottom) = self.offsets.bottom {
            self.dimensions.content.y = containing_block.content.bottom()
                - bottom
                - self.dimensions.margin.bottom
                - self.dimensions.border.bottom
                - self.dimensions.padding.bottom
                - self.dimensions.content.height;
        }
    }

    /// Calculate block width.
    fn calculate_block_width(&mut self, containing_block: &Dimensions) {
        let style = &self.style;

        // Get values from style
        let margin_left = self.length_to_px(style.margin_left, containing_block.content.width);
        let margin_right = self.length_to_px(style.margin_right, containing_block.content.width);
        let border_left =
            self.length_to_px(style.border_left_width, containing_block.content.width);
        let border_right =
            self.length_to_px(style.border_right_width, containing_block.content.width);
        let padding_left = self.length_to_px(style.padding_left, containing_block.content.width);
        let padding_right = self.length_to_px(style.padding_right, containing_block.content.width);

        let total_margin_border_padding =
            margin_left + margin_right + border_left + border_right + padding_left + padding_right;

        // Calculate content width
        let content_width = match style.width {
            Length::Auto => {
                // Fill available space
                (containing_block.content.width - total_margin_border_padding).max(0.0)
            }
            _ => self.length_to_px(style.width, containing_block.content.width),
        };

        self.dimensions.content.width = content_width;
        self.dimensions.margin.left = margin_left;
        self.dimensions.margin.right = margin_right;
        self.dimensions.border.left = border_left;
        self.dimensions.border.right = border_right;
        self.dimensions.padding.left = padding_left;
        self.dimensions.padding.right = padding_right;
    }

    /// Calculate block position.
    fn calculate_block_position(&mut self, containing_block: &Dimensions) {
        let style = &self.style;

        self.dimensions.margin.top =
            self.length_to_px(style.margin_top, containing_block.content.width);
        self.dimensions.margin.bottom =
            self.length_to_px(style.margin_bottom, containing_block.content.width);
        self.dimensions.border.top =
            self.length_to_px(style.border_top_width, containing_block.content.width);
        self.dimensions.border.bottom =
            self.length_to_px(style.border_bottom_width, containing_block.content.width);
        self.dimensions.padding.top =
            self.length_to_px(style.padding_top, containing_block.content.width);
        self.dimensions.padding.bottom =
            self.length_to_px(style.padding_bottom, containing_block.content.width);

        // Position below the containing block's content
        self.dimensions.content.x = containing_block.content.x
            + self.dimensions.margin.left
            + self.dimensions.border.left
            + self.dimensions.padding.left;

        self.dimensions.content.y = containing_block.content.y
            + containing_block.content.height
            + self.dimensions.margin.top
            + self.dimensions.border.top
            + self.dimensions.padding.top;
    }

    /// Layout block children.
    fn layout_block_children(&mut self) {
        let mut cursor_y = 0.0;

        for child in &mut self.children {
            // Create a containing block at current cursor position
            let mut cb = self.dimensions.clone();
            cb.content.height = cursor_y;

            child.layout(&cb);

            // Advance cursor by child's margin box height (unless floated or positioned)
            if child.float == Float::None
                && child.position != Position::Absolute
                && child.position != Position::Fixed
            {
                cursor_y += child.dimensions.margin_box().height;
            }
        }

        self.dimensions.content.height = cursor_y;
    }

    /// Layout block children with margin collapse.
    fn layout_block_children_with_collapse(
        &mut self,
        margin_context: &mut MarginCollapseContext,
        float_context: &mut FloatContext,
    ) {
        let mut cursor_y = 0.0;

        for child in &mut self.children {
            // Create a containing block at current cursor position
            let mut cb = self.dimensions.clone();
            cb.content.height = cursor_y;

            child.layout_with_collapse(&cb, margin_context, float_context);

            // Advance cursor by child's box height (unless floated or positioned)
            if child.float == Float::None
                && child.position != Position::Absolute
                && child.position != Position::Fixed
            {
                // Use border box height plus margin top (bottom margin collapses with next sibling)
                cursor_y = child.dimensions.border_box().bottom() - self.dimensions.content.y;
            }
        }

        self.dimensions.content.height = cursor_y;
    }

    /// Calculate block height.
    fn calculate_block_height(&mut self) {
        // If height is explicitly set, use it
        if let Length::Px(h) = self.style.height {
            self.dimensions.content.height = h;
        }
        // Otherwise, content.height was set by layout_block_children
    }

    /// Convert a Length to pixels.
    fn length_to_px(&self, length: Length, container_size: f32) -> f32 {
        let font_size = match self.style.font_size {
            Length::Px(px) => px,
            _ => 16.0,
        };
        length.to_px(font_size, 16.0, container_size)
    }

    /// Get children sorted by z-index for painting.
    pub fn get_paint_order(&self) -> Vec<&LayoutBox> {
        let mut normal_flow: Vec<&LayoutBox> = Vec::new();
        let mut positioned: Vec<(&LayoutBox, i32)> = Vec::new();

        for child in &self.children {
            if child.position == Position::Static {
                normal_flow.push(child);
            } else {
                positioned.push((child, child.z_index));
            }
        }

        // Sort positioned elements by z-index
        positioned.sort_by(|a, b| a.1.cmp(&b.1));

        // Combine: negative z-index, normal flow, positive z-index
        let mut result: Vec<&LayoutBox> = Vec::new();

        // Add negative z-index positioned elements first
        for (child, z) in positioned.iter() {
            if *z < 0 {
                result.push(child);
            }
        }

        // Add normal flow elements
        result.extend(normal_flow);

        // Add zero and positive z-index positioned elements
        for (child, z) in positioned.iter() {
            if *z >= 0 {
                result.push(child);
            }
        }

        result
    }

    /// Perform hit testing at the given point.
    /// Returns the hit test result with information about the element at the point.
    pub fn hit_test(&self, x: f32, y: f32) -> Option<HitTestResult> {
        self.hit_test_internal(x, y, 0)
    }

    /// Internal hit test that tracks depth.
    fn hit_test_internal(&self, x: f32, y: f32, depth: u32) -> Option<HitTestResult> {
        // Get the border box for this element
        let border_box = self.dimensions.border_box();

        // Check if the point is within our border box
        if !border_box.contains(x, y) {
            return None;
        }

        // Check children in reverse paint order (topmost first)
        let paint_order = self.get_paint_order();
        for child in paint_order.iter().rev() {
            if let Some(mut result) = child.hit_test_internal(x, y, depth + 1) {
                // Found a hit in a child - add ourselves to the path
                result.ancestors.push(HitTestAncestor {
                    box_type: self.box_type.clone(),
                    border_box: self.dimensions.border_box(),
                    content_box: self.dimensions.content,
                    z_index: self.z_index,
                    position: self.position,
                });
                return Some(result);
            }
        }

        // No child was hit, so we are the target
        Some(HitTestResult {
            box_type: self.box_type.clone(),
            border_box,
            content_box: self.dimensions.content,
            padding_box: self.dimensions.padding_box(),
            local_x: x - border_box.x,
            local_y: y - border_box.y,
            depth,
            ancestors: Vec::new(),
            z_index: self.z_index,
            position: self.position,
            is_scrollable: false, // TODO: detect overflow
        })
    }

    /// Check if a point is within the border box.
    pub fn contains_point(&self, x: f32, y: f32) -> bool {
        self.dimensions.border_box().contains(x, y)
    }

    /// Get all elements at a point (including overlapping elements).
    pub fn hit_test_all(&self, x: f32, y: f32) -> Vec<HitTestResult> {
        let mut results = Vec::new();
        self.hit_test_all_internal(x, y, 0, &mut results);
        results
    }

    /// Internal hit test that collects all results.
    fn hit_test_all_internal(&self, x: f32, y: f32, depth: u32, results: &mut Vec<HitTestResult>) {
        let border_box = self.dimensions.border_box();

        if !border_box.contains(x, y) {
            return;
        }

        // Add this element
        results.push(HitTestResult {
            box_type: self.box_type.clone(),
            border_box,
            content_box: self.dimensions.content,
            padding_box: self.dimensions.padding_box(),
            local_x: x - border_box.x,
            local_y: y - border_box.y,
            depth,
            ancestors: Vec::new(),
            z_index: self.z_index,
            position: self.position,
            is_scrollable: false,
        });

        // Check all children
        for child in &self.children {
            child.hit_test_all_internal(x, y, depth + 1, results);
        }
    }
}

/// Result of a hit test operation.
#[derive(Debug, Clone)]
pub struct HitTestResult {
    /// The type of the hit box.
    pub box_type: BoxType,
    /// The border box of the hit element.
    pub border_box: Rect,
    /// The content box of the hit element.
    pub content_box: Rect,
    /// The padding box of the hit element.
    pub padding_box: Rect,
    /// X coordinate relative to the border box.
    pub local_x: f32,
    /// Y coordinate relative to the border box.
    pub local_y: f32,
    /// Depth in the layout tree (0 = root).
    pub depth: u32,
    /// Ancestor chain from parent to root.
    pub ancestors: Vec<HitTestAncestor>,
    /// Z-index of the hit element.
    pub z_index: i32,
    /// Position property of the hit element.
    pub position: Position,
    /// Whether the element is scrollable.
    pub is_scrollable: bool,
}

impl HitTestResult {
    /// Check if the hit was in the content area.
    pub fn is_in_content(&self) -> bool {
        self.content_box.contains(
            self.border_box.x + self.local_x,
            self.border_box.y + self.local_y,
        )
    }

    /// Check if the hit was in the padding area.
    pub fn is_in_padding(&self) -> bool {
        let abs_x = self.border_box.x + self.local_x;
        let abs_y = self.border_box.y + self.local_y;
        self.padding_box.contains(abs_x, abs_y) && !self.content_box.contains(abs_x, abs_y)
    }

    /// Check if the hit was in the border area.
    pub fn is_in_border(&self) -> bool {
        let abs_x = self.border_box.x + self.local_x;
        let abs_y = self.border_box.y + self.local_y;
        self.border_box.contains(abs_x, abs_y) && !self.padding_box.contains(abs_x, abs_y)
    }
}

/// Information about an ancestor in the hit test path.
#[derive(Debug, Clone)]
pub struct HitTestAncestor {
    /// Box type.
    pub box_type: BoxType,
    /// Border box.
    pub border_box: Rect,
    /// Content box.
    pub content_box: Rect,
    /// Z-index.
    pub z_index: i32,
    /// Position property.
    pub position: Position,
}

/// A paint command for rendering.
#[derive(Debug, Clone)]
pub enum DisplayCommand {
    /// Fill a rectangle with a solid color.
    SolidColor(Color, Rect),
    /// Draw a border.
    Border {
        color: Color,
        rect: Rect,
        top: f32,
        right: f32,
        bottom: f32,
        left: f32,
    },
    /// Draw text.
    Text {
        text: String,
        x: f32,
        y: f32,
        color: Color,
        font_size: f32,
        font_family: String,
        font_weight: u16,
        font_style: u8,
    },
    /// Draw text decoration line (underline, strikethrough, overline).
    TextDecoration {
        x: f32,
        y: f32,
        width: f32,
        thickness: f32,
        color: Color,
        style: TextDecorationStyleValue,
    },
    /// Draw an image.
    Image {
        /// URL or cache key of the image
        url: String,
        /// Source rectangle in the image (for sprites or cropping)
        src_rect: Option<Rect>,
        /// Destination rectangle on screen
        dest_rect: Rect,
        /// Object-fit mode
        object_fit: ObjectFit,
        /// Opacity (0.0 - 1.0)
        opacity: f32,
    },
    /// Draw a background image.
    BackgroundImage {
        /// URL or cache key of the image
        url: String,
        /// Destination rectangle
        rect: Rect,
        /// Background size
        size: BackgroundSize,
        /// Background position (0-1 range)
        position: (f32, f32),
        /// Background repeat
        repeat: BackgroundRepeat,
    },
    /// Draw a linear gradient.
    LinearGradient {
        rect: Rect,
        direction: rustkit_css::GradientDirection,
        stops: Vec<rustkit_css::ColorStop>,
        repeating: bool,
        border_radius: BorderRadius,
    },
    /// Draw a radial gradient.
    RadialGradient {
        rect: Rect,
        shape: rustkit_css::RadialShape,
        size: rustkit_css::RadialSize,
        center: (f32, f32),
        stops: Vec<rustkit_css::ColorStop>,
        repeating: bool,
        border_radius: BorderRadius,
    },
    /// Draw a conic gradient.
    ConicGradient {
        rect: Rect,
        from_angle: f32,
        center: (f32, f32),
        stops: Vec<rustkit_css::ColorStop>,
        repeating: bool,
        border_radius: BorderRadius,
    },
    /// Push a clip rect (for overflow handling).
    PushClip(Rect),
    /// Pop clip rect.
    PopClip,
    /// Start stacking context.
    PushStackingContext { z_index: i32, rect: Rect },
    /// End stacking context.
    PopStackingContext,

    // SVG-specific commands
    /// Fill a rectangle with solid color.
    FillRect { rect: Rect, color: Color },
    /// Stroke a rectangle.
    StrokeRect { rect: Rect, color: Color, width: f32 },
    /// Fill a circle.
    FillCircle { cx: f32, cy: f32, radius: f32, color: Color },
    /// Stroke a circle.
    StrokeCircle { cx: f32, cy: f32, radius: f32, color: Color, width: f32 },
    /// Fill an ellipse.
    FillEllipse { rect: Rect, color: Color },
    /// Draw a line.
    Line { x1: f32, y1: f32, x2: f32, y2: f32, color: Color, width: f32 },
    /// Draw a polyline (connected line segments).
    Polyline { points: Vec<(f32, f32)>, color: Color, width: f32 },
    /// Fill a polygon.
    FillPolygon { points: Vec<(f32, f32)>, color: Color },
    /// Stroke a polygon.
    StrokePolygon { points: Vec<(f32, f32)>, color: Color, width: f32 },
}

/// Text decoration style for display commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextDecorationStyleValue {
    Solid,
    Double,
    Dotted,
    Dashed,
    Wavy,
}

/// CSS object-fit values for images.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ObjectFit {
    /// Fill the box, possibly distorting the image
    Fill,
    /// Scale to fit inside the box, preserving aspect ratio
    #[default]
    Contain,
    /// Scale to cover the box, preserving aspect ratio
    Cover,
    /// Don't scale the image
    None,
    /// Like contain but never scale up
    ScaleDown,
}

impl ObjectFit {
    /// Parse from CSS value
    pub fn from_css(value: &str) -> Self {
        match value.trim().to_lowercase().as_str() {
            "fill" => ObjectFit::Fill,
            "contain" => ObjectFit::Contain,
            "cover" => ObjectFit::Cover,
            "none" => ObjectFit::None,
            "scale-down" => ObjectFit::ScaleDown,
            _ => ObjectFit::default(),
        }
    }

    /// Calculate the image rectangle within a container
    pub fn compute_rect(
        &self,
        container: Rect,
        image_width: f32,
        image_height: f32,
        position: (f32, f32),
    ) -> ImageDrawRect {
        if image_width == 0.0 || image_height == 0.0 {
            return ImageDrawRect {
                dest: container,
                src: None,
            };
        }

        let image_aspect = image_width / image_height;
        let container_aspect = container.width / container.height;

        let (draw_width, draw_height) = match self {
            ObjectFit::Fill => (container.width, container.height),

            ObjectFit::Contain => {
                if image_aspect > container_aspect {
                    (container.width, container.width / image_aspect)
                } else {
                    (container.height * image_aspect, container.height)
                }
            }

            ObjectFit::Cover => {
                if image_aspect > container_aspect {
                    (container.height * image_aspect, container.height)
                } else {
                    (container.width, container.width / image_aspect)
                }
            }

            ObjectFit::None => (image_width, image_height),

            ObjectFit::ScaleDown => {
                if image_width <= container.width && image_height <= container.height {
                    (image_width, image_height)
                } else if image_aspect > container_aspect {
                    (container.width, container.width / image_aspect)
                } else {
                    (container.height * image_aspect, container.height)
                }
            }
        };

        let x = container.x + (container.width - draw_width) * position.0;
        let y = container.y + (container.height - draw_height) * position.1;

        ImageDrawRect {
            dest: Rect {
                x,
                y,
                width: draw_width,
                height: draw_height,
            },
            src: None,
        }
    }
}

/// Result of computing image draw rectangle
#[derive(Debug, Clone)]
pub struct ImageDrawRect {
    /// Destination rectangle
    pub dest: Rect,
    /// Source rectangle (for cropping, e.g., in cover mode)
    pub src: Option<Rect>,
}

/// CSS background-size values.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum BackgroundSize {
    /// Stretch to fill
    Cover,
    /// Scale to fit
    Contain,
    /// Explicit size
    Explicit { width: Option<f32>, height: Option<f32> },
    /// Auto sizing
    #[default]
    Auto,
}

impl BackgroundSize {
    /// Parse from CSS value
    pub fn from_css(value: &str) -> Self {
        match value.trim().to_lowercase().as_str() {
            "cover" => BackgroundSize::Cover,
            "contain" => BackgroundSize::Contain,
            "auto" => BackgroundSize::Auto,
            _ => {
                // Try to parse explicit size
                let parts: Vec<&str> = value.split_whitespace().collect();
                let width = parts.first().and_then(|s| parse_length(s));
                let height = parts.get(1).and_then(|s| parse_length(s));
                BackgroundSize::Explicit { width, height }
            }
        }
    }

    /// Calculate the background image size
    pub fn compute_size(
        &self,
        container: Rect,
        image_width: f32,
        image_height: f32,
    ) -> (f32, f32) {
        if image_width == 0.0 || image_height == 0.0 {
            return (0.0, 0.0);
        }

        let image_aspect = image_width / image_height;
        let container_aspect = container.width / container.height;

        match self {
            BackgroundSize::Cover => {
                if image_aspect > container_aspect {
                    (container.height * image_aspect, container.height)
                } else {
                    (container.width, container.width / image_aspect)
                }
            }

            BackgroundSize::Contain => {
                if image_aspect > container_aspect {
                    (container.width, container.width / image_aspect)
                } else {
                    (container.height * image_aspect, container.height)
                }
            }

            BackgroundSize::Auto => (image_width, image_height),

            BackgroundSize::Explicit { width, height } => {
                match (width, height) {
                    (Some(w), Some(h)) => (*w, *h),
                    (Some(w), None) => (*w, *w / image_aspect),
                    (None, Some(h)) => (*h * image_aspect, *h),
                    (None, None) => (image_width, image_height),
                }
            }
        }
    }
}

/// CSS background-repeat values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BackgroundRepeat {
    /// Repeat in both directions
    #[default]
    Repeat,
    /// Repeat horizontally only
    RepeatX,
    /// Repeat vertically only
    RepeatY,
    /// No repeat
    NoRepeat,
    /// Space evenly
    Space,
    /// Round to fill
    Round,
}

impl BackgroundRepeat {
    /// Parse from CSS value
    pub fn from_css(value: &str) -> Self {
        match value.trim().to_lowercase().as_str() {
            "repeat" => BackgroundRepeat::Repeat,
            "repeat-x" => BackgroundRepeat::RepeatX,
            "repeat-y" => BackgroundRepeat::RepeatY,
            "no-repeat" => BackgroundRepeat::NoRepeat,
            "space" => BackgroundRepeat::Space,
            "round" => BackgroundRepeat::Round,
            _ => BackgroundRepeat::default(),
        }
    }

    /// Check if repeating on x-axis
    pub fn repeats_x(&self) -> bool {
        matches!(self, BackgroundRepeat::Repeat | BackgroundRepeat::RepeatX | BackgroundRepeat::Space | BackgroundRepeat::Round)
    }

    /// Check if repeating on y-axis
    pub fn repeats_y(&self) -> bool {
        matches!(self, BackgroundRepeat::Repeat | BackgroundRepeat::RepeatY | BackgroundRepeat::Space | BackgroundRepeat::Round)
    }
}

/// Parse a CSS length value to pixels
fn parse_length(value: &str) -> Option<f32> {
    let value = value.trim();
    if value.ends_with("px") {
        value.trim_end_matches("px").parse().ok()
    } else if value.ends_with('%') {
        // Percentages would need container size - return None for now
        None
    } else {
        value.parse().ok()
    }
}

/// A paint item with z-index for sorting.
#[derive(Debug, Clone)]
#[allow(dead_code)]
struct PaintItem {
    z_index: i32,
    layer: u32, // For stable sort within same z-index
    commands: Vec<DisplayCommand>,
}

#[allow(dead_code)]
impl PaintItem {
    fn new(z_index: i32, layer: u32) -> Self {
        Self {
            z_index,
            layer,
            commands: Vec::new(),
        }
    }
}

/// A display list of paint commands.
#[derive(Debug, Default)]
pub struct DisplayList {
    pub commands: Vec<DisplayCommand>,
}

impl DisplayList {
    /// Create an empty display list.
    pub fn new() -> Self {
        Self {
            commands: Vec::new(),
        }
    }

    /// Build display list from a layout box with proper stacking order.
    pub fn build(root: &LayoutBox) -> Self {
        let mut list = DisplayList::new();
        list.render_stacking_context(root, 0, &mut 0);
        list
    }

    /// Render a stacking context with proper z-ordering.
    fn render_stacking_context(&mut self, layout_box: &LayoutBox, parent_z: i32, layer: &mut u32) {
        let z_index = if layout_box.position != Position::Static {
            layout_box.z_index
        } else {
            parent_z
        };

        // Check if this creates a new stacking context
        let creates_context = layout_box
            .stacking_context
            .as_ref()
            .map(|ctx| ctx.creates_context)
            .unwrap_or(false);

        if creates_context {
            self.commands.push(DisplayCommand::PushStackingContext {
                z_index,
                rect: layout_box.dimensions.border_box(),
            });
        }

        // Render this box
        self.render_box_content(layout_box);

        // Collect children grouped by paint order
        let mut negative_z: Vec<(&LayoutBox, u32)> = Vec::new();
        let mut normal_flow: Vec<(&LayoutBox, u32)> = Vec::new();
        let mut positive_z: Vec<(&LayoutBox, u32)> = Vec::new();

        for child in &layout_box.children {
            *layer += 1;
            let child_layer = *layer;

            if child.position != Position::Static {
                if child.z_index < 0 {
                    negative_z.push((child, child_layer));
                } else {
                    positive_z.push((child, child_layer));
                }
            } else if child.float != Float::None {
                // Floats paint between normal flow and positioned
                positive_z.push((child, child_layer));
            } else {
                normal_flow.push((child, child_layer));
            }
        }

        // Sort by z-index, then by layer for stability
        negative_z.sort_by(|a, b| {
            let z_cmp = a.0.z_index.cmp(&b.0.z_index);
            if z_cmp == Ordering::Equal {
                a.1.cmp(&b.1)
            } else {
                z_cmp
            }
        });
        positive_z.sort_by(|a, b| {
            let z_cmp = a.0.z_index.cmp(&b.0.z_index);
            if z_cmp == Ordering::Equal {
                a.1.cmp(&b.1)
            } else {
                z_cmp
            }
        });

        // Render in correct order:
        // 1. Negative z-index positioned descendants
        for (child, _) in negative_z {
            self.render_stacking_context(child, z_index, layer);
        }

        // 2. Normal flow block children
        for (child, _) in &normal_flow {
            self.render_stacking_context(child, z_index, layer);
        }

        // 3. Floats and positive/zero z-index positioned descendants
        for (child, _) in positive_z {
            self.render_stacking_context(child, z_index, layer);
        }

        if creates_context {
            self.commands.push(DisplayCommand::PopStackingContext);
        }
    }

    /// Render a layout box's own content (background, borders, text).
    fn render_box_content(&mut self, layout_box: &LayoutBox) {
        self.render_background(layout_box);
        self.render_borders(layout_box);
        self.render_text(layout_box);
    }

    /// Render a layout box and its children (legacy method).
    #[allow(dead_code)]
    fn render_box(&mut self, layout_box: &LayoutBox) {
        self.render_background(layout_box);
        self.render_borders(layout_box);
        self.render_text(layout_box);

        for child in &layout_box.children {
            self.render_box(child);
        }
    }

    /// Render background.
    fn render_background(&mut self, layout_box: &LayoutBox) {
        let rect = layout_box.dimensions.border_box();

        // Check for gradient first
        if let Some(ref gradient) = layout_box.style.background_gradient {
            // TODO: Get border_radius from style (not yet implemented in ComputedStyle)
            let border_radius = BorderRadius::default();

            match gradient {
                rustkit_css::Gradient::Linear(linear) => {
                    self.commands.push(DisplayCommand::LinearGradient {
                        rect,
                        direction: linear.direction,
                        stops: linear.stops.clone(),
                        repeating: linear.repeating,
                        border_radius,
                    });
                }
                rustkit_css::Gradient::Radial(radial) => {
                    self.commands.push(DisplayCommand::RadialGradient {
                        rect,
                        shape: radial.shape,
                        size: radial.size,
                        center: radial.center,
                        stops: radial.stops.clone(),
                        repeating: radial.repeating,
                        border_radius,
                    });
                }
                rustkit_css::Gradient::Conic(conic) => {
                    self.commands.push(DisplayCommand::ConicGradient {
                        rect,
                        from_angle: conic.from_angle,
                        center: conic.center,
                        stops: conic.stops.clone(),
                        repeating: conic.repeating,
                        border_radius,
                    });
                }
            }
        } else {
            // Fallback to solid color
            let color = layout_box.style.background_color;
            if color.a > 0.0 {
                self.commands.push(DisplayCommand::SolidColor(
                    color,
                    rect,
                ));
            }
        }
    }

    /// Render borders.
    fn render_borders(&mut self, layout_box: &LayoutBox) {
        let d = &layout_box.dimensions;
        let s = &layout_box.style;

        // Render each border side separately for correct colors
        // Top border
        if d.border.top > 0.0 {
            let rect = Rect::new(
                d.border_box().x,
                d.border_box().y,
                d.border_box().width,
                d.border.top,
            );
            self.commands
                .push(DisplayCommand::SolidColor(s.border_top_color, rect));
        }

        // Right border
        if d.border.right > 0.0 {
            let rect = Rect::new(
                d.border_box().right() - d.border.right,
                d.border_box().y,
                d.border.right,
                d.border_box().height,
            );
            self.commands
                .push(DisplayCommand::SolidColor(s.border_right_color, rect));
        }

        // Bottom border
        if d.border.bottom > 0.0 {
            let rect = Rect::new(
                d.border_box().x,
                d.border_box().bottom() - d.border.bottom,
                d.border_box().width,
                d.border.bottom,
            );
            self.commands
                .push(DisplayCommand::SolidColor(s.border_bottom_color, rect));
        }

        // Left border
        if d.border.left > 0.0 {
            let rect = Rect::new(
                d.border_box().x,
                d.border_box().y,
                d.border.left,
                d.border_box().height,
            );
            self.commands
                .push(DisplayCommand::SolidColor(s.border_left_color, rect));
        }
    }

    /// Render text with decorations.
    fn render_text(&mut self, layout_box: &LayoutBox) {
        if let BoxType::Text(ref text) = layout_box.box_type {
            let style = &layout_box.style;
            let font_size = match style.font_size {
                Length::Px(px) => px,
                _ => 16.0,
            };

            let x = layout_box.dimensions.content.x;
            let y = layout_box.dimensions.content.y;
            let text_width = layout_box.dimensions.content.width;

            // Draw text
            self.commands.push(DisplayCommand::Text {
                text: text.clone(),
                x,
                y,
                color: style.color,
                font_size,
                font_family: style.font_family.clone(),
                font_weight: style.font_weight.0,
                font_style: match style.font_style {
                    rustkit_css::FontStyle::Normal => 0,
                    rustkit_css::FontStyle::Italic => 1,
                    rustkit_css::FontStyle::Oblique => 2,
                },
            });

            // Draw text decorations
            let decoration_line = style.text_decoration_line;
            if decoration_line.underline || decoration_line.overline || decoration_line.line_through
            {
                let decoration_color = style.text_decoration_color.unwrap_or(style.color);
                let decoration_style = match style.text_decoration_style {
                    rustkit_css::TextDecorationStyle::Solid => TextDecorationStyleValue::Solid,
                    rustkit_css::TextDecorationStyle::Double => TextDecorationStyleValue::Double,
                    rustkit_css::TextDecorationStyle::Dotted => TextDecorationStyleValue::Dotted,
                    rustkit_css::TextDecorationStyle::Dashed => TextDecorationStyleValue::Dashed,
                    rustkit_css::TextDecorationStyle::Wavy => TextDecorationStyleValue::Wavy,
                };

                // Calculate thickness
                let thickness = match style.text_decoration_thickness {
                    Length::Px(px) => px,
                    Length::Em(em) => em * font_size,
                    _ => font_size / 14.0, // Auto thickness
                };

                let ascent = font_size * 0.8;
                let descent = font_size * 0.2;

                // Underline
                if decoration_line.underline {
                    self.commands.push(DisplayCommand::TextDecoration {
                        x,
                        y: y + ascent + descent * 0.3,
                        width: text_width,
                        thickness,
                        color: decoration_color,
                        style: decoration_style,
                    });
                }

                // Overline
                if decoration_line.overline {
                    self.commands.push(DisplayCommand::TextDecoration {
                        x,
                        y: y - thickness,
                        width: text_width,
                        thickness,
                        color: decoration_color,
                        style: decoration_style,
                    });
                }

                // Line-through (strikethrough)
                if decoration_line.line_through {
                    self.commands.push(DisplayCommand::TextDecoration {
                        x,
                        y: y + ascent * 0.35,
                        width: text_width,
                        thickness,
                        color: decoration_color,
                        style: decoration_style,
                    });
                }
            }
        }
    }
}

/// Measure text using the text shaper.
///
/// This provides accurate text measurement using DirectWrite on Windows.
pub fn measure_text_advanced(
    text: &str,
    font_family: &str,
    font_size: f32,
    font_weight: rustkit_css::FontWeight,
    font_style: rustkit_css::FontStyle,
) -> TextMetrics {
    let shaper = TextShaper::new();
    let chain = FontFamilyChain::from_css_value(font_family);

    match shaper.shape(
        text,
        &chain,
        font_weight,
        font_style,
        rustkit_css::FontStretch::Normal,
        font_size,
    ) {
        Ok(run) => run.metrics,
        Err(_) => {
            // Fallback to simple measurement
            measure_text_simple(text, font_size)
        }
    }
}

/// Simple text measurement (fallback when shaping is unavailable).
pub fn measure_text_simple(text: &str, font_size: f32) -> TextMetrics {
    // Approximate metrics based on font size
    // Typical Latin font has ~0.5em average character width
    let avg_char_width = font_size * 0.5;
    let width = text.chars().count() as f32 * avg_char_width;

    TextMetrics {
        width,
        ..TextMetrics::with_font_size(font_size)
    }
}

/// Measure text (simplified - uses average character width approximation).
///
/// For more accurate measurement, use `measure_text_advanced`.
#[deprecated(
    since = "0.1.0",
    note = "Use measure_text_advanced for accurate measurement"
)]
pub fn measure_text(text: &str, _font_family: &str, font_size: f32) -> text::TextMetrics {
    measure_text_simple(text, font_size)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rect() {
        let r = Rect::new(10.0, 20.0, 100.0, 50.0);
        assert_eq!(r.right(), 110.0);
        assert_eq!(r.bottom(), 70.0);
        assert!(r.contains(50.0, 30.0));
        assert!(!r.contains(0.0, 0.0));
    }

    #[test]
    fn test_dimensions_boxes() {
        let d = Dimensions {
            content: Rect::new(20.0, 20.0, 100.0, 50.0),
            padding: EdgeSizes {
                top: 5.0,
                right: 5.0,
                bottom: 5.0,
                left: 5.0,
            },
            border: EdgeSizes {
                top: 1.0,
                right: 1.0,
                bottom: 1.0,
                left: 1.0,
            },
            margin: EdgeSizes {
                top: 10.0,
                right: 10.0,
                bottom: 10.0,
                left: 10.0,
            },
        };

        let pb = d.padding_box();
        assert_eq!(pb.width, 110.0);
        assert_eq!(pb.height, 60.0);

        let bb = d.border_box();
        assert_eq!(bb.width, 112.0);
        assert_eq!(bb.height, 62.0);

        let mb = d.margin_box();
        assert_eq!(mb.width, 132.0);
        assert_eq!(mb.height, 82.0);
    }

    #[test]
    fn test_layout_box_creation() {
        let style = ComputedStyle::new();
        let layout_box = LayoutBox::new(BoxType::Block, style);
        assert!(matches!(layout_box.box_type, BoxType::Block));
        assert_eq!(layout_box.position, Position::Static);
        assert_eq!(layout_box.float, Float::None);
    }

    #[test]
    fn test_layout_box_with_position() {
        let style = ComputedStyle::new();
        let layout_box = LayoutBox::with_position(BoxType::Block, style, Position::Relative);
        assert_eq!(layout_box.position, Position::Relative);
        assert!(layout_box.stacking_context.is_some());
    }

    #[test]
    fn test_layout_box_with_float() {
        let style = ComputedStyle::new();
        let layout_box = LayoutBox::with_float(BoxType::Block, style, Float::Left);
        assert_eq!(layout_box.float, Float::Left);
    }

    #[test]
    fn test_margin_collapse_positive() {
        let mut ctx = MarginCollapseContext::new();
        ctx.add_margin(10.0);
        ctx.add_margin(20.0);
        assert_eq!(ctx.resolve(), 20.0); // Max of positive margins
    }

    #[test]
    fn test_margin_collapse_negative() {
        let mut ctx = MarginCollapseContext::new();
        ctx.add_margin(-10.0);
        ctx.add_margin(-20.0);
        assert_eq!(ctx.resolve(), -20.0); // Min of negative margins
    }

    #[test]
    fn test_margin_collapse_mixed() {
        let mut ctx = MarginCollapseContext::new();
        ctx.add_margin(20.0);
        ctx.add_margin(-10.0);
        assert_eq!(ctx.resolve(), 10.0); // Sum of max positive and min negative
    }

    #[test]
    fn test_float_context() {
        let mut ctx = FloatContext::new();

        // Add a left float
        ctx.add_left(Rect::new(0.0, 0.0, 100.0, 50.0));

        // Check available width at y=25 (within float)
        let (left, right) = ctx.available_width(25.0, 500.0);
        assert_eq!(left, 100.0); // Left edge is after the float
        assert_eq!(right, 500.0); // Right edge is container width

        // Check available width at y=60 (below float)
        let (left, right) = ctx.available_width(60.0, 500.0);
        assert_eq!(left, 0.0); // No float at this y
        assert_eq!(right, 500.0);
    }

    #[test]
    fn test_float_clear() {
        let mut ctx = FloatContext::new();

        ctx.add_left(Rect::new(0.0, 0.0, 100.0, 50.0));
        ctx.add_right(Rect::new(400.0, 0.0, 100.0, 80.0));

        assert_eq!(ctx.clear(Clear::Left), 50.0);
        assert_eq!(ctx.clear(Clear::Right), 80.0);
        assert_eq!(ctx.clear(Clear::Both), 80.0);
        assert_eq!(ctx.clear(Clear::None), 0.0);
    }

    #[test]
    fn test_position_offsets() {
        let style = ComputedStyle::new();
        let mut layout_box = LayoutBox::with_position(BoxType::Block, style, Position::Relative);
        layout_box.set_offsets(Some(10.0), None, None, Some(20.0));

        assert_eq!(layout_box.offsets.top, Some(10.0));
        assert_eq!(layout_box.offsets.left, Some(20.0));
        assert_eq!(layout_box.offsets.right, None);
        assert_eq!(layout_box.offsets.bottom, None);
    }

    #[test]
    fn test_z_index_stacking() {
        let style = ComputedStyle::new();
        let mut layout_box = LayoutBox::with_position(BoxType::Block, style, Position::Absolute);
        layout_box.set_z_index(5);

        assert_eq!(layout_box.z_index, 5);
        let ctx = layout_box.stacking_context.as_ref().unwrap();
        assert!(ctx.creates_context);
        assert_eq!(ctx.z_index, 5);
    }

    #[test]
    fn test_display_list_build() {
        let mut style = ComputedStyle::new();
        style.background_color = Color::from_rgb(255, 255, 255);

        let layout_box = LayoutBox::new(BoxType::Block, style);
        let display_list = DisplayList::build(&layout_box);

        assert!(!display_list.commands.is_empty());
    }

    #[test]
    fn test_display_list_with_positioned() {
        let style = ComputedStyle::new();
        let mut parent = LayoutBox::new(BoxType::Block, style.clone());

        let mut child = LayoutBox::with_position(BoxType::Block, style, Position::Absolute);
        child.set_z_index(-1);
        parent.children.push(child);

        let display_list = DisplayList::build(&parent);

        // Should have commands for both parent and child
        assert!(!display_list.commands.is_empty());
    }

    #[test]
    fn test_paint_order() {
        let style = ComputedStyle::new();
        let mut parent = LayoutBox::new(BoxType::Block, style.clone());

        // Add normal flow child
        let normal = LayoutBox::new(BoxType::Block, style.clone());
        parent.children.push(normal);

        // Add positioned child with positive z-index
        let mut positive_z =
            LayoutBox::with_position(BoxType::Block, style.clone(), Position::Absolute);
        positive_z.set_z_index(1);
        parent.children.push(positive_z);

        // Add positioned child with negative z-index
        let mut negative_z = LayoutBox::with_position(BoxType::Block, style, Position::Absolute);
        negative_z.set_z_index(-1);
        parent.children.push(negative_z);

        let paint_order = parent.get_paint_order();

        // Order should be: negative z-index, normal flow, positive z-index
        assert_eq!(paint_order.len(), 3);
        assert_eq!(paint_order[0].z_index, -1);
        assert_eq!(paint_order[1].position, Position::Static);
        assert_eq!(paint_order[2].z_index, 1);
    }
}
