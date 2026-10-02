//! # RustKit Canvas
//!
//! HTML5 Canvas 2D API for the RustKit browser engine.
//!
//! ## Features
//!
//! - **CanvasRenderingContext2D**: Full 2D drawing context
//! - **Paths**: Path building with lines, arcs, beziers
//! - **Drawing**: Fill, stroke, images, text
//! - **Transforms**: Translate, rotate, scale, matrix
//! - **State**: Save/restore state stack
//! - **Pixel Manipulation**: ImageData get/put
//!
//! ## Architecture
//!
//! ```text
//! Canvas Element
//!    └── CanvasRenderingContext2D
//!           ├── State Stack
//!           ├── Current Path
//!           ├── Pixel Buffer
//!           └── Transform Matrix
//! ```

use rustkit_css::Color;
use std::collections::VecDeque;
use std::f32::consts::PI;
use thiserror::Error;

// ==================== Errors ====================

/// Errors that can occur in canvas operations.
#[derive(Error, Debug)]
pub enum CanvasError {
    #[error("Invalid state: {0}")]
    InvalidState(String),

    #[error("Invalid argument: {0}")]
    InvalidArgument(String),

    #[error("Out of bounds: {0}")]
    OutOfBounds(String),
}

// ==================== Color & Style ====================

/// A canvas fill or stroke style.
#[derive(Debug, Clone)]
pub enum CanvasStyle {
    /// Solid color.
    Color(Color),
    /// Linear gradient.
    LinearGradient(LinearGradient),
    /// Radial gradient.
    RadialGradient(RadialGradient),
    /// Pattern (image-based).
    Pattern(CanvasPattern),
}

impl Default for CanvasStyle {
    fn default() -> Self {
        CanvasStyle::Color(Color::BLACK)
    }
}

impl CanvasStyle {
    /// Parse from CSS color string.
    pub fn from_color_string(s: &str) -> Self {
        if let Some(color) = parse_canvas_color(s) {
            CanvasStyle::Color(color)
        } else {
            CanvasStyle::Color(Color::BLACK)
        }
    }
}

/// A color stop in a gradient.
#[derive(Debug, Clone)]
pub struct ColorStop {
    pub offset: f32,
    pub color: Color,
}

/// Linear gradient.
#[derive(Debug, Clone)]
pub struct LinearGradient {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
    pub stops: Vec<ColorStop>,
}

impl LinearGradient {
    /// Create a new linear gradient.
    pub fn new(x0: f32, y0: f32, x1: f32, y1: f32) -> Self {
        Self {
            x0, y0, x1, y1,
            stops: Vec::new(),
        }
    }

    /// Add a color stop.
    pub fn add_color_stop(&mut self, offset: f32, color: Color) {
        self.stops.push(ColorStop {
            offset: offset.clamp(0.0, 1.0),
            color,
        });
        self.stops.sort_by(|a, b| a.offset.partial_cmp(&b.offset).unwrap());
    }

    /// Sample the gradient at a given position.
    pub fn sample(&self, t: f32) -> Color {
        if self.stops.is_empty() {
            return Color::TRANSPARENT;
        }
        if self.stops.len() == 1 {
            return self.stops[0].color;
        }

        let t = t.clamp(0.0, 1.0);

        // Find surrounding stops
        for i in 0..self.stops.len() - 1 {
            if t >= self.stops[i].offset && t <= self.stops[i + 1].offset {
                let range = self.stops[i + 1].offset - self.stops[i].offset;
                let local_t = if range > 0.0 {
                    (t - self.stops[i].offset) / range
                } else {
                    0.0
                };
                return interpolate_color(&self.stops[i].color, &self.stops[i + 1].color, local_t);
            }
        }

        self.stops.last().map(|s| s.color).unwrap_or(Color::TRANSPARENT)
    }
}

/// Radial gradient.
#[derive(Debug, Clone)]
pub struct RadialGradient {
    pub x0: f32,
    pub y0: f32,
    pub r0: f32,
    pub x1: f32,
    pub y1: f32,
    pub r1: f32,
    pub stops: Vec<ColorStop>,
}

impl RadialGradient {
    /// Create a new radial gradient.
    pub fn new(x0: f32, y0: f32, r0: f32, x1: f32, y1: f32, r1: f32) -> Self {
        Self {
            x0, y0, r0, x1, y1, r1,
            stops: Vec::new(),
        }
    }

    /// Add a color stop.
    pub fn add_color_stop(&mut self, offset: f32, color: Color) {
        self.stops.push(ColorStop {
            offset: offset.clamp(0.0, 1.0),
            color,
        });
        self.stops.sort_by(|a, b| a.offset.partial_cmp(&b.offset).unwrap());
    }
}

/// Canvas pattern.
#[derive(Debug, Clone)]
pub struct CanvasPattern {
    pub image_data: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub repetition: PatternRepetition,
}

/// Pattern repetition mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PatternRepetition {
    #[default]
    Repeat,
    RepeatX,
    RepeatY,
    NoRepeat,
}

// ==================== Transform ====================

/// 2D affine transform matrix.
#[derive(Debug, Clone, Copy)]
pub struct Transform2D {
    pub a: f32,
    pub b: f32,
    pub c: f32,
    pub d: f32,
    pub e: f32,
    pub f: f32,
}

impl Default for Transform2D {
    fn default() -> Self {
        Self::identity()
    }
}

impl Transform2D {
    /// Create identity transform.
    pub fn identity() -> Self {
        Self {
            a: 1.0, b: 0.0,
            c: 0.0, d: 1.0,
            e: 0.0, f: 0.0,
        }
    }

