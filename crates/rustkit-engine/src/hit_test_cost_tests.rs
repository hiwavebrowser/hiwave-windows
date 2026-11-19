//! What a pointer move's hit test costs (R2 MED 6, R3 LOW 8 and R4 MED C on #1151). The timer wraps
//! `Engine::hit_at` only: stamping the sticky shifts and the layout's hit test (fixed boxes, then the page
//! in stacking order), with no event dispatch and no script. Every figure is a median over a loop.
//!
//! Page shapes, each with a small static scroller (`#sc`), an animated box (`#an`) and a box whose colour a
//! script changes (`#pp`), all at the top right:
//! - flat: 2000 `position: relative` cards in one wrapper, each with an absolute badge, then a chain of
//!   300 nested `position: relative` boxes;
//! - nested: 500 such cards, each under ten plain wrappers, and the same chain;
//! - badge_out: the flat page with each badge sticking out of its card (no card can be skipped by its box);
//! - scrolled_list: 2000 such cards in a static `overflow: auto` list scrolled by 400px (the hit test cannot
//!   skip by bounds under a scroll it escapes).
//!
//! Moments, each a loop of an action then one timed hit:
//! - steady: hits with nothing between them;
//! - after_layout: a full relayout and its frame;
//! - after_scroll: `#sc.scrollTop` set by script (the list is rebuilt over the same boxes);
//! - after_patch: `#pp`'s colour set by script (a paint-only restyle);
//! - after_frame: one live turn of 16 ms (`#an`'s animation frame).
//! Each moment also prints how many full relayouts and skipped layouts its loop caused, so the label is
//! what happened. Ignored: run by hand with
//! `cargo test --profile parity -p rustkit-engine --features headless --lib hit_test_cost -- --ignored --nocapture`.

use super::*;
use crate::script_net_tests::serve_routes;
use std::time::Instant;

fn page(shape: &str) -> String {
    let card = |i: usize, badge: &str| {
        format!("<div class=card><div class=pic></div><span class=badge style='{badge}'>{i}</span><p>card {i}</p></div>")
    };
    let mut body = String::from(
        "<div id=sc style='position:fixed;right:0;top:0;width:120px;height:60px;overflow:auto'><div style='height:400px'>scroll</div></div>\
         <div id=an style='position:fixed;right:0;top:70px;width:40px;height:40px;background:#08f;animation:pulse 1s linear infinite'></div>\
         <div id=pp style='position:fixed;right:0;top:120px;width:40px;height:40px;background:#0a0'></div>",
    );
    match shape {
        "scrolled_list" => {
            body.push_str("<div id=list style='height:780px;overflow:auto'><div class=wrap>");
            for i in 0..2000 {
                body.push_str(&card(i, ""));
            }
            body.push_str("</div></div>");
        }
        _ => {
            let (cards, depth) = if shape == "nested" { (500, 10) } else { (2000, 0) };
            let badge = if shape == "badge_out" { "right:-12px;top:-6px" } else { "" };
            body.push_str("<div class=wrap>");
            for i in 0..cards {
                body.push_str(&"<div class=nest>".repeat(depth));
                body.push_str(&card(i, badge));
                body.push_str(&"</div>".repeat(depth));
            }
            body.push_str("</div>");
            body.push_str(&"<div class=chain>".repeat(300));
            body.push_str("<div class=leaf>leaf</div>");
            body.push_str(&"</div>".repeat(300));
        }
    }
    format!(
        "<!doctype html><html><head><style>\
         body{{margin:0;font:12px/16px sans-serif}}\
         .wrap{{display:flex;flex-wrap:wrap}}\
         .card{{position:relative;width:180px;height:120px;margin:4px;background:#eee}}\
         .pic{{height:80px;background:#ccd}}\
         .badge{{position:absolute;right:4px;top:4px;background:#c00;color:#fff}}\
         .chain{{position:relative;padding-left:1px}}\
         @keyframes pulse{{0%{{opacity:1}}50%{{opacity:.3}}100%{{opacity:1}}}}\
         </style></head><body>{body}</body></html>"
    )
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}

/// One timed hit, in microseconds, and the node it names.
fn timed(engine: &Engine, view: EngineViewId, (x, y): (f32, f32)) -> (f64, Option<usize>) {
    let started = Instant::now();
    let hit = Engine::hit_at(&engine.views[&view], x, y);
    (started.elapsed().as_secs_f64() * 1e6, hit.and_then(|h| h.node_id))
}

#[test]
#[ignore]
fn a_pointer_move_hit_test_on_a_large_page() {
    for shape in ["flat", "nested", "badge_out", "scrolled_list"] {
        measure(shape);
    }
}

fn measure(shape: &str) {
    let server = serve_routes(vec![("/", "text/html", page(shape))]);
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine.create_headless_view(Bounds::new(0, 0, 1280, 800)).expect("view");
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let url = Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap();
    rt.block_on(engine.load_url(view, url)).expect("load_url");
    if shape == "scrolled_list" {
        engine.execute_script(view, "document.getElementById('list').scrollTop = 400; 0").unwrap();
    }
    engine.render_view(view).expect("render");
    // Points on the page, away from the fixed boxes at the top right.
    let points: Vec<(f32, f32)> = (0..400).map(|i| ((i * 97 % 1100) as f32, (i * 61 % 780) as f32)).collect();
    let stats = |engine: &Engine| {
        let s = engine.relayout_stats(view).expect("view");
        (s.relayouts, s.layouts_skipped)
    };

    // Steady, and which node each point hits (the same signature on two builds: the same answers).
    let _ = timed(&engine, view, points[0]);
    let (mut steady, mut sig) = (Vec::new(), 0u64);
    for _ in 0..3 {
        for &p in &points {
            let (us, node) = timed(&engine, view, p);
            steady.push(us);
            sig = sig.wrapping_mul(31).wrapping_add(node.unwrap_or(0) as u64);
        }
    }
    let mut line = format!("HIT_COST {shape} steady={:.1}", median(steady));

    let mut moment = |name: &str, n: usize, engine: &mut Engine, act: &mut dyn FnMut(&mut Engine, usize)| {
        let before = stats(engine);
        let mut v = Vec::new();
        for k in 0..n {
            act(engine, k);
            v.push(timed(engine, view, points[k % points.len()]).0);
        }
        let after = stats(engine);
        line.push_str(&format!(
            " {name}={:.1}(relayouts+{} skipped+{})",
            median(v),
            after.0 - before.0,
            after.1 - before.1
        ));
    };
    moment("after_layout", 11, &mut engine, &mut |e, _| {
        e.relayout(view).expect("relayout");
        e.render_view(view).expect("render");
    });
    moment("after_scroll", 31, &mut engine, &mut |e, k| {
        e.execute_script(view, &format!("document.getElementById('sc').scrollTop = {}; 0", (k % 2) * 30 + 10))
            .expect("scroll");
    });
    moment("after_patch", 31, &mut engine, &mut |e, k| {
        let colour = if k % 2 == 0 { "#a00" } else { "#0a0" };
        e.execute_script(view, &format!("document.getElementById('pp').style.backgroundColor = '{colour}'; 0"))
            .expect("patch");
    });
    moment("after_frame", 31, &mut engine, &mut |e, _| {
        rt.block_on(e.pump_live(view, 16));
    });
    println!("{line} sig={sig}");
}
