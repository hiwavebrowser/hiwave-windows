//! # RustKit SVG
//!
//! SVG parsing and rendering for the RustKit browser engine.
//!
//! ## Features
//!
//! - **SVG Parsing**: Parse SVG documents and elements
//! - **Basic Shapes**: rect, circle, ellipse, line, polyline, polygon
//! - **Paths**: SVG path commands (M, L, C, S, Q, T, A, Z)
//! - **Styling**: fill, stroke, opacity, transforms
//! - **Text**: Basic SVG text rendering
//! - **Rendering**: Convert SVG to display commands
//!
//! ## Architecture
//!
//! ```text
//! SVG Document
//!    └── SVG Elements
//!           ├── Shapes (rect, circle, path)
//!           ├── Text
//!           └── Groups (<g>)
//!              └── Transform Stack
//! ```

use rustkit_css::Color;
use rustkit_layout::{DisplayCommand, Rect};
use std::collections::HashMap;
use std::f32::consts::PI;
use thiserror::Error;

// ==================== Errors ====================

/// Errors that can occur in SVG operations.
#[derive(Error, Debug)]
pub enum SvgError {
    #[error("Parse error: {0}")]
    ParseError(String),

    #[error("Invalid path: {0}")]
    InvalidPath(String),

    #[error("Invalid attribute: {0}")]
    InvalidAttribute(String),

    #[error("Unsupported element: {0}")]
    UnsupportedElement(String),
}

// ==================== SVG Document ====================

/// An SVG document.
#[derive(Debug, Clone)]
pub struct SvgDocument {
    /// Root SVG element.
    pub root: SvgElement,
    /// ViewBox (min-x, min-y, width, height).
    pub view_box: Option<ViewBox>,
    /// Document width.
    pub width: Option<SvgLength>,
    /// Document height.
    pub height: Option<SvgLength>,
    /// Defined elements (for use references).
    pub defs: HashMap<String, SvgElement>,
}

impl SvgDocument {
    /// Create a new empty SVG document.
    pub fn new() -> Self {
        Self {
            root: SvgElement::Group(SvgGroup::new()),
            view_box: None,
            width: None,
            height: None,
            defs: HashMap::new(),
        }
    }

    /// Parse SVG from XML string.
    pub fn parse(xml: &str) -> Result<Self, SvgError> {
        let mut doc = Self::new();
        // Simple XML-like parser
        let xml = xml.trim();

        if !xml.contains("<svg") {
            return Err(SvgError::ParseError("No <svg> element found".into()));
        }

        // Extract SVG attributes
        if let Some(svg_start) = xml.find("<svg") {
            if let Some(svg_end) = xml[svg_start..].find('>') {
                let attrs = &xml[svg_start..svg_start + svg_end + 1];
                
                // Parse viewBox
                if let Some(vb) = extract_attr(attrs, "viewBox") {
                    doc.view_box = ViewBox::parse(&vb);
                }
                
                // Parse width/height
                if let Some(w) = extract_attr(attrs, "width") {
                    doc.width = SvgLength::parse(&w);
                }
                if let Some(h) = extract_attr(attrs, "height") {
                    doc.height = SvgLength::parse(&h);
                }
            }
        }

        // Parse elements (simplified)
        doc.root = parse_svg_content(xml)?;

        Ok(doc)
    }

    /// Get computed size (using viewBox or explicit dimensions).
    pub fn get_size(&self, container_width: f32, container_height: f32) -> (f32, f32) {
        let width = self.width
            .as_ref()
            .map(|l| l.to_px(container_width))
            .or_else(|| self.view_box.as_ref().map(|vb| vb.width))
            .unwrap_or(300.0);

        let height = self.height
            .as_ref()
            .map(|l| l.to_px(container_height))
            .or_else(|| self.view_box.as_ref().map(|vb| vb.height))
            .unwrap_or(150.0);

        (width, height)
    }

    /// Render to display commands.
    pub fn render(&self, x: f32, y: f32, width: f32, height: f32) -> Vec<DisplayCommand> {
        let mut commands = Vec::new();
        // Apply viewBox transform if present
        let transform = if let Some(vb) = &self.view_box {
            let scale_x = width / vb.width;
            let scale_y = height / vb.height;
            let scale = scale_x.min(scale_y);

            Transform2D::identity()
                .translate(x - vb.min_x * scale, y - vb.min_y * scale)
                .scale(scale, scale)
        } else {
            Transform2D::identity().translate(x, y)
        };

        self.root.render(&transform, &SvgStyle::default(), &mut commands);

        commands
    }
}

impl Default for SvgDocument {
    fn default() -> Self {
        Self::new()
    }
}

/// SVG viewBox.
#[derive(Debug, Clone, Copy)]
pub struct ViewBox {
    pub min_x: f32,
    pub min_y: f32,
    pub width: f32,
    pub height: f32,
}

impl ViewBox {
    /// Parse viewBox attribute.
    pub fn parse(s: &str) -> Option<Self> {
        let parts: Vec<f32> = s
            .split(|c: char| c.is_whitespace() || c == ',')
            .filter_map(|p| p.trim().parse().ok())
            .collect();

        if parts.len() >= 4 {
            Some(ViewBox {
                min_x: parts[0],
                min_y: parts[1],
                width: parts[2],
                height: parts[3],
            })
        } else {
            None
        }
    }
}

// ==================== SVG Length ====================

/// SVG length value.
#[derive(Debug, Clone, Copy)]
pub enum SvgLength {
    /// Pixels.
    Px(f32),
    /// Percentage.
    Percent(f32),
    /// Em units.
    Em(f32),
    /// User units (no unit specified).
    User(f32),
}

impl SvgLength {
    /// Parse length string.
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        
        if s.ends_with('%') {
            let val: f32 = s.trim_end_matches('%').parse().ok()?;
            Some(SvgLength::Percent(val))
        } else if s.ends_with("px") {
            let val: f32 = s.trim_end_matches("px").parse().ok()?;
            Some(SvgLength::Px(val))
        } else if s.ends_with("em") {
            let val: f32 = s.trim_end_matches("em").parse().ok()?;
            Some(SvgLength::Em(val))
        } else {
            let val: f32 = s.parse().ok()?;
            Some(SvgLength::User(val))
        }
    }

    /// Convert to pixels.
    pub fn to_px(&self, container_size: f32) -> f32 {
        match self {
            SvgLength::Px(v) | SvgLength::User(v) => *v,
            SvgLength::Percent(p) => container_size * p / 100.0,
            SvgLength::Em(em) => em * 16.0, // Default font size
        }
    }
}

// ==================== Transform ====================

/// 2D affine transform matrix.
#[derive(Debug, Clone, Copy)]
pub struct Transform2D {
    /// Matrix elements [a, b, c, d, e, f]
    /// Represents: [a c e]
    ///             [b d f]
    ///             [0 0 1]
    pub a: f32,
    pub b: f32,
    pub c: f32,
    pub d: f32,
    pub e: f32,
    pub f: f32,
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

