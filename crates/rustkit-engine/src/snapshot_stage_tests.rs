//! The snapshot stage (LAT8). After a layout, the first script read of
//! geometry or computed style builds the maps those reads answer from
//! (`publish_geometry`): every element's box and, until LAT8, every
//! element's computed style formatted as `name\tvalue` rows, though a read
//! asks for one element. LAT0's baseline: 33 s of `snapshot_ms` inside live
//! turns, en.wikipedia's p50 436 ms a turn; a page with a timer that writes
//! and reads paid it every turn.
//!
//! Now a layout's computed styles are formatted per element, on the first
//! read of that element, and kept for the next read of the same layout; a
//! turn whose frame did not change publishes nothing, so its reads answer
//! from the maps already built.

use super::*;
use crate::script_net_tests::serve_routes;

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

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

/// One live turn of 16 ms.
fn turn(engine: &mut Engine, view: EngineViewId) {
    rt().block_on(engine.pump_live(view, 16));
}

fn read(engine: &mut Engine, view: EngineViewId, expr: &str) -> String {
    let value = engine
        .execute_script(view, &format!("String({expr})"))
        .unwrap();
    value
        .strip_prefix("String(\"")
        .and_then(|v| v.strip_suffix("\")"))
        .unwrap_or(&value)
        .to_string()
}

fn builds(engine: &Engine, view: EngineViewId) -> u64 {
    engine.views[&view]
        .bindings
        .as_ref()
        .expect("bindings")
        .geometry_builds()
}

/// A page of `n` boxes and one timer running `body` every 16 ms.
fn boxes_with_timer(n: usize, body: &str) -> String {
    let mut page = String::from(
        "<html><head><style>body{margin:0;font:14px sans-serif} .b{display:inline-block;width:40px;height:20px;\
         margin:2px;padding:1px;border:1px solid #888;color:#333;background:#eef}</style></head><body>",
    );
    for i in 0..n {
        page.push_str(&format!("<div class=b id=b{i}>{i}</div>"));
    }
    page.push_str(&format!(
        "<script>var i=0, el=document.getElementById('b0'); window.__reads=[]; setInterval(function(){{ {body} }}, 16);</script></body></html>"
    ));
    page
}

/// What the snapshot cost from `before`: (builds, element style rows
/// formatted, ms).
fn snapshot_since(engine: &Engine, view: EngineViewId, before: (u64, u64, f64)) -> (u64, u64, f64) {
    let (lines, ms) = snapshot_counters();
    (builds(engine, view) - before.0, lines - before.1, ms - before.2)
}

fn snapshot_mark(engine: &Engine, view: EngineViewId) -> (u64, u64, f64) {
    let (lines, ms) = snapshot_counters();
    (builds(engine, view), lines, ms)
}

/// A turn whose timer writes one box and reads it back builds one snapshot
/// and formats the computed style of no more elements than it reads, not
/// the page's 500.
#[test]
fn a_changed_frame_snapshots_once_and_only_what_is_read() {
    let (mut engine, view, _server) = load(&boxes_with_timer(
        500,
        "el.style.width = (40 + (i++ % 20)) + 'px'; window.__reads.push(getComputedStyle(el).width);",
    ));
    for _ in 0..3 {
        turn(&mut engine, view);
    }
    let before = snapshot_mark(&engine, view);
    for _ in 0..10 {
        turn(&mut engine, view);
    }
    let (built, lines, _) = snapshot_since(&engine, view, before);
    assert!((1..=20).contains(&built), "one snapshot a changed turn (forced layout and flush): {built}");
    assert!(lines <= 20, "{lines} element style rows formatted for 10 one-element reads");
    let reads = read(&mut engine, view, "window.__reads.slice(-3).join()");
    assert!(reads.split(',').all(|w| w.ends_with("px") && w != "40px" || w == "40px"), "{reads}");
}

/// The read answers what the write made, and the next layout's read sees
/// the next layout (a lazily formatted style is not kept past its layout).
#[test]
fn a_lazily_formatted_style_follows_the_layout() {
    let (mut engine, view, _server) = load(&boxes_with_timer(20, ""));
    assert_eq!(read(&mut engine, view, "getComputedStyle(document.getElementById('b3')).width"), "40px");
    assert_eq!(read(&mut engine, view, "getComputedStyle(document.getElementById('b3')).color"), "rgb(51, 51, 51)");
    read(&mut engine, view, "document.getElementById('b3').style.width = '55px'");
    assert_eq!(read(&mut engine, view, "getComputedStyle(document.getElementById('b3')).width"), "55px");
    read(&mut engine, view, "document.getElementById('b3').style.color = 'rgb(1, 2, 3)'");
    turn(&mut engine, view);
    assert_eq!(read(&mut engine, view, "getComputedStyle(document.getElementById('b3')).color"), "rgb(1, 2, 3)");
    assert_eq!(read(&mut engine, view, "document.body.innerText.length > 20"), "true");
}