    /// Multiply two transforms.
    pub fn multiply(&self, other: &Transform2D) -> Self {
        Transform2D {
            a: self.a * other.a + self.c * other.b,
            b: self.b * other.a + self.d * other.b,
            c: self.a * other.c + self.c * other.d,
            d: self.b * other.c + self.d * other.d,
            e: self.a * other.e + self.c * other.f + self.e,
            f: self.b * other.e + self.d * other.f + self.f,
        }
    }

    /// Translate.
    pub fn translate(&self, tx: f32, ty: f32) -> Self {
        self.multiply(&Transform2D {
            a: 1.0, b: 0.0,
            c: 0.0, d: 1.0,
            e: tx, f: ty,
        })
    }

    /// Scale.
    pub fn scale(&self, sx: f32, sy: f32) -> Self {
        self.multiply(&Transform2D {
            a: sx, b: 0.0,
            c: 0.0, d: sy,
            e: 0.0, f: 0.0,
        })
    }

    /// Rotate (radians).
    pub fn rotate(&self, angle: f32) -> Self {
        let cos = angle.cos();
        let sin = angle.sin();
        self.multiply(&Transform2D {
            a: cos, b: sin,
            c: -sin, d: cos,
            e: 0.0, f: 0.0,
        })
    }

    /// Apply transform to a point.
    pub fn apply(&self, x: f32, y: f32) -> (f32, f32) {
        (
            self.a * x + self.c * y + self.e,
            self.b * x + self.d * y + self.f,
        )
    }

    /// Get inverse transform.
    pub fn inverse(&self) -> Option<Self> {
        let det = self.a * self.d - self.b * self.c;
        if det.abs() < 1e-10 {
            return None;
        }
        let inv_det = 1.0 / det;
        Some(Transform2D {
            a: self.d * inv_det,
            b: -self.b * inv_det,
            c: -self.c * inv_det,
            d: self.a * inv_det,
            e: (self.c * self.f - self.d * self.e) * inv_det,
            f: (self.b * self.e - self.a * self.f) * inv_det,
        })
    }
}

// ==================== Path ====================

/// Path command.
#[derive(Debug, Clone)]
pub enum PathCommand {
    MoveTo(f32, f32),
    LineTo(f32, f32),
    QuadraticCurveTo(f32, f32, f32, f32),
    BezierCurveTo(f32, f32, f32, f32, f32, f32),
    ArcTo(f32, f32, f32, f32, f32),
    Arc(f32, f32, f32, f32, f32, bool),
    Ellipse(f32, f32, f32, f32, f32, f32, f32, bool),
    Rect(f32, f32, f32, f32),
    ClosePath,
}

/// A canvas path.
#[derive(Debug, Clone, Default)]
pub struct Path2D {
    commands: Vec<PathCommand>,
    current_x: f32,
    current_y: f32,
    start_x: f32,
    start_y: f32,
}

impl Path2D {
    /// Create a new empty path.
    pub fn new() -> Self {
        Self::default()
    }

    /// Begin a new sub-path.
    pub fn move_to(&mut self, x: f32, y: f32) {
        self.commands.push(PathCommand::MoveTo(x, y));
        self.current_x = x;
        self.current_y = y;
        self.start_x = x;
        self.start_y = y;
    }

    /// Add a line to the path.
    pub fn line_to(&mut self, x: f32, y: f32) {
        self.commands.push(PathCommand::LineTo(x, y));
        self.current_x = x;
        self.current_y = y;
    }

    /// Add a quadratic curve.
    pub fn quadratic_curve_to(&mut self, cpx: f32, cpy: f32, x: f32, y: f32) {
        self.commands.push(PathCommand::QuadraticCurveTo(cpx, cpy, x, y));
        self.current_x = x;
        self.current_y = y;
    }

    /// Add a bezier curve.
    pub fn bezier_curve_to(&mut self, cp1x: f32, cp1y: f32, cp2x: f32, cp2y: f32, x: f32, y: f32) {
        self.commands.push(PathCommand::BezierCurveTo(cp1x, cp1y, cp2x, cp2y, x, y));
        self.current_x = x;
        self.current_y = y;
    }