    /// Create translation transform.
    pub fn translate(self, tx: f32, ty: f32) -> Self {
        self.multiply(&Transform2D {
            a: 1.0, b: 0.0,
            c: 0.0, d: 1.0,
            e: tx, f: ty,
        })
    }

    /// Create scale transform.
    pub fn scale(self, sx: f32, sy: f32) -> Self {
        self.multiply(&Transform2D {
            a: sx, b: 0.0,
            c: 0.0, d: sy,
            e: 0.0, f: 0.0,
        })
    }

    /// Create rotation transform (radians).
    pub fn rotate(self, angle: f32) -> Self {
        let cos = angle.cos();
        let sin = angle.sin();
        self.multiply(&Transform2D {
            a: cos, b: sin,
            c: -sin, d: cos,
            e: 0.0, f: 0.0,
        })
    }

    /// Create skew X transform.
    pub fn skew_x(self, angle: f32) -> Self {
        self.multiply(&Transform2D {
            a: 1.0, b: 0.0,
            c: angle.tan(), d: 1.0,
            e: 0.0, f: 0.0,
        })
    }

    /// Create skew Y transform.
    pub fn skew_y(self, angle: f32) -> Self {
        self.multiply(&Transform2D {
            a: 1.0, b: angle.tan(),
            c: 0.0, d: 1.0,
            e: 0.0, f: 0.0,
        })
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

    /// Transform a point.
    pub fn apply(&self, x: f32, y: f32) -> (f32, f32) {
        (
            self.a * x + self.c * y + self.e,
            self.b * x + self.d * y + self.f,
        )
    }

    /// Parse SVG transform attribute.
    pub fn parse(s: &str) -> Self {
        let mut result = Self::identity();
        
        // Parse transform functions
        let mut s = s.trim();
        while !s.is_empty() {
            if let Some((func, rest)) = parse_transform_function(s) {
                result = result.multiply(&func);
                s = rest.trim();
            } else {
                break;
            }
        }

        result
    }
}

impl Default for Transform2D {
    fn default() -> Self {
        Self::identity()
    }
}

/// Parse a single transform function.
fn parse_transform_function(s: &str) -> Option<(Transform2D, &str)> {
    // Find function name
    let open = s.find('(')?;
    let close = s.find(')')?;
    
    let name = s[..open].trim();
    let args: Vec<f32> = s[open + 1..close]
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter_map(|p| p.trim().parse().ok())
        .collect();

    let transform = match name {
        "translate" => {
            let tx = args.first().copied().unwrap_or(0.0);
            let ty = args.get(1).copied().unwrap_or(0.0);
            Transform2D::identity().translate(tx, ty)
        }
        "scale" => {
            let sx = args.first().copied().unwrap_or(1.0);
            let sy = args.get(1).copied().unwrap_or(sx);
            Transform2D::identity().scale(sx, sy)
        }
        "rotate" => {
            let angle = args.first().copied().unwrap_or(0.0) * PI / 180.0;
            if args.len() >= 3 {
                let cx = args[1];
                let cy = args[2];
                Transform2D::identity()
                    .translate(cx, cy)
                    .rotate(angle)
                    .translate(-cx, -cy)
            } else {
                Transform2D::identity().rotate(angle)
            }
        }
        "skewX" => {
            let angle = args.first().copied().unwrap_or(0.0) * PI / 180.0;
            Transform2D::identity().skew_x(angle)
        }
        "skewY" => {
            let angle = args.first().copied().unwrap_or(0.0) * PI / 180.0;
            Transform2D::identity().skew_y(angle)
        }
        "matrix" if args.len() >= 6 => {
            Transform2D {
                a: args[0], b: args[1],
                c: args[2], d: args[3],
                e: args[4], f: args[5],
            }
        }
        _ => return None,
    };

    Some((transform, &s[close + 1..]))
}

// ==================== SVG Style ====================

/// Paint value (fill or stroke).
#[derive(Debug, Clone)]
pub enum Paint {
    /// No paint.
    None,
    /// Solid color.
    Color(Color),
    /// URL reference (gradients, patterns).
    Url(String),
    /// Current color.
    CurrentColor,
}

impl Default for Paint {
    fn default() -> Self {
        Paint::Color(Color::BLACK)
    }
}

impl Paint {
    /// Parse paint attribute.
    pub fn parse(s: &str) -> Self {
        let s = s.trim().to_lowercase();
        
        match s.as_str() {
            "none" => Paint::None,
            "currentcolor" => Paint::CurrentColor,
            _ if s.starts_with("url(") => {
                let url = s.trim_start_matches("url(")
                    .trim_end_matches(')')
                    .trim_matches(|c| c == '"' || c == '\'' || c == '#')
                    .to_string();
                Paint::Url(url)
            }
            _ => {
                if let Some(color) = parse_svg_color(&s) {
                    Paint::Color(color)
                } else {
                    Paint::Color(Color::BLACK)
                }
            }
        }
    }

    /// Get color if this is a solid color.
    pub fn as_color(&self) -> Option<Color> {
        match self {
            Paint::Color(c) => Some(*c),
            Paint::CurrentColor => Some(Color::BLACK), // Would need context
            _ => None,
        }
    }
}

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

/// Fill rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FillRule {
    #[default]
    NonZero,
    EvenOdd,
}

/// SVG styling properties.
#[derive(Debug, Clone)]
pub struct SvgStyle {
    /// Fill paint.
    pub fill: Paint,
    /// Fill opacity.
    pub fill_opacity: f32,
    /// Fill rule.
    pub fill_rule: FillRule,
    /// Stroke paint.
    pub stroke: Paint,
    /// Stroke width.
    pub stroke_width: f32,
    /// Stroke opacity.
    pub stroke_opacity: f32,
    /// Line cap.
    pub stroke_linecap: LineCap,
    /// Line join.
    pub stroke_linejoin: LineJoin,
    /// Miter limit.
    pub stroke_miterlimit: f32,
    /// Dash array.
    pub stroke_dasharray: Vec<f32>,
    /// Dash offset.
    pub stroke_dashoffset: f32,
    /// Overall opacity.
    pub opacity: f32,
    /// Visibility.
    pub visibility: bool,
}

impl Default for SvgStyle {
    fn default() -> Self {
        Self {
            fill: Paint::Color(Color::BLACK),
            fill_opacity: 1.0,
            fill_rule: FillRule::NonZero,
            stroke: Paint::None,
            stroke_width: 1.0,
            stroke_opacity: 1.0,
            stroke_linecap: LineCap::Butt,
            stroke_linejoin: LineJoin::Miter,
            stroke_miterlimit: 4.0,
            stroke_dasharray: Vec::new(),
            stroke_dashoffset: 0.0,
            opacity: 1.0,
            visibility: true,
        }
    }
}

impl SvgStyle {
    /// Merge with parent style (inherited properties).
    pub fn inherit_from(&mut self, parent: &SvgStyle) {
        // Some properties inherit if not explicitly set
        // For simplicity, we keep explicit values
        if self.opacity == 1.0 {
            self.opacity = parent.opacity;
        }
    }

