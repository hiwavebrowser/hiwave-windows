//! A vector background is clipped to its box where its commands are made.
//!
//! The sprite-sheet idiom (wikipedia.org's portal): every icon is a small box
//! whose background is the whole sheet, moved by `background-position` so one
//! icon shows. The splice emitted the whole sheet for every box between a
//! `PushClip` and a `PopClip`, and the renderer does not clip polygons or
//! circles: 21 boxes painted the 176x811 sheet 21 times, unclipped, as 1.67
//! million commands that the app's window thread re-executed on every frame.

use super::*;
use rustkit_layout::{BackgroundRepeat, BackgroundSize, DisplayCommand, Rect};

/// A 20x40 sheet of two 20x20 icons, as the commands for a 20x20 box at
/// (100, 100) that shows the lower one.
fn lower_icon(sheet_content: &str) -> Vec<DisplayCommand> {
    let svg = rustkit_svg::SvgDocument::parse(&format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="40" viewBox="0 0 20 40">{sheet_content}</svg>"#
    ))
    .expect("svg");
    svg_background_commands(
        &svg,
        Rect::new(100.0, 100.0, 20.0, 20.0),
        &BackgroundSize::Auto,
        (0.0, 0.0),
        (0.0, -20.0),
        BackgroundRepeat::NoRepeat,
    )
}

#[test]
fn what_lies_outside_the_box_is_not_emitted() {
    let commands = lower_icon(
        r##"<rect width="20" height="20" fill="#ff0000"/>
            <circle cx="10" cy="10" r="5" fill="#ff0000"/>
            <polygon points="0,0 20,0 0,20" fill="#ff0000"/>
            <rect y="20" width="20" height="20" fill="#0000ff"/>"##,
    );
    assert!(
        matches!(commands.first(), Some(DisplayCommand::PushClip(_)))
            && matches!(commands.last(), Some(DisplayCommand::PopClip)),
        "{commands:?}"
    );
    let painted = &commands[1..commands.len() - 1];
    assert!(
        matches!(painted, [DisplayCommand::FillRect { rect, color }]
            if (rect.x, rect.y, rect.width, rect.height) == (100.0, 100.0, 20.0, 20.0) && color.b == 255),
        "only the icon the box shows: {painted:?}"
    );
}