    /// Add an arc using tangent points.
    pub fn arc_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, radius: f32) {
        self.commands.push(PathCommand::ArcTo(x1, y1, x2, y2, radius));
        // Current position updated based on arc calculation
    }

    /// Add an arc.
    pub fn arc(&mut self, x: f32, y: f32, radius: f32, start_angle: f32, end_angle: f32, ccw: bool) {
        self.commands.push(PathCommand::Arc(x, y, radius, start_angle, end_angle, ccw));
        self.current_x = x + radius * end_angle.cos();
        self.current_y = y + radius * end_angle.sin();
    }

    /// Add an ellipse.
    pub fn ellipse(&mut self, x: f32, y: f32, rx: f32, ry: f32, rotation: f32, start_angle: f32, end_angle: f32, ccw: bool) {
        self.commands.push(PathCommand::Ellipse(x, y, rx, ry, rotation, start_angle, end_angle, ccw));
    }

    /// Add a rectangle.
    pub fn rect(&mut self, x: f32, y: f32, w: f32, h: f32) {
        self.commands.push(PathCommand::Rect(x, y, w, h));
    }

    /// Close the current sub-path.
    pub fn close_path(&mut self) {
        self.commands.push(PathCommand::ClosePath);
        self.current_x = self.start_x;
        self.current_y = self.start_y;
    }

    /// Get commands.
    pub fn commands(&self) -> &[PathCommand] {
        &self.commands
    }

    /// Convert to line segments for rendering.
    pub fn to_line_segments(&self) -> Vec<Vec<(f32, f32)>> {
        let mut segments = Vec::new();
        let mut current_segment = Vec::new();
        let mut current_x = 0.0_f32;
        let mut current_y = 0.0_f32;
        let mut start_x = 0.0_f32;
        let mut start_y = 0.0_f32;

        for cmd in &self.commands {
            match cmd {
                PathCommand::MoveTo(x, y) => {
                    if !current_segment.is_empty() {
                        segments.push(std::mem::take(&mut current_segment));
                    }
                    current_x = *x;
                    current_y = *y;
                    start_x = *x;
                    start_y = *y;
                    current_segment.push((current_x, current_y));
                }
                PathCommand::LineTo(x, y) => {
                    current_x = *x;
                    current_y = *y;
                    current_segment.push((current_x, current_y));
                }
                PathCommand::QuadraticCurveTo(cpx, cpy, x, y) => {
                    let points = quad_bezier_points((current_x, current_y), (*cpx, *cpy), (*x, *y), 20);
                    current_segment.extend(points);
                    current_x = *x;
                    current_y = *y;
                }
                PathCommand::BezierCurveTo(cp1x, cp1y, cp2x, cp2y, x, y) => {
                    let points = cubic_bezier_points(
                        (current_x, current_y),
                        (*cp1x, *cp1y),
                        (*cp2x, *cp2y),
                        (*x, *y),
                        20
                    );
                    current_segment.extend(points);
                    current_x = *x;
                    current_y = *y;
                }
                PathCommand::Arc(cx, cy, r, start, end, ccw) => {
                    let points = arc_points(*cx, *cy, *r, *start, *end, *ccw, 32);
                    current_segment.extend(points);
                    current_x = cx + r * end.cos();
                    current_y = cy + r * end.sin();
                }
                PathCommand::Rect(x, y, w, h) => {
                    if !current_segment.is_empty() {
                        segments.push(std::mem::take(&mut current_segment));
                    }
                    current_segment.push((*x, *y));
                    current_segment.push((x + w, *y));
                    current_segment.push((x + w, y + h));
                    current_segment.push((*x, y + h));
                    current_segment.push((*x, *y));
                    segments.push(std::mem::take(&mut current_segment));
                    current_x = *x;
                    current_y = *y;
                    start_x = *x;
                    start_y = *y;
                }
                PathCommand::ClosePath => {
                    if current_x != start_x || current_y != start_y {
                        current_segment.push((start_x, start_y));
                    }
                    current_x = start_x;
                    current_y = start_y;
                    if !current_segment.is_empty() {
                        segments.push(std::mem::take(&mut current_segment));
                    }
                }
                _ => {}
            }
        }

        if !current_segment.is_empty() {
            segments.push(current_segment);
        }

        segments
    }
}

// ==================== Canvas State ====================

/// Line cap style.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LineCap {
    #[default]
    Butt,
    Round,
    Square,
}

/// Line join style.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LineJoin {
    #[default]
    Miter,
    Round,
    Bevel,
}

/// Text alignment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TextAlign {
    #[default]
    Start,
    End,
    Left,
    Right,
    Center,
}

/// Text baseline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TextBaseline {
    Top,
    Hanging,
    Middle,
    #[default]
    Alphabetic,
    Ideographic,
    Bottom,
}

/// Compositing operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CompositeOperation {
    #[default]
    SourceOver,
    SourceIn,
    SourceOut,
    SourceAtop,
    DestinationOver,
    DestinationIn,
    DestinationOut,
    DestinationAtop,
    Lighter,
    Copy,
    Xor,
    Multiply,
    Screen,
    Overlay,
    Darken,
    Lighten,
}

/// Canvas drawing state.
#[derive(Debug, Clone)]
pub struct CanvasState {
    pub transform: Transform2D,
    pub fill_style: CanvasStyle,
    pub stroke_style: CanvasStyle,
    pub line_width: f32,
    pub line_cap: LineCap,
    pub line_join: LineJoin,
    pub miter_limit: f32,
    pub line_dash: Vec<f32>,
    pub line_dash_offset: f32,
    pub font: String,
    pub text_align: TextAlign,
    pub text_baseline: TextBaseline,
    pub global_alpha: f32,
    pub global_composite_operation: CompositeOperation,
    pub shadow_blur: f32,
    pub shadow_color: Color,
    pub shadow_offset_x: f32,
    pub shadow_offset_y: f32,
    pub clip_path: Option<Path2D>,
}

impl Default for CanvasState {
    fn default() -> Self {
        Self {
            transform: Transform2D::identity(),
            fill_style: CanvasStyle::Color(Color::BLACK),
            stroke_style: CanvasStyle::Color(Color::BLACK),
            line_width: 1.0,
            line_cap: LineCap::Butt,
            line_join: LineJoin::Miter,
            miter_limit: 10.0,
            line_dash: Vec::new(),
            line_dash_offset: 0.0,
            font: "10px sans-serif".to_string(),
            text_align: TextAlign::Start,
            text_baseline: TextBaseline::Alphabetic,
            global_alpha: 1.0,
            global_composite_operation: CompositeOperation::SourceOver,
            shadow_blur: 0.0,
            shadow_color: Color::TRANSPARENT,
            shadow_offset_x: 0.0,
            shadow_offset_y: 0.0,
            clip_path: None,
        }
    }
}

// ==================== ImageData ====================