    /// Parse style attributes.
    pub fn parse_attributes(&mut self, attrs: &HashMap<String, String>) {
        if let Some(fill) = attrs.get("fill") {
            self.fill = Paint::parse(fill);
        }
        if let Some(fill_opacity) = attrs.get("fill-opacity") {
            self.fill_opacity = fill_opacity.parse().unwrap_or(1.0);
        }
        if let Some(stroke) = attrs.get("stroke") {
            self.stroke = Paint::parse(stroke);
        }
        if let Some(stroke_width) = attrs.get("stroke-width") {
            if let Some(len) = SvgLength::parse(stroke_width) {
                self.stroke_width = len.to_px(1.0);
            }
        }
        if let Some(stroke_opacity) = attrs.get("stroke-opacity") {
            self.stroke_opacity = stroke_opacity.parse().unwrap_or(1.0);
        }
        if let Some(opacity) = attrs.get("opacity") {
            self.opacity = opacity.parse().unwrap_or(1.0);
        }
        if let Some(linecap) = attrs.get("stroke-linecap") {
            self.stroke_linecap = match linecap.as_str() {
                "round" => LineCap::Round,
                "square" => LineCap::Square,
                _ => LineCap::Butt,
            };
        }
        if let Some(linejoin) = attrs.get("stroke-linejoin") {
            self.stroke_linejoin = match linejoin.as_str() {
                "round" => LineJoin::Round,
                "bevel" => LineJoin::Bevel,
                _ => LineJoin::Miter,
            };
        }
    }
}

// ==================== SVG Elements ====================

/// An SVG element.
#[derive(Debug, Clone)]
pub enum SvgElement {
    /// Group element.
    Group(SvgGroup),
    /// Rectangle.
    Rect(SvgRect),
    /// Circle.
    Circle(SvgCircle),
    /// Ellipse.
    Ellipse(SvgEllipse),
    /// Line.
    Line(SvgLine),
    /// Polyline.
    Polyline(SvgPolyline),
    /// Polygon.
    Polygon(SvgPolygon),
    /// Path.
    Path(SvgPath),
    /// Text.
    Text(SvgText),
    /// Use reference.
    Use(SvgUse),
}

impl SvgElement {
    /// Render this element to display commands.
    pub fn render(&self, transform: &Transform2D, parent_style: &SvgStyle, commands: &mut Vec<DisplayCommand>) {
        match self {
            SvgElement::Group(g) => g.render(transform, parent_style, commands),
            SvgElement::Rect(r) => r.render(transform, parent_style, commands),
            SvgElement::Circle(c) => c.render(transform, parent_style, commands),
            SvgElement::Ellipse(e) => e.render(transform, parent_style, commands),
            SvgElement::Line(l) => l.render(transform, parent_style, commands),
            SvgElement::Polyline(p) => p.render(transform, parent_style, commands),
            SvgElement::Polygon(p) => p.render(transform, parent_style, commands),
            SvgElement::Path(p) => p.render(transform, parent_style, commands),
            SvgElement::Text(t) => t.render(transform, parent_style, commands),
            SvgElement::Use(_) => {} // TODO: resolve references
        }
    }
}

/// Group element (<g>).
#[derive(Debug, Clone, Default)]
pub struct SvgGroup {
    /// Child elements.
    pub children: Vec<SvgElement>,
    /// Local transform.
    pub transform: Transform2D,
    /// Style.
    pub style: SvgStyle,
    /// ID.
    pub id: Option<String>,
}

impl SvgGroup {
    /// Create a new group.
    pub fn new() -> Self {
        Self::default()
    }

    /// Render the group.
    pub fn render(&self, parent_transform: &Transform2D, parent_style: &SvgStyle, commands: &mut Vec<DisplayCommand>) {
        let transform = parent_transform.multiply(&self.transform);
        let mut style = self.style.clone();
        style.inherit_from(parent_style);

        for child in &self.children {
            child.render(&transform, &style, commands);
        }
    }
}

/// Rectangle element (<rect>).
#[derive(Debug, Clone)]
pub struct SvgRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub rx: f32,
    pub ry: f32,
    pub transform: Transform2D,
    pub style: SvgStyle,
}

impl Default for SvgRect {
    fn default() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            width: 0.0,
            height: 0.0,
            rx: 0.0,
            ry: 0.0,
            transform: Transform2D::identity(),
            style: SvgStyle::default(),
        }
    }
}

impl SvgRect {
    /// Render the rectangle.
    pub fn render(&self, parent_transform: &Transform2D, parent_style: &SvgStyle, commands: &mut Vec<DisplayCommand>) {
        let transform = parent_transform.multiply(&self.transform);
        let mut style = self.style.clone();
        style.inherit_from(parent_style);

        if !style.visibility {
            return;
        }

        // Transform corners
        let (x1, y1) = transform.apply(self.x, self.y);
        let (x2, y2) = transform.apply(self.x + self.width, self.y + self.height);

        let rect = Rect {
            x: x1.min(x2),
            y: y1.min(y2),
            width: (x2 - x1).abs(),
            height: (y2 - y1).abs(),
        };

        // Fill
        if let Some(color) = style.fill.as_color() {
            let alpha = (color.a * style.fill_opacity * style.opacity).clamp(0.0, 1.0);
            let fill_color = Color { a: alpha, ..color };
            commands.push(DisplayCommand::FillRect { rect: rect.clone(), color: fill_color });
        }

        // Stroke
        if let Some(color) = style.stroke.as_color() {
            let alpha = (color.a * style.stroke_opacity * style.opacity).clamp(0.0, 1.0);
            let stroke_color = Color { a: alpha, ..color };
            commands.push(DisplayCommand::StrokeRect {
                rect: rect.clone(),
                color: stroke_color,
                width: style.stroke_width,
            });
        }
    }
}

/// Circle element (<circle>).
#[derive(Debug, Clone)]
pub struct SvgCircle {
    pub cx: f32,
    pub cy: f32,
    pub r: f32,
    pub transform: Transform2D,
    pub style: SvgStyle,
}

impl Default for SvgCircle {
    fn default() -> Self {
        Self {
            cx: 0.0,
            cy: 0.0,
            r: 0.0,
            transform: Transform2D::identity(),
            style: SvgStyle::default(),
        }
    }
}

impl SvgCircle {
    /// Render the circle.
    pub fn render(&self, parent_transform: &Transform2D, parent_style: &SvgStyle, commands: &mut Vec<DisplayCommand>) {
        let transform = parent_transform.multiply(&self.transform);
        let mut style = self.style.clone();
        style.inherit_from(parent_style);

        if !style.visibility {
            return;
        }

        let (cx, cy) = transform.apply(self.cx, self.cy);
        // Approximate radius scaling (ignoring skew)
        let scale = ((transform.a * transform.a + transform.b * transform.b).sqrt()
            + (transform.c * transform.c + transform.d * transform.d).sqrt()) / 2.0;
        let r = self.r * scale;

        // Fill
        if let Some(color) = style.fill.as_color() {
            let alpha = (color.a * style.fill_opacity * style.opacity).clamp(0.0, 1.0);
            let fill_color = Color { a: alpha, ..color };
            commands.push(DisplayCommand::FillCircle {
                cx,
                cy,
                radius: r,
                color: fill_color,
            });
        }

        // Stroke
        if let Some(color) = style.stroke.as_color() {
            let alpha = (color.a * style.stroke_opacity * style.opacity).clamp(0.0, 1.0);
            let stroke_color = Color { a: alpha, ..color };
            commands.push(DisplayCommand::StrokeCircle {
                cx,
                cy,
                radius: r,
                color: stroke_color,
                width: style.stroke_width,
            });
        }
    }
}

