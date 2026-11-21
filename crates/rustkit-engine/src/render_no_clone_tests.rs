//! P3 of the threads plan (`docs/design/THREADS_PLAN_2026-10.md` §5): render
//! without clones.
//!
//! Until this file every frame copied the whole display list before it drew:
//! once to collect the images it names (`commands.clone()`), and once more,
//! on every scrolled window frame, to wrap the list in the scroll's translate
//! (`scrolled_commands`). On the Wikipedia portal that is 2.2M commands
//! copied per wheel event. Now the presenter borrows the page's list for
//! both, and the scroll is an offset the renderer applies as it draws
//! (`Renderer::set_scroll_offset`).
//!
//! What must not change is the picture: a scrolled frame drawn with the
//! offset is byte for byte the frame the copied list drew.

use super::*;
use crate::script_net_tests::serve_routes;
use rustkit_layout::DisplayCommand;

/// A red and a blue 4x4 PNG.
const RED: &str = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAQAAAAECAIAAAAmkwkpAAAAEElEQVR4nGM4wcAARwzEcQBRwwyB7RhOggAAAABJRU5ErkJggg==";
const BLUE: &str = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAQAAAAECAIAAAAmkwkpAAAAEElEQVR4nGNgYDiBhIjiAAA4wwyBGwNoGQAAAABJRU5ErkJggg==";

const WIDTH: u32 = 400;
const HEIGHT: u32 = 300;

fn load(body: &str) -> (Engine, EngineViewId, crate::script_net_tests::Server) {
    let page = format!(
        "<html><head><style>body{{margin:0;font:14px sans-serif}} \
         .row{{height:18px;border-bottom:1px solid #ccc;background:#f8f8f8}} \
         .row img{{width:12px;height:12px}} \
         #bar{{position:fixed;left:0;top:0;width:{WIDTH}px;height:24px;background:#000}} \
         #stick{{position:sticky;top:0;height:20px;background:#08f}}</style></head>\
         <body>{body}</body></html>"
    );
    let server = serve_routes(vec![("/", "text/html", page)]);
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: WIDTH, height: HEIGHT })
        .expect("view");
    let url = Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap();
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    rt.block_on(engine.load_url(view, url)).expect("load_url");
    (engine, view, server)
}

/// `rows` rows of text, each with an image, under a fixed bar and a sticky
/// one, with an iframe and a clipping box among them.
fn large_body(rows: usize) -> String {
    let mut body = String::from(
        "<div id=bar></div><div style='height:24px'></div><div id=stick>sticky</div>\
         <iframe style='width:200px;height:60px;border:2px solid #f00'></iframe>\
         <section style='overflow:hidden;height:40px'><div style='height:200px;background:#0a0'></div></section>",
    );
    for i in 0..rows {
        let img = if i % 2 == 0 { RED } else { BLUE };
        body.push_str(&format!(
            "<div class=row><img src='{img}'> row {i} <b>bold</b> <i>italic</i> <a href='#r{i}'>link</a></div>"
        ));
    }
    body
}

fn list_ptr(engine: &Engine, view: EngineViewId) -> (*const DisplayCommand, usize) {
    let commands = &engine.views[&view].display_list.as_ref().expect("display list").commands;
    (commands.as_ptr(), commands.len())
}

fn last_drawn(engine: &mut Engine) -> (*const DisplayCommand, usize) {
    engine.presenter.as_mut().expect("window presenter").renderer_mut().expect("renderer").last_executed_list()
}