/// Canvas image data for pixel manipulation.
#[derive(Debug, Clone)]
pub struct ImageData {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>, // RGBA format
}

impl ImageData {
    /// Create new image data.
    pub fn new(width: u32, height: u32) -> Self {
        let len = (width * height * 4) as usize;
        Self {
            width,
            height,
            data: vec![0; len],
        }
    }

    /// Create from existing data.
    pub fn from_data(width: u32, height: u32, data: Vec<u8>) -> Result<Self, CanvasError> {
        let expected_len = (width * height * 4) as usize;
        if data.len() != expected_len {
            return Err(CanvasError::InvalidArgument(format!(
                "Data length {} doesn't match expected {}",
                data.len(), expected_len
            )));
        }
        Ok(Self { width, height, data })
    }

    /// Get pixel at (x, y).
    pub fn get_pixel(&self, x: u32, y: u32) -> Option<(u8, u8, u8, u8)> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let idx = ((y * self.width + x) * 4) as usize;
        Some((self.data[idx], self.data[idx + 1], self.data[idx + 2], self.data[idx + 3]))
    }

    /// Set pixel at (x, y).
    pub fn set_pixel(&mut self, x: u32, y: u32, r: u8, g: u8, b: u8, a: u8) {
        if x < self.width && y < self.height {
            let idx = ((y * self.width + x) * 4) as usize;
            self.data[idx] = r;
            self.data[idx + 1] = g;
            self.data[idx + 2] = b;
            self.data[idx + 3] = a;
        }
    }
}

// ==================== Text Metrics ====================

/// Text measurement result.
#[derive(Debug, Clone, Default)]
pub struct TextMetrics {
    pub width: f32,
    pub actual_bounding_box_left: f32,
    pub actual_bounding_box_right: f32,
    pub font_bounding_box_ascent: f32,
    pub font_bounding_box_descent: f32,
    pub actual_bounding_box_ascent: f32,
    pub actual_bounding_box_descent: f32,
    pub em_height_ascent: f32,
    pub em_height_descent: f32,
    pub hanging_baseline: f32,
    pub alphabetic_baseline: f32,
    pub ideographic_baseline: f32,
}

// ==================== Drawing Commands ====================

/// A recorded drawing command.
#[derive(Debug, Clone)]
pub enum DrawCommand {
    FillRect { x: f32, y: f32, w: f32, h: f32, style: CanvasStyle, transform: Transform2D },
    StrokeRect { x: f32, y: f32, w: f32, h: f32, style: CanvasStyle, line_width: f32, transform: Transform2D },
    ClearRect { x: f32, y: f32, w: f32, h: f32, transform: Transform2D },
    FillPath { segments: Vec<Vec<(f32, f32)>>, style: CanvasStyle, transform: Transform2D },
    StrokePath { segments: Vec<Vec<(f32, f32)>>, style: CanvasStyle, line_width: f32, transform: Transform2D },
    FillText { text: String, x: f32, y: f32, style: CanvasStyle, font: String, transform: Transform2D },
    StrokeText { text: String, x: f32, y: f32, style: CanvasStyle, font: String, line_width: f32, transform: Transform2D },
    DrawImage { image_id: String, sx: f32, sy: f32, sw: f32, sh: f32, dx: f32, dy: f32, dw: f32, dh: f32, transform: Transform2D },
    PutImageData { data: ImageData, x: i32, y: i32 },
}

// ==================== Canvas Rendering Context ====================

/// CanvasRenderingContext2D implementation.
#[derive(Debug)]
pub struct CanvasRenderingContext2D {
    /// Canvas width.
    pub width: u32,
    /// Canvas height.
    pub height: u32,
    /// Current state.
    pub state: CanvasState,
    /// State stack.
    state_stack: VecDeque<CanvasState>,
    /// Current path.
    path: Path2D,
    /// Recorded draw commands.
    commands: Vec<DrawCommand>,
    /// Pixel buffer (optional, for getImageData).
    pixel_buffer: Option<ImageData>,
}