/// Ellipse element (<ellipse>).
#[derive(Debug, Clone)]
pub struct SvgEllipse {
    pub cx: f32,
    pub cy: f32,
    pub rx: f32,
    pub ry: f32,
    pub transform: Transform2D,
    pub style: SvgStyle,
}

impl Default for SvgEllipse {
    fn default() -> Self {
        Self {
            cx: 0.0,
            cy: 0.0,
            rx: 0.0,
            ry: 0.0,
            transform: Transform2D::identity(),
            style: SvgStyle::default(),
        }
    }
}

impl SvgEllipse {
    /// Render the ellipse.
    pub fn render(&self, parent_transform: &Transform2D, parent_style: &SvgStyle, commands: &mut Vec<DisplayCommand>) {
        let transform = parent_transform.multiply(&self.transform);
        let mut style = self.style.clone();
        style.inherit_from(parent_style);

        if !style.visibility {
            return;
        }

        let (cx, cy) = transform.apply(self.cx, self.cy);

        // For now, render as bounding rect (proper ellipse would need path or special command)
        let rect = Rect {
            x: cx - self.rx,
            y: cy - self.ry,
            width: self.rx * 2.0,
            height: self.ry * 2.0,
        };

        if let Some(color) = style.fill.as_color() {
            let alpha = (color.a * style.fill_opacity * style.opacity).clamp(0.0, 1.0);
            let fill_color = Color { a: alpha, ..color };
            commands.push(DisplayCommand::FillEllipse {
                rect: rect.clone(),
                color: fill_color,
            });
        }
    }
}

/// Line element (<line>).
#[derive(Debug, Clone)]
pub struct SvgLine {
    pub x1: f32,
    pub y1: f32,
    pub x2: f32,
    pub y2: f32,
    pub transform: Transform2D,
    pub style: SvgStyle,
}

impl Default for SvgLine {
    fn default() -> Self {
        Self {
            x1: 0.0,
            y1: 0.0,
            x2: 0.0,
            y2: 0.0,
            transform: Transform2D::identity(),
            style: SvgStyle::default(),
        }
    }
}

impl SvgLine {
    /// Render the line.
    pub fn render(&self, parent_transform: &Transform2D, parent_style: &SvgStyle, commands: &mut Vec<DisplayCommand>) {
        let transform = parent_transform.multiply(&self.transform);
        let mut style = self.style.clone();
        style.inherit_from(parent_style);

        if !style.visibility {
            return;
        }

        let (x1, y1) = transform.apply(self.x1, self.y1);
        let (x2, y2) = transform.apply(self.x2, self.y2);

        if let Some(color) = style.stroke.as_color() {
            let alpha = (color.a * style.stroke_opacity * style.opacity).clamp(0.0, 1.0);
            let stroke_color = Color { a: alpha, ..color };
            commands.push(DisplayCommand::Line {
                x1,
                y1,
                x2,
                y2,
                color: stroke_color,
                width: style.stroke_width,
            });
        }
    }
}

/// Polyline element (<polyline>).
#[derive(Debug, Clone, Default)]
pub struct SvgPolyline {
    pub points: Vec<(f32, f32)>,
    pub transform: Transform2D,
    pub style: SvgStyle,
}

impl SvgPolyline {
    /// Render the polyline.
    pub fn render(&self, parent_transform: &Transform2D, parent_style: &SvgStyle, commands: &mut Vec<DisplayCommand>) {
        let transform = parent_transform.multiply(&self.transform);
        let mut style = self.style.clone();
        style.inherit_from(parent_style);

        if !style.visibility || self.points.len() < 2 {
            return;
        }

        let points: Vec<(f32, f32)> = self.points
            .iter()
            .map(|(x, y)| transform.apply(*x, *y))
            .collect();

        if let Some(color) = style.stroke.as_color() {
            let alpha = (color.a * style.stroke_opacity * style.opacity).clamp(0.0, 1.0);
            let stroke_color = Color { a: alpha, ..color };
            commands.push(DisplayCommand::Polyline {
                points: points.clone(),
                color: stroke_color,
                width: style.stroke_width,
            });
        }
    }
}

/// Polygon element (<polygon>).
#[derive(Debug, Clone, Default)]
pub struct SvgPolygon {
    pub points: Vec<(f32, f32)>,
    pub transform: Transform2D,
    pub style: SvgStyle,
}

impl SvgPolygon {
    /// Render the polygon.
    pub fn render(&self, parent_transform: &Transform2D, parent_style: &SvgStyle, commands: &mut Vec<DisplayCommand>) {
        let transform = parent_transform.multiply(&self.transform);
        let mut style = self.style.clone();
        style.inherit_from(parent_style);

        if !style.visibility || self.points.len() < 3 {
            return;
        }

        let points: Vec<(f32, f32)> = self.points
            .iter()
            .map(|(x, y)| transform.apply(*x, *y))
            .collect();

        if let Some(color) = style.fill.as_color() {
            let alpha = (color.a * style.fill_opacity * style.opacity).clamp(0.0, 1.0);
            let fill_color = Color { a: alpha, ..color };
            commands.push(DisplayCommand::FillPolygon {
                points: points.clone(),
                color: fill_color,
            });
        }

        if let Some(color) = style.stroke.as_color() {
            let alpha = (color.a * style.stroke_opacity * style.opacity).clamp(0.0, 1.0);
            let stroke_color = Color { a: alpha, ..color };
            commands.push(DisplayCommand::StrokePolygon {
                points,
                color: stroke_color,
                width: style.stroke_width,
            });
        }
    }
}

// ==================== SVG Path ====================

/// Path command.
#[derive(Debug, Clone, Copy)]
pub enum PathCommand {
    /// Move to (absolute).
    MoveTo(f32, f32),
    /// Move to (relative).
    MoveToRel(f32, f32),
    /// Line to (absolute).
    LineTo(f32, f32),
    /// Line to (relative).
    LineToRel(f32, f32),
    /// Horizontal line (absolute).
    HorizontalTo(f32),
    /// Horizontal line (relative).
    HorizontalToRel(f32),
    /// Vertical line (absolute).
    VerticalTo(f32),
    /// Vertical line (relative).
    VerticalToRel(f32),
    /// Cubic bezier (absolute).
    CubicTo(f32, f32, f32, f32, f32, f32),
    /// Cubic bezier (relative).
    CubicToRel(f32, f32, f32, f32, f32, f32),
    /// Smooth cubic bezier (absolute).
    SmoothCubicTo(f32, f32, f32, f32),
    /// Smooth cubic bezier (relative).
    SmoothCubicToRel(f32, f32, f32, f32),
    /// Quadratic bezier (absolute).
    QuadTo(f32, f32, f32, f32),
    /// Quadratic bezier (relative).
    QuadToRel(f32, f32, f32, f32),
    /// Smooth quadratic bezier (absolute).
    SmoothQuadTo(f32, f32),
    /// Smooth quadratic bezier (relative).
    SmoothQuadToRel(f32, f32),
    /// Arc (absolute).
    ArcTo(f32, f32, f32, bool, bool, f32, f32),
    /// Arc (relative).
    ArcToRel(f32, f32, f32, bool, bool, f32, f32),
    /// Close path.
    Close,
}