/// The pointer-identity check: a scrolled frame of a large page draws the
/// page's own list. Before P3 the window drew `scrolled_commands`, a new
/// vector, so the renderer saw another address.
#[test]
fn scrolling_a_large_page_draws_the_pages_own_list_not_a_copy() {
    let (mut engine, view, _server) = load(&large_body(400));
    let (ptr, len) = list_ptr(&engine, view);
    assert!(len > 4_000, "a large list: {len} commands");
    let max = engine.views[&view].max_scroll_offset;
    assert!(max.1 > 5_000.0, "a long page: {max:?}");

    let viewhost_id = engine.views[&view].viewhost_id;
    for offset in [(0.0, 120.0), (0.0, 5_000.0), (0.0, max.1)] {
        let display_list = engine.views[&view].display_list.as_ref().unwrap();
        engine.presenter.as_mut().expect("window presenter").draw_scrolled(viewhost_id, display_list, offset, None).expect("draw");
        assert_eq!(last_drawn(&mut engine), (ptr, len), "at {offset:?} the renderer drew the page's list in place");
    }

    // A wheel event: the offset moves and the frame is presented from the
    // list as it stands. Nothing is rebuilt, and nothing is copied.
    let generation = engine.views[&view].frame_generation;
    assert!(engine.scroll_view(view, 0.0, -400.0).unwrap());
    assert_eq!(engine.views[&view].frame_generation, generation, "nothing was laid out for the scroll");
    assert_eq!(list_ptr(&engine, view), (ptr, len), "the page's list is the one it had");
    assert_eq!(last_drawn(&mut engine), (ptr, len), "and the scroll's frame drew it in place");
}

fn capture(engine: &mut Engine, view: EngineViewId, offset: (f32, f32), copied: bool, tag: &str) -> Vec<u8> {
    let viewhost_id = engine.views[&view].viewhost_id;
    let path = std::env::temp_dir().join(format!("rustkit-p3-{tag}-{copied}-{}.ppm", std::process::id()));
    let path_str = path.to_str().unwrap();
    if copied {
        let list = engine.views[&view].display_list.as_ref().unwrap().commands.clone();
        engine
            .presenter
            .as_mut()
            .expect("window presenter")
            .capture_commands(viewhost_id, (WIDTH, HEIGHT), path_str, &scrolled_commands(&list, offset))
            .expect("capture");
    } else {
        let display_list = engine.views[&view].display_list.as_ref().unwrap();
        engine
            .presenter
            .as_mut()
            .expect("window presenter")
            .draw_scrolled(viewhost_id, display_list, offset, Some(path_str))
            .expect("capture");
    }
    let ppm = std::fs::read(&path).expect("frame");
    let _ = std::fs::remove_file(&path);
    ppm
}

/// RGB of a pixel in a `P6` frame of the view's size.
fn pixel(ppm: &[u8], x: usize, y: usize) -> [u8; 3] {
    let pixels = WIDTH as usize * HEIGHT as usize * 3;
    let at = ppm.len() - pixels + (y * WIDTH as usize + x) * 3;
    [ppm[at], ppm[at + 1], ppm[at + 2]]
}

/// Fixed, sticky, images, an iframe and a clipping box, at offsets across
/// the page, one of them sideways: the offset draws the same bytes as the
/// copied list, and the fixed bar stays at the top of the window.
#[test]
fn fixed_sticky_images_and_iframes_draw_the_same_under_scroll() {
    let (mut engine, view, _server) = load(&large_body(80));
    // Images are uploaded by a presented frame; the captures below draw
    // without uploading.
    engine.render(view).expect("render");
    for (i, offset) in [(0.0, 0.0), (0.0, 30.0), (0.0, 95.0), (0.0, 1234.5), (17.0, 400.0)].into_iter().enumerate() {
        let tag = format!("mix{i}");
        let copied = capture(&mut engine, view, offset, true, &tag);
        let drawn = capture(&mut engine, view, offset, false, &tag);
        assert!(drawn == copied, "at {offset:?} the offset draws what the copied list drew");
        if offset.0 == 0.0 {
            // Below 20: once stuck, the sticky bar is drawn over the fixed
            // one, its elder in the tree, as in Chromium.
            assert_eq!(pixel(&drawn, 200, 22), [0, 0, 0], "at {offset:?} the fixed bar is at the top");
        }
    }
    // The sticky bar scrolls with the page until it reaches the window's top,
    // then stays there.
    let top = capture(&mut engine, view, (0.0, 0.0), false, "stick-top");
    let down = capture(&mut engine, view, (0.0, 10.0), false, "stick-down");
    let stuck = capture(&mut engine, view, (0.0, 400.0), false, "stick-stuck");
    assert_eq!(pixel(&top, 200, 40), [0, 136, 255], "the sticky bar at 24..44");
    assert_eq!(pixel(&down, 200, 30), [0, 136, 255], "10 up after a 10 px scroll");
    assert_ne!(pixel(&down, 200, 40), [0, 136, 255], "and gone from where it ended");
    assert_eq!(pixel(&stuck, 200, 10), [0, 136, 255], "held at the top, over the fixed bar");
    assert_ne!(pixel(&stuck, 200, 40), [0, 136, 255], "and no further down");
}