impl CanvasRenderingContext2D {
    /// Create a new context.
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            state: CanvasState::default(),
            state_stack: VecDeque::new(),
            path: Path2D::new(),
            commands: Vec::new(),
            pixel_buffer: None,
        }
    }

    // ==================== State Management ====================

    /// Save current state.
    pub fn save(&mut self) {
        self.state_stack.push_back(self.state.clone());
    }

    /// Restore previous state.
    pub fn restore(&mut self) {
        if let Some(state) = self.state_stack.pop_back() {
            self.state = state;
        }
    }

    /// Reset the context.
    pub fn reset(&mut self) {
        self.state = CanvasState::default();
        self.state_stack.clear();
        self.path = Path2D::new();
        self.commands.clear();
    }

    // ==================== Transform ====================

    /// Scale the transform.
    pub fn scale(&mut self, x: f32, y: f32) {
        self.state.transform = self.state.transform.scale(x, y);
    }

    /// Rotate the transform.
    pub fn rotate(&mut self, angle: f32) {
        self.state.transform = self.state.transform.rotate(angle);
    }

    /// Translate the transform.
    pub fn translate(&mut self, x: f32, y: f32) {
        self.state.transform = self.state.transform.translate(x, y);
    }

    /// Apply a transform.
    pub fn transform(&mut self, a: f32, b: f32, c: f32, d: f32, e: f32, f: f32) {
        let new_transform = Transform2D { a, b, c, d, e, f };
        self.state.transform = self.state.transform.multiply(&new_transform);
    }

    /// Set the transform (replace current).
    pub fn set_transform(&mut self, a: f32, b: f32, c: f32, d: f32, e: f32, f: f32) {
        self.state.transform = Transform2D { a, b, c, d, e, f };
    }

    /// Reset to identity transform.
    pub fn reset_transform(&mut self) {
        self.state.transform = Transform2D::identity();
    }

    /// Get current transform.
    pub fn get_transform(&self) -> Transform2D {
        self.state.transform
    }

    // ==================== Style Properties ====================

    /// Set fill style from color string.
    pub fn set_fill_style_color(&mut self, color: &str) {
        self.state.fill_style = CanvasStyle::from_color_string(color);
    }

    /// Set fill style.
    pub fn set_fill_style(&mut self, style: CanvasStyle) {
        self.state.fill_style = style;
    }

    /// Set stroke style from color string.
    pub fn set_stroke_style_color(&mut self, color: &str) {
        self.state.stroke_style = CanvasStyle::from_color_string(color);
    }

    /// Set stroke style.
    pub fn set_stroke_style(&mut self, style: CanvasStyle) {
        self.state.stroke_style = style;
    }

    /// Set line width.
    pub fn set_line_width(&mut self, width: f32) {
        self.state.line_width = width.max(0.0);
    }

    /// Set line cap.
    pub fn set_line_cap(&mut self, cap: LineCap) {
        self.state.line_cap = cap;
    }

    /// Set line join.
    pub fn set_line_join(&mut self, join: LineJoin) {
        self.state.line_join = join;
    }

    /// Set miter limit.
    pub fn set_miter_limit(&mut self, limit: f32) {
        self.state.miter_limit = limit.max(0.0);
    }

    /// Set line dash.
    pub fn set_line_dash(&mut self, segments: Vec<f32>) {
        self.state.line_dash = segments;
    }

    /// Get line dash.
    pub fn get_line_dash(&self) -> &[f32] {
        &self.state.line_dash
    }

    /// Set line dash offset.
    pub fn set_line_dash_offset(&mut self, offset: f32) {
        self.state.line_dash_offset = offset;
    }

    /// Set font.
    pub fn set_font(&mut self, font: &str) {
        self.state.font = font.to_string();
    }

    /// Set text align.
    pub fn set_text_align(&mut self, align: TextAlign) {
        self.state.text_align = align;
    }

    /// Set text baseline.
    pub fn set_text_baseline(&mut self, baseline: TextBaseline) {
        self.state.text_baseline = baseline;
    }

    /// Set global alpha.
    pub fn set_global_alpha(&mut self, alpha: f32) {
        self.state.global_alpha = alpha.clamp(0.0, 1.0);
    }

    /// Set global composite operation.
    pub fn set_global_composite_operation(&mut self, op: CompositeOperation) {
        self.state.global_composite_operation = op;
    }

    /// Set shadow blur.
    pub fn set_shadow_blur(&mut self, blur: f32) {
        self.state.shadow_blur = blur.max(0.0);
    }

    /// Set shadow color.
    pub fn set_shadow_color(&mut self, color: &str) {
        if let Some(c) = parse_canvas_color(color) {
            self.state.shadow_color = c;
        }
    }

    /// Set shadow offset X.
    pub fn set_shadow_offset_x(&mut self, x: f32) {
        self.state.shadow_offset_x = x;
    }

    /// Set shadow offset Y.
    pub fn set_shadow_offset_y(&mut self, y: f32) {
        self.state.shadow_offset_y = y;
    }

    // ==================== Gradients & Patterns ====================

    /// Create a linear gradient.
    pub fn create_linear_gradient(&self, x0: f32, y0: f32, x1: f32, y1: f32) -> LinearGradient {
        LinearGradient::new(x0, y0, x1, y1)
    }

    /// Create a radial gradient.
    pub fn create_radial_gradient(&self, x0: f32, y0: f32, r0: f32, x1: f32, y1: f32, r1: f32) -> RadialGradient {
        RadialGradient::new(x0, y0, r0, x1, y1, r1)
    }

    // ==================== Path Methods ====================

    /// Begin a new path.
    pub fn begin_path(&mut self) {
        self.path = Path2D::new();
    }

    /// Move to.
    pub fn move_to(&mut self, x: f32, y: f32) {
        self.path.move_to(x, y);
    }

    /// Line to.
    pub fn line_to(&mut self, x: f32, y: f32) {
        self.path.line_to(x, y);
    }

    /// Quadratic curve to.
    pub fn quadratic_curve_to(&mut self, cpx: f32, cpy: f32, x: f32, y: f32) {
        self.path.quadratic_curve_to(cpx, cpy, x, y);
    }

    /// Bezier curve to.
    pub fn bezier_curve_to(&mut self, cp1x: f32, cp1y: f32, cp2x: f32, cp2y: f32, x: f32, y: f32) {
        self.path.bezier_curve_to(cp1x, cp1y, cp2x, cp2y, x, y);
    }

    /// Arc to.
    pub fn arc_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, radius: f32) {
        self.path.arc_to(x1, y1, x2, y2, radius);
    }

    /// Arc.
    pub fn arc(&mut self, x: f32, y: f32, radius: f32, start_angle: f32, end_angle: f32, counterclockwise: bool) {
        self.path.arc(x, y, radius, start_angle, end_angle, counterclockwise);
    }

    /// Ellipse.
    pub fn ellipse(&mut self, x: f32, y: f32, rx: f32, ry: f32, rotation: f32, start_angle: f32, end_angle: f32, counterclockwise: bool) {
        self.path.ellipse(x, y, rx, ry, rotation, start_angle, end_angle, counterclockwise);
    }

    /// Rect.
    pub fn rect(&mut self, x: f32, y: f32, w: f32, h: f32) {
        self.path.rect(x, y, w, h);
    }

    /// Close path.
    pub fn close_path(&mut self) {
        self.path.close_path();
    }

    // ==================== Drawing Rects ====================

    /// Fill a rectangle.
    pub fn fill_rect(&mut self, x: f32, y: f32, w: f32, h: f32) {
        self.commands.push(DrawCommand::FillRect {
            x, y, w, h,
            style: self.state.fill_style.clone(),
            transform: self.state.transform,
        });
    }

    /// Stroke a rectangle.
    pub fn stroke_rect(&mut self, x: f32, y: f32, w: f32, h: f32) {
        self.commands.push(DrawCommand::StrokeRect {
            x, y, w, h,
            style: self.state.stroke_style.clone(),
            line_width: self.state.line_width,
            transform: self.state.transform,
        });
    }

    /// Clear a rectangle.
    pub fn clear_rect(&mut self, x: f32, y: f32, w: f32, h: f32) {
        self.commands.push(DrawCommand::ClearRect {
            x, y, w, h,
            transform: self.state.transform,
        });
    }

    // ==================== Drawing Paths ====================

    /// Fill the current path.
    pub fn fill(&mut self) {
        let segments = self.path.to_line_segments();
        self.commands.push(DrawCommand::FillPath {
            segments,
            style: self.state.fill_style.clone(),
            transform: self.state.transform,
        });
    }

    /// Stroke the current path.
    pub fn stroke(&mut self) {
        let segments = self.path.to_line_segments();
        self.commands.push(DrawCommand::StrokePath {
            segments,
            style: self.state.stroke_style.clone(),
            line_width: self.state.line_width,
            transform: self.state.transform,
        });
    }

    /// Clip to current path.
    pub fn clip(&mut self) {
        self.state.clip_path = Some(self.path.clone());
    }

    /// Check if point is in path.
    pub fn is_point_in_path(&self, x: f32, y: f32) -> bool {
        // Simple bounding box check for now
        let segments = self.path.to_line_segments();
        for segment in &segments {
            if segment.len() < 3 {
                continue;
            }
            // Point-in-polygon test using ray casting
            let mut inside = false;
            let mut j = segment.len() - 1;
            for i in 0..segment.len() {
                let (xi, yi) = segment[i];
                let (xj, yj) = segment[j];
                if ((yi > y) != (yj > y)) && (x < (xj - xi) * (y - yi) / (yj - yi) + xi) {
                    inside = !inside;
                }
                j = i;
            }
            if inside {
                return true;
            }
        }
        false
    }

    // ==================== Text ====================

    /// Fill text.
    pub fn fill_text(&mut self, text: &str, x: f32, y: f32) {
        self.commands.push(DrawCommand::FillText {
            text: text.to_string(),
            x, y,
            style: self.state.fill_style.clone(),
            font: self.state.font.clone(),
            transform: self.state.transform,
        });
    }

    /// Stroke text.
    pub fn stroke_text(&mut self, text: &str, x: f32, y: f32) {
        self.commands.push(DrawCommand::StrokeText {
            text: text.to_string(),
            x, y,
            style: self.state.stroke_style.clone(),
            font: self.state.font.clone(),
            line_width: self.state.line_width,
            transform: self.state.transform,
        });
    }

    /// Measure text.
    pub fn measure_text(&self, text: &str) -> TextMetrics {
        // Simplified: estimate based on font size
        let font_size = parse_font_size(&self.state.font).unwrap_or(10.0);
        let width = text.len() as f32 * font_size * 0.6;

        TextMetrics {
            width,
            actual_bounding_box_left: 0.0,
            actual_bounding_box_right: width,
            font_bounding_box_ascent: font_size * 0.8,
            font_bounding_box_descent: font_size * 0.2,
            actual_bounding_box_ascent: font_size * 0.8,
            actual_bounding_box_descent: font_size * 0.2,
            em_height_ascent: font_size * 0.8,
            em_height_descent: font_size * 0.2,
            hanging_baseline: font_size * 0.1,
            alphabetic_baseline: 0.0,
            ideographic_baseline: -font_size * 0.1,
        }
    }

    // ==================== Images ====================

    /// Draw image.
    pub fn draw_image(&mut self, image_id: &str, dx: f32, dy: f32) {
        self.commands.push(DrawCommand::DrawImage {
            image_id: image_id.to_string(),
            sx: 0.0, sy: 0.0, sw: 0.0, sh: 0.0,
            dx, dy, dw: 0.0, dh: 0.0,
            transform: self.state.transform,
        });
    }

    /// Draw image with size.
    pub fn draw_image_sized(&mut self, image_id: &str, dx: f32, dy: f32, dw: f32, dh: f32) {
        self.commands.push(DrawCommand::DrawImage {
            image_id: image_id.to_string(),
            sx: 0.0, sy: 0.0, sw: 0.0, sh: 0.0,
            dx, dy, dw, dh,
            transform: self.state.transform,
        });
    }

    /// Draw image with source and destination.
    pub fn draw_image_full(&mut self, image_id: &str, sx: f32, sy: f32, sw: f32, sh: f32, dx: f32, dy: f32, dw: f32, dh: f32) {
        self.commands.push(DrawCommand::DrawImage {
            image_id: image_id.to_string(),
            sx, sy, sw, sh,
            dx, dy, dw, dh,
            transform: self.state.transform,
        });
    }

    // ==================== Pixel Manipulation ====================

    /// Create image data.
    pub fn create_image_data(&self, width: u32, height: u32) -> ImageData {
        ImageData::new(width, height)
    }

    /// Get image data from canvas.
    pub fn get_image_data(&self, x: i32, y: i32, width: u32, height: u32) -> ImageData {
        // Return from pixel buffer if available
        if let Some(ref buffer) = self.pixel_buffer {
            let mut data = ImageData::new(width, height);
            for dy in 0..height {
                for dx in 0..width {
                    let sx = (x + dx as i32) as u32;
                    let sy = (y + dy as i32) as u32;
                    if let Some((r, g, b, a)) = buffer.get_pixel(sx, sy) {
                        data.set_pixel(dx, dy, r, g, b, a);
                    }
                }
            }
            return data;
        }
        ImageData::new(width, height)
    }

    /// Put image data to canvas.
    pub fn put_image_data(&mut self, data: ImageData, x: i32, y: i32) {
        self.commands.push(DrawCommand::PutImageData { data, x, y });
    }

    // ==================== Commands ====================

    /// Get recorded draw commands.
    pub fn get_commands(&self) -> &[DrawCommand] {
        &self.commands
    }

    /// Take recorded draw commands.
    pub fn take_commands(&mut self) -> Vec<DrawCommand> {
        std::mem::take(&mut self.commands)
    }

    /// Clear commands.
    pub fn clear_commands(&mut self) {
        self.commands.clear();
    }
}

