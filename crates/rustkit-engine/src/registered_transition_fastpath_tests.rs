//! Row REG-TRANS-FASTPATH: a running transition of a registered custom property (#1374, TRANS-F)
//! whose moving value reaches only paint (`background-image`, colours) restyles only the elements
//! that animate, on the LPF1 short path (`repaint_script_restyled`), not a full build of the page
//! every frame. Counters, not timing: during the transition every relayout is a script-restyle hit.
//! An `inherits: true` property restyles the subtree that reads it. A value that reaches a layout
//! property (`width: var(--w)`) falls back to the full build.
//!
//! Correctness: every frame is compared with the same page run with the short path off (a full
//! build every frame, as #1374 does): the computed values read by script and the painted pixels
//! must be the same. #1374's Chrome pins (`css_transitions_property_tests`) cover the values
//! themselves.

use super::*;
use crate::script_net_tests::serve_routes;

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

/// 40 filler elements after the case's own, so a full build is visibly more than the restyle.
fn page(css: &str, body: &str) -> String {
    let filler: String = (0..40).map(|i| format!("<p class=f>filler {i}</p>")).collect();
    format!("<!doctype html><style>body{{margin:0}}.f{{margin:0;height:4px}}{css}</style>{body}{filler}")
}

#[derive(Debug, PartialEq)]
struct Frame {
    read: String,
    pixels: u64,
}

struct Run {
    frames: Vec<Frame>,
    /// Relayouts and script-restyle hits over frames 2.. of the transition.
    relayouts: u32,
    hits: u32,
}

fn hash(bytes: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut h);
    h.finish()
}

enum Trigger {
    Script(&'static str),
    Hover(f32, f32),
}

fn run(html: &str, trigger: &Trigger, read: &str, short_path: bool) -> Run {
    crate::set_script_restyle_for_test(short_path);
    let server = serve_routes(vec![("/", "text/html", html.to_string())]);
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds {
            x: 0,
            y: 0,
            width: 400,
            height: 300,
        })
        .expect("view");
    let url = Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap();
    let rt = rt();
    rt.block_on(engine.load_url(view, url)).expect("load_url");
    for _ in 0..5 {
        rt.block_on(engine.pump_live(view, 16));
    }
    match trigger {
        Trigger::Script(js) => {
            engine.execute_script(view, js).unwrap();
        }
        Trigger::Hover(x, y) => engine.mouse_move_at_point(view, *x, *y),
    }
    let mut frames = Vec::new();
    let (mut relayouts, mut hits) = (0, 0);
    for i in 0..20 {
        let before = engine.views[&view].relayout_stats.clone();
        rt.block_on(engine.pump_live(view, 16));
        let after = engine.views[&view].relayout_stats.clone();
        // Frame 0 carries the trigger; frame 1 may fall back once to keep a tree for the short path.
        if (2..18).contains(&i) {
            relayouts += after.relayouts - before.relayouts;
            hits += after.script_restyles - before.script_restyles;
        }
        let value = engine.execute_script(view, &format!("String({read})")).unwrap();
        let (_, _, rgba) = engine.frame_rgba(view).expect("frame");
        frames.push(Frame {
            read: value,
            pixels: hash(&rgba),
        });
    }
    crate::set_script_restyle_for_test(true);
    Run {
        frames,
        relayouts,
        hits,
    }
}

/// Runs the case with and without the short path; asserts the frames agree and returns both.
fn both(html: &str, trigger: Trigger, read: &str) -> (Run, Run) {
    let fast = run(html, &trigger, read, true);
    let full = run(html, &trigger, read, false);
    for (i, (f, g)) in fast.frames.iter().zip(&full.frames).enumerate() {
        assert_eq!(f, g, "frame {i}: short path vs full build");
    }
    // The transition moved: the read changed between the first and a middle frame.
    assert_ne!(fast.frames[1].read, fast.frames[9].read, "the transition did not run");
    assert_eq!(full.hits, 0, "the full-build run took no short path");
    (fast, full)
}

