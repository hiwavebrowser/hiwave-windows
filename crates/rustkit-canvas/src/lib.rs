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
use std::sync::atomic::{AtomicU64, Ordering};
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

    /// This style with every colour's alpha multiplied by `alpha`
    /// (`globalAlpha`, applied when a command is recorded).
    pub fn with_alpha(&self, alpha: f32) -> Self {
        if alpha >= 1.0 {
            return self.clone();
        }
        let fade = |c: Color| Color { a: (c.a * alpha).clamp(0.0, 1.0), ..c };
        match self {
            CanvasStyle::Color(c) => CanvasStyle::Color(fade(*c)),
            CanvasStyle::LinearGradient(g) => {
                let mut g = g.clone();
                g.stops.iter_mut().for_each(|s| s.color = fade(s.color));
                CanvasStyle::LinearGradient(g)
            }
            CanvasStyle::RadialGradient(g) => {
                let mut g = g.clone();
                g.stops.iter_mut().for_each(|s| s.color = fade(s.color));
                CanvasStyle::RadialGradient(g)
            }
            CanvasStyle::Pattern(p) => CanvasStyle::Pattern(p.clone()),
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
    ///
    /// A sub-path with no current point starts where its first command
    /// does (HTML canvas §4.12.5.1.6: "ensure there is a subpath"), so a
    /// lone `arc` or `lineTo` does not draw from the origin.
    pub fn to_line_segments(&self) -> Vec<Vec<(f32, f32)>> {
        let mut segments = Vec::new();
        let mut current_segment: Vec<(f32, f32)> = Vec::new();
        let mut current_x = 0.0_f32;
        let mut current_y = 0.0_f32;
        let mut start_x = 0.0_f32;
        let mut start_y = 0.0_f32;

        for cmd in &self.commands {
            // No current point: the command's own start point begins one.
            if current_segment.is_empty() {
                let first = match cmd {
                    PathCommand::LineTo(x, y) => Some((*x, *y)),
                    PathCommand::QuadraticCurveTo(cx, cy, ..) => Some((*cx, *cy)),
                    PathCommand::BezierCurveTo(cx, cy, ..) => Some((*cx, *cy)),
                    PathCommand::ArcTo(x1, y1, ..) => Some((*x1, *y1)),
                    _ => None,
                };
                if let Some((x, y)) = first {
                    current_x = x;
                    current_y = y;
                    start_x = x;
                    start_y = y;
                    current_segment.push((x, y));
                }
            }
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
                    let points = ellipse_points(*cx, *cy, *r, *r, 0.0, *start, *end, *ccw);
                    if current_segment.is_empty() {
                        if let Some(&(x, y)) = points.first() {
                            start_x = x;
                            start_y = y;
                        }
                    }
                    if let Some(&(x, y)) = points.last() {
                        current_x = x;
                        current_y = y;
                    }
                    current_segment.extend(points);
                }
                PathCommand::Ellipse(cx, cy, rx, ry, rotation, start, end, ccw) => {
                    let points = ellipse_points(*cx, *cy, *rx, *ry, *rotation, *start, *end, *ccw);
                    if current_segment.is_empty() {
                        if let Some(&(x, y)) = points.first() {
                            start_x = x;
                            start_y = y;
                        }
                    }
                    if let Some(&(x, y)) = points.last() {
                        current_x = x;
                        current_y = y;
                    }
                    current_segment.extend(points);
                }
                PathCommand::ArcTo(x1, y1, x2, y2, r) => {
                    let points = arc_to_points((current_x, current_y), (*x1, *y1), (*x2, *y2), *r);
                    if let Some(&(x, y)) = points.last() {
                        current_x = x;
                        current_y = y;
                    }
                    current_segment.extend(points);
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
    /// The clip region in bitmap pixels `[x0, y0, x1, y1]`: the bounding
    /// box of every clip path intersected so far (a non-rectangular clip
    /// clips to its box). `None` is the whole bitmap.
    pub clip_rect: Option<[f32; 4]>,
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
            clip_rect: None,
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
    StrokeRect { x: f32, y: f32, w: f32, h: f32, style: CanvasStyle, line_width: f32, dash: LineDash, transform: Transform2D },
    /// A clear of part of the bitmap (a clear of all of it drops the
    /// commands before it instead of being recorded).
    ClearRect { x: f32, y: f32, w: f32, h: f32, transform: Transform2D },
    /// `even_odd`: the `"evenodd"` fill rule, else non-zero.
    FillPath { segments: Vec<Vec<(f32, f32)>>, style: CanvasStyle, even_odd: bool, transform: Transform2D },
    StrokePath { segments: Vec<Vec<(f32, f32)>>, style: CanvasStyle, line_width: f32, dash: LineDash, transform: Transform2D },
    FillText { text: String, x: f32, y: f32, style: CanvasStyle, font: String, align: TextAlign, baseline: TextBaseline, max_width: Option<f32>, transform: Transform2D },
    StrokeText { text: String, x: f32, y: f32, style: CanvasStyle, font: String, line_width: f32, align: TextAlign, baseline: TextBaseline, max_width: Option<f32>, transform: Transform2D },
    /// The clip region from here on, in bitmap pixels (`CanvasState::clip_rect`).
    Clip { rect: Option<[f32; 4]> },
    DrawImage { image_id: String, sx: f32, sy: f32, sw: f32, sh: f32, dx: f32, dy: f32, dw: f32, dh: f32, transform: Transform2D },
    PutImageData { data: ImageData, x: i32, y: i32 },
}

/// The dash pattern a stroke was recorded with (`setLineDash`, already
/// doubled to an even count) and its offset. Empty is a solid line.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LineDash {
    pub segments: Vec<f32>,
    pub offset: f32,
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
    /// Where each recorded command can paint, in bitmap pixels
    /// `[x0, y0, x1, y1]` (conservative, cut to the bitmap and the clip it
    /// was recorded under); `None` for a `Clip`.
    bounds: Vec<Option<[f32; 4]>>,
    /// [`command_bytes`] of the recording.
    bytes: usize,
    /// See [`Self::generation`].
    generation: u64,
    /// Pixel buffer (optional, for getImageData).
    pixel_buffer: Option<ImageData>,
}

static GENERATION: AtomicU64 = AtomicU64::new(1);

fn next_generation() -> u64 {
    GENERATION.fetch_add(1, Ordering::Relaxed)
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
            bounds: Vec::new(),
            bytes: 0,
            generation: next_generation(),
            pixel_buffer: None,
        }
    }

    // ==================== The recording ====================
    //
    // The recording is the canvas's content, so it is kept to what can
    // still be seen (PR #905 R1): a clear or an opaque fill drops the
    // commands it wholly covers, a clear of the whole bitmap drops them
    // all, and past RECORDING_BYTE_BUDGET the oldest commands go.

    fn canvas_rect(&self) -> [f32; 4] {
        [0.0, 0.0, self.width as f32, self.height as f32]
    }

    /// `rect` cut to the bitmap and the clip in force; `None` when nothing
    /// of it is left.
    fn visible(&self, rect: [f32; 4]) -> Option<[f32; 4]> {
        let mut r = intersect4(rect, self.canvas_rect());
        if let Some(c) = self.state.clip_rect {
            r = intersect4(r, c);
        }
        (r[0] < r[2] && r[1] < r[3]).then_some(r)
    }

    fn record(&mut self, command: DrawCommand) {
        let bounds = match &command {
            DrawCommand::Clip { .. } => None,
            other => match self.visible(command_extent(other, self.canvas_rect())) {
                Some(b) => Some(b),
                // Nothing of it can show.
                None => return,
            },
        };
        self.bytes += command_bytes(&command);
        self.commands.push(command);
        self.bounds.push(bounds);
        self.generation = next_generation();
        if self.bytes > RECORDING_BYTE_BUDGET {
            self.trim_oldest();
        }
    }

    fn record_clip(&mut self) {
        let rect = self.state.clip_rect;
        // A clip with nothing drawn under it since the last is replaced.
        if matches!(self.commands.last(), Some(DrawCommand::Clip { .. })) {
            self.commands.pop();
            self.bounds.pop();
            self.bytes -= std::mem::size_of::<DrawCommand>();
        }
        let in_force = self.commands.iter().rev().find_map(|c| match c {
            DrawCommand::Clip { rect } => Some(*rect),
            _ => None,
        });
        if in_force.unwrap_or(None) == rect {
            self.generation = next_generation();
            return;
        }
        self.record(DrawCommand::Clip { rect });
    }

    /// Drop everything recorded (a clear of the whole bitmap, a reset);
    /// the clip in force is recorded again for what follows.
    fn drop_all(&mut self) {
        self.commands.clear();
        self.bounds.clear();
        self.bytes = 0;
        self.generation = next_generation();
        if self.state.clip_rect.is_some() {
            self.record_clip();
        }
    }

    /// Drop the drawing (and clears) wholly inside `region` (bitmap
    /// pixels): something painted or cleared over all of it hides it.
    fn occlude(&mut self, region: [f32; 4]) {
        let inside = |b: &[f32; 4]| b[0] >= region[0] && b[1] >= region[1] && b[2] <= region[2] && b[3] <= region[3];
        if !self.bounds.iter().flatten().any(inside) {
            return;
        }
        let commands = std::mem::take(&mut self.commands);
        let bounds = std::mem::take(&mut self.bounds);
        self.bytes = 0;
        for (command, b) in commands.into_iter().zip(bounds) {
            match b {
                Some(b) if inside(&b) => continue,
                Some(_) => {}
                None => {
                    // A clip nothing is left under is replaced by this one.
                    if matches!(self.commands.last(), Some(DrawCommand::Clip { .. })) {
                        self.commands.pop();
                        self.bounds.pop();
                        self.bytes -= std::mem::size_of::<DrawCommand>();
                    }
                    if self.commands.is_empty() && matches!(command, DrawCommand::Clip { rect: None }) {
                        continue;
                    }
                }
            }
            self.bytes += command_bytes(&command);
            self.commands.push(command);
            self.bounds.push(b);
        }
        self.generation = next_generation();
    }

    /// Past the budget: drop the oldest commands down to three quarters of
    /// it (so this runs once per quarter-budget of drawing, not per
    /// command), keeping the clip they left in force.
    fn trim_oldest(&mut self) {
        let target = RECORDING_BYTE_BUDGET / 4 * 3;
        let mut freed = 0;
        let mut cut = 0;
        let mut clip = None;
        while cut < self.commands.len() && self.bytes - freed > target {
            if let DrawCommand::Clip { rect } = &self.commands[cut] {
                clip = Some(*rect);
            }
            freed += command_bytes(&self.commands[cut]);
            cut += 1;
        }
        self.commands.drain(..cut);
        self.bounds.drain(..cut);
        self.bytes -= freed;
        if let Some(rect @ Some(_)) = clip {
            if !matches!(self.commands.first(), Some(DrawCommand::Clip { .. })) {
                self.commands.insert(0, DrawCommand::Clip { rect });
                self.bounds.insert(0, None);
                self.bytes += std::mem::size_of::<DrawCommand>();
            }
        }
        self.generation = next_generation();
    }

    // ==================== State Management ====================

    /// Save current state.
    pub fn save(&mut self) {
        self.state_stack.push_back(self.state.clone());
    }

    /// Restore previous state.
    pub fn restore(&mut self) {
        if let Some(state) = self.state_stack.pop_back() {
            let clip_changed = state.clip_rect != self.state.clip_rect;
            self.state = state;
            if clip_changed {
                self.record_clip();
            }
        }
    }

    /// Reset the context.
    pub fn reset(&mut self) {
        self.state = CanvasState::default();
        self.state_stack.clear();
        self.path = Path2D::new();
        self.drop_all();
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
        if width.is_finite() && width > 0.0 {
            self.state.line_width = width;
        }
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
        if segments.iter().any(|v| !v.is_finite() || *v < 0.0) {
            return;
        }
        let mut segments = segments;
        if segments.len() % 2 == 1 {
            segments.extend(segments.clone());
        }
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
        if alpha.is_finite() && (0.0..=1.0).contains(&alpha) {
            self.state.global_alpha = alpha;
        }
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
        if !(x.is_finite() && y.is_finite() && w.is_finite() && h.is_finite()) || w == 0.0 || h == 0.0 {
            return;
        }
        let style = self.state.fill_style.with_alpha(self.state.global_alpha);
        let t = self.state.transform;
        // An opaque axis-aligned fill hides what is wholly under it.
        let opaque = matches!(style, CanvasStyle::Color(c) if c.a >= 1.0)
            && self.state.global_composite_operation == CompositeOperation::SourceOver
            && (self.state.shadow_color.a == 0.0 || self.state.shadow_blur == 0.0 && self.state.shadow_offset_x == 0.0 && self.state.shadow_offset_y == 0.0);
        if opaque && t.b == 0.0 && t.c == 0.0 {
            if let Some(region) = self.visible(quad_bounds(&quad(&t, x, y, w, h))) {
                self.occlude(region);
            }
        }
        self.record(DrawCommand::FillRect { x, y, w, h, style, transform: t });
    }

    /// Stroke a rectangle.
    pub fn stroke_rect(&mut self, x: f32, y: f32, w: f32, h: f32) {
        if !(x.is_finite() && y.is_finite() && w.is_finite() && h.is_finite()) {
            return;
        }
        self.record(DrawCommand::StrokeRect {
            x, y, w, h,
            style: self.state.stroke_style.with_alpha(self.state.global_alpha),
            line_width: self.state.line_width,
            dash: self.dash(),
            transform: self.state.transform,
        });
    }

    fn dash(&self) -> LineDash {
        LineDash { segments: self.state.line_dash.clone(), offset: self.state.line_dash_offset }
    }

    /// Clear a rectangle. A clear that covers the whole bitmap in bitmap
    /// space, under the transform and the clip in force (Chart.js clears
    /// that way before every frame), drops everything drawn before it. A
    /// partial axis-aligned clear drops what it wholly covers and is
    /// recorded, for the painter to erase, when it still covers something.
    /// A partial clear under a rotation or skew is not recorded (the
    /// painter erases axis-aligned regions only).
    pub fn clear_rect(&mut self, x: f32, y: f32, w: f32, h: f32) {
        if !(x.is_finite() && y.is_finite() && w.is_finite() && h.is_finite()) {
            return;
        }
        let t = self.state.transform;
        let q = quad(&t, x, y, w, h);
        let canvas = self.canvas_rect();
        let clip_covers = self.state.clip_rect.is_none_or(|c| contains4(c, canvas));
        let corners = [(0.0, 0.0), (canvas[2], 0.0), (canvas[2], canvas[3]), (0.0, canvas[3])];
        if clip_covers && corners.iter().all(|&p| in_convex_quad(&q, p)) {
            self.drop_all();
            return;
        }
        if !(t.b == 0.0 && t.c == 0.0) {
            return;
        }
        let Some(region) = self.visible(quad_bounds(&q)) else {
            return;
        };
        self.occlude(region);
        let overlaps = |b: &[f32; 4]| b[0] < region[2] && b[2] > region[0] && b[1] < region[3] && b[3] > region[1];
        let erases = self
            .commands
            .iter()
            .zip(&self.bounds)
            .any(|(c, b)| !matches!(c, DrawCommand::ClearRect { .. }) && b.as_ref().is_some_and(overlaps));
        if erases {
            self.record(DrawCommand::ClearRect { x, y, w, h, transform: t });
        }
    }

    /// The bitmap was resized (`canvas.width = n`, even to the same
    /// value): cleared, and the context back to its initial state.
    pub fn reset_bitmap(&mut self, width: u32, height: u32) {
        self.width = width;
        self.height = height;
        self.reset();
        self.generation = next_generation();
    }

    // ==================== Drawing Paths ====================

    /// Fill the current path.
    pub fn fill(&mut self) {
        self.fill_with_rule(false);
    }

    /// Fill the current path under the even-odd rule or non-zero.
    pub fn fill_with_rule(&mut self, even_odd: bool) {
        let segments = self.path.to_line_segments();
        if segments.is_empty() {
            return;
        }
        self.record(DrawCommand::FillPath {
            segments,
            style: self.state.fill_style.with_alpha(self.state.global_alpha),
            even_odd,
            transform: self.state.transform,
        });
    }

    /// Stroke the current path.
    pub fn stroke(&mut self) {
        let segments = self.path.to_line_segments();
        if segments.is_empty() {
            return;
        }
        self.record(DrawCommand::StrokePath {
            segments,
            style: self.state.stroke_style.with_alpha(self.state.global_alpha),
            line_width: self.state.line_width,
            dash: self.dash(),
            transform: self.state.transform,
        });
    }

    /// Clip to current path: the clip becomes its intersection with the
    /// path's bounding box in bitmap pixels.
    pub fn clip(&mut self) {
        self.state.clip_path = Some(self.path.clone());
        let t = self.state.transform;
        let mut b = [f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY];
        for &(x, y) in self.path.to_line_segments().iter().flatten() {
            let (x, y) = t.apply(x, y);
            b = [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)];
        }
        if !(b[0] <= b[2] && b[1] <= b[3]) {
            b = [0.0, 0.0, 0.0, 0.0];
        }
        if let Some(c) = self.state.clip_rect {
            b = [b[0].max(c[0]), b[1].max(c[1]), b[2].min(c[2]), b[3].min(c[3])];
            b[2] = b[2].max(b[0]);
            b[3] = b[3].max(b[1]);
        }
        self.state.clip_rect = Some(b);
        self.record_clip();
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
        self.fill_text_max(text, x, y, None);
    }

    /// Fill text, squeezed to `max_width` when wider.
    pub fn fill_text_max(&mut self, text: &str, x: f32, y: f32, max_width: Option<f32>) {
        if !(x.is_finite() && y.is_finite()) || max_width.is_some_and(|w| !(w > 0.0)) {
            return;
        }
        self.record(DrawCommand::FillText {
            text: text.to_string(),
            x, y,
            style: self.state.fill_style.with_alpha(self.state.global_alpha),
            font: self.state.font.clone(),
            align: self.state.text_align,
            baseline: self.state.text_baseline,
            max_width,
            transform: self.state.transform,
        });
    }

    /// Stroke text.
    pub fn stroke_text(&mut self, text: &str, x: f32, y: f32) {
        self.stroke_text_max(text, x, y, None);
    }

    /// Stroke text, squeezed to `max_width` when wider.
    pub fn stroke_text_max(&mut self, text: &str, x: f32, y: f32, max_width: Option<f32>) {
        if !(x.is_finite() && y.is_finite()) || max_width.is_some_and(|w| !(w > 0.0)) {
            return;
        }
        self.record(DrawCommand::StrokeText {
            text: text.to_string(),
            x, y,
            style: self.state.stroke_style.with_alpha(self.state.global_alpha),
            font: self.state.font.clone(),
            line_width: self.state.line_width,
            align: self.state.text_align,
            baseline: self.state.text_baseline,
            max_width,
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
        self.record(DrawCommand::DrawImage {
            image_id: image_id.to_string(),
            sx: 0.0, sy: 0.0, sw: 0.0, sh: 0.0,
            dx, dy, dw: 0.0, dh: 0.0,
            transform: self.state.transform,
        });
    }

    /// Draw image with size.
    pub fn draw_image_sized(&mut self, image_id: &str, dx: f32, dy: f32, dw: f32, dh: f32) {
        self.record(DrawCommand::DrawImage {
            image_id: image_id.to_string(),
            sx: 0.0, sy: 0.0, sw: 0.0, sh: 0.0,
            dx, dy, dw, dh,
            transform: self.state.transform,
        });
    }

    /// Draw image with source and destination.
    pub fn draw_image_full(&mut self, image_id: &str, sx: f32, sy: f32, sw: f32, sh: f32, dx: f32, dy: f32, dw: f32, dh: f32) {
        self.record(DrawCommand::DrawImage {
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
        self.record(DrawCommand::PutImageData { data, x, y });
    }

    // ==================== Commands ====================

    /// A number that changes whenever the recording does (a draw, a clear,
    /// a resize), unique across every context in the process, so a reader
    /// that cached what it built from the recording knows when to rebuild.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// An estimate of the heap the recording holds, in bytes (see
    /// [`RECORDING_BYTE_BUDGET`]).
    pub fn recorded_bytes(&self) -> usize {
        self.bytes
    }

    /// Get recorded draw commands.
    pub fn get_commands(&self) -> &[DrawCommand] {
        &self.commands
    }

    /// Take recorded draw commands.
    pub fn take_commands(&mut self) -> Vec<DrawCommand> {
        self.bounds.clear();
        self.bytes = 0;
        self.generation = next_generation();
        std::mem::take(&mut self.commands)
    }

    /// Clear commands.
    pub fn clear_commands(&mut self) {
        self.commands.clear();
        self.bounds.clear();
        self.bytes = 0;
        self.generation = next_generation();
    }
}

// ==================== Helper Functions ====================

fn intersect4(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    [a[0].max(b[0]), a[1].max(b[1]), a[2].min(b[2]), a[3].min(b[3])]
}

fn contains4(outer: [f32; 4], inner: [f32; 4]) -> bool {
    outer[0] <= inner[0] && outer[1] <= inner[1] && outer[2] >= inner[2] && outer[3] >= inner[3]
}

/// The user-space rect (x, y, w, h) under `t`, as its four corners in order.
fn quad(t: &Transform2D, x: f32, y: f32, w: f32, h: f32) -> [(f32, f32); 4] {
    [t.apply(x, y), t.apply(x + w, y), t.apply(x + w, y + h), t.apply(x, y + h)]
}

fn quad_bounds(q: &[(f32, f32)]) -> [f32; 4] {
    q.iter().fold([f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY], |b, &(x, y)| {
        [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)]
    })
}

/// Whether `p` is inside (or on) the parallelogram `q` (a rect under an
/// affine transform: convex, either winding).
fn in_convex_quad(q: &[(f32, f32); 4], p: (f32, f32)) -> bool {
    let mut sign = 0.0f32;
    for i in 0..4 {
        let (a, b) = (q[i], q[(i + 1) % 4]);
        let cross = (b.0 - a.0) * (p.1 - a.1) - (b.1 - a.1) * (p.0 - a.0);
        // A tolerance for the float error of the transform (bitmap px).
        let edge = ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt();
        if cross.abs() <= 1e-3 * edge.max(1.0) {
            continue;
        }
        if sign == 0.0 {
            sign = cross.signum();
        } else if cross.signum() != sign {
            return false;
        }
    }
    true
}

/// Where `command` can paint in bitmap pixels, before the clip: a
/// conservative box (a stroke's width on every side; text an estimate
/// two ems per character wide either side of its anchor, two ems above
/// and below its baseline). `canvas` stands for what has no extent of its own.
pub fn command_extent(command: &DrawCommand, canvas: [f32; 4]) -> [f32; 4] {
    fn user(t: &Transform2D, b: [f32; 4]) -> [f32; 4] {
        if !(b[0] <= b[2] && b[1] <= b[3]) {
            return [0.0, 0.0, 0.0, 0.0];
        }
        quad_bounds(&quad(t, b[0], b[1], b[2] - b[0], b[3] - b[1]))
    }
    fn grow(b: [f32; 4], by: f32) -> [f32; 4] {
        [b[0] - by, b[1] - by, b[2] + by, b[3] + by]
    }
    fn points(segments: &[Vec<(f32, f32)>]) -> [f32; 4] {
        quad_bounds(&segments.iter().flatten().copied().collect::<Vec<_>>())
    }
    let rect = |x: f32, y: f32, w: f32, h: f32| [x.min(x + w), y.min(y + h), x.max(x + w), y.max(y + h)];
    match command {
        DrawCommand::FillRect { x, y, w, h, transform, .. } => user(transform, rect(*x, *y, *w, *h)),
        DrawCommand::StrokeRect { x, y, w, h, line_width, transform, .. } => {
            user(transform, grow(rect(*x, *y, *w, *h), *line_width))
        }
        DrawCommand::ClearRect { x, y, w, h, transform } => user(transform, rect(*x, *y, *w, *h)),
        DrawCommand::FillPath { segments, transform, .. } => user(transform, points(segments)),
        DrawCommand::StrokePath { segments, line_width, transform, .. } => {
            user(transform, grow(points(segments), *line_width))
        }
        DrawCommand::FillText { text, x, y, font, max_width, transform, .. }
        | DrawCommand::StrokeText { text, x, y, font, max_width, transform, .. } => {
            let size = CanvasFont::parse(font, 16.0).map_or(16.0, |f| f.size);
            let mut wide = text.chars().count() as f32 * size * 2.0;
            if let Some(m) = max_width {
                if m.is_finite() && *m >= 0.0 {
                    wide = wide.min(*m);
                }
            }
            user(transform, [x - wide - size, y - 2.0 * size, x + wide + size, y + 2.0 * size])
        }
        DrawCommand::Clip { .. } | DrawCommand::DrawImage { .. } | DrawCommand::PutImageData { .. } => canvas,
    }
}

/// The most a canvas's recording may hold (estimated by [`command_bytes`]).
pub const RECORDING_BYTE_BUDGET: usize = 8 << 20;

/// What one recorded command costs: the enum itself plus what it owns.
pub fn command_bytes(command: &DrawCommand) -> usize {
    fn points(segments: &[Vec<(f32, f32)>]) -> usize {
        segments.iter().map(|s| s.len() * std::mem::size_of::<(f32, f32)>() + std::mem::size_of::<Vec<(f32, f32)>>()).sum()
    }
    fn style(style: &CanvasStyle) -> usize {
        match style {
            CanvasStyle::LinearGradient(g) => g.stops.len() * std::mem::size_of::<ColorStop>(),
            CanvasStyle::RadialGradient(g) => g.stops.len() * std::mem::size_of::<ColorStop>(),
            _ => 0,
        }
    }
    std::mem::size_of::<DrawCommand>()
        + match command {
            DrawCommand::FillRect { style: s, .. } => style(s),
            DrawCommand::StrokeRect { style: s, dash, .. } => style(s) + dash.segments.len() * 4,
            DrawCommand::FillPath { segments, style: s, .. } => points(segments) + style(s),
            DrawCommand::StrokePath { segments, style: s, dash, .. } => points(segments) + style(s) + dash.segments.len() * 4,
            DrawCommand::FillText { text, style: s, font, .. } | DrawCommand::StrokeText { text, style: s, font, .. } => {
                text.len() + font.len() + style(s)
            }
            DrawCommand::DrawImage { image_id, .. } => image_id.len(),
            DrawCommand::PutImageData { data, .. } => data.data.len(),
            DrawCommand::ClearRect { .. } | DrawCommand::Clip { .. } => 0,
        }
}

/// Parse a canvas colour string: any CSS colour the engine's parser
/// reads, else the short list below.
pub fn parse_canvas_color(s: &str) -> Option<Color> {
    if let Some(c) = rustkit_css::parse_color(s) {
        return Some(c);
    }
    let s = s.trim().to_lowercase();

    // Hex colors
    if let Some(hex) = s.strip_prefix('#') {
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
        let parts: Vec<&str> = inner.split([',', '/']).collect();
        
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

/// A colour as `fillStyle`/`strokeStyle` read it back (HTML canvas
/// §4.12.5.1.10 "serialization of a color"): `#rrggbb` when opaque, else
/// `rgba(r, g, b, a)`.
pub fn serialize_canvas_color(c: Color) -> String {
    if c.a >= 1.0 {
        format!("#{:02x}{:02x}{:02x}", c.r, c.g, c.b)
    } else {
        // Alpha as Chromium prints it: the shortest decimal of the byte
        // value it keeps (0.1 stays 0.1, 0 is "0").
        let a = (c.a.clamp(0.0, 1.0) * 255.0).round() / 255.0;
        let mut text = format!("{:.6}", a);
        while text.ends_with('0') {
            text.pop();
        }
        if text.ends_with('.') {
            text.pop();
        }
        // Prefer the author's short form when it rounds to the same byte.
        let authored = format!("{}", c.a);
        if ((c.a * 255.0).round() - (a * 255.0)).abs() < 0.5 && authored.len() < text.len() {
            text = authored;
        }
        format!("rgba({}, {}, {}, {})", c.r, c.g, c.b, text)
    }
}

/// A canvas `font` value, parsed (CSS Fonts 4 §3.7 `font` shorthand, the
/// subset canvas takes: style, variant, weight and stretch keywords, a
/// size with an optional `/line-height`, then the family list).
#[derive(Debug, Clone, PartialEq)]
pub struct CanvasFont {
    pub italic: bool,
    pub small_caps: bool,
    pub weight: u16,
    pub size: f32,
    pub family: String,
}

impl CanvasFont {
    /// Parse `font`; `None` when it is not a valid shorthand (the setter
    /// then keeps the font it had). `em`/`%`/`rem` sizes are relative to
    /// `base` px.
    pub fn parse(font: &str, base: f32) -> Option<CanvasFont> {
        let font = font.trim();
        let mut out = CanvasFont { italic: false, small_caps: false, weight: 400, size: 0.0, family: String::new() };
        let mut rest = font;
        loop {
            let (word, tail) = match rest.find(char::is_whitespace) {
                Some(i) => (&rest[..i], rest[i..].trim_start()),
                None => (rest, ""),
            };
            if word.is_empty() {
                return None;
            }
            let lower = word.to_ascii_lowercase();
            match lower.as_str() {
                "normal" => {}
                "italic" | "oblique" => out.italic = true,
                "small-caps" => out.small_caps = true,
                "bold" | "bolder" => out.weight = 700,
                "lighter" => out.weight = 100,
                "ultra-condensed" | "extra-condensed" | "condensed" | "semi-condensed" | "semi-expanded" | "expanded"
                | "extra-expanded" | "ultra-expanded" => {}
                w if w.len() == 3 && w.bytes().all(|b| b.is_ascii_digit()) && w.ends_with("00") => {
                    out.weight = w.parse().ok()?;
                }
                _ => {
                    // The size, then the family.
                    let size_part = lower.split('/').next().unwrap_or("");
                    out.size = parse_font_length(size_part, base)?;
                    let family = tail.trim();
                    if family.is_empty() {
                        return None;
                    }
                    out.family = family.to_string();
                    return Some(out);
                }
            }
            rest = tail;
        }
    }

    /// The value `font` reads back (Chromium's serialization: the
    /// non-initial keywords, the size in px, the family as given).
    pub fn serialize(&self) -> String {
        let mut out = String::new();
        if self.italic {
            out.push_str("italic ");
        }
        if self.small_caps {
            out.push_str("small-caps ");
        }
        match self.weight {
            400 => {}
            700 => out.push_str("bold "),
            w => out.push_str(&format!("{w} ")),
        }
        out.push_str(&format!("{}px {}", self.size, self.family));
        out
    }
}

fn parse_font_length(s: &str, base: f32) -> Option<f32> {
    let keyword = match s {
        "xx-small" => Some(9.0),
        "x-small" => Some(10.0),
        "small" => Some(13.0),
        "medium" => Some(16.0),
        "large" => Some(18.0),
        "x-large" => Some(24.0),
        "xx-large" => Some(32.0),
        _ => None,
    };
    if keyword.is_some() {
        return keyword;
    }
    let split = s.find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == '+'))?;
    let (num, unit) = s.split_at(split);
    let n: f32 = num.parse().ok()?;
    if !n.is_finite() || n < 0.0 {
        return None;
    }
    let px = match unit {
        "px" => n,
        "pt" => n * 4.0 / 3.0,
        "pc" => n * 16.0,
        "in" => n * 96.0,
        "cm" => n * 96.0 / 2.54,
        "mm" => n * 96.0 / 25.4,
        "em" | "rem" => n * base,
        "%" => n * base / 100.0,
        _ => return None,
    };
    Some(px)
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

/// Points along an ellipse arc (HTML canvas §4.12.5.1.6 `ellipse()`):
/// a sweep of 2π or more one way is the whole ellipse; otherwise the end
/// angle is brought within one turn of the start in the arc's direction.
/// Enough points that no chord strays more than a quarter pixel.
#[allow(clippy::too_many_arguments)]
fn ellipse_points(cx: f32, cy: f32, rx: f32, ry: f32, rotation: f32, start: f32, end: f32, ccw: bool) -> Vec<(f32, f32)> {
    const TAU: f32 = 2.0 * PI;
    if !(cx.is_finite() && cy.is_finite() && rx.is_finite() && ry.is_finite() && start.is_finite() && end.is_finite()) {
        return Vec::new();
    }
    let (rx, ry) = (rx.abs(), ry.abs());
    let sweep = if !ccw && end - start >= TAU {
        TAU
    } else if ccw && start - end >= TAU {
        -TAU
    } else {
        let d = (end - start).rem_euclid(TAU);
        if ccw {
            if d == 0.0 { 0.0 } else { d - TAU }
        } else {
            d
        }
    };
    let r = rx.max(ry);
    // Chord error r(1 - cos(θ/2)) <= 0.25 px.
    let step = if r > 0.25 { 2.0 * (1.0 - 0.25 / r).acos() } else { TAU };
    let n = ((sweep.abs() / step.max(0.01)).ceil() as usize).clamp(1, 1024);
    let (rs, rc) = rotation.sin_cos();
    (0..=n)
        .map(|i| {
            let a = start + sweep * i as f32 / n as f32;
            let (x, y) = (rx * a.cos(), ry * a.sin());
            (cx + x * rc - y * rs, cy + x * rs + y * rc)
        })
        .collect()
}

/// `arcTo(x1, y1, x2, y2, r)` from `p0`: a line to the first tangent point
/// and the arc to the second, or a line to `p1` when the corner is
/// degenerate (HTML canvas §4.12.5.1.6).
fn arc_to_points(p0: (f32, f32), p1: (f32, f32), p2: (f32, f32), r: f32) -> Vec<(f32, f32)> {
    let (v1x, v1y) = (p0.0 - p1.0, p0.1 - p1.1);
    let (v2x, v2y) = (p2.0 - p1.0, p2.1 - p1.1);
    let (l1, l2) = (v1x.hypot(v1y), v2x.hypot(v2y));
    let cross = v1x * v2y - v1y * v2x;
    if r <= 0.0 || l1 == 0.0 || l2 == 0.0 || cross.abs() < 1e-6 {
        return vec![p1];
    }
    let (u1x, u1y, u2x, u2y) = (v1x / l1, v1y / l1, v2x / l2, v2y / l2);
    let angle = (u1x * u2x + u1y * u2y).clamp(-1.0, 1.0).acos();
    let tangent = r / (angle / 2.0).tan();
    let t1 = (p1.0 + u1x * tangent, p1.1 + u1y * tangent);
    let t2 = (p1.0 + u2x * tangent, p1.1 + u2y * tangent);
    // The centre is along the bisector, r from both lines.
    let (bx, by) = (u1x + u2x, u1y + u2y);
    let bl = bx.hypot(by);
    let dist = r / (angle / 2.0).sin();
    let (cx, cy) = (p1.0 + bx / bl * dist, p1.1 + by / bl * dist);
    let a0 = (t1.1 - cy).atan2(t1.0 - cx);
    let a1 = (t2.1 - cy).atan2(t2.0 - cx);
    // Sweep the short way round (less than π).
    let mut d = a1 - a0;
    while d > PI {
        d -= 2.0 * PI;
    }
    while d < -PI {
        d += 2.0 * PI;
    }
    let mut points = vec![t1];
    points.extend(ellipse_points(cx, cy, r, r, 0.0, a0, a0 + d, d < 0.0));
    points.push(t2);
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
    // ---- Bounded recording (PR #905 R1 HOLD, blocker 2)

    fn drawing(ctx: &CanvasRenderingContext2D) -> usize {
        ctx.get_commands().iter().filter(|c| !matches!(c, DrawCommand::Clip { .. })).count()
    }

    /// A clear that covers the whole bitmap in bitmap space drops the
    /// recording: under a clip that itself covers the bitmap, and under a
    /// quarter turn, not only unclipped and axis-aligned.
    #[test]
    fn a_full_clear_resets_under_a_covering_clip_or_a_rotation() {
        let mut ctx = CanvasRenderingContext2D::new(100, 50);
        ctx.fill_rect(0.0, 0.0, 10.0, 10.0);
        ctx.begin_path();
        ctx.rect(-5.0, -5.0, 200.0, 200.0);
        ctx.clip();
        ctx.fill_rect(20.0, 20.0, 5.0, 5.0);
        ctx.clear_rect(0.0, 0.0, 100.0, 50.0);
        assert_eq!(drawing(&ctx), 0, "{:?}", ctx.get_commands());

        let mut ctx = CanvasRenderingContext2D::new(100, 50);
        ctx.fill_rect(0.0, 0.0, 10.0, 10.0);
        // (x, y) -> (100 - y, x): user rect 50x100 is the whole bitmap.
        ctx.set_transform(0.0, 1.0, -1.0, 0.0, 100.0, 0.0);
        ctx.clear_rect(0.0, 0.0, 50.0, 100.0);
        assert_eq!(drawing(&ctx), 0, "{:?}", ctx.get_commands());
    }

    /// A partial clear drops what it wholly covers and is recorded only
    /// when it has something left to erase.
    #[test]
    fn a_partial_clear_drops_what_it_covers() {
        let mut ctx = CanvasRenderingContext2D::new(100, 50);
        ctx.fill_rect(10.0, 10.0, 20.0, 20.0);
        ctx.clear_rect(0.0, 0.0, 50.0, 50.0);
        assert_eq!(ctx.get_commands().len(), 0, "{:?}", ctx.get_commands());
        ctx.clear_rect(60.0, 0.0, 10.0, 10.0);
        assert_eq!(ctx.get_commands().len(), 0, "a clear over nothing");
        ctx.fill_rect(0.0, 0.0, 100.0, 50.0);
        ctx.clear_rect(0.0, 0.0, 50.0, 50.0);
        assert!(matches!(ctx.get_commands(), [DrawCommand::FillRect { .. }, DrawCommand::ClearRect { .. }]), "{:?}", ctx.get_commands());
    }

    /// An opaque fillRect hides what is wholly under it; a translucent one
    /// does not.
    #[test]
    fn an_opaque_fill_hides_what_it_covers() {
        let mut ctx = CanvasRenderingContext2D::new(100, 50);
        ctx.fill_rect(10.0, 10.0, 5.0, 5.0);
        ctx.set_fill_style(CanvasStyle::Color(Color::WHITE));
        ctx.fill_rect(0.0, 0.0, 100.0, 50.0);
        assert_eq!(ctx.get_commands().len(), 1);
        ctx.set_fill_style(CanvasStyle::Color(Color::new(0, 0, 0, 0.5)));
        ctx.fill_rect(0.0, 0.0, 100.0, 50.0);
        assert_eq!(ctx.get_commands().len(), 2);
    }

    /// 10k frames of a chart redrawing its plot area (clip, partial clear,
    /// three bars) over an axis drawn once: the recording stays one frame.
    #[test]
    fn a_partial_clear_animation_stays_bounded() {
        let mut ctx = CanvasRenderingContext2D::new(100, 50);
        ctx.begin_path();
        ctx.move_to(0.0, 45.0);
        ctx.line_to(100.0, 45.0);
        ctx.stroke();
        for frame in 0..10_000 {
            ctx.save();
            ctx.begin_path();
            ctx.rect(0.0, 0.0, 100.0, 40.0);
            ctx.clip();
            ctx.clear_rect(0.0, 0.0, 100.0, 40.0);
            for bar in 0..3 {
                let h = 10.0 + ((frame + bar * 7) % 30) as f32;
                ctx.fill_rect(10.0 + bar as f32 * 30.0, 40.0 - h, 20.0, h);
            }
            ctx.restore();
            assert!(ctx.get_commands().len() <= 8, "frame {frame}: {:?}", ctx.get_commands());
        }
        assert_eq!(drawing(&ctx), 4, "the axis and the last frame's bars");
    }

    /// Drawing that is never cleared or covered stops at the byte budget:
    /// the oldest commands go, the newest stay.
    #[test]
    fn a_recording_stays_within_its_byte_budget() {
        let mut ctx = CanvasRenderingContext2D::new(100, 50);
        ctx.set_fill_style(CanvasStyle::Color(Color::new(255, 0, 0, 0.5)));
        let n = 2 * RECORDING_BYTE_BUDGET / std::mem::size_of::<DrawCommand>();
        for i in 0..n {
            ctx.fill_rect((i % 100) as f32, ((i / 100) % 50) as f32, 1.0, 1.0);
        }
        assert!(ctx.recorded_bytes() <= RECORDING_BYTE_BUDGET, "{} bytes", ctx.recorded_bytes());
        let last = (n - 1) as f32;
        assert!(matches!(ctx.get_commands().last(), Some(DrawCommand::FillRect { x, y, .. })
            if *x == last % 100.0 && *y == ((n - 1) / 100 % 50) as f32));
    }

    /// The generation moves with every change to the recording and only
    /// then, and two contexts never share one.
    #[test]
    fn the_generation_moves_with_the_recording() {
        let mut ctx = CanvasRenderingContext2D::new(100, 50);
        let other = CanvasRenderingContext2D::new(100, 50);
        assert_ne!(ctx.generation(), other.generation());
        let g0 = ctx.generation();
        ctx.save();
        ctx.translate(1.0, 1.0);
        assert_eq!(ctx.generation(), g0);
        ctx.fill_rect(0.0, 0.0, 5.0, 5.0);
        let g1 = ctx.generation();
        assert_ne!(g1, g0);
        ctx.clear_rect(-10.0, -10.0, 200.0, 200.0);
        let g2 = ctx.generation();
        assert_ne!(g2, g1);
        ctx.reset_bitmap(10, 10);
        assert_ne!(ctx.generation(), g2);
    }
}