// ==================== Helper Functions ====================

/// Parse canvas color string.
fn parse_canvas_color(s: &str) -> Option<Color> {
    let s = s.trim().to_lowercase();

    // Hex colors
    if s.starts_with('#') {
        let hex = &s[1..];
        return match hex.len() {
            3 => {
                let r = u8::from_str_radix(&hex[0..1].repeat(2), 16).ok()?;
                let g = u8::from_str_radix(&hex[1..2].repeat(2), 16).ok()?;
                let b = u8::from_str_radix(&hex[2..3].repeat(2), 16).ok()?;
                Some(Color::from_rgb(r, g, b))
            }
            6 => {
                let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
                let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
                let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
                Some(Color::from_rgb(r, g, b))
            }
            8 => {
                let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
                let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
                let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
                let a = u8::from_str_radix(&hex[6..8], 16).ok()?;
                Some(Color::new(r, g, b, a as f32 / 255.0))
            }
            _ => None,
        };
    }

    // RGB/RGBA
    if s.starts_with("rgb") {
        let inner = s.trim_start_matches("rgba(")
            .trim_start_matches("rgb(")
            .trim_end_matches(')');
        let parts: Vec<&str> = inner.split(|c| c == ',' || c == '/').collect();
        
        if parts.len() >= 3 {
            let r: u8 = parts[0].trim().parse().ok()?;
            let g: u8 = parts[1].trim().parse().ok()?;
            let b: u8 = parts[2].trim().parse().ok()?;
            let a: f32 = parts.get(3).and_then(|s| s.trim().parse().ok()).unwrap_or(1.0);
            return Some(Color::new(r, g, b, a));
        }
    }

    // Named colors
    match s.as_str() {
        "black" => Some(Color::from_rgb(0, 0, 0)),
        "white" => Some(Color::from_rgb(255, 255, 255)),
        "red" => Some(Color::from_rgb(255, 0, 0)),
        "green" => Some(Color::from_rgb(0, 128, 0)),
        "blue" => Some(Color::from_rgb(0, 0, 255)),
        "yellow" => Some(Color::from_rgb(255, 255, 0)),
        "cyan" => Some(Color::from_rgb(0, 255, 255)),
        "magenta" => Some(Color::from_rgb(255, 0, 255)),
        "gray" | "grey" => Some(Color::from_rgb(128, 128, 128)),
        "orange" => Some(Color::from_rgb(255, 165, 0)),
        "purple" => Some(Color::from_rgb(128, 0, 128)),
        "transparent" => Some(Color::TRANSPARENT),
        _ => None,
    }
}