/// Path element (<path>).
#[derive(Debug, Clone, Default)]
pub struct SvgPath {
    pub commands: Vec<PathCommand>,
    pub transform: Transform2D,
    pub style: SvgStyle,
}

impl SvgPath {
    /// Parse path data string.
    pub fn parse(d: &str) -> Vec<PathCommand> {
        let mut commands = Vec::new();
        let mut chars = d.chars().peekable();
        let mut current_cmd = ' ';

        while let Some(&c) = chars.peek() {
            if c.is_whitespace() || c == ',' {
                chars.next();
                continue;
            }

            if c.is_alphabetic() {
                current_cmd = c;
                chars.next();
                
                // Handle commands that don't take arguments immediately
                if current_cmd == 'Z' || current_cmd == 'z' {
                    commands.push(PathCommand::Close);
                }
                continue;
            }

            match current_cmd {
                'M' => {
                    if let (Some(x), Some(y)) = (parse_number(&mut chars), parse_number(&mut chars)) {
                        commands.push(PathCommand::MoveTo(x, y));
                        current_cmd = 'L'; // Subsequent coordinates are lines
                    }
                }
                'm' => {
                    if let (Some(x), Some(y)) = (parse_number(&mut chars), parse_number(&mut chars)) {
                        commands.push(PathCommand::MoveToRel(x, y));
                        current_cmd = 'l';
                    }
                }
                'L' => {
                    if let (Some(x), Some(y)) = (parse_number(&mut chars), parse_number(&mut chars)) {
                        commands.push(PathCommand::LineTo(x, y));
                    }
                }
                'l' => {
                    if let (Some(x), Some(y)) = (parse_number(&mut chars), parse_number(&mut chars)) {
                        commands.push(PathCommand::LineToRel(x, y));
                    }
                }
                'H' => {
                    if let Some(x) = parse_number(&mut chars) {
                        commands.push(PathCommand::HorizontalTo(x));
                    }
                }
                'h' => {
                    if let Some(x) = parse_number(&mut chars) {
                        commands.push(PathCommand::HorizontalToRel(x));
                    }
                }
                'V' => {
                    if let Some(y) = parse_number(&mut chars) {
                        commands.push(PathCommand::VerticalTo(y));
                    }
                }
                'v' => {
                    if let Some(y) = parse_number(&mut chars) {
                        commands.push(PathCommand::VerticalToRel(y));
                    }
                }
                'C' => {
                    if let (Some(x1), Some(y1), Some(x2), Some(y2), Some(x), Some(y)) = (
                        parse_number(&mut chars),
                        parse_number(&mut chars),
                        parse_number(&mut chars),
                        parse_number(&mut chars),
                        parse_number(&mut chars),
                        parse_number(&mut chars),
                    ) {
                        commands.push(PathCommand::CubicTo(x1, y1, x2, y2, x, y));
                    }
                }
                'c' => {
                    if let (Some(x1), Some(y1), Some(x2), Some(y2), Some(x), Some(y)) = (
                        parse_number(&mut chars),
                        parse_number(&mut chars),
                        parse_number(&mut chars),
                        parse_number(&mut chars),
                        parse_number(&mut chars),
                        parse_number(&mut chars),
                    ) {
                        commands.push(PathCommand::CubicToRel(x1, y1, x2, y2, x, y));
                    }
                }
                'S' => {
                    if let (Some(x2), Some(y2), Some(x), Some(y)) = (
                        parse_number(&mut chars),
                        parse_number(&mut chars),
                        parse_number(&mut chars),
                        parse_number(&mut chars),
                    ) {
                        commands.push(PathCommand::SmoothCubicTo(x2, y2, x, y));
                    }
                }
                's' => {
                    if let (Some(x2), Some(y2), Some(x), Some(y)) = (
                        parse_number(&mut chars),
                        parse_number(&mut chars),
                        parse_number(&mut chars),
                        parse_number(&mut chars),
                    ) {
                        commands.push(PathCommand::SmoothCubicToRel(x2, y2, x, y));
                    }
                }
                'Q' => {
                    if let (Some(x1), Some(y1), Some(x), Some(y)) = (
                        parse_number(&mut chars),
                        parse_number(&mut chars),
                        parse_number(&mut chars),
                        parse_number(&mut chars),
                    ) {
                        commands.push(PathCommand::QuadTo(x1, y1, x, y));
                    }
                }
                'q' => {
                    if let (Some(x1), Some(y1), Some(x), Some(y)) = (
                        parse_number(&mut chars),
                        parse_number(&mut chars),
                        parse_number(&mut chars),
                        parse_number(&mut chars),
                    ) {
                        commands.push(PathCommand::QuadToRel(x1, y1, x, y));
                    }
                }
                'T' => {
                    if let (Some(x), Some(y)) = (parse_number(&mut chars), parse_number(&mut chars)) {
                        commands.push(PathCommand::SmoothQuadTo(x, y));
                    }
                }
                't' => {
                    if let (Some(x), Some(y)) = (parse_number(&mut chars), parse_number(&mut chars)) {
                        commands.push(PathCommand::SmoothQuadToRel(x, y));
                    }
                }
                'A' => {
                    if let (Some(rx), Some(ry), Some(angle), Some(large_arc), Some(sweep), Some(x), Some(y)) = (
                        parse_number(&mut chars),
                        parse_number(&mut chars),
                        parse_number(&mut chars),
                        parse_flag(&mut chars),
                        parse_flag(&mut chars),
                        parse_number(&mut chars),
                        parse_number(&mut chars),
                    ) {
                        commands.push(PathCommand::ArcTo(rx, ry, angle, large_arc, sweep, x, y));
                    }
                }
                'a' => {
                    if let (Some(rx), Some(ry), Some(angle), Some(large_arc), Some(sweep), Some(x), Some(y)) = (
                        parse_number(&mut chars),
                        parse_number(&mut chars),
                        parse_number(&mut chars),
                        parse_flag(&mut chars),
                        parse_flag(&mut chars),
                        parse_number(&mut chars),
                        parse_number(&mut chars),
                    ) {
                        commands.push(PathCommand::ArcToRel(rx, ry, angle, large_arc, sweep, x, y));
                    }
                }
                'Z' | 'z' => {
                    commands.push(PathCommand::Close);
                    chars.next();
                }
                _ => {
                    chars.next();
                }
            }
        }

        commands
    }

