//! LAT14: animation frames after a document replacement, and frames of a
//! page whose Web Animations target elements no build visits.
//!
//! Pete's hand test of 2026-11-08 logged every CSS animation tick on live
//! pages as `relayout cause=animation ... dirty_nodes=0 would_path=full
//! full_reason=no_dom_record`: a full build and layout per frame (670 ms
//! on wikipedia, 453 times), with LAT2's paint-only path never engaging.
//! The first five sites of the session had no such tick; every site after
//! the first page that ran `element.animate()` did. The view's animated-
//! style layer (`AnimatedStyles`, shared with each document's bindings) was
//! never cleared on navigation, and its entries are keyed by raw NodeId,
//! which restarts with every document: `LayoutCtx::animation_frame` then
//! found a layer entry for a node the new document never captured and
//! refused every frame (`element N animates with no captured style`),
//! and a new element that happened to get the old node's id took the old
//! page's animated values.
//!
//! `no_dom_record` was never about a missing record: it is the label of a
//! relayout nobody computed a script-write dirty set for. An animation
//! relayout now says `full_reason=animation`, and the relayout line names
//! why the frame could not be painted (`anim_miss=...`).

use super::*;
use crate::script_net_tests::serve_routes;
use rustkit_layout::DisplayCommand;

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

/// A page for each route, the first one loaded.
fn load_routes(routes: Vec<(&'static str, &str)>) -> (Engine, EngineViewId, crate::script_net_tests::Server) {
    let first = routes[0].0;
    let server = serve_routes(
        routes
            .into_iter()
            .map(|(path, page)| (path, "text/html", page.to_string()))
            .collect(),
    );
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds {
            x: 0,
            y: 0,
            width: 800,
            height: 600,
        })
        .expect("view");
    go(&mut engine, view, &server, first);
    (engine, view, server)
}

/// Navigate the view to `path` on `server` (a document replacement).
fn go(engine: &mut Engine, view: EngineViewId, server: &crate::script_net_tests::Server, path: &str) {
    let url = Url::parse(&format!("http://127.0.0.1:{}{path}", server.port)).unwrap();
    rt().block_on(engine.load_url(view, url)).expect("load_url");
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

/// One live turn of `ms` (one animation frame at 16 ms).
fn frame(engine: &mut Engine, view: EngineViewId, ms: u64) {
    rt().block_on(engine.pump_live(view, ms));
}

/// Full relayouts of the current document whose first cause was an
/// animation frame.
fn animation_relayouts(engine: &Engine, view: EngineViewId) -> u32 {
    engine
        .relayout_stats(view)
        .expect("view")
        .by_first_cause
        .get("animation")
}

fn relayouts(engine: &Engine, view: EngineViewId) -> u32 {
    engine.relayout_stats(view).expect("view").relayouts
}

fn skipped(engine: &Engine, view: EngineViewId) -> u32 {
    engine.relayout_stats(view).expect("view").layouts_skipped
}

fn opacities(engine: &Engine, view: EngineViewId) -> Vec<f32> {
    engine.views[&view]
        .display_list
        .as_ref()
        .expect("a display list")
        .commands
        .iter()
        .filter_map(|c| match c {
            DisplayCommand::PushOpacity(o) => Some(*o),
            _ => None,
        })
        .collect()
}

/// The reason the last relayout took the full path, as its line says.
fn last_full_reason(engine: &Engine, view: EngineViewId) -> Option<&'static str> {
    engine.views[&view].last_dirty.as_ref().and_then(|d| d.full)
}

/// A page with many elements before the one script animates: its raw
/// NodeId is higher than any node of the small pages below.
const WAAPI_PAGE: &str = "<!doctype html><style>body{margin:0}#a{width:40px;height:40px;background:green}</style>\
    <p>1</p><p>2</p><p>3</p><p>4</p><p>5</p><p>6</p><p>7</p><p>8</p><p>9</p><p>10</p>\
    <p>11</p><p>12</p><p>13</p><p>14</p><p>15</p><p>16</p><p>17</p><p>18</p><p>19</p><p>20</p>\
    <div id=a></div><script>document.getElementById('a').animate(\
    [{opacity:1},{opacity:.2}],{duration:1000,iterations:Infinity});</script>";

/// A small page with a paint-only CSS animation.
const PULSE_PAGE: &str = "<!doctype html><style>body{margin:0}\
    #p{width:50px;height:50px;background:blue;animation:pulse 1s linear infinite}\
    @keyframes pulse{0%{opacity:1}50%{opacity:.2}100%{opacity:1}}\
    </style><div id=p></div><p>text below</p>";

/// Pete's hand test: the page before ran a Web Animation; every frame of
/// the next page's paint-only CSS animation was a full relayout.
#[test]
fn the_last_documents_web_animation_does_not_lay_out_the_next_documents_frames() {
    let (mut engine, view, server) = load_routes(vec![("/a", WAAPI_PAGE), ("/b", PULSE_PAGE)]);
    for _ in 0..3 {
        frame(&mut engine, view, 16);
    }
    go(&mut engine, view, &server, "/b");
    let (relaid0, skip0) = (relayouts(&engine, view), skipped(&engine, view));
    let mut last = opacities(&engine, view);
    for i in 0..6u32 {
        frame(&mut engine, view, 16);
        let now = opacities(&engine, view);
        assert_eq!(now.len(), 1, "frame {i}: the pulse's opacity is painted: {now:?}");
        assert_ne!(now, last, "frame {i}: the opacity moved with the clock");
        last = now;
        assert_eq!(animation_relayouts(&engine, view), 0, "frame {i}: no animation relayout on the new document");
        assert_eq!(relayouts(&engine, view), relaid0, "frame {i}: no relayout at all");
        assert_eq!(skipped(&engine, view), skip0 + i + 1, "frame {i}: the layout was skipped");
    }
}

