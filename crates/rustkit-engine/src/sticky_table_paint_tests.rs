//! Plan row C7: a stuck table part paints and hits over the rows scrolled
//! under it. Chromium 143.0.7499.4 on the same page, at `scrollTop` 100:
//! the stuck header's blue covers (10,10) and (150,10), the rows' lime
//! shows at (10,30), and `elementFromPoint(10,10)` is the header cell `a`
//! for a sticky `th`, `tr` or `thead` and for a `th` with `z-index: 1`.
//!
//! Ignored: the engine orders positioned boxes among siblings only, and
//! lifting the row group put it over positioned body-cell content that
//! Chromium draws above it (#868 R1 delta 3). These wait for stacking-context
//! hoisting, a follow-up row after C7.

use super::*;
use crate::script_net_tests::serve_routes;

const W: usize = 800;
const H: usize = 600;
const BLUE: [u8; 3] = [0, 0, 255];
const LIME: [u8; 3] = [0, 255, 0];

fn page(sticky: &str) -> String {
    let rows = "<tr><td></td><td></td></tr>".repeat(20);
    format!(
        "<html><head><style>body{{margin:0}}#s{{overflow:auto;width:300px;height:100px}}\
         table{{border-collapse:collapse;border-spacing:0}}td,th{{padding:0;height:20px;width:100px}}\
         td{{background:#0f0}}th{{background:#00f}}{sticky}</style></head><body><div id=s><table>\
         <thead id=hd><tr id=tr><th id=a></th><th id=b></th></tr></thead><tbody>{rows}</tbody></table></div></body></html>"
    )
}

fn pixel(ppm: &[u8], x: usize, y: usize) -> [u8; 3] {
    let at = ppm.len() - W * H * 3 + (y * W + x) * 3;
    [ppm[at], ppm[at + 1], ppm[at + 2]]
}

/// The frame and the two hits at `scrollTop` 100.
fn scrolled(sticky: &str, tag: &str) -> (Vec<u8>, String) {
    let server = serve_routes(vec![("/", "text/html", page(sticky))]);
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: W as u32, height: H as u32 })
        .expect("view");
    let url = Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap();
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    rt.block_on(engine.load_url(view, url)).expect("load_url");
    let hits = engine
        .execute_script(
            view,
            "document.getElementById('s').scrollTop = 100; \
             var e = document.elementFromPoint(10, 10), f = document.elementFromPoint(150, 30); \
             (e && (e.id || e.tagName)) + ' ' + (f && (f.id || f.tagName))",
        )
        .expect("script");
    engine.render_view(view).expect("render");
    let viewhost_id = engine.views[&view].viewhost_id;
    let path = std::env::temp_dir().join(format!("rustkit-c7-{tag}-{}.ppm", std::process::id()));
    let display_list = engine.views[&view].display_list.as_ref().expect("list");
    engine
        .presenter
        .as_mut()
        .expect("presenter")
        .draw_scrolled(viewhost_id, display_list, (0.0, 0.0), Some(path.to_str().unwrap()))
        .expect("capture");
    let ppm = std::fs::read(&path).expect("frame");
    let _ = std::fs::remove_file(&path);
    (ppm, hits)
}

#[track_caller]
fn stuck_on_top(sticky: &str, tag: &str) {
    let (ppm, hits) = scrolled(sticky, tag);
    // Hits, then (10,10) and (150,10) on the stuck header, (10,30) on a row.
    assert_eq!(
        (hits.as_str(), pixel(&ppm, 10, 10), pixel(&ppm, 150, 10), pixel(&ppm, 10, 30)),
        ("String(\"a TD\")", BLUE, BLUE, LIME),
        "{tag}"
    );
}

#[test]
#[ignore = "sticky table parts need stacking-context hoisting: PLAN-z follow-up row after C7 (#868 R1 delta 3); expected values are Chromium 143"]
fn a_stuck_th_paints_and_hits_over_the_scrolled_rows() {
    stuck_on_top("th{position:sticky;top:0}", "th");
}

#[test]
#[ignore = "sticky table parts need stacking-context hoisting: PLAN-z follow-up row after C7 (#868 R1 delta 3); expected values are Chromium 143"]
fn a_stuck_tr_paints_and_hits_over_the_scrolled_rows() {
    stuck_on_top("#tr{position:sticky;top:0}", "tr");
}

#[test]
#[ignore = "sticky table parts need stacking-context hoisting: PLAN-z follow-up row after C7 (#868 R1 delta 3); expected values are Chromium 143"]
fn a_stuck_thead_paints_and_hits_over_the_scrolled_rows() {
    stuck_on_top("#hd{position:sticky;top:0}", "thead");
}

#[test]
#[ignore = "sticky table parts need stacking-context hoisting: PLAN-z follow-up row after C7 (#868 R1 delta 3); expected values are Chromium 143"]
fn a_stuck_th_with_a_z_index_paints_and_hits_over_the_scrolled_rows() {
    stuck_on_top("th{position:sticky;top:0;z-index:1}", "th-z");
}
