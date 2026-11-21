//! A view that goes from page A to page B must end as if it had loaded
//! page B first (hand test 8, H27: "does layout state carry over between
//! pages"). The per-document state is dropped where the new document is
//! stored; until 2026-10-08 `load_url` left the last page's external
//! sheets on the view until its own had been fetched, so page B's first
//! layout was cascaded with page A's rules and then thrown away, and
//! neither load path dropped the record of the images page A had asked for.

use super::*;
use crate::script_net_tests::serve_routes;

const PAGE_A: &str = "<html><head><link rel=stylesheet href=/a.css></head><body>\
    <p id=x>page a</p></body></html>";
const A_CSS: &str = "p { color: red; margin-left: 50px; width: 123px; background: yellow }";
/// Links no sheet: its first layout is not deferred.
const PAGE_B: &str = "<html><head><style>body{margin:0}</style></head><body>\
    <p id=x>page b</p><div style=\"width:40px;height:40px;background:blue\"></div></body></html>";

fn routes() -> Vec<(&'static str, &'static str, String)> {
    vec![
        ("/a", "text/html", PAGE_A.to_string()),
        ("/a.css", "text/css", A_CSS.to_string()),
        ("/b", "text/html", PAGE_B.to_string()),
    ]
}

/// Loads `paths` one after the other in one view and returns the last
/// page's display list with the number of display lists that load built.
fn frame_after(paths: &[&str]) -> (String, u64) {
    let server = serve_routes(routes());
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 400, height: 200 })
        .expect("view");
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let mut before = 0;
    for path in paths {
        before = engine.views[&view].frame_generation;
        let url = Url::parse(&format!("http://127.0.0.1:{}{path}", server.port)).unwrap();
        rt.block_on(engine.load_url(view, url)).expect("load_url");
    }
    let state = &engine.views[&view];
    (format!("{:?}", state.display_list), state.frame_generation - before)
}

#[test]
fn page_b_after_page_a_is_page_b_loaded_fresh() {
    let (fresh, _) = frame_after(&["/b"]);
    let (after_a, _) = frame_after(&["/a", "/b"]);
    assert_ne!(fresh, "None", "page b was laid out");
    assert_eq!(after_a, fresh, "page b's frame after page a");
}

/// Page A's sheet is gone before page B is first laid out: no layout of
/// page B under page A's rules, to be done over once they are dropped.
#[test]
fn page_b_is_not_laid_out_with_page_a_s_sheet_first() {
    let (_, fresh) = frame_after(&["/b"]);
    let (_, after_a) = frame_after(&["/a", "/b"]);
    assert_eq!(after_a, fresh, "display lists built for page b after page a, and fresh");
}

/// The record of the images a document asked for belongs to that document.
/// `load_html` fetches nothing, so nothing after it starts the record over.
#[test]
fn a_document_loaded_from_a_string_starts_with_no_image_record() {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 400, height: 200 })
        .expect("view");
    engine
        .views
        .get_mut(&view)
        .unwrap()
        .images_attempted
        .insert(Url::parse("http://127.0.0.1:9/gone.png").unwrap());
    engine.load_html(view, "<html><body><p>next</p></body></html>").expect("load_html");
    assert!(
        engine.views[&view].images_attempted.is_empty(),
        "the last document's image record was kept for the next one"
    );
}
