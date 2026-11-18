//! The relayout line's stage times on a frame's document (FRAME-STAGES).
//!
//! batch 97's MotionMark Multiply logs: every relayout of the suite's frame document (355 in one
//! run) logged `layout_ms=0.0` and `render_ms=0.0`, short-path hits included, and booked about 92%
//! of `took_ms` to `build_ms`. A top-level document's relayout of the same kind reports its layout.
//! Each stage's time must be booked once, to its own field, on the line of the view that spent it.

#![cfg(all(target_os = "macos", feature = "headless"))]

use super::*;
use crate::canvas_context_tests::serve_with_redirects;
use crate::relayout_cause_engine_tests::relayout_lines;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;

const INNER: &str = "<!doctype html><html><head><style>.c{display:inline-block;width:20px;height:10px;margin:1px}</style></head>\
<body><div id=a style='width:100px;height:10px'></div><div id=g></div><script>\
var g = document.getElementById('g'); for (var i = 0; i < 2000; i++) { var d = document.createElement('div'); d.className = 'c'; g.appendChild(d); }\
var n = 0; function step() { document.getElementById('a').style.width = (100 + n) + 'px'; if (++n < 30) setTimeout(step, 20); else window.__done = 1; }\
setTimeout(step, 20);</script></body></html>";

fn field(line: &str, key: &str) -> f64 {
    let at = line.find(&format!(" {key}=")).unwrap_or_else(|| panic!("no {key}: {line}")) + key.len() + 2;
    let rest = &line[at..];
    rest[..rest.find(' ').unwrap_or(rest.len())].parse().unwrap_or_else(|_| panic!("{key}: {line}"))
}

fn view_of(line: &str) -> String {
    let at = line.rfind("id=EngineViewId(").expect("a view id");
    line[at..line[at..].find(')').map(|end| at + end + 1).unwrap()].to_string()
}

/// Where the engine draws: through a presenter, as parity-capture and the engine tests do
/// (`create_headless_view`), or with none, as the app's page thread does (`build_page`: the
/// window thread presents what the page publishes).
#[derive(Clone, Copy)]
enum Host {
    Presenter,
    PageThread,
}

/// The `relayout cause=` lines of a page's script relayouts after its load. `framed`: the page is
/// in a frame of a parent document, on the nested contexts.
fn script_relayouts(host: Host, framed: bool) -> Vec<String> {
    let port = serve_with_redirects(|_| {
        vec![
            ("/parent.html".into(), 200, String::new(), "text/html",
             b"<!doctype html><body style='margin:0'><iframe src='/inner.html' style='width:780px;height:560px;border:0'></iframe><script>window.parentRan = 1;</script></body>".to_vec()),
            ("/inner.html".into(), 200, String::new(), "text/html", INNER.as_bytes().to_vec()),
        ]
    });
    let mut builder = EngineBuilder::new().nested_contexts(framed).post_load_requests_live(true);
    // No timer runs inside the load (horizon 0): the steps are live turns, as on the suite's page.
    builder.config.timer_horizon_ms = 0;
    let bounds = Bounds { x: 0, y: 0, width: 800, height: 600 };
    let (mut engine, view) = match host {
        Host::Presenter => {
            let mut engine = builder.build().expect("engine");
            let view = engine.create_headless_view(bounds).expect("view");
            (engine, view)
        }
        Host::PageThread => {
            let mut engine = builder.build_page(Arc::new(AtomicU64::new(0))).expect("page engine");
            let view = engine.create_page_view(bounds).expect("page view");
            (engine, view)
        }
    };
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let page = if framed { "parent.html" } else { "inner.html" };
    rt.block_on(engine.load_url(view, Url::parse(&format!("http://127.0.0.1:{port}/{page}")).unwrap())).expect("load_url");
    let lines = relayout_lines(|| {
        // Counted in turns: 30 steps 20 ms apart on the page's clock, with room to spare. The short
        // sleep lets the frame's document arrive from the test server (the frame's fetch is real
        // network); no figure below depends on it.
        for _ in 0..300 {
            std::thread::sleep(std::time::Duration::from_millis(5));
            rt.block_on(engine.pump_live(view, 20));
        }
    });
    // The page's steps all ran: 30 writes.
    let target = if framed { "document.querySelector('iframe').contentWindow" } else { "window" };
    assert_eq!(engine.execute_script(view, &format!("String({target}.__done)")).unwrap(), r#"String("1")"#);
    lines.into_iter().filter(|line| line.contains("script_mutation")).collect()
}

fn assert_stages_booked(lines: &[String], what: &str) {
    // At least one: the page's 30 writes all ran (checked by `__done` above), but how many relayouts
    // they fold into depends on how long each relayout takes, so a count above one would test the
    // machine's speed, not the stage booking.
    assert!(!lines.is_empty(), "{what}: no script relayout: {lines:#?}");
    let full: Vec<&String> = lines.iter().filter(|l| l.contains("layout_skipped=false") && !l.contains("script_restyle=hit")).collect();
    assert!(!full.is_empty(), "{what}: no relayout that laid out: {lines:#?}");
    for line in &full {
        assert!(field(line, "layout_ms") > 0.0, "{what}: a relayout that laid 2000 boxes out reports no layout time: {line}");
        assert!(field(line, "display_list_ms") > 0.0, "{what}: and no display list: {line}");
    }
    for line in lines {
        let sum: f64 = ["build_ms", "layout_ms", "display_list_ms", "snapshot_ms", "render_ms", "svg_ms"].iter().map(|k| field(line, k)).sum();
        // Six figures printed to 0.1 ms each.
        assert!(sum <= field(line, "took_ms") + 0.3, "{what}: the stages sum past the whole: {line}");
    }
}

fn assert_frame_stages_booked(host: Host, what: &str) {
    let lines = script_relayouts(host, true);
    let frame_view = lines.first().map(|l| view_of(l)).unwrap_or_default();
    assert!(!lines.is_empty() && frame_view != "id=EngineViewId(1)", "{what}: no relayout line of the frame's: {lines:#?}");
    let frame: Vec<String> = lines.iter().filter(|l| view_of(l) == frame_view).cloned().collect();
    assert_stages_booked(&frame, what);
}

#[test]
fn a_top_level_documents_relayout_books_each_stage() {
    assert_stages_booked(&script_relayouts(Host::Presenter, false), "top level, presenter");
}

#[test]
fn a_top_level_documents_relayout_books_each_stage_on_the_page_thread() {
    assert_stages_booked(&script_relayouts(Host::PageThread, false), "top level, page thread");
}

#[test]
fn a_frame_documents_relayout_books_each_stage_to_its_own_line() {
    assert_frame_stages_booked(Host::Presenter, "frame, presenter");
}

#[test]
fn a_frame_documents_relayout_books_each_stage_on_the_page_thread() {
    assert_frame_stages_booked(Host::PageThread, "frame, page thread");
}