/// An image is uploaded the first time a frame names it, and never again
/// however often the page is scrolled and drawn.
#[test]
fn an_image_is_uploaded_once_however_often_the_page_scrolls() {
    let (mut engine, view, _server) = load(&large_body(60));
    engine.render(view).expect("render");
    let uploaded = engine.presenter.as_ref().expect("window presenter").images_uploaded;
    assert_eq!(uploaded, 2, "the red and the blue image, each once, however many rows name them");
    for _ in 0..5 {
        assert!(engine.scroll_view(view, 0.0, -150.0).unwrap());
    }
    let viewhost_id = engine.views[&view].viewhost_id;
    let display_list = engine.views[&view].display_list.as_ref().unwrap();
    engine.presenter.as_mut().expect("window presenter").draw_scrolled(viewhost_id, display_list, (0.0, 600.0), None).expect("draw");
    engine.render(view).expect("render");
    assert_eq!(engine.presenter.as_ref().expect("window presenter").images_uploaded, 2, "no image was uploaded again");
}

fn median_ms(mut samples: Vec<std::time::Duration>) -> f64 {
    samples.sort();
    samples[samples.len() / 2].as_secs_f64() * 1000.0
}

/// The before/after numbers on the PR: `render_ms` (what T1's relayout line
/// reports as the render stage) and one wheel scroll's frame, on a page of
/// `P3_ROWS` rows (default 20000). Run by hand in a release build:
/// `P3_ROWS=20000 cargo test --release -p rustkit-engine --features headless --lib -- --ignored timing_of_render_and_scroll --nocapture`
#[test]
#[ignore = "a timing, run by hand in a release build"]
fn timing_of_render_and_scroll_on_a_large_page() {
    let rows = std::env::var("P3_ROWS").ok().and_then(|r| r.parse().ok()).unwrap_or(20_000);
    let (mut engine, view, _server) = load(&large_body(rows));
    let commands = engine.views[&view].display_list.as_ref().unwrap().commands.len();
    let max = engine.views[&view].max_scroll_offset;
    let mut render = Vec::new();
    for _ in 0..7 {
        let t = std::time::Instant::now();
        engine.render(view).expect("render");
        render.push(t.elapsed());
    }
    let viewhost_id = engine.views[&view].viewhost_id;
    let mut scroll = Vec::new();
    let mut copy = Vec::new();
    for i in 1..=7 {
        let offset = (0.0, max.1 * i as f32 / 8.0);
        let display_list = engine.views[&view].display_list.as_ref().unwrap();
        let t = std::time::Instant::now();
        let copied = scrolled_commands(&display_list.commands, offset);
        copy.push(t.elapsed());
        drop(copied);
        let t = std::time::Instant::now();
        engine.presenter.as_mut().expect("window presenter").draw_scrolled(viewhost_id, display_list, offset, None).expect("draw");
        scroll.push(t.elapsed());
    }
    println!(
        "P3 rows={rows} commands={commands} render_ms={:.2} scroll_frame_ms={:.2} copy_ms={:.2}",
        median_ms(render),
        median_ms(scroll),
        median_ms(copy)
    );
}
