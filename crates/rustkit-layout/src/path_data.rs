//! SVG path data (SVG 1.1 §8.3): the parser and the flattener, shared by
//! `rustkit-svg`'s `<path>` and `clip-path: path()` / `shape()` here (C8b).
//! Moved from `rustkit-svg`, which depends on this crate, unchanged.

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

/// Parse path data (`d`) into commands; what does not parse is skipped.
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

/// `commands` as polylines, one per subpath, flattening every curve and arc
/// so no point of it strays more than `tolerance` (in the path's own units)
/// from the flattened outline. A closed subpath ends on its start point.
pub fn flatten(commands: &[PathCommand], tolerance: f32) -> Vec<Vec<(f32, f32)>> {
    let mut segments = Vec::new();
    let mut current_segment = Vec::new();
    let mut current_pos = (0.0_f32, 0.0_f32);
    let mut start_pos = (0.0_f32, 0.0_f32);
    // The previous segment's second control point, for S/s (after C/S)
    // and T/t (after Q/T) to reflect. Any other command clears both.
    let mut last_cubic = None::<(f32, f32)>;
    let mut last_quad = None::<(f32, f32)>;

    for cmd in commands {
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
                let points = cubic_bezier_points(
                    current_pos,
                    (*x1, *y1),
                    (*x2, *y2),
                    (*x, *y),
                    tolerance,
                );
                current_segment.extend(points);
                current_pos = (*x, *y);
                last_cubic = Some((*x2, *y2));
                last_quad = None;
            }
            PathCommand::CubicToRel(dx1, dy1, dx2, dy2, dx, dy) => {
                let (x1, y1) = (current_pos.0 + dx1, current_pos.1 + dy1);
                let (x2, y2) = (current_pos.0 + dx2, current_pos.1 + dy2);
                let (x, y) = (current_pos.0 + dx, current_pos.1 + dy);
                let points =
                    cubic_bezier_points(current_pos, (x1, y1), (x2, y2), (x, y), tolerance);
                current_segment.extend(points);
                current_pos = (x, y);
                last_cubic = Some((x2, y2));
                last_quad = None;
            }
            PathCommand::QuadTo(x1, y1, x, y) => {
                let points = quad_bezier_points(current_pos, (*x1, *y1), (*x, *y), tolerance);
                current_segment.extend(points);
                current_pos = (*x, *y);
                last_quad = Some((*x1, *y1));
                last_cubic = None;
            }
            PathCommand::QuadToRel(dx1, dy1, dx, dy) => {
                let (x1, y1) = (current_pos.0 + dx1, current_pos.1 + dy1);
                let (x, y) = (current_pos.0 + dx, current_pos.1 + dy);
                let points = quad_bezier_points(current_pos, (x1, y1), (x, y), tolerance);
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
                current_segment.extend(cubic_bezier_points(
                    current_pos,
                    c1,
                    (x2, y2),
                    (x, y),
                    tolerance,
                ));
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
                current_segment.extend(quad_bezier_points(current_pos, c1, (x, y), tolerance));
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
                current_segment.extend(arc_points(
                    current_pos,
                    rx,
                    ry,
                    angle,
                    large_arc,
                    sweep,
                    (x, y),
                    tolerance,
                ));
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

/// The most line segments one curve or arc flattens to, however big it's drawn.
const MAX_CURVE_STEPS: usize = 256;

/// Steps that keep a Bézier of degree `degree` within `tolerance` of its
/// chords, from the largest second difference of its control points
/// (Wang's formula: n = sqrt(degree * (degree - 1) / 8 * dd / tolerance)).
/// A NaN or infinite input (a degenerate transform) falls to one step.
fn bezier_steps(degree: f32, dd: f32, tolerance: f32) -> usize {
    let n = (degree * (degree - 1.0) / 8.0 * dd / tolerance)
        .sqrt()
        .ceil();
    if n.is_finite() {
        (n as usize).clamp(1, MAX_CURVE_STEPS)
    } else if n.is_nan() {
        1
    } else {
        MAX_CURVE_STEPS
    }
}

/// The length of `a - 2b + c`.
fn second_difference(a: (f32, f32), b: (f32, f32), c: (f32, f32)) -> f32 {
    (a.0 - 2.0 * b.0 + c.0).hypot(a.1 - 2.0 * b.1 + c.1)
}

/// Generate points along a cubic bezier curve, within `tolerance` of it.
fn cubic_bezier_points(
    p0: (f32, f32),
    p1: (f32, f32),
    p2: (f32, f32),
    p3: (f32, f32),
    tolerance: f32,
) -> Vec<(f32, f32)> {
    let dd = second_difference(p0, p1, p2).max(second_difference(p1, p2, p3));
    let segments = bezier_steps(3.0, dd, tolerance);
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
    tolerance: f32,
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

    // Each step's chord sags r * (1 - cos(step / 2)) from the arc; keep
    // that within `tolerance` of the larger radius.
    let sag = (f64::from(tolerance) / rx.max(ry)).min(1.0);
    let step = 2.0 * (1.0 - sag).acos();
    let steps = (delta.abs() / step).ceil();
    let steps = if steps.is_finite() {
        (steps as usize).clamp(2, MAX_CURVE_STEPS)
    } else if steps.is_nan() {
        2
    } else {
        MAX_CURVE_STEPS
    };
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

/// Generate points along a quadratic bezier curve, within `tolerance` of it.
fn quad_bezier_points(
    p0: (f32, f32),
    p1: (f32, f32),
    p2: (f32, f32),
    tolerance: f32,
) -> Vec<(f32, f32)> {
    let segments = bezier_steps(2.0, second_difference(p0, p1, p2), tolerance);
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

/// Whether `d` is valid path data as a whole (SVG 2 path grammar): it starts
/// with a moveto, every command has complete argument groups, arc flags are
/// `0` or `1`, and nothing else is in it. `parse` is lenient on purpose (an
/// SVG `d` renders up to its first error); CSS `path()` is not: an
/// unparsable string makes the declaration invalid, and Chrome for Testing
/// 148 then leaves the box unclipped, `path('')` included (C8b, #1052 nit 5).
pub fn is_valid(d: &str) -> bool {
    let b = d.as_bytes();
    let mut i = 0usize;
    let skip_ws = |i: &mut usize| {
        while *i < b.len() && matches!(b[*i], b' ' | b'\t' | b'\n' | b'\r' | b'\x0c') {
            *i += 1;
        }
    };
    let skip_sep = |i: &mut usize| {
        skip_ws(i);
        if *i < b.len() && b[*i] == b',' {
            *i += 1;
            skip_ws(i);
        }
    };
    let number = |i: &mut usize| -> bool {
        let start = *i;
        if *i < b.len() && matches!(b[*i], b'+' | b'-') {
            *i += 1;
        }
        let mut digits = 0;
        while *i < b.len() && b[*i].is_ascii_digit() {
            *i += 1;
            digits += 1;
        }
        if *i < b.len() && b[*i] == b'.' {
            *i += 1;
            while *i < b.len() && b[*i].is_ascii_digit() {
                *i += 1;
                digits += 1;
            }
        }
        if digits == 0 {
            *i = start;
            return false;
        }
        if *i < b.len() && matches!(b[*i], b'e' | b'E') {
            let save = *i;
            *i += 1;
            if *i < b.len() && matches!(b[*i], b'+' | b'-') {
                *i += 1;
            }
            let mut exp = 0;
            while *i < b.len() && b[*i].is_ascii_digit() {
                *i += 1;
                exp += 1;
            }
            if exp == 0 {
                *i = save;
            }
        }
        true
    };
    let flag = |i: &mut usize| -> bool {
        if *i < b.len() && matches!(b[*i], b'0' | b'1') {
            *i += 1;
            true
        } else {
            false
        }
    };
    skip_ws(&mut i);
    if i >= b.len() || !matches!(b[i], b'M' | b'm') {
        return false;
    }
    while i < b.len() {
        let cmd = b[i].to_ascii_lowercase();
        i += 1;
        let args = match cmd {
            b'z' => 0,
            b'm' | b'l' | b't' => 2,
            b'h' | b'v' => 1,
            b'c' => 6,
            b's' | b'q' => 4,
            b'a' => 7,
            _ => return false,
        };
        skip_ws(&mut i);
        if args == 0 {
            continue;
        }
        // One or more argument groups.
        let mut groups = 0;
        loop {
            let at = i;
            let mut ok = true;
            for k in 0..args {
                if k > 0 {
                    skip_sep(&mut i);
                }
                let good = if cmd == b'a' && (k == 3 || k == 4) { flag(&mut i) } else { number(&mut i) };
                if !good {
                    ok = false;
                    break;
                }
            }
            if !ok {
                if groups == 0 {
                    return false;
                }
                i = at;
                break;
            }
            groups += 1;
            let before_sep = i;
            skip_sep(&mut i);
            if i >= b.len() || b[i].is_ascii_alphabetic() && !matches!(b[i], b'e' | b'E') {
                break;
            }
            if i == before_sep && !matches!(b[i], b'+' | b'-' | b'.') && !b[i].is_ascii_digit() {
                return false;
            }
        }
        skip_ws(&mut i);
        if i < b.len() && !b[i].is_ascii_alphabetic() {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod is_valid_tests {
    use super::{is_valid, parse};

    /// C8b / #1052 nit 5: CSS `path()` drops an unparsable string
    /// (including empty). The headless frame pin is gated; these exercise
    /// the pure validator Linux `--lib` always runs.
    #[test]
    fn invalid_and_empty_path_data_are_rejected() {
        for d in [
            "",
            "   ",
            "garbage",
            "L 10 10",
            "M 0 0 L 10",
            "M 0 0 L 100 0 L 0 100 Z garbage",
            "zz",
            "M",
            "M 0",
        ] {
            assert!(!is_valid(d), "{d:?} must be invalid for CSS path()");
        }
    }

    #[test]
    fn valid_path_data_is_accepted_even_when_empty_of_area() {
        for d in [
            "M 0 0 L 100 0 L 0 100 Z",
            "M 0 0 L 100 100",
            "M 10 10",
            "m0,0c0,0,200,0,200,100q-100,150-200,0z",
            "M 0 0 A 50 50 0 0 1 100 0",
        ] {
            assert!(is_valid(d), "{d:?} must be valid");
            assert!(
                !parse(d).is_empty(),
                "{d:?}: valid data should parse at least one command"
            );
        }
    }

    #[test]
    fn parse_is_lenient_where_is_valid_is_strict() {
        // SVG `d` keeps commands up to the first error; CSS path() rejects
        // the whole string. Trailing garbage is the C8b distinction.
        let d = "M 0 0 L 100 0 L 0 100 Z garbage";
        assert!(!is_valid(d));
        assert!(
            !parse(d).is_empty(),
            "lenient parse still yields the triangle before garbage"
        );
    }
}
