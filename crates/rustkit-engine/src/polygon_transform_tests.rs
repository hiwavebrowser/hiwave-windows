//! A filled polygon and a line are drawn where the transform on the stack
//! puts them, as a rectangle and a circle are. The page's scroll is such a
//! transform (one translate around the whole list), so a shape that skips
//! it stays where it was while the page moves under it: every inline svg
//! path is filled as polygons, which is ebay's icons standing still over
//! a scrolling page (hand test 9, H24).

use super::*;
use rustkit_css::Color;
use crate::script_net_tests::serve_routes;
use rustkit_layout::{DisplayCommand, Rect};

const BLACK: Color = Color { r: 0, g: 0, b: 0, a: 1.0 };

fn frame(commands: &[DisplayCommand], tag: &str) -> Vec<u8> {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 200, height: 100 })
        .expect("view");
    capture(&mut engine, view, commands, tag)
}

fn capture(engine: &mut Engine, view: EngineViewId, commands: &[DisplayCommand], tag: &str) -> Vec<u8> {
    let viewhost_id = engine.views[&view].viewhost_id;
    let path = std::env::temp_dir().join(format!("rustkit-polygon-{tag}-{}.ppm", std::process::id()));
    engine
        .presenter
        .as_mut()
        .expect("window presenter")
        .capture_commands(viewhost_id, (200, 100), path.to_str().unwrap(), commands)
        .expect("capture");
    let ppm = std::fs::read(&path).expect("frame");
    let _ = std::fs::remove_file(&path);
    ppm
}

/// Whether the pixel is dark, in a `P6` frame 200 wide and 100 tall.
fn dark(ppm: &[u8], x: usize, y: usize) -> bool {
    let pixels = 200 * 100 * 3;
    assert!(ppm.len() > pixels, "a 200x100 frame");
    ppm[ppm.len() - pixels + (y * 200 + x) * 3] < 128
}

fn square(x: f32, y: f32, side: f32) -> Vec<(f32, f32)> {
    vec![(x, y), (x + side, y), (x + side, y + side), (x, y + side)]
}

fn under(matrix: [f32; 6], commands: Vec<DisplayCommand>) -> Vec<DisplayCommand> {
    let mut list = vec![DisplayCommand::PushTransform { matrix, origin: (0.0, 0.0) }];
    list.extend(commands);
    list.push(DisplayCommand::PopTransform);
    list
}

/// The list the app draws for a page scrolled by 50: three 20px squares
/// at y 60, a rectangle, a polygon and a thick line.
#[test]
fn a_scrolled_page_moves_its_polygons_and_lines() {
    let ppm = frame(
        &under(
            [1.0, 0.0, 0.0, 1.0, 0.0, -50.0],
            vec![
                DisplayCommand::FillRect { rect: Rect::new(10.0, 60.0, 20.0, 20.0), color: BLACK },
                DisplayCommand::FillPolygon { points: square(50.0, 60.0, 20.0), color: BLACK },
                DisplayCommand::Line { x1: 90.0, y1: 70.0, x2: 110.0, y2: 70.0, color: BLACK, width: 20.0 },
            ],
        ),
        "scroll",
    );
    assert!(dark(&ppm, 20, 20), "the rectangle is drawn 50 up");
    assert!(!dark(&ppm, 20, 70), "and not where it was");
    assert!(dark(&ppm, 60, 20), "the polygon is drawn 50 up");
    assert!(!dark(&ppm, 60, 70), "the polygon is not where it was");
    assert!(dark(&ppm, 100, 20), "the line is drawn 50 up");
    assert!(!dark(&ppm, 100, 70), "the line is not where it was");
}