/// The old page animated node K to opacity 0; the new page's node K is an
/// ordinary element and paints at the opacity its own style gives it.
#[test]
fn the_last_documents_script_layer_does_not_style_the_next_documents_elements() {
    let faded = "<!doctype html><style>body{margin:0}#x{width:50px;height:50px;background:red}</style>\
        <div id=x></div><script>document.getElementById('x').animate(\
        [{opacity:0},{opacity:0}],{duration:100000,iterations:Infinity});</script>";
    // The same markup (so the same raw NodeIds), with a script that animates nothing.
    let plain = "<!doctype html><style>body{margin:0}#x{width:50px;height:50px;background:red}</style>\
        <div id=x></div><script>var x=document.getElementById('x');</script>";
    let (mut engine, view, server) = load_routes(vec![("/faded", faded), ("/plain", plain)]);
    for _ in 0..3 {
        frame(&mut engine, view, 16);
    }
    assert_eq!(read(&mut engine, view, "getComputedStyle(document.getElementById('x')).opacity"), "0");
    go(&mut engine, view, &server, "/plain");
    for _ in 0..2 {
        frame(&mut engine, view, 16);
    }
    assert_eq!(
        read(&mut engine, view, "getComputedStyle(document.getElementById('x')).opacity"),
        "1",
        "the new document's element has its own style"
    );
    assert!(opacities(&engine, view).is_empty(), "nothing on the new page paints with an opacity: {:?}", opacities(&engine, view));
    assert_eq!(read(&mut engine, view, "document.getAnimations().length"), "0");
}

/// A Web Animation on an element under `display: none`: no build visits
/// it, so it has no captured style; the visible animation's frames still
/// take the paint path.
#[test]
fn a_web_animation_in_a_hidden_subtree_does_not_lay_out_every_frame() {
    let page = "<!doctype html><style>body{margin:0}\
        #p{width:50px;height:50px;background:blue;animation:pulse 1s linear infinite}\
        @keyframes pulse{0%{opacity:1}50%{opacity:.2}100%{opacity:1}}\
        </style><div style='display:none'><div id=h></div></div><div id=p></div>\
        <script>document.getElementById('h').animate([{opacity:1},{opacity:0}],{duration:1000,iterations:Infinity});</script>";
    let (mut engine, view, _server) = load_routes(vec![("/", page)]);
    // The first frame writes the layer for the first time: a layout.
    frame(&mut engine, view, 16);
    frame(&mut engine, view, 16);
    let (relaid0, skip0) = (relayouts(&engine, view), skipped(&engine, view));
    for i in 0..5u32 {
        frame(&mut engine, view, 16);
        assert_eq!(relayouts(&engine, view), relaid0, "frame {i}: no relayout");
        assert_eq!(skipped(&engine, view), skip0 + i + 1, "frame {i}: the layout was skipped");
    }
}

/// A Web Animation on an element that is not in the document.
#[test]
fn a_web_animation_on_a_detached_element_does_not_lay_out_every_frame() {
    let page = "<!doctype html><style>body{margin:0}\
        #p{width:50px;height:50px;background:blue;animation:pulse 1s linear infinite}\
        @keyframes pulse{0%{opacity:1}50%{opacity:.2}100%{opacity:1}}\
        </style><div id=p></div>\
        <script>window.d=document.createElement('div');d.animate([{opacity:1},{opacity:0}],{duration:1000,iterations:Infinity});</script>";
    let (mut engine, view, _server) = load_routes(vec![("/", page)]);
    frame(&mut engine, view, 16);
    frame(&mut engine, view, 16);
    let (relaid0, skip0) = (relayouts(&engine, view), skipped(&engine, view));
    for i in 0..5u32 {
        frame(&mut engine, view, 16);
        assert_eq!(relayouts(&engine, view), relaid0, "frame {i}: no relayout");
        assert_eq!(skipped(&engine, view), skip0 + i + 1, "frame {i}: the layout was skipped");
    }
}

/// After a document replacement, an animation of a property layout reads
/// still lays out every frame, the geometry moves, and the relayout line
/// says why (`full_reason=animation`, not `no_dom_record`).
#[test]
fn a_width_animation_after_a_document_replacement_still_relays_out() {
    let grow = "<!doctype html><style>body{margin:0}\
        #w{height:20px;background:red;animation:grow 1s linear infinite}\
        @keyframes grow{from{width:10px}to{width:400px}}\
        </style><div id=w></div><script>var w=document.getElementById('w');</script>";
    let (mut engine, view, server) = load_routes(vec![("/a", WAAPI_PAGE), ("/grow", grow)]);
    for _ in 0..3 {
        frame(&mut engine, view, 16);
    }
    go(&mut engine, view, &server, "/grow");
    let first: f64 = read(&mut engine, view, "w.getBoundingClientRect().width").parse().unwrap();
    let (anim0, skip0) = (animation_relayouts(&engine, view), skipped(&engine, view));
    const N: u32 = 5;
    for _ in 0..N {
        frame(&mut engine, view, 16);
    }
    let later: f64 = read(&mut engine, view, "w.getBoundingClientRect().width").parse().unwrap();
    assert!(later > first + 5.0, "the width moved: {first} -> {later}");
    assert_eq!(animation_relayouts(&engine, view), anim0 + N, "each width frame is laid out");
    assert_eq!(skipped(&engine, view), skip0, "no width frame skips layout");
    assert_eq!(last_full_reason(&engine, view), Some("animation"));
}