/// Parse font size from CSS font string.
fn parse_font_size(font: &str) -> Option<f32> {
    for part in font.split_whitespace() {
        if part.ends_with("px") {
            return part.trim_end_matches("px").parse().ok();
        }
        if part.ends_with("pt") {
            let pt: f32 = part.trim_end_matches("pt").parse().ok()?;
            return Some(pt * 1.333);
        }
        if part.ends_with("em") {
            let em: f32 = part.trim_end_matches("em").parse().ok()?;
            return Some(em * 16.0);
        }
    }
    None
}

/// Interpolate between two colors in sRGB space.
/// Canvas 2D gradients use sRGB interpolation for browser compatibility.
fn interpolate_color(a: &Color, b: &Color, t: f32) -> Color {
    let lerp = |a: u8, b: u8, t: f32| -> u8 {
        ((a as f32) + (b as f32 - a as f32) * t).round() as u8
    };
    Color {
        r: lerp(a.r, b.r, t),
        g: lerp(a.g, b.g, t),
        b: lerp(a.b, b.b, t),
        a: a.a + (b.a - a.a) * t,
    }
}

/// Generate points along a quadratic bezier curve.
fn quad_bezier_points(p0: (f32, f32), p1: (f32, f32), p2: (f32, f32), segments: usize) -> Vec<(f32, f32)> {
    let mut points = Vec::with_capacity(segments);
    for i in 1..=segments {
        let t = i as f32 / segments as f32;
        let mt = 1.0 - t;
        let x = mt * mt * p0.0 + 2.0 * mt * t * p1.0 + t * t * p2.0;
        let y = mt * mt * p0.1 + 2.0 * mt * t * p1.1 + t * t * p2.1;
        points.push((x, y));
    }
    points
}

