//! LAT13: what compositing layers cost a frame in a 400x300 headless view.
//! An opacity group (and a polygon/path clip, which takes the same layer
//! path) is drawn into a layer and composited back; before LAT13 each layer
//! cost four GPU submits and 50 opacity layers took 15.3 ms a frame against
//! 0.47 ms with none.
//!
//! The table: `cargo test --release -p rustkit-engine --features headless --lib -- --ignored layer_frame_time_table --nocapture`
//! (debug works too; the numbers in the row are both).

use super::*;
use crate::script_net_tests::serve_routes;

const WIDTH: u32 = 400;
const HEIGHT: u32 = 300;

fn load(body: &str) -> (Engine, EngineViewId, crate::script_net_tests::Server) {
    let page = format!(
        "<html><head><style>body{{margin:0}} \
         .b{{position:absolute;width:36px;height:36px;background:#2060c0}} \
         .o{{opacity:0.5}} \
         .p{{clip-path:polygon(50% 0, 100% 100%, 0 100%)}}</style></head>\
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

/// `n` boxes of class `class` in a grid of 10 columns, 40 px apart.
pub(crate) fn boxes(n: usize, class: &str) -> String {
    (0..n)
        .map(|i| {
            let (x, y) = ((i % 10) * 40 + 2, (i / 10) * 40 + 2);
            format!("<div class='b {class}' style='left:{x}px;top:{y}px'></div>")
        })
        .collect()
}

/// Median of `frames` frames of the page's list drawn into the view:
/// (CPU ms per draw, ms per draw including the wait for the GPU).
pub(crate) fn frame_ms(body: &str, frames: usize) -> (f64, f64) {
    let (mut engine, view, _server) = load(body);
    engine.render(view).expect("render");
    let viewhost_id = engine.views[&view].viewhost_id;
    let list = engine.views[&view].display_list.clone().expect("display list");
    let presenter = engine.presenter.as_mut().expect("window presenter");
    let (mut cpu, mut total) = (Vec::new(), Vec::new());
    for i in 0..frames + 3 {
        let t = std::time::Instant::now();
        presenter.draw_scrolled(viewhost_id, &list, (0.0, 0.0), None).expect("draw");
        let drawn = t.elapsed();
        presenter.wait_for_gpu();
        if i >= 3 {
            cpu.push(drawn.as_secs_f64() * 1e3);
            total.push(t.elapsed().as_secs_f64() * 1e3);
        }
    }
    let median = |mut v: Vec<f64>| {
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        v[v.len() / 2]
    };
    (median(cpu), median(total))
}

#[test]
#[ignore = "a timing, run by hand"]
fn layer_frame_time_table() {
    for (what, body) in [
        ("0 layers", boxes(50, "")),
        ("1 opacity layer", boxes(1, "o") + &boxes(49, "")),
        ("50 opacity layers", boxes(50, "o")),
        ("50 polygon clips", boxes(50, "p")),
    ] {
        let (cpu, total) = frame_ms(&body, 21);
        println!("LAT13 {what:>20}: cpu {cpu:.3} ms/frame, with GPU wait {total:.3} ms/frame");
    }
}