/// A stroked outline and an open path are made of lines.
#[test]
fn a_scrolled_page_moves_its_outlines() {
    let ppm = frame(
        &under(
            [1.0, 0.0, 0.0, 1.0, 0.0, -50.0],
            vec![
                DisplayCommand::StrokePolygon { points: square(20.0, 60.0, 20.0), color: BLACK, width: 4.0 },
                DisplayCommand::Polyline { points: vec![(80.0, 70.0), (120.0, 70.0)], color: BLACK, width: 4.0 },
            ],
        ),
        "outline",
    );
    assert!(dark(&ppm, 30, 10), "the outline's top edge is drawn 50 up");
    assert!(!dark(&ppm, 30, 60), "and not where it was");
    assert!(dark(&ppm, 100, 20), "the open path is drawn 50 up");
    assert!(!dark(&ppm, 100, 70), "and not where it was");
}

/// `transform: scale(2)` on an ancestor: the shapes double from the
/// origin, and a line's width doubles with them.
#[test]
fn a_scaled_ancestor_scales_its_polygons_and_lines() {
    let ppm = frame(
        &under(
            [2.0, 0.0, 0.0, 2.0, 0.0, 0.0],
            vec![
                DisplayCommand::FillPolygon { points: square(10.0, 10.0, 10.0), color: BLACK },
                DisplayCommand::Line { x1: 30.0, y1: 15.0, x2: 40.0, y2: 15.0, color: BLACK, width: 10.0 },
            ],
        ),
        "scale",
    );
    assert!(dark(&ppm, 35, 35), "the polygon covers 20 to 40");
    assert!(!dark(&ppm, 12, 12), "and not 10 to 20");
    assert!(dark(&ppm, 70, 22), "the line covers 60 to 80 across");
    assert!(dark(&ppm, 70, 38), "and 20 to 40 down: its width is doubled");
    assert!(!dark(&ppm, 35, 12), "the line is not where it was");
}

/// With no transform the shapes are drawn where the list says.
#[test]
fn with_no_transform_the_shapes_stay_put() {
    let ppm = frame(
        &[
            DisplayCommand::FillPolygon { points: square(50.0, 60.0, 20.0), color: BLACK },
            DisplayCommand::Line { x1: 90.0, y1: 70.0, x2: 110.0, y2: 70.0, color: BLACK, width: 20.0 },
        ],
        "plain",
    );
    assert!(dark(&ppm, 60, 70) && dark(&ppm, 100, 70));
    assert!(!dark(&ppm, 60, 20) && !dark(&ppm, 100, 20));
}

/// A page with an inline svg path, 60 down a long page. The engine's own
/// list for it, inside the translate the app's window path puts around a
/// list when the page is scrolled by 50 (`render_view`): the icon moves
/// with the block beside it.
#[test]
fn an_inline_svg_path_scrolls_with_its_page() {
    let page = "<html><head><style>body{margin:0} #gap{height:60px} #tall{height:1000px}\
        #block{position:absolute;left:100px;top:60px;width:40px;height:40px;background:#000}\
        svg{display:block}</style></head><body><div id=gap></div>\
        <svg width=40 height=40 viewBox='0 0 40 40'><path d='M0 0L40 0L0 40Z' fill='#000'/></svg>\
        <div id=block></div><div id=tall></div></body></html>";
    let server = serve_routes(vec![("/", "text/html", page.to_string())]);
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 200, height: 100 })
        .expect("view");
    let url = Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap();
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    rt.block_on(engine.load_url(view, url)).expect("load_url");

    let list = engine.views[&view].display_list.as_ref().expect("display list").commands.clone();
    assert!(
        list.iter().any(|c| matches!(c, DisplayCommand::FillPolygon { .. })),
        "the path is filled as a polygon"
    );

    let at_top = capture(&mut engine, view, &list, "page-top");
    assert!(dark(&at_top, 5, 65) && dark(&at_top, 120, 80), "the icon and the block, 60 down");

    let scrolled = capture(&mut engine, view, &under([1.0, 0.0, 0.0, 1.0, 0.0, -50.0], list), "page-scrolled");
    assert!(dark(&scrolled, 120, 30), "the block is drawn 50 up");
    assert!(dark(&scrolled, 5, 15), "the icon is drawn 50 up");
    assert!(!dark(&scrolled, 5, 65), "the icon is not where it was");
}