    /// Convert path to line segments.
    pub fn to_line_segments(&self) -> Vec<Vec<(f32, f32)>> {
        let mut segments = Vec::new();
        let mut current_segment = Vec::new();
        let mut current_pos = (0.0_f32, 0.0_f32);
        let mut start_pos = (0.0_f32, 0.0_f32);
        let mut _last_control = None::<(f32, f32)>;

        for cmd in &self.commands {
            match cmd {
                PathCommand::MoveTo(x, y) => {
                    if !current_segment.is_empty() {
                        segments.push(std::mem::take(&mut current_segment));
                    }
                    current_pos = (*x, *y);
                    start_pos = current_pos;
                    current_segment.push(current_pos);
                    _last_control = None;
                }
                PathCommand::MoveToRel(dx, dy) => {
                    if !current_segment.is_empty() {
                        segments.push(std::mem::take(&mut current_segment));
                    }
                    current_pos = (current_pos.0 + dx, current_pos.1 + dy);
                    start_pos = current_pos;
                    current_segment.push(current_pos);
                    _last_control = None;
                }
                PathCommand::LineTo(x, y) => {
                    current_pos = (*x, *y);
                    current_segment.push(current_pos);
                    _last_control = None;
                }
                PathCommand::LineToRel(dx, dy) => {
                    current_pos = (current_pos.0 + dx, current_pos.1 + dy);
                    current_segment.push(current_pos);
                    _last_control = None;
                }
                PathCommand::HorizontalTo(x) => {
                    current_pos = (*x, current_pos.1);
                    current_segment.push(current_pos);
                    _last_control = None;
                }
                PathCommand::HorizontalToRel(dx) => {
                    current_pos = (current_pos.0 + dx, current_pos.1);
                    current_segment.push(current_pos);
                    _last_control = None;
                }
                PathCommand::VerticalTo(y) => {
                    current_pos = (current_pos.0, *y);
                    current_segment.push(current_pos);
                    _last_control = None;
                }
                PathCommand::VerticalToRel(dy) => {
                    current_pos = (current_pos.0, current_pos.1 + dy);
                    current_segment.push(current_pos);
                    _last_control = None;
                }
                PathCommand::CubicTo(x1, y1, x2, y2, x, y) => {
                    let points = cubic_bezier_points(current_pos, (*x1, *y1), (*x2, *y2), (*x, *y), 20);
                    current_segment.extend(points);
                    current_pos = (*x, *y);
                    _last_control = Some((*x2, *y2));
                }
                PathCommand::CubicToRel(dx1, dy1, dx2, dy2, dx, dy) => {
                    let (x1, y1) = (current_pos.0 + dx1, current_pos.1 + dy1);
                    let (x2, y2) = (current_pos.0 + dx2, current_pos.1 + dy2);
                    let (x, y) = (current_pos.0 + dx, current_pos.1 + dy);
                    let points = cubic_bezier_points(current_pos, (x1, y1), (x2, y2), (x, y), 20);
                    current_segment.extend(points);
                    current_pos = (x, y);
                    _last_control = Some((x2, y2));
                }
                PathCommand::QuadTo(x1, y1, x, y) => {
                    let points = quad_bezier_points(current_pos, (*x1, *y1), (*x, *y), 20);
                    current_segment.extend(points);
                    current_pos = (*x, *y);
                    _last_control = Some((*x1, *y1));
                }
                PathCommand::QuadToRel(dx1, dy1, dx, dy) => {
                    let (x1, y1) = (current_pos.0 + dx1, current_pos.1 + dy1);
                    let (x, y) = (current_pos.0 + dx, current_pos.1 + dy);
                    let points = quad_bezier_points(current_pos, (x1, y1), (x, y), 20);
                    current_segment.extend(points);
                    current_pos = (x, y);
                    _last_control = Some((x1, y1));
                }
                PathCommand::Close => {
                    if current_pos != start_pos {
                        current_segment.push(start_pos);
                    }
                    current_pos = start_pos;
                    if !current_segment.is_empty() {
                        segments.push(std::mem::take(&mut current_segment));
                    }
                    _last_control = None;
                }
                // Handle other commands as lines for simplicity
                _ => {
                    _last_control = None;
                }
            }
        }

        if !current_segment.is_empty() {
            segments.push(current_segment);
        }

        segments
    }

    /// Render the path.
    pub fn render(&self, parent_transform: &Transform2D, parent_style: &SvgStyle, commands: &mut Vec<DisplayCommand>) {
        let transform = parent_transform.multiply(&self.transform);
        let mut style = self.style.clone();
        style.inherit_from(parent_style);

        if !style.visibility {
            return;
        }

        let segments = self.to_line_segments();

        for segment in segments {
            let points: Vec<(f32, f32)> = segment
                .iter()
                .map(|(x, y)| transform.apply(*x, *y))
                .collect();

            if points.len() < 2 {
                continue;
            }

            // Fill (only for closed paths)
            if let Some(color) = style.fill.as_color() {
                if points.len() >= 3 {
                    let alpha = (color.a * style.fill_opacity * style.opacity).clamp(0.0, 1.0);
                    let fill_color = Color { a: alpha, ..color };
                    commands.push(DisplayCommand::FillPolygon {
                        points: points.clone(),
                        color: fill_color,
                    });
                }
            }

            // Stroke
            if let Some(color) = style.stroke.as_color() {
                let alpha = (color.a * style.stroke_opacity * style.opacity).clamp(0.0, 1.0);
                let stroke_color = Color { a: alpha, ..color };
                commands.push(DisplayCommand::Polyline {
                    points,
                    color: stroke_color,
                    width: style.stroke_width,
                });
            }
        }
    }
}

/// Text element (<text>).
#[derive(Debug, Clone, Default)]
pub struct SvgText {
    pub x: f32,
    pub y: f32,
    pub content: String,
    pub font_family: String,
    pub font_size: f32,
    pub transform: Transform2D,
    pub style: SvgStyle,
}

impl SvgText {
    /// Render the text.
    pub fn render(&self, parent_transform: &Transform2D, parent_style: &SvgStyle, commands: &mut Vec<DisplayCommand>) {
        let transform = parent_transform.multiply(&self.transform);
        let mut style = self.style.clone();
        style.inherit_from(parent_style);

        if !style.visibility || self.content.is_empty() {
            return;
        }

        let (x, y) = transform.apply(self.x, self.y);

        if let Some(color) = style.fill.as_color() {
            let alpha = (color.a * style.fill_opacity * style.opacity).clamp(0.0, 1.0);
            let text_color = Color { a: alpha, ..color };
            commands.push(DisplayCommand::Text {
                x,
                y,
                text: self.content.clone(),
                font_family: if self.font_family.is_empty() { "sans-serif".to_string() } else { self.font_family.clone() },
                font_size: self.font_size,
                color: text_color,
                font_weight: 400, // Normal
                font_style: 0, // Normal
                gradient: None,
                gradient_rect: Rect::default(),
            });
        }
    }
}