#[test]
fn a_polygon_across_the_edge_is_cut_at_it() {
    // In the sheet the triangle runs to x = 40, twice the box's width.
    let commands = lower_icon(r##"<polygon points="0,20 40,20 0,40" fill="#0000ff"/>"##);
    let points: Vec<(f32, f32)> = commands
        .iter()
        .filter_map(|c| match c {
            DisplayCommand::FillPolygon { points, .. } => Some(points.clone()),
            _ => None,
        })
        .flatten()
        .collect();
    assert!(points.len() >= 3, "the part inside the box is painted: {commands:?}");
    assert!(
        points
            .iter()
            .all(|(x, y)| (100.0..=120.0).contains(x) && (100.0..=120.0).contains(y)),
        "no point outside the box: {points:?}"
    );
    let has = |x: f32, y: f32| points.iter().any(|p| (p.0 - x).abs() < 0.01 && (p.1 - y).abs() < 0.01);
    assert!(
        has(100.0, 100.0) && has(120.0, 100.0) && has(120.0, 110.0) && has(100.0, 120.0),
        "cut along x = 120: {points:?}"
    );
}

#[test]
fn a_circle_across_the_edge_does_not_paint_past_it() {
    // Centre on the box's right edge: half of it is outside.
    let commands = lower_icon(r##"<circle cx="20" cy="30" r="8" fill="#0000ff"/>"##);
    let mut painted = 0;
    for command in &commands {
        match command {
            DisplayCommand::FillPolygon { points, .. } => {
                painted += 1;
                assert!(points.iter().all(|(x, _)| *x <= 120.0), "{points:?}");
            }
            DisplayCommand::FillCircle { cx, radius, .. } => {
                painted += 1;
                assert!(cx + radius <= 120.0, "a whole circle is kept only inside the box");
            }
            _ => {}
        }
    }
    assert!(painted > 0, "the half inside the box is painted: {commands:?}");
}

/// The furthest right any command paints, strokes at their full width.
fn right_edge(commands: &[DisplayCommand]) -> f32 {
    let reach = |points: &[(f32, f32)], half: f32| points.iter().map(|p| p.0 + half).fold(f32::MIN, f32::max);
    commands
        .iter()
        .map(|command| match command {
            DisplayCommand::FillPolygon { points, .. } => reach(points, 0.0),
            DisplayCommand::Polyline { points, width, .. } | DisplayCommand::StrokePolygon { points, width, .. } => {
                reach(points, width * 0.5)
            }
            DisplayCommand::Line { x1, x2, width, .. } => x1.max(*x2) + width * 0.5,
            DisplayCommand::FillRect { rect, .. } | DisplayCommand::FillEllipse { rect, .. } => rect.x + rect.width,
            DisplayCommand::StrokeRect { rect, width, .. } => rect.x + rect.width + width * 0.5,
            DisplayCommand::FillCircle { cx, radius, .. } => cx + radius,
            DisplayCommand::StrokeCircle { cx, radius, width, .. } => cx + radius + width * 0.5,
            _ => f32::MIN,
        })
        .fold(f32::MIN, f32::max)
}

#[test]
fn a_stroke_across_the_edge_is_cut_at_it() {
    // The sheet's neighbours of the icon a box shows are often strokes: the
    // portal's wordmark box painted a ring and a burst of rays from the
    // icons beside it in the sheet, whole.
    for (what, shape) in [
        ("line", r##"<line x1="0" y1="30" x2="40" y2="30" stroke="#0000ff" stroke-width="4"/>"##),
        ("polyline", r##"<polyline points="5,25 40,25 40,35" fill="none" stroke="#0000ff" stroke-width="2"/>"##),
        ("path", r##"<path d="M5 25 L40 35" fill="none" stroke="#0000ff" stroke-width="2"/>"##),
        ("ring", r##"<circle cx="20" cy="30" r="8" fill="none" stroke="#0000ff" stroke-width="6"/>"##),
        ("ellipse", r##"<ellipse cx="20" cy="30" rx="9" ry="4" fill="#0000ff"/>"##),
    ] {
        let commands = lower_icon(shape);
        assert!(commands.len() > 2, "{what}: the part inside the box is painted");
        let edge = right_edge(&commands);
        assert!(edge <= 120.01, "{what}: paints to x = {edge}, past the box's 120: {commands:?}");
    }
}

/// CSS Backgrounds 3 §3.10: a property with fewer values than there are
/// layers repeats its list. The portal's sprite is the second of two images
/// (`linear-gradient(transparent, transparent), url(sprite.svg)`, the old
/// SVG-support test) under one `background-repeat` and one
/// `background-position`: both reached the gradient only, so every icon box
/// showed the sheet's top left corner, tiled.
#[test]
#[cfg(target_os = "macos")]
fn a_short_value_list_is_repeated_over_the_layers() {
    let engine = Engine::new(EngineConfig::default()).expect("engine");
    let sheet = Stylesheet::parse(
        ".sprite { background-image: linear-gradient(transparent, transparent), url(sheet.svg); \
                   background-repeat: no-repeat; background-origin: content-box } \
         .wordmark { background-position: 0 -203px; background-size: 176px 811px }",
    )
    .expect("sheet");
    let mut attributes = HashMap::new();
    attributes.insert("class".to_string(), "sprite wordmark".to_string());
    let style = engine.compute_style_for_element(
        "span",
        &attributes,
        std::slice::from_ref(&sheet),
        &HashMap::new(),
        &[],
        &[],
        SiblingContext::SOLE,
        None,
    );
    assert_eq!(style.background_layers.len(), 2);
    for layer in &style.background_layers {
        assert_eq!(layer.repeat, rustkit_css::BackgroundRepeat::NoRepeat, "{:?}", layer.image);
        assert_eq!(layer.position, parse_background_position("0 -203px"), "{:?}", layer.image);
        assert_eq!(layer.size, parse_background_size("176px 811px"), "{:?}", layer.image);
        assert_eq!(layer.origin, rustkit_css::BackgroundOrigin::ContentBox, "{:?}", layer.image);
    }
}