/// A turn that changes nothing (the timer only reads) builds no snapshot:
/// the reads answer from the one already built.
#[test]
fn an_unchanged_frames_turn_takes_no_snapshot() {
    let (mut engine, view, _server) = load(&boxes_with_timer(
        500,
        "window.__reads.push(el.getBoundingClientRect().width + ' ' + getComputedStyle(el).color);",
    ));
    for _ in 0..3 {
        turn(&mut engine, view);
    }
    let before = snapshot_mark(&engine, view);
    for _ in 0..10 {
        turn(&mut engine, view);
    }
    let (built, lines, ms) = snapshot_since(&engine, view, before);
    assert_eq!((built, lines), (0, 0), "unchanged turns built {built} snapshots ({ms:.1} ms)");
    assert_eq!(read(&mut engine, view, "window.__reads.length >= 10"), "true");
}

/// A paint-only animation frame that changes nothing publishes nothing.
#[test]
fn a_held_animation_frame_takes_no_snapshot() {
    let page = "<html><head><style>@keyframes f{from{opacity:.5}to{opacity:.5}} #a{width:50px;height:50px;\
        animation:f 10s infinite;background:red}</style></head><body><div id=a></div>\
        <script>window.__n=0;setInterval(function(){ window.__n += document.getElementById('a').offsetWidth; },16)</script></body></html>";
    let (mut engine, view, _server) = load(page);
    for _ in 0..3 {
        turn(&mut engine, view);
    }
    let before = snapshot_mark(&engine, view);
    for _ in 0..10 {
        turn(&mut engine, view);
    }
    let (built, _, _) = snapshot_since(&engine, view, before);
    assert_eq!(built, 0, "frames of a held animation built {built} snapshots");
}

/// Measurement (not a gate): snapshot ms per turn on a 500-box page with
/// one timer that writes a box and reads it.
/// `cargo test -p rustkit-engine --features headless --release snapshot_stage_measure -- --ignored --nocapture`
#[test]
#[ignore]
fn snapshot_stage_measure() {
    for (name, body) in [
        ("write+getComputedStyle", "el.style.width = (40 + (i++ % 20)) + 'px'; window.__reads.push(getComputedStyle(el).width);"),
        ("write+getBoundingClientRect", "el.style.width = (40 + (i++ % 20)) + 'px'; window.__reads.push(el.getBoundingClientRect().width);"),
        ("read only", "window.__reads.push(el.getBoundingClientRect().width);"),
    ] {
        let (mut engine, view, _server) = load(&boxes_with_timer(500, body));
        for _ in 0..3 {
            turn(&mut engine, view);
        }
        let before = snapshot_mark(&engine, view);
        let started = std::time::Instant::now();
        for _ in 0..30 {
            turn(&mut engine, view);
        }
        let wall = started.elapsed().as_secs_f64() * 1000.0;
        let (built, lines, ms) = snapshot_since(&engine, view, before);
        eprintln!(
            "LAT8 {name}: 30 turns, {built} snapshots, {lines} style rows, snapshot {:.2} ms/turn, turn {:.2} ms",
            ms / 30.0,
            wall / 30.0
        );
    }
}

/// Each element's lazily formatted style is the row set the whole-page
/// snapshot built for it: boxes, an element split over lines, boxless
/// elements (`display: contents`, `display: none`), custom properties
/// (own and inherited) and pseudo-elements.
#[test]
fn lazily_formatted_styles_equal_the_whole_page_snapshot() {
    let page = "<html><head><style>:root{--a:1px;font-size:12px} body{margin:0} .c{display:contents;color:red}\
        .n{display:none} .v{--b:2em;width:var(--b)} p::before{content:'x';color:blue;--p:3} p::after{content:'y'}\
        span{border-left:1px solid} li{margin-left:1rem}</style></head><body>\
        <div class=c><p>in contents <span>a span that wraps over more than one line of the narrow block it is in, \
        and more, and more</span></p></div><div class=n><b>hidden</b></div><div class=v><ul><li>one<li>two</ul></div>\
        <table><tr><td>cell</td></tr></table><svg width=10 height=10><rect width=5 height=5></rect></svg></body></html>";
    let (engine, view, _server) = load(page);
    let state = &engine.views[&view];
    let layout = state.layout.clone().expect("laid out");
    let root = root_element_id(state);
    let eager = geometry_with_root(&layout, root, &state.lay.boxless_styles.borrow(), &state.lay.pseudo_styles.borrow()).1;
    let lazy_layout = layout.clone();
    let lazy = computed_lines_source(move || Some(lazy_layout.clone()), root, state.lay.clone());
    assert!(eager.len() >= 12, "{} elements", eager.len());
    // The rows as a set: custom properties come out of a hash map, in no
    // fixed order, in either snapshot (`custom_property_lines`).
    let rows = |lines: &str| {
        let mut rows: Vec<&str> = lines.lines().collect();
        rows.sort_unstable();
        rows.join("\n")
    };
    for (node, lines) in &eager {
        let got = lazy(*node).unwrap_or_else(|| panic!("node {node} has no lazy style"));
        assert_eq!(rows(&got), rows(lines), "node {node}");
    }
    let max = eager.keys().max().copied().unwrap_or(0);
    for node in 0..max + 5 {
        if !eager.contains_key(&node) {
            assert_eq!(lazy(node), None, "node {node} has no style in the whole-page snapshot");
        }
    }
}