/// Use element (<use>).
#[derive(Debug, Clone, Default)]
pub struct SvgUse {
    pub href: String,
    pub x: f32,
    pub y: f32,
    pub width: Option<f32>,
    pub height: Option<f32>,
    pub transform: Transform2D,
}

// ==================== Helper Functions ====================

/// Parse a number from character iterator.
fn parse_number<I: Iterator<Item = char>>(chars: &mut std::iter::Peekable<I>) -> Option<f32> {
    // Skip whitespace and commas
    while chars.peek().is_some_and(|c| c.is_whitespace() || *c == ',') {
        chars.next();
    }

    let mut s = String::new();
    let mut has_dot = false;
    let mut has_exp = false;

    // Handle sign
    if chars.peek().is_some_and(|c| *c == '-' || *c == '+') {
        s.push(chars.next().unwrap());
    }

    while let Some(&c) = chars.peek() {
        if c.is_ascii_digit() {
            s.push(chars.next().unwrap());
        } else if c == '.' && !has_dot {
            has_dot = true;
            s.push(chars.next().unwrap());
        } else if (c == 'e' || c == 'E') && !has_exp {
            has_exp = true;
            s.push(chars.next().unwrap());
            if chars.peek().is_some_and(|c| *c == '-' || *c == '+') {
                s.push(chars.next().unwrap());
            }
        } else {
            break;
        }
    }

    if s.is_empty() || s == "-" || s == "+" {
        None
    } else {
        s.parse().ok()
    }
}

/// Parse a flag (0 or 1).
fn parse_flag<I: Iterator<Item = char>>(chars: &mut std::iter::Peekable<I>) -> Option<bool> {
    while chars.peek().is_some_and(|c| c.is_whitespace() || *c == ',') {
        chars.next();
    }

    match chars.next() {
        Some('0') => Some(false),
        Some('1') => Some(true),
        _ => None,
    }
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

/// Extract attribute value from XML tag.
fn extract_attr(tag: &str, name: &str) -> Option<String> {
    let pattern = format!("{}=", name);
    if let Some(start) = tag.find(&pattern) {
        let rest = &tag[start + pattern.len()..];
        let quote = rest.chars().next()?;
        if quote == '"' || quote == '\'' {
            let end = rest[1..].find(quote)?;
            return Some(rest[1..1 + end].to_string());
        }
    }
    None
}

/// Parse SVG color.
fn parse_svg_color(s: &str) -> Option<Color> {
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
            _ => None,
        };
    }

    // RGB/RGBA functions
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
        "pink" => Some(Color::from_rgb(255, 192, 203)),
        "brown" => Some(Color::from_rgb(165, 42, 42)),
        "transparent" => Some(Color::TRANSPARENT),
        _ => None,
    }
}

/// Parse SVG content into elements.
fn parse_svg_content(xml: &str) -> Result<SvgElement, SvgError> {
    let mut group = SvgGroup::new();
    
    // Simple element parsing
    let mut pos = 0;
    while pos < xml.len() {
        if let Some(tag_start) = xml[pos..].find('<') {
            let tag_start = pos + tag_start;
            
            // Skip comments
            if xml[tag_start..].starts_with("<!--") {
                if let Some(end) = xml[tag_start..].find("-->") {
                    pos = tag_start + end + 3;
                    continue;
                }
            }
            
            // Skip closing tags
            if xml[tag_start..].starts_with("</") {
                if let Some(end) = xml[tag_start..].find('>') {
                    pos = tag_start + end + 1;
                    continue;
                }
            }
            
            // Find tag end
            if let Some(tag_end) = xml[tag_start..].find('>') {
                let tag = &xml[tag_start..tag_start + tag_end + 1];
                
                // Parse element
                if let Some(element) = parse_element(tag) {
                    group.children.push(element);
                }
                
                pos = tag_start + tag_end + 1;
            } else {
                break;
            }
        } else {
            break;
        }
    }

    Ok(SvgElement::Group(group))
}

/// Parse a single SVG element.
fn parse_element(tag: &str) -> Option<SvgElement> {
    let tag = tag.trim_start_matches('<').trim_end_matches('>').trim_end_matches('/');
    let parts: Vec<&str> = tag.splitn(2, char::is_whitespace).collect();
    let name = parts.first()?.to_lowercase();
    let attrs_str = parts.get(1).unwrap_or(&"");
    
    let mut attrs = HashMap::new();
    let mut attr_str = *attrs_str;
    while let Some((key, value, rest)) = parse_attr(attr_str) {
        attrs.insert(key.to_lowercase(), value);
        attr_str = rest;
    }

    match name.as_str() {
        "rect" => {
            let mut rect = SvgRect::default();
            rect.x = attrs.get("x").and_then(|s| s.parse().ok()).unwrap_or(0.0);
            rect.y = attrs.get("y").and_then(|s| s.parse().ok()).unwrap_or(0.0);
            rect.width = attrs.get("width").and_then(|s| SvgLength::parse(s)).map(|l| l.to_px(0.0)).unwrap_or(0.0);
            rect.height = attrs.get("height").and_then(|s| SvgLength::parse(s)).map(|l| l.to_px(0.0)).unwrap_or(0.0);
            rect.rx = attrs.get("rx").and_then(|s| s.parse().ok()).unwrap_or(0.0);
            rect.ry = attrs.get("ry").and_then(|s| s.parse().ok()).unwrap_or(rect.rx);
            if let Some(t) = attrs.get("transform") {
                rect.transform = Transform2D::parse(t);
            }
            rect.style.parse_attributes(&attrs);
            Some(SvgElement::Rect(rect))
        }
        "circle" => {
            let mut circle = SvgCircle::default();
            circle.cx = attrs.get("cx").and_then(|s| s.parse().ok()).unwrap_or(0.0);
            circle.cy = attrs.get("cy").and_then(|s| s.parse().ok()).unwrap_or(0.0);
            circle.r = attrs.get("r").and_then(|s| s.parse().ok()).unwrap_or(0.0);
            if let Some(t) = attrs.get("transform") {
                circle.transform = Transform2D::parse(t);
            }
            circle.style.parse_attributes(&attrs);
            Some(SvgElement::Circle(circle))
        }
        "ellipse" => {
            let mut ellipse = SvgEllipse::default();
            ellipse.cx = attrs.get("cx").and_then(|s| s.parse().ok()).unwrap_or(0.0);
            ellipse.cy = attrs.get("cy").and_then(|s| s.parse().ok()).unwrap_or(0.0);
            ellipse.rx = attrs.get("rx").and_then(|s| s.parse().ok()).unwrap_or(0.0);
            ellipse.ry = attrs.get("ry").and_then(|s| s.parse().ok()).unwrap_or(0.0);
            if let Some(t) = attrs.get("transform") {
                ellipse.transform = Transform2D::parse(t);
            }
            ellipse.style.parse_attributes(&attrs);
            Some(SvgElement::Ellipse(ellipse))
        }
        "line" => {
            let mut line = SvgLine::default();
            line.x1 = attrs.get("x1").and_then(|s| s.parse().ok()).unwrap_or(0.0);
            line.y1 = attrs.get("y1").and_then(|s| s.parse().ok()).unwrap_or(0.0);
            line.x2 = attrs.get("x2").and_then(|s| s.parse().ok()).unwrap_or(0.0);
            line.y2 = attrs.get("y2").and_then(|s| s.parse().ok()).unwrap_or(0.0);
            if let Some(t) = attrs.get("transform") {
                line.transform = Transform2D::parse(t);
            }
            line.style.parse_attributes(&attrs);
            Some(SvgElement::Line(line))
        }
        "path" => {
            let mut path = SvgPath::default();
            if let Some(d) = attrs.get("d") {
                path.commands = SvgPath::parse(d);
            }
            if let Some(t) = attrs.get("transform") {
                path.transform = Transform2D::parse(t);
            }
            path.style.parse_attributes(&attrs);
            Some(SvgElement::Path(path))
        }
        "polyline" => {
            let mut polyline = SvgPolyline::default();
            if let Some(points_str) = attrs.get("points") {
                polyline.points = parse_points(points_str);
            }
            if let Some(t) = attrs.get("transform") {
                polyline.transform = Transform2D::parse(t);
            }
            polyline.style.parse_attributes(&attrs);
            Some(SvgElement::Polyline(polyline))
        }
        "polygon" => {
            let mut polygon = SvgPolygon::default();
            if let Some(points_str) = attrs.get("points") {
                polygon.points = parse_points(points_str);
            }
            if let Some(t) = attrs.get("transform") {
                polygon.transform = Transform2D::parse(t);
            }
            polygon.style.parse_attributes(&attrs);
            Some(SvgElement::Polygon(polygon))
        }
        _ => None,
    }
}