/// Generate points along a cubic bezier curve.
fn cubic_bezier_points(p0: (f32, f32), p1: (f32, f32), p2: (f32, f32), p3: (f32, f32), segments: usize) -> Vec<(f32, f32)> {
    let mut points = Vec::with_capacity(segments);
    for i in 1..=segments {
        let t = i as f32 / segments as f32;
        let t2 = t * t;
        let t3 = t2 * t;
        let mt = 1.0 - t;
        let mt2 = mt * mt;
        let mt3 = mt2 * mt;
        let x = mt3 * p0.0 + 3.0 * mt2 * t * p1.0 + 3.0 * mt * t2 * p2.0 + t3 * p3.0;
        let y = mt3 * p0.1 + 3.0 * mt2 * t * p1.1 + 3.0 * mt * t2 * p2.1 + t3 * p3.1;
        points.push((x, y));
    }
    points
}

/// Generate points along an arc.
fn arc_points(cx: f32, cy: f32, r: f32, start: f32, end: f32, ccw: bool, segments: u32) -> Vec<(f32, f32)> {
    let mut points = Vec::new();
    let mut angle_diff = end - start;
    
    if ccw {
        if angle_diff > 0.0 {
            angle_diff -= 2.0 * PI;
        }
    } else if angle_diff < 0.0 {
        angle_diff += 2.0 * PI;
    }

    let step = angle_diff / segments as f32;
    for i in 0..=segments {
        let angle = start + step * i as f32;
        points.push((cx + r * angle.cos(), cy + r * angle.sin()));
    }
    points
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_context_creation() {
        let ctx = CanvasRenderingContext2D::new(800, 600);
        assert_eq!(ctx.width, 800);
        assert_eq!(ctx.height, 600);
    }

    #[test]
    fn test_state_save_restore() {
        let mut ctx = CanvasRenderingContext2D::new(100, 100);
        ctx.set_line_width(5.0);
        ctx.save();
        ctx.set_line_width(10.0);
        assert_eq!(ctx.state.line_width, 10.0);
        ctx.restore();
        assert_eq!(ctx.state.line_width, 5.0);
    }

    #[test]
    fn test_transform() {
        let mut ctx = CanvasRenderingContext2D::new(100, 100);
        ctx.translate(10.0, 20.0);
        let t = ctx.get_transform();
        assert_eq!(t.e, 10.0);
        assert_eq!(t.f, 20.0);
    }

    #[test]
    fn test_transform_scale() {
        let mut ctx = CanvasRenderingContext2D::new(100, 100);
        ctx.scale(2.0, 3.0);
        let t = ctx.get_transform();
        assert_eq!(t.a, 2.0);
        assert_eq!(t.d, 3.0);
    }

    #[test]
    fn test_path_building() {
        let mut path = Path2D::new();
        path.move_to(0.0, 0.0);
        path.line_to(100.0, 0.0);
        path.line_to(100.0, 100.0);
        path.close_path();

        let segments = path.to_line_segments();
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].len(), 4);
    }

    #[test]
    fn test_fill_rect() {
        let mut ctx = CanvasRenderingContext2D::new(100, 100);
        ctx.fill_rect(10.0, 10.0, 50.0, 50.0);
        assert_eq!(ctx.get_commands().len(), 1);
    }

    #[test]
    fn test_path_with_arc() {
        let mut ctx = CanvasRenderingContext2D::new(100, 100);
        ctx.begin_path();
        ctx.arc(50.0, 50.0, 40.0, 0.0, 2.0 * PI, false);
        ctx.fill();
        assert_eq!(ctx.get_commands().len(), 1);
    }

    #[test]
    fn test_image_data() {
        let mut data = ImageData::new(10, 10);
        data.set_pixel(5, 5, 255, 0, 0, 255);
        let pixel = data.get_pixel(5, 5).unwrap();
        assert_eq!(pixel, (255, 0, 0, 255));
    }

    #[test]
    fn test_linear_gradient() {
        let mut grad = LinearGradient::new(0.0, 0.0, 100.0, 0.0);
        grad.add_color_stop(0.0, Color::from_rgb(255, 0, 0));
        grad.add_color_stop(1.0, Color::from_rgb(0, 0, 255));

        let c = grad.sample(0.5);
        assert_eq!(c.r, 128);
        assert_eq!(c.b, 128);
    }

    #[test]
    fn test_parse_color() {
        let c = parse_canvas_color("#ff0000").unwrap();
        assert_eq!(c.r, 255);
        assert_eq!(c.g, 0);
        assert_eq!(c.b, 0);

        let c = parse_canvas_color("rgb(0, 255, 0)").unwrap();
        assert_eq!(c.g, 255);

        let c = parse_canvas_color("blue").unwrap();
        assert_eq!(c.b, 255);
    }

    #[test]
    fn test_text_measure() {
        let ctx = CanvasRenderingContext2D::new(100, 100);
        let metrics = ctx.measure_text("Hello");
        assert!(metrics.width > 0.0);
    }

    #[test]
    fn test_point_in_path() {
        let mut ctx = CanvasRenderingContext2D::new(100, 100);
        ctx.begin_path();
        ctx.rect(10.0, 10.0, 50.0, 50.0);
        
        // Point inside
        assert!(ctx.is_point_in_path(25.0, 25.0));
        // Point outside
        assert!(!ctx.is_point_in_path(5.0, 5.0));
    }
}