const GRADIENT: &str = "@property --g{syntax:'<color>';inherits:false;initial-value:#000}\
#t{width:100px;height:40px;--g:#0000ff;background-image:linear-gradient(to right,var(--g),#00ff00);transition:--g 300ms linear}";

#[test]
fn a_paint_only_registered_transition_builds_only_the_animating_element() {
    let html = page(&format!("{GRADIENT}#t.on{{--g:#ff0000}}"), "<div id=t></div>");
    let (fast, full) = both(
        &html,
        Trigger::Script("document.getElementById('t').classList.add('on')"),
        "getComputedStyle(document.getElementById('t')).getPropertyValue('--g')",
    );
    assert!(fast.relayouts > 0 && full.relayouts > 0);
    assert_eq!(
        fast.hits, fast.relayouts,
        "every frame of a paint-only registered transition takes the short path (no full build)"
    );
}

#[test]
fn a_hovered_registered_transition_builds_only_the_animating_element() {
    let html = page(&format!("{GRADIENT}#t:hover{{--g:#ff0000}}"), "<div id=t></div>");
    let (fast, _) = both(
        &html,
        Trigger::Hover(10.0, 10.0),
        "getComputedStyle(document.getElementById('t')).getPropertyValue('--g')",
    );
    assert!(fast.relayouts > 0);
    assert_eq!(fast.hits, fast.relayouts, "a :hover-started registered transition takes the short path");
}

#[test]
fn an_inherited_registered_transition_restyles_the_subtree_that_reads_it() {
    let css = "@property --c{syntax:'<color>';inherits:true;initial-value:#000}\
#t{--c:#0000ff;transition:--c 300ms linear}#t.on{--c:#ff0000}\
#k{height:20px;background-color:var(--c)}#k span{color:var(--c)}";
    let html = page(css, "<div id=t><div id=k><span>x</span></div></div>");
    let (fast, _) = both(
        &html,
        Trigger::Script("document.getElementById('t').classList.add('on')"),
        "getComputedStyle(document.getElementById('k')).backgroundColor",
    );
    assert!(fast.relayouts > 0);
    assert_eq!(fast.hits, fast.relayouts);
}

#[test]
fn a_registered_value_that_reaches_width_falls_back_to_the_full_build() {
    let css = "@property --w{syntax:'<length>';inherits:false;initial-value:0px}\
#t{--w:100px;width:var(--w);height:20px;background:#00f;transition:--w 300ms linear}#t.on{--w:200px}";
    let html = page(css, "<div id=t></div><div id=after style='width:var(--x,50px);height:10px;background:#f00'></div>");
    let (fast, _) = both(
        &html,
        Trigger::Script("document.getElementById('t').classList.add('on')"),
        "document.getElementById('t').offsetWidth",
    );
    assert!(fast.relayouts > 0);
    assert!(fast.hits < fast.relayouts, "a moving width is laid out, not patched");
}

/// R1 (#1382): custom properties inherit into a host's shadow tree, so the restyle under a root
/// reaches a shadow-DOM descendant that reads an `inherits: true` moving value; a slotted light
/// child reads it too.
#[test]
fn an_inherited_registered_transition_reaches_into_a_shadow_tree() {
    let css = "@property --c{syntax:'<color>';inherits:true;initial-value:#000}\
#t{--c:#0000ff;transition:--c 300ms linear}#t.on{--c:#ff0000}\
#slotted{display:block;height:10px;background-color:var(--c)}";
    let body = "<div id=t><div id=host><span id=slotted slot=s>x</span></div></div>\
<script>var r=document.getElementById('host').attachShadow({mode:'open'});\
r.innerHTML=\"<style>#s{height:20px;background-color:var(--c)}</style><div id=s></div><slot name=s></slot>\";</script>";
    let html = page(css, body);
    let (fast, _) = both(
        &html,
        Trigger::Script("document.getElementById('t').classList.add('on')"),
        "getComputedStyle(document.getElementById('host').shadowRoot.getElementById('s')).backgroundColor\
+'|'+getComputedStyle(document.getElementById('slotted')).backgroundColor",
    );
    assert!(fast.relayouts > 0);
    assert_eq!(fast.hits, fast.relayouts, "the restyle under the root reaches the shadow tree on the short path");
}