/// Parse a single attribute.
fn parse_attr(s: &str) -> Option<(String, String, &str)> {
    let s = s.trim_start();
    if s.is_empty() {
        return None;
    }
    
    // Find equals sign
    let eq = s.find('=')?;
    let key = s[..eq].trim();
    let rest = s[eq + 1..].trim_start();
    
    // Find quoted value
    let quote = rest.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    
    let value_start = 1;
    let value_end = rest[value_start..].find(quote)? + value_start;
    let value = &rest[value_start..value_end];
    
    Some((key.to_string(), value.to_string(), &rest[value_end + 1..]))
}

/// Parse points attribute for polyline/polygon.
fn parse_points(s: &str) -> Vec<(f32, f32)> {
    let mut points = Vec::new();
    let numbers: Vec<f32> = s
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter_map(|p| p.trim().parse().ok())
        .collect();
    
    for chunk in numbers.chunks(2) {
        if chunk.len() == 2 {
            points.push((chunk[0], chunk[1]));
        }
    }
    
    points
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_transform_identity() {
        let t = Transform2D::identity();
        let (x, y) = t.apply(10.0, 20.0);
        assert_eq!(x, 10.0);
        assert_eq!(y, 20.0);
    }

    #[test]
    fn test_transform_translate() {
        let t = Transform2D::identity().translate(5.0, 10.0);
        let (x, y) = t.apply(10.0, 20.0);
        assert_eq!(x, 15.0);
        assert_eq!(y, 30.0);
    }

    #[test]
    fn test_transform_scale() {
        let t = Transform2D::identity().scale(2.0, 3.0);
        let (x, y) = t.apply(10.0, 20.0);
        assert_eq!(x, 20.0);
        assert_eq!(y, 60.0);
    }

    #[test]
    fn test_transform_parse() {
        let t = Transform2D::parse("translate(10, 20)");
        let (x, y) = t.apply(0.0, 0.0);
        assert_eq!(x, 10.0);
        assert_eq!(y, 20.0);

        let t = Transform2D::parse("scale(2)");
        let (x, y) = t.apply(5.0, 5.0);
        assert_eq!(x, 10.0);
        assert_eq!(y, 10.0);
    }

    #[test]
    fn test_viewbox_parse() {
        let vb = ViewBox::parse("0 0 100 50").unwrap();
        assert_eq!(vb.min_x, 0.0);
        assert_eq!(vb.min_y, 0.0);
        assert_eq!(vb.width, 100.0);
        assert_eq!(vb.height, 50.0);

        let vb = ViewBox::parse("10,20,30,40").unwrap();
        assert_eq!(vb.min_x, 10.0);
        assert_eq!(vb.min_y, 20.0);
    }

    #[test]
    fn test_svg_length_parse() {
        assert!(matches!(SvgLength::parse("100"), Some(SvgLength::User(100.0))));
        assert!(matches!(SvgLength::parse("50px"), Some(SvgLength::Px(50.0))));
        assert!(matches!(SvgLength::parse("50%"), Some(SvgLength::Percent(50.0))));
    }

    #[test]
    fn test_paint_parse() {
        assert!(matches!(Paint::parse("none"), Paint::None));
        assert!(matches!(Paint::parse("#ff0000"), Paint::Color(_)));
        assert!(matches!(Paint::parse("url(#gradient)"), Paint::Url(_)));
    }

    #[test]
    fn test_path_parse() {
        let commands = SvgPath::parse("M 10 20 L 30 40 Z");
        assert_eq!(commands.len(), 3);
        assert!(matches!(commands[0], PathCommand::MoveTo(10.0, 20.0)));
        assert!(matches!(commands[1], PathCommand::LineTo(30.0, 40.0)));
        assert!(matches!(commands[2], PathCommand::Close));
    }

    #[test]
    fn test_path_bezier() {
        let commands = SvgPath::parse("M 0 0 C 10 20 30 40 50 60");
        assert_eq!(commands.len(), 2);
        assert!(matches!(commands[1], PathCommand::CubicTo(10.0, 20.0, 30.0, 40.0, 50.0, 60.0)));
    }

    #[test]
    fn test_parse_color() {
        let color = parse_svg_color("#ff0000").unwrap();
        assert_eq!(color.r, 255);
        assert_eq!(color.g, 0);
        assert_eq!(color.b, 0);

        let color = parse_svg_color("#f00").unwrap();
        assert_eq!(color.r, 255);
        assert_eq!(color.g, 0);
        assert_eq!(color.b, 0);

        let color = parse_svg_color("blue").unwrap();
        assert_eq!(color.r, 0);
        assert_eq!(color.g, 0);
        assert_eq!(color.b, 255);
    }

    #[test]
    fn test_parse_points() {
        let points = parse_points("10,20 30,40 50,60");
        assert_eq!(points.len(), 3);
        assert_eq!(points[0], (10.0, 20.0));
        assert_eq!(points[1], (30.0, 40.0));
        assert_eq!(points[2], (50.0, 60.0));
    }

    #[test]
    fn test_svg_document_parse() {
        let svg = r#"<svg viewBox="0 0 100 100"><rect x="10" y="10" width="80" height="80" fill="red"/></svg>"#;
        let doc = SvgDocument::parse(svg).unwrap();
        assert!(doc.view_box.is_some());
    }
}

