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
        let mut root_style = SvgStyle::default();
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

                // Root presentation attributes seed every shape's style: the
                // parser is FLAT (no nesting), so this is the only way
                // `<svg fill="none" stroke="currentColor">` — the standard
                // icon idiom — reaches its shapes. Without it the circles of
                // every stroke-only icon painted a default-black disc.
                let mut root_attrs = HashMap::new();
                let mut attr_str = attrs
                    .trim_start_matches("<svg")
                    .trim_end_matches('>')
                    .trim_end_matches('/');
                while let Some((key, value, rest)) = parse_attr(attr_str) {
                    root_attrs.insert(key.to_lowercase(), value);
                    attr_str = rest;
                }
                root_style.parse_attributes(&root_attrs);
            }
        }

        // Parse elements (simplified)
        doc.root = parse_svg_content(xml, &root_style)?;

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

    /// Render to display commands with `currentColor` resolving to black —
    /// the initial value of CSS `color`, which is what a standalone SVG
    /// document (an `<img src=*.svg>`) sees.
    pub fn render(&self, x: f32, y: f32, width: f32, height: f32) -> Vec<DisplayCommand> {
        self.render_with_color(x, y, width, height, Color::BLACK)
    }

    /// Render to display commands with `currentColor` resolving to
    /// `current_color` — an inline `<svg>` inherits the CSS `color` of the
    /// element it sits in, and every `fill="currentColor"` /
    /// `stroke="currentColor"` icon on a real page takes its color from
    /// there. The paint keyword lives on the parsed shapes; only the value
    /// it resolves to is a render-time input.
    pub fn render_with_color(
        &self,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        current_color: Color,
    ) -> Vec<DisplayCommand> {
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

        let base = SvgStyle {
            current_color,
            ..SvgStyle::default()
        };
        self.root.render(&transform, &base, &mut commands);

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

    /// Get color if this is a solid color, with `currentColor` taken as
    /// black (the initial CSS `color`). Prefer [`Paint::resolve`] wherever
    /// the surrounding CSS color is known.
    pub fn as_color(&self) -> Option<Color> {
        self.resolve(Color::BLACK)
    }

    /// Get the solid color this paint draws with, resolving `currentColor`
    /// to `current_color`. `None` for `none` and unresolved `url()` paints.
    pub fn resolve(&self, current_color: Color) -> Option<Color> {
        match self {
            Paint::Color(c) => Some(*c),
            Paint::CurrentColor => Some(current_color),
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
    /// The CSS `color` in force where this SVG is painted — what
    /// `currentColor` resolves to. Render context, not an authored SVG
    /// property: it is seeded by the document's render call and inherited
    /// unconditionally down the element tree.
    pub current_color: Color,
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
            current_color: Color::BLACK,
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
        // The CSS color is context, never authored on a shape: always the
        // parent's, so the render call's value reaches every element.
        self.current_color = parent.current_color;
    }

    /// The solid fill color, with `currentColor` resolved.
    pub fn fill_color(&self) -> Option<Color> {
        self.fill.resolve(self.current_color)
    }

    /// The solid stroke color, with `currentColor` resolved.
    pub fn stroke_color(&self) -> Option<Color> {
        self.stroke.resolve(self.current_color)
    }

    /// Parse style attributes.
    pub fn parse_attributes(&mut self, attrs: &HashMap<String, String>) {
        // An inline `style="fill: #fbf1e2"` sets the same properties as the
        // presentation attributes and wins over them (SVG 2 §6.8). linkedin's
        // hero paints all 148 of its shapes this way; without it every one
        // fell back to the initial black fill.
        if let Some(style) = attrs.get("style") {
            let mut merged = attrs.clone();
            merged.remove("style");
            for decl in style.split(';') {
                if let Some((name, value)) = decl.split_once(':') {
                    let value = value.trim();
                    let value = value
                        .strip_suffix("!important")
                        .map(str::trim_end)
                        .unwrap_or(value);
                    merged.insert(name.trim().to_ascii_lowercase(), value.to_string());
                }
            }
            return self.parse_attributes(&merged);
        }
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
        if let Some(rule) = attrs.get("fill-rule") {
            self.fill_rule = match rule.trim() {
                "evenodd" => FillRule::EvenOdd,
                _ => FillRule::NonZero,
            };
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

// ==================== Fill tessellation ====================

/// Pair checks spent looking for edge crossings in one fill. Past it the
/// fill still paints; a self-crossing edge pair just isn't split exactly.
const MAX_CROSSING_CHECKS: usize = 2_000_000;

/// Fill closed contours (SVG 2 §13.4.1 `fill-rule`) as convex pieces.
///
/// The renderer fills a `FillPolygon` as a triangle fan, which is exact
/// only for one convex polygon. So a lone convex contour (rects, circles,
/// most triangles) goes through as-is, and anything else (concave outlines,
/// holes, self-crossings, several subpaths) is swept into horizontal
/// trapezoids, each inside under `rule`.
fn fill_contours(
    contours: &[Vec<(f32, f32)>],
    rule: FillRule,
    color: Color,
    commands: &mut Vec<DisplayCommand>,
) {
    let contours: Vec<Vec<(f32, f32)>> = contours
        .iter()
        .map(|c| {
            let mut pts: Vec<(f32, f32)> = Vec::with_capacity(c.len());
            for &p in c {
                if !(p.0.is_finite() && p.1.is_finite()) {
                    continue;
                }
                if pts.last() != Some(&p) {
                    pts.push(p);
                }
            }
            while pts.len() > 1 && pts.first() == pts.last() {
                pts.pop();
            }
            pts
        })
        .filter(|c| c.len() >= 3)
        .collect();

    match contours.as_slice() {
        [] => return,
        [only] if is_convex(only) => {
            commands.push(DisplayCommand::FillPolygon { points: only.clone(), color });
            return;
        }
        _ => {}
    }

    // Edges, top to bottom, with the winding each one adds when crossed.
    struct Edge {
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
        dir: i32,
    }
    impl Edge {
        fn x_at(&self, y: f32) -> f32 {
            self.x0 + (self.x1 - self.x0) * (y - self.y0) / (self.y1 - self.y0)
        }
    }
    let mut edges: Vec<Edge> = Vec::new();
    for c in &contours {
        for i in 0..c.len() {
            let (a, b) = (c[i], c[(i + 1) % c.len()]);
            if a.1 == b.1 {
                continue;
            }
            edges.push(if a.1 < b.1 {
                Edge { x0: a.0, y0: a.1, x1: b.0, y1: b.1, dir: 1 }
            } else {
                Edge { x0: b.0, y0: b.1, x1: a.0, y1: a.1, dir: -1 }
            });
        }
    }
    if edges.is_empty() {
        return;
    }
    edges.sort_by(|a, b| a.y0.total_cmp(&b.y0));

    // Band boundaries: every vertex y, plus every y where two edges cross,
    // so inside one band the edges keep their left-to-right order.
    let mut ys: Vec<f32> = edges.iter().flat_map(|e| [e.y0, e.y1]).collect();
    let mut checks = 0usize;
    'crossings: for i in 0..edges.len() {
        let a = &edges[i];
        for b in &edges[i + 1..] {
            if b.y0 >= a.y1 {
                break;
            }
            checks += 1;
            if checks > MAX_CROSSING_CHECKS {
                break 'crossings;
            }
            let (lo, hi) = (a.y0.max(b.y0), a.y1.min(b.y1));
            if hi <= lo {
                continue;
            }
            let (d_lo, d_hi) = (a.x_at(lo) - b.x_at(lo), a.x_at(hi) - b.x_at(hi));
            if (d_lo < 0.0 && d_hi > 0.0) || (d_lo > 0.0 && d_hi < 0.0) {
                ys.push(lo + (hi - lo) * d_lo / (d_lo - d_hi));
            }
        }
    }
    ys.sort_by(f32::total_cmp);
    ys.dedup();

    let mut next = 0usize;
    let mut active: Vec<usize> = Vec::new();
    let mut crossing: Vec<(f32, f32, f32, i32)> = Vec::new();
    for band in ys.windows(2) {
        let (top, bottom) = (band[0], band[1]);
        while next < edges.len() && edges[next].y0 <= top {
            active.push(next);
            next += 1;
        }
        active.retain(|&i| edges[i].y1 > top);
        if bottom - top < 1e-4 {
            continue;
        }
        let mid = (top + bottom) * 0.5;
        crossing.clear();
        crossing.extend(active.iter().map(|&i| {
            let e = &edges[i];
            (e.x_at(mid), e.x_at(top), e.x_at(bottom), e.dir)
        }));
        crossing.sort_by(|a, b| a.0.total_cmp(&b.0));

        let mut winding = 0;
        let mut left: Option<(f32, f32)> = None;
        for &(_, x_top, x_bottom, dir) in &crossing {
            winding += dir;
            let inside = match rule {
                FillRule::NonZero => winding != 0,
                FillRule::EvenOdd => winding % 2 != 0,
            };
            match (left, inside) {
                (None, true) => left = Some((x_top, x_bottom)),
                (Some((l_top, l_bottom)), false) => {
                    commands.push(DisplayCommand::FillPolygon {
                        points: vec![(l_top, top), (x_top, top), (x_bottom, bottom), (l_bottom, bottom)],
                        color,
                    });
                    left = None;
                }
                _ => {}
            }
        }
    }
}

/// A simple convex polygon: every turn the same way, and one full turn in
/// total (a pentagram turns one way too, but twice around).
fn is_convex(points: &[(f32, f32)]) -> bool {
    let n = points.len();
    let mut sign = 0.0f32;
    let mut turning = 0.0f32;
    for i in 0..n {
        let (a, b, c) = (points[i], points[(i + 1) % n], points[(i + 2) % n]);
        let (u, v) = ((b.0 - a.0, b.1 - a.1), (c.0 - b.0, c.1 - b.1));
        let cross = u.0 * v.1 - u.1 * v.0;
        if cross != 0.0 {
            if sign != 0.0 && cross.signum() != sign {
                return false;
            }
            sign = cross.signum();
        }
        turning += cross.atan2(u.0 * v.0 + u.1 * v.1);
    }
    turning.abs() < 3.0 * std::f32::consts::PI
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
        if let Some(color) = style.fill_color() {
            let alpha = (color.a * style.fill_opacity * style.opacity).clamp(0.0, 1.0);
            let fill_color = Color { a: alpha, ..color };
            commands.push(DisplayCommand::FillRect { rect: rect.clone(), color: fill_color });
        }

        // Stroke
        if let Some(color) = style.stroke_color() {
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
        if let Some(color) = style.fill_color() {
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
        if let Some(color) = style.stroke_color() {
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

        if let Some(color) = style.fill_color() {
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

        if let Some(color) = style.stroke_color() {
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

        if let Some(color) = style.stroke_color() {
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

        if let Some(color) = style.fill_color() {
            let alpha = (color.a * style.fill_opacity * style.opacity).clamp(0.0, 1.0);
            let fill_color = Color { a: alpha, ..color };
            fill_contours(std::slice::from_ref(&points), style.fill_rule, fill_color, commands);
        }

        if let Some(color) = style.stroke_color() {
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
        // The previous segment's second control point, for S/s (after C/S)
        // and T/t (after Q/T) to reflect. Any other command clears both.
        let mut last_cubic = None::<(f32, f32)>;
        let mut last_quad = None::<(f32, f32)>;

        for cmd in &self.commands {
            match cmd {
                PathCommand::MoveTo(x, y) => {
                    if !current_segment.is_empty() {
                        segments.push(std::mem::take(&mut current_segment));
                    }
                    current_pos = (*x, *y);
                    start_pos = current_pos;
                    current_segment.push(current_pos);
                    last_cubic = None;
                    last_quad = None;
                }
                PathCommand::MoveToRel(dx, dy) => {
                    if !current_segment.is_empty() {
                        segments.push(std::mem::take(&mut current_segment));
                    }
                    current_pos = (current_pos.0 + dx, current_pos.1 + dy);
                    start_pos = current_pos;
                    current_segment.push(current_pos);
                    last_cubic = None;
                    last_quad = None;
                }
                PathCommand::LineTo(x, y) => {
                    current_pos = (*x, *y);
                    current_segment.push(current_pos);
                    last_cubic = None;
                    last_quad = None;
                }
                PathCommand::LineToRel(dx, dy) => {
                    current_pos = (current_pos.0 + dx, current_pos.1 + dy);
                    current_segment.push(current_pos);
                    last_cubic = None;
                    last_quad = None;
                }
                PathCommand::HorizontalTo(x) => {
                    current_pos = (*x, current_pos.1);
                    current_segment.push(current_pos);
                    last_cubic = None;
                    last_quad = None;
                }
                PathCommand::HorizontalToRel(dx) => {
                    current_pos = (current_pos.0 + dx, current_pos.1);
                    current_segment.push(current_pos);
                    last_cubic = None;
                    last_quad = None;
                }
                PathCommand::VerticalTo(y) => {
                    current_pos = (current_pos.0, *y);
                    current_segment.push(current_pos);
                    last_cubic = None;
                    last_quad = None;
                }
                PathCommand::VerticalToRel(dy) => {
                    current_pos = (current_pos.0, current_pos.1 + dy);
                    current_segment.push(current_pos);
                    last_cubic = None;
                    last_quad = None;
                }
                PathCommand::CubicTo(x1, y1, x2, y2, x, y) => {
                    let points = cubic_bezier_points(current_pos, (*x1, *y1), (*x2, *y2), (*x, *y), 20);
                    current_segment.extend(points);
                    current_pos = (*x, *y);
                    last_cubic = Some((*x2, *y2));
                    last_quad = None;
                }
                PathCommand::CubicToRel(dx1, dy1, dx2, dy2, dx, dy) => {
                    let (x1, y1) = (current_pos.0 + dx1, current_pos.1 + dy1);
                    let (x2, y2) = (current_pos.0 + dx2, current_pos.1 + dy2);
                    let (x, y) = (current_pos.0 + dx, current_pos.1 + dy);
                    let points = cubic_bezier_points(current_pos, (x1, y1), (x2, y2), (x, y), 20);
                    current_segment.extend(points);
                    current_pos = (x, y);
                    last_cubic = Some((x2, y2));
                    last_quad = None;
                }
                PathCommand::QuadTo(x1, y1, x, y) => {
                    let points = quad_bezier_points(current_pos, (*x1, *y1), (*x, *y), 20);
                    current_segment.extend(points);
                    current_pos = (*x, *y);
                    last_quad = Some((*x1, *y1));
                    last_cubic = None;
                }
                PathCommand::QuadToRel(dx1, dy1, dx, dy) => {
                    let (x1, y1) = (current_pos.0 + dx1, current_pos.1 + dy1);
                    let (x, y) = (current_pos.0 + dx, current_pos.1 + dy);
                    let points = quad_bezier_points(current_pos, (x1, y1), (x, y), 20);
                    current_segment.extend(points);
                    current_pos = (x, y);
                    last_quad = Some((x1, y1));
                    last_cubic = None;
                }
                PathCommand::Close => {
                    if current_pos != start_pos {
                        current_segment.push(start_pos);
                    }
                    current_pos = start_pos;
                    if !current_segment.is_empty() {
                        segments.push(std::mem::take(&mut current_segment));
                    }
                    last_cubic = None;
                    last_quad = None;
                }
                // S/s and T/t: the first control point is the reflection of
                // the previous segment's (SVG 1.1 §8.3.6, §8.3.7), or the
                // current point when the previous command wasn't the same kind.
                PathCommand::SmoothCubicTo(..) | PathCommand::SmoothCubicToRel(..) => {
                    let (x2, y2, x, y) = match cmd {
                        PathCommand::SmoothCubicTo(x2, y2, x, y) => (*x2, *y2, *x, *y),
                        PathCommand::SmoothCubicToRel(dx2, dy2, dx, dy) => (
                            current_pos.0 + dx2,
                            current_pos.1 + dy2,
                            current_pos.0 + dx,
                            current_pos.1 + dy,
                        ),
                        _ => unreachable!(),
                    };
                    let c1 = reflect(last_cubic, current_pos);
                    current_segment.extend(cubic_bezier_points(current_pos, c1, (x2, y2), (x, y), 20));
                    current_pos = (x, y);
                    last_cubic = Some((x2, y2));
                    last_quad = None;
                }
                PathCommand::SmoothQuadTo(..) | PathCommand::SmoothQuadToRel(..) => {
                    let (x, y) = match cmd {
                        PathCommand::SmoothQuadTo(x, y) => (*x, *y),
                        PathCommand::SmoothQuadToRel(dx, dy) => (current_pos.0 + dx, current_pos.1 + dy),
                        _ => unreachable!(),
                    };
                    let c1 = reflect(last_quad, current_pos);
                    current_segment.extend(quad_bezier_points(current_pos, c1, (x, y), 20));
                    current_pos = (x, y);
                    last_quad = Some(c1);
                    last_cubic = None;
                }
                PathCommand::ArcTo(..) | PathCommand::ArcToRel(..) => {
                    let (rx, ry, angle, large_arc, sweep, x, y) = match cmd {
                        PathCommand::ArcTo(rx, ry, a, l, s, x, y) => (*rx, *ry, *a, *l, *s, *x, *y),
                        PathCommand::ArcToRel(rx, ry, a, l, s, dx, dy) => {
                            (*rx, *ry, *a, *l, *s, current_pos.0 + dx, current_pos.1 + dy)
                        }
                        _ => unreachable!(),
                    };
                    current_segment.extend(arc_points(current_pos, rx, ry, angle, large_arc, sweep, (x, y)));
                    current_pos = (x, y);
                    last_cubic = None;
                    last_quad = None;
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

        let subpaths: Vec<Vec<(f32, f32)>> = self
            .to_line_segments()
            .into_iter()
            .map(|segment| segment.iter().map(|(x, y)| transform.apply(*x, *y)).collect())
            .collect();

        // Fill: every subpath is one contour of a single fill, so a hole
        // (a reversed inner subpath, or any inner one under evenodd) stays
        // empty and overlapping subpaths don't double their alpha.
        if let Some(color) = style.fill_color() {
            let alpha = (color.a * style.fill_opacity * style.opacity).clamp(0.0, 1.0);
            let fill_color = Color { a: alpha, ..color };
            fill_contours(&subpaths, style.fill_rule, fill_color, commands);
        }

        for points in subpaths {
            if points.len() < 2 {
                continue;
            }

            // Stroke
            if let Some(color) = style.stroke_color() {
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

/// Horizontal anchoring of a text run (`text-anchor`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TextAnchor {
    #[default]
    Start,
    Middle,
    End,
}

/// Text element (<text>).
#[derive(Debug, Clone, Default)]
pub struct SvgText {
    pub x: f32,
    pub y: f32,
    pub content: String,
    pub font_family: String,
    pub font_size: f32,
    pub anchor: TextAnchor,
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

        let font_size = if self.font_size > 0.0 { self.font_size } else { 16.0 };
        let font_family = if self.font_family.is_empty() {
            "sans-serif".to_string()
        } else {
            self.font_family.clone()
        };

        // text-anchor offsets the run in LOCAL units before the transform:
        // the shaper measures at the local font size, so the offset scales
        // with the viewBox mapping like every other coordinate.
        let anchor_dx = match self.anchor {
            TextAnchor::Start => 0.0,
            TextAnchor::Middle | TextAnchor::End => {
                let width = rustkit_layout::measure_text_advanced(
                    &self.content,
                    &font_family,
                    font_size,
                    rustkit_css::FontWeight(400),
                    rustkit_css::FontStyle::Normal,
                )
                .width;
                if self.anchor == TextAnchor::Middle {
                    -width / 2.0
                } else {
                    -width
                }
            }
        };

        let (x, y) = transform.apply(self.x + anchor_dx, self.y);
        // Uniform scale (a == d for the viewBox mapping); fonts don't
        // anisotropically scale here.
        let scaled_font_size = font_size * transform.a;

        if let Some(color) = style.fill_color() {
            let alpha = (color.a * style.fill_opacity * style.opacity).clamp(0.0, 1.0);
            let text_color = Color { a: alpha, ..color };
            commands.push(DisplayCommand::Text {
                x,
                y,
                text: self.content.clone(),
                font_family,
                font_size: scaled_font_size,
                color: text_color,
                font_weight: 400, // Normal
                font_style: 0, // Normal
                // ADVANCE CONTRACT: svg <text> is a legacy path — it has no
                // layout shaper of its own, so paint falls back to its own
                // advances (the None arm the contract documents).
                advances: None,
                // SVG y is the BASELINE; the renderer computes
                // baseline = y + ascent, so a zero ascent hands it the
                // baseline directly instead of a run-top.
                ascent: Some(0.0),
                run: None,
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

/// The reflection of `control` about `about`; `about` itself when there is no
/// control to reflect (S/T after a command of another kind).
fn reflect(control: Option<(f32, f32)>, about: (f32, f32)) -> (f32, f32) {
    match control {
        Some((cx, cy)) => (2.0 * about.0 - cx, 2.0 * about.1 - cy),
        None => about,
    }
}

/// Points along an SVG elliptical arc from `p0` to `p1`, excluding `p0` and
/// ending exactly on `p1` (SVG 1.1 appendix F.6: endpoint to center
/// parameterization, with out-of-range radii scaled up and a zero radius
/// meaning a straight line).
fn arc_points(
    p0: (f32, f32),
    rx: f32,
    ry: f32,
    x_axis_rotation_deg: f32,
    large_arc: bool,
    sweep: bool,
    p1: (f32, f32),
) -> Vec<(f32, f32)> {
    if p0 == p1 {
        return Vec::new();
    }
    let (mut rx, mut ry) = (f64::from(rx.abs()), f64::from(ry.abs()));
    if rx == 0.0 || ry == 0.0 {
        return vec![p1];
    }
    let (x0, y0) = (f64::from(p0.0), f64::from(p0.1));
    let (x1, y1) = (f64::from(p1.0), f64::from(p1.1));
    let phi = f64::from(x_axis_rotation_deg).to_radians();
    let (sin_phi, cos_phi) = phi.sin_cos();

    // F.6.5.1: the midpoint in the ellipse's rotated frame.
    let (hx, hy) = ((x0 - x1) / 2.0, (y0 - y1) / 2.0);
    let xp = cos_phi * hx + sin_phi * hy;
    let yp = -sin_phi * hx + cos_phi * hy;

    // F.6.6.2: radii too small to span the endpoints are scaled up.
    let lambda = (xp * xp) / (rx * rx) + (yp * yp) / (ry * ry);
    if lambda > 1.0 {
        let k = lambda.sqrt();
        rx *= k;
        ry *= k;
    }

    // F.6.5.2: the center in the rotated frame.
    let num = rx * rx * ry * ry - rx * rx * yp * yp - ry * ry * xp * xp;
    let den = rx * rx * yp * yp + ry * ry * xp * xp;
    let mut coef = if den == 0.0 { 0.0 } else { (num / den).max(0.0).sqrt() };
    if large_arc == sweep {
        coef = -coef;
    }
    let cxp = coef * rx * yp / ry;
    let cyp = -coef * ry * xp / rx;

    // F.6.5.3: back to user space.
    let cx = cos_phi * cxp - sin_phi * cyp + (x0 + x1) / 2.0;
    let cy = sin_phi * cxp + cos_phi * cyp + (y0 + y1) / 2.0;

    // F.6.5.5-6: start angle and sweep.
    let angle = |ux: f64, uy: f64, vx: f64, vy: f64| (ux * vy - uy * vx).atan2(ux * vx + uy * vy);
    let (ux, uy) = ((xp - cxp) / rx, (yp - cyp) / ry);
    let (vx, vy) = ((-xp - cxp) / rx, (-yp - cyp) / ry);
    let theta1 = angle(1.0, 0.0, ux, uy);
    let mut delta = angle(ux, uy, vx, vy);
    if !sweep && delta > 0.0 {
        delta -= std::f64::consts::TAU;
    } else if sweep && delta < 0.0 {
        delta += std::f64::consts::TAU;
    }

    // About one point per 11.25 degrees, as fine as the 20-step beziers.
    let steps = ((delta.abs() / (std::f64::consts::PI / 16.0)).ceil() as usize).max(2);
    let mut points = Vec::with_capacity(steps);
    for i in 1..steps {
        let (sin_t, cos_t) = (theta1 + delta * i as f64 / steps as f64).sin_cos();
        points.push((
            (cx + rx * cos_phi * cos_t - ry * sin_phi * sin_t) as f32,
            (cy + rx * sin_phi * cos_t + ry * cos_phi * sin_t) as f32,
        ));
    }
    points.push(p1);
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
    // Both spellings: HTML's tree builder lowercases attribute names, so an
    // inline <svg viewBox=...> serialized back out of the DOM carries
    // `viewbox=` — the camelCase spelling only survives in external .svg
    // files. (The spec-correct place to re-case it is the HTML parser's
    // "adjust SVG attributes" step, which rustkit-html does not have.)
    let lower = name.to_lowercase();
    for candidate in [name, lower.as_str()] {
        let pattern = format!("{}=", candidate);
        if let Some(start) = tag.find(&pattern) {
            let rest = &tag[start + pattern.len()..];
            let quote = rest.chars().next()?;
            if quote == '"' || quote == '\'' {
                let end = rest[1..].find(quote)?;
                return Some(rest[1..1 + end].to_string());
            }
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
fn parse_svg_content(xml: &str, base_style: &SvgStyle) -> Result<SvgElement, SvgError> {
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
                let after_tag = tag_start + tag_end + 1;

                // <text> carries its content BETWEEN the tags, which
                // parse_element (open tag only) can never see. Grab up to
                // the closing tag and consume the whole element.
                let tag_name = tag
                    .trim_start_matches('<')
                    .split(|c: char| c.is_whitespace() || c == '>' || c == '/')
                    .next()
                    .unwrap_or("")
                    .to_lowercase();
                if tag_name == "text" && !tag.ends_with("/>") {
                    if let Some(close) = xml[after_tag..].find("</text") {
                        let content = &xml[after_tag..after_tag + close];
                        if let Some(element) = parse_text_element(tag, content, base_style) {
                            group.children.push(element);
                        }
                        let rest = after_tag + close;
                        pos = xml[rest..]
                            .find('>')
                            .map(|e| rest + e + 1)
                            .unwrap_or(xml.len());
                        continue;
                    }
                }

                // Parse element
                if let Some(element) = parse_element(tag, base_style) {
                    group.children.push(element);
                }

                pos = after_tag;
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
fn parse_element(tag: &str, base_style: &SvgStyle) -> Option<SvgElement> {
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
            rect.style = base_style.clone();
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
            circle.style = base_style.clone();
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
            ellipse.style = base_style.clone();
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
            line.style = base_style.clone();
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
            path.style = base_style.clone();
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
            polyline.style = base_style.clone();
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
            polygon.style = base_style.clone();
            polygon.style.parse_attributes(&attrs);
            Some(SvgElement::Polygon(polygon))
        }
        _ => None,
    }
}

/// Parse a `<text>` element from its open tag and the content between the
/// tags. Nested markup (tspan) is stripped to its text; the three basic
/// XML entities are decoded because the content is read literally.
fn parse_text_element(tag: &str, content: &str, base_style: &SvgStyle) -> Option<SvgElement> {
    let attrs_str = tag
        .trim_start_matches('<')
        .trim_end_matches('>')
        .splitn(2, char::is_whitespace)
        .nth(1)
        .unwrap_or("");

    let mut attrs = HashMap::new();
    let mut attr_str = attrs_str;
    while let Some((key, value, rest)) = parse_attr(attr_str) {
        attrs.insert(key.to_lowercase(), value);
        attr_str = rest;
    }

    let mut text = String::new();
    let mut in_tag = false;
    for c in content.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => text.push(c),
            _ => {}
        }
    }
    let text = text
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&amp;", "&")
        .trim()
        .to_string();
    if text.is_empty() {
        return None;
    }

    let mut t = SvgText {
        x: attrs.get("x").and_then(|s| s.parse().ok()).unwrap_or(0.0),
        y: attrs.get("y").and_then(|s| s.parse().ok()).unwrap_or(0.0),
        content: text,
        font_family: attrs.get("font-family").cloned().unwrap_or_default(),
        font_size: attrs
            .get("font-size")
            .and_then(|s| SvgLength::parse(s))
            .map(|l| l.to_px(16.0))
            .unwrap_or(16.0),
        anchor: match attrs.get("text-anchor").map(|s| s.trim()) {
            Some("middle") => TextAnchor::Middle,
            Some("end") => TextAnchor::End,
            _ => TextAnchor::Start,
        },
        ..Default::default()
    };
    if let Some(tr) = attrs.get("transform") {
        t.transform = Transform2D::parse(tr);
    }
    t.style = base_style.clone();
    t.style.parse_attributes(&attrs);
    Some(SvgElement::Text(t))
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
    /// Every point `to_line_segments` produces for path data `d`, in order.
    fn flattened(d: &str) -> Vec<(f32, f32)> {
        let path = super::SvgPath { commands: super::SvgPath::parse(d), ..Default::default() };
        path.to_line_segments().into_iter().flatten().collect()
    }

    fn near(a: (f32, f32), b: (f32, f32)) -> bool {
        (a.0 - b.0).abs() < 1e-3 && (a.1 - b.1).abs() < 1e-3
    }

    #[test]
    fn smooth_cubic_reflects_the_previous_control_point() {
        // C's second control (10,10) reflects about (10,0) to (10,-10), so
        // the S half dips below the axis.
        let pts = flattened("M0 0 C0 10 10 10 10 0 S20 -10 20 0");
        assert!(near(*pts.last().unwrap(), (20.0, 0.0)), "{:?}", pts.last());
        let second_half: Vec<_> = pts.iter().filter(|p| p.0 > 10.0).collect();
        assert!(second_half.iter().all(|p| p.1 <= 1e-3), "S must bend away from C: {second_half:?}");
        assert!(second_half.iter().any(|p| p.1 < -5.0));
    }

    #[test]
    fn smooth_commands_advance_the_current_point() {
        // Relative commands after s/t/a start from where they ended.
        for d in ["M0 0 s10 10 10 0 l5 0", "M0 0 t10 0 l5 0", "M0 0 a5 5 0 0 1 10 0 l5 0"] {
            assert!(near(*flattened(d).last().unwrap(), (15.0, 0.0)), "{d}: {:?}", flattened(d));
        }
    }

    #[test]
    fn smooth_quad_reflects_only_a_quad_control() {
        // Q's control (5,10) reflects about (10,0) to (15,-10).
        let pts = flattened("M0 0 Q5 10 10 0 T20 0");
        assert!(pts.iter().filter(|p| p.0 > 10.0).any(|p| p.1 < -2.0), "{pts:?}");
        // After a cubic, T has no quad control to reflect: a straight line.
        let pts = flattened("M0 0 C0 10 10 10 10 0 T20 0");
        assert!(pts.iter().filter(|p| p.0 > 10.0).all(|p| p.1.abs() < 1e-3), "{pts:?}");
    }

    #[test]
    fn arcs_follow_the_ellipse_and_honour_the_flags() {
        // A semicircle of radius 10 about (10,0). sweep=1 is the positive
        // angle direction, which is upward (negative y) here.
        let up = flattened("M0 0 A10 10 0 0 1 20 0");
        assert!(near(*up.last().unwrap(), (20.0, 0.0)));
        for p in &up {
            let r = ((p.0 - 10.0).powi(2) + p.1.powi(2)).sqrt();
            assert!((r - 10.0).abs() < 1e-2, "off the circle: {p:?}");
        }
        assert!(up.iter().any(|p| p.1 < -9.9));
        let down = flattened("M0 0 A10 10 0 0 0 20 0");
        assert!(down.iter().any(|p| p.1 > 9.9));

        // large-arc picks the long way round a circle through both points.
        let small = flattened("M0 0 A10 10 0 0 1 10 10");
        let large = flattened("M0 0 A10 10 0 1 1 10 10");
        assert!(large.len() > small.len() * 2, "{} vs {}", large.len(), small.len());

        // Radii too small are scaled up to just span the endpoints.
        let scaled = flattened("M0 0 a1 1 0 0 1 20 0");
        assert!(near(*scaled.last().unwrap(), (20.0, 0.0)));
        assert!(scaled.iter().any(|p| p.1 < -9.9));

        // A zero radius is a straight line.
        assert_eq!(flattened("M0 0 A0 5 0 0 1 20 0"), vec![(0.0, 0.0), (20.0, 0.0)]);
    }

    #[test]
    fn packed_decimals_split_into_separate_numbers() {
        // facebook's logo: `1.727.125` is two numbers, 1.727 and .125.
        let cmds = super::SvgPath::parse("M1.727.125l-.5.25");
        assert!(matches!(cmds[0], super::PathCommand::MoveTo(x, y) if x == 1.727 && y == 0.125), "{cmds:?}");
        assert!(matches!(cmds[1], super::PathCommand::LineToRel(x, y) if x == -0.5 && y == 0.25), "{cmds:?}");
    }

    use super::*;

    #[test]
    fn test_lowercase_viewbox_scales_the_document() {
        // HTML's tree builder lowercases attribute names, so an inline
        // <svg viewBox=...> serialized out of the DOM reads `viewbox=`.
        // The case-sensitive lookup dropped the viewBox entirely and every
        // path/circle under a viewBox != box-size painted UNSCALED (the
        // repro triangle: 20px where Chrome draws 40).
        let doc = SvgDocument::parse(
            r##"<svg width="48" height="48" viewbox="0 0 24 24">
                <path d="M12 2L2 22h20z" fill="#d9534f"/>
            </svg>"##,
        )
        .expect("parse");
        assert!(doc.view_box.is_some(), "lowercased viewbox must still parse");

        let commands = doc.render(0.0, 0.0, 48.0, 48.0);
        let points = commands
            .iter()
            .find_map(|c| match c {
                DisplayCommand::FillPolygon { points, .. } => Some(points.clone()),
                _ => None,
            })
            .expect("path fill");
        let max_x = points.iter().map(|p| p.0).fold(f32::MIN, f32::max);
        let min_x = points.iter().map(|p| p.0).fold(f32::MAX, f32::min);
        // Path x spans 2..22 in a 24-unit viewBox mapped to 48px: 4..44.
        assert!((min_x - 4.0).abs() < 0.01 && (max_x - 44.0).abs() < 0.01,
            "viewBox scale must reach path points: {min_x}..{max_x}");
    }

    #[test]
    fn test_text_element_parses_content_between_tags() {
        let doc = SvgDocument::parse(
            r##"<svg width="200" height="150" viewBox="0 0 200 150">
                <rect fill="#4a90d9" width="200" height="150"/>
                <text x="100" y="75" text-anchor="middle" fill="white" font-size="14">200&#215;150 &amp; more</text>
            </svg>"##,
        )
        .expect("parse");

        let commands = doc.render(0.0, 0.0, 200.0, 150.0);
        let text = commands
            .iter()
            .find_map(|c| match c {
                DisplayCommand::Text { text, y, font_size, ascent, .. } => {
                    Some((text.clone(), *y, *font_size, *ascent))
                }
                _ => None,
            })
            .expect("text command");
        // Numeric entities are not decoded (only the named basics), so the
        // raw &#215; stays; the point is the content and the & decode.
        assert!(text.0.contains("150 & more"), "content must reach the command: {:?}", text.0);
        // y is the BASELINE and must be handed over as one (zero ascent).
        assert_eq!(text.1, 75.0);
        assert_eq!(text.3, Some(0.0));
        assert_eq!(text.2, 14.0);

        // Anchor=middle shifts the run left of x=100.
        if let Some(DisplayCommand::Text { x, .. }) = commands.iter().find(|c| matches!(c, DisplayCommand::Text { .. })) {
            assert!(*x < 100.0, "middle anchor must shift the run left: x={x}");
        }
    }

    /// How many times the renderer's triangle fans paint the point `p`.
    fn fan_coverage(commands: &[DisplayCommand], p: (f32, f32)) -> usize {
        let in_tri = |a: (f32, f32), b: (f32, f32), c: (f32, f32)| {
            let side = |u: (f32, f32), v: (f32, f32)| (v.0 - u.0) * (p.1 - u.1) - (v.1 - u.1) * (p.0 - u.0);
            let (d1, d2, d3) = (side(a, b), side(b, c), side(c, a));
            !((d1 < 0.0 || d2 < 0.0 || d3 < 0.0) && (d1 > 0.0 || d2 > 0.0 || d3 > 0.0))
        };
        commands
            .iter()
            .filter_map(|c| match c {
                DisplayCommand::FillPolygon { points, .. } => Some(points),
                _ => None,
            })
            .map(|pts| (1..pts.len() - 1).filter(|&i| in_tri(pts[0], pts[i], pts[i + 1])).count().min(1))
            .sum()
    }

    fn render_path(d: &str, extra: &str) -> Vec<DisplayCommand> {
        let doc = SvgDocument::parse(&format!(
            r##"<svg width="10" height="10"><path d="{d}" fill="#000" {extra}/></svg>"##
        ))
        .expect("parse");
        doc.render(0.0, 0.0, 10.0, 10.0)
    }

    #[test]
    fn test_evenodd_leaves_an_inner_subpath_empty() {
        // linkedin's chair outline: an outer and an inner subpath, both the
        // same direction, under fill-rule="evenodd". Chrome paints a ring.
        let d = "M0 0H10V10H0Z M3 3H7V7H3Z";
        let ring = render_path(d, r#"fill-rule="evenodd""#);
        assert_eq!(fan_coverage(&ring, (5.0, 5.0)), 0, "evenodd hole must stay empty");
        assert_eq!(fan_coverage(&ring, (1.0, 5.0)), 1, "the ring itself paints once");
        // The same path under the initial nonzero rule is solid.
        let solid = render_path(d, "");
        assert_eq!(fan_coverage(&solid, (5.0, 5.0)), 1, "nonzero, same direction: solid, painted once");
        // The style property spells it the same way.
        let styled = render_path(d, r#"style="fill-rule: evenodd""#);
        assert_eq!(fan_coverage(&styled, (5.0, 5.0)), 0);
    }

    #[test]
    fn test_nonzero_reversed_inner_subpath_is_a_hole() {
        // The icon-font idiom: the counter of an "O" is drawn counter-wise.
        let commands = render_path("M0 0H10V10H0Z M3 3V7H7V3Z", "");
        assert_eq!(fan_coverage(&commands, (5.0, 5.0)), 0);
        assert_eq!(fan_coverage(&commands, (8.5, 5.0)), 1);
    }

    #[test]
    fn test_concave_path_does_not_fill_its_notch() {
        // A dart: a fan from (0,0) would paint the notch at x < 5.
        let commands = render_path("M0 0L10 5L0 10L5 5Z", "");
        assert_eq!(fan_coverage(&commands, (2.0, 4.5)), 0, "the notch is outside the dart");
        assert_eq!(fan_coverage(&commands, (7.0, 4.5)), 1);
        // A convex shape still goes through as one polygon.
        let tri = render_path("M0 0L10 0L5 10Z", "");
        assert_eq!(tri.iter().filter(|c| matches!(c, DisplayCommand::FillPolygon { .. })).count(), 1);
    }

    #[test]
    fn test_self_crossing_star_follows_the_fill_rule() {
        // A pentagram: its centre winds twice, so nonzero fills it and
        // evenodd leaves it empty.
        let d = "M5 0L8 10L0 3.5H10L2 10Z";
        assert_eq!(fan_coverage(&render_path(d, ""), (5.0, 5.5)), 1);
        assert_eq!(fan_coverage(&render_path(d, r#"fill-rule="evenodd""#), (5.0, 5.5)), 0);
        assert_eq!(fan_coverage(&render_path(d, r#"fill-rule="evenodd""#), (5.0, 2.0)), 1);
    }

    #[test]
    fn test_inline_style_sets_paint_and_beats_presentation_attributes() {
        // linkedin's hero: `<path d=".." style="fill: #fbf1e2"/>`, 148 times.
        let doc = SvgDocument::parse(
            r##"<svg width="10" height="10"><rect width="10" height="10" fill="#0000ff" style="fill: #fbf1e2; stroke:#ff0000 !important;stroke-width: 2"/></svg>"##,
        )
        .expect("parse");
        let SvgElement::Group(root) = &doc.root else { panic!("root is a group") };
        let SvgElement::Rect(rect) = &root.children[0] else { panic!("rect") };
        let rgb = |c: Option<Color>| c.map(|c| (c.r, c.g, c.b));
        assert_eq!(rgb(rect.style.fill_color()), Some((0xfb, 0xf1, 0xe2)));
        assert_eq!(rgb(rect.style.stroke_color()), Some((0xff, 0, 0)));
        assert_eq!(rect.style.stroke_width, 2.0);
    }

    #[test]
    fn test_root_presentation_attributes_seed_shape_styles() {
        // The stroke-only icon idiom: fill/stroke live on the <svg> root and
        // the shapes carry none of their own. The flat parser must seed
        // every shape from the root or the circle paints a default-black
        // disc where Chrome draws an outline.
        let doc = SvgDocument::parse(
            r#"<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                <circle cx="11" cy="11" r="8"/>
                <path d="M21 21l-4.35-4.35"/>
            </svg>"#,
        )
        .expect("parse");

        let commands = doc.render(0.0, 0.0, 14.0, 14.0);
        assert!(
            !commands.iter().any(|c| matches!(c, DisplayCommand::FillPolygon { .. })),
            "fill=none on the root must reach the shapes (no fills)"
        );
        assert!(
            commands.iter().any(|c| matches!(c, DisplayCommand::Polyline { .. })),
            "stroke=currentColor on the root must reach the shapes (strokes present)"
        );
    }

    #[test]
    fn test_current_color_resolves_to_the_render_calls_css_color() {
        // The shelf's search icon: `stroke="currentColor"` on the root, the
        // <svg> sitting in an element whose CSS color is rgb(148,163,184).
        // Chrome strokes it in that color; we stroked it in black because
        // the paint keyword resolved with no context. The CSS color is a
        // render-time input and must reach every shape — including the
        // path (polyline) and the circle (stroke-circle), and a shape that
        // names currentColor itself rather than inheriting the root's.
        let doc = SvgDocument::parse(
            r#"<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                <circle cx="11" cy="11" r="8"/>
                <path d="M21 21l-4.35-4.35"/>
                <rect x="1" y="1" width="4" height="4" fill="currentColor" stroke="none"/>
            </svg>"#,
        )
        .expect("parse");
        let css = Color::new(148, 163, 184, 1.0);

        let commands = doc.render_with_color(0.0, 0.0, 14.0, 14.0, css);
        let mut seen = 0;
        for c in &commands {
            let color = match c {
                DisplayCommand::Polyline { color, .. } => *color,
                DisplayCommand::StrokeCircle { color, .. } => *color,
                DisplayCommand::FillRect { color, .. } => *color,
                other => panic!("unexpected command for the icon: {other:?}"),
            };
            assert_eq!(
                (color.r, color.g, color.b),
                (css.r, css.g, css.b),
                "currentColor must resolve to the CSS color, got {color:?} in {c:?}"
            );
            seen += 1;
        }
        assert_eq!(seen, 3, "circle stroke + path stroke + rect fill: {commands:?}");

        // The context-free render (an <img src=*.svg>, whose own CSS color
        // is the initial black) keeps black.
        let plain = doc.render(0.0, 0.0, 14.0, 14.0);
        let black = plain
            .iter()
            .filter(|c| matches!(c, DisplayCommand::Polyline { color, .. } if color.r == 0 && color.g == 0 && color.b == 0))
            .count();
        assert_eq!(black, 1, "render() must still resolve currentColor to black: {plain:?}");
    }

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

