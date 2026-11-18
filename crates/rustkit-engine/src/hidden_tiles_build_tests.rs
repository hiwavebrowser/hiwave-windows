//! TREE-REUSE step 0: what a full build costs on a page of N tiles of which all but `k` are
//! `display: none` (MotionMark Multiply's shape: every tile is created up front and the stage
//! shows the first `complexity`). Each hidden tile is its own `display: none` element, so the
//! walk stops at it but still cascades its own style.

use super::*;
use crate::script_net_tests::serve_routes;

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

/// `n` absolutely placed tiles; the first `k` shown. `wrapped` puts the hidden ones inside one
/// `display: none` container instead of hiding each.
pub(crate) fn tiles_page(n: usize, k: usize, wrapped: bool) -> String {
    tiles_page_shaped(n, k, wrapped, true)
}

/// [`tiles_page`]; with `inline` false a tile has no `style` attribute (hidden by class).
pub(crate) fn tiles_page_shaped(n: usize, k: usize, wrapped: bool, inline: bool) -> String {
    let mut page = String::from(
        "<!doctype html><html><head><style>body{margin:0} #stage{position:relative;width:1000px;height:700px}\
         .tile{position:absolute;width:18px;height:18px;border-radius:4px;transform:rotate(10deg)}\
         .off{display:none}</style></head><body><div id=stage>",
    );
    let tile = |i: usize, hidden: bool| {
        if !inline {
            return format!(
                "<div class='tile{}' id=t{i}></div>",
                if hidden { " off" } else { "" }
            );
        }
        format!(
            "<div class=tile id=t{i} style='left:{}px;top:{}px;background-color:hsl({},80%,50%){}'></div>",
            (i % 50) * 20,
            (i / 50 % 35) * 20,
            i % 360,
            if hidden { ";display:none" } else { "" }
        )
    };
    for i in 0..k.min(n) {
        page.push_str(&tile(i, false));
    }
    if wrapped {
        page.push_str("<div class=off>");
    }
    for i in k.min(n)..n {
        page.push_str(&tile(i, !wrapped));
    }
    if wrapped {
        page.push_str("</div>");
    }
    page.push_str("</div></body></html>");
    page
}

fn load(page: &str) -> (Engine, EngineViewId, crate::script_net_tests::Server) {
    let server = serve_routes(vec![("/", "text/html", page.to_string())]);
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds {
            x: 0,
            y: 0,
            width: 1024,
            height: 768,
        })
        .expect("view");
    let url = Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap();
    rt().block_on(engine.load_url(view, url)).expect("load_url");
    (engine, view, server)
}

fn median(mut ms: Vec<f64>) -> f64 {
    ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
    ms[ms.len() / 2]
}

/// Prints `HIDDEN-TILES n k shape build_ms` (median of 7 full relayouts).
#[test]
#[ignore]
fn measure_a_full_build_over_hidden_tiles() {
    for &n in &[1000usize, 4000, 8000] {
        let shapes = [
            ("each-hidden", 10, false, true),
            ("each-hidden-by-class", 10, false, false),
            ("one-hidden-parent", 10, true, true),
            ("all-shown", n, false, true),
            ("all-shown-no-inline", n, false, false),
        ];
        for (shape, k, wrapped, inline) in shapes {
            let (mut engine, view, _server) = load(&tiles_page_shaped(n, k, wrapped, inline));
            let mut ms = Vec::new();
            for _ in 0..7 {
                engine.relayout(view).expect("relayout");
                ms.push(engine.relayout_stats(view).expect("view").last_build_ms);
            }
            eprintln!(
                "HIDDEN-TILES n {n} k {k} {shape} build_ms {:.2}",
                median(ms)
            );
        }
    }
}

/// How much of an inline-styled tile's cost is splitting its `style` attribute (done three times
/// per cascade) and how much is the cascade applying the values. Prints `INLINE-SPLIT us/tile`.
#[test]
#[ignore]
fn measure_the_inline_style_split() {
    let attr = "left:380px;top:260px;background-color:hsl(213,80%,50%);display:none";
    let t = std::time::Instant::now();
    let mut n = 0;
    for _ in 0..8000 * 3 {
        n += rustkit_css::split_declaration_list(std::hint::black_box(attr)).len();
    }
    eprintln!(
        "INLINE-SPLIT {:.2} us/tile (3 splits) n={n}",
        t.elapsed().as_secs_f64() * 1e6 / 8000.0
    );
    for (shape, k) in [("inline-display-only", 10usize)] {
        let mut page = String::from("<!doctype html><html><head><style>.tile{position:absolute;width:18px;height:18px}</style></head><body>");
        for i in 0..8000 {
            page.push_str(&format!(
                "<div class=tile id=t{i}{}></div>",
                if i >= k { " style='display:none'" } else { "" }
            ));
        }
        let (mut engine, view, _server) = load(&page);
        let mut ms = Vec::new();
        for _ in 0..7 {
            engine.relayout(view).expect("relayout");
            ms.push(engine.relayout_stats(view).expect("view").last_build_ms);
        }
        eprintln!(
            "HIDDEN-TILES n 8000 k {k} {shape} build_ms {:.2}",
            median(ms)
        );
    }
}
