//! CH2.E16: `@keyframes` animations and transitions on `::before` and
//! `::after`. The pseudo-element's style was cascaded and painted, but
//! never handed to the animations: its `animation` and `transition` were
//! read and nothing ran, so the RustKit chrome's tab loading bar
//! (`.tab.loading::before`, `tabLoading`) and the decay pulse
//! (`.tab[data-decay="5"]::after`, `decayPulse`) painted one still frame.
//!
//! Oracle (Chromium 143.0.7499.4, headless): a `::before` opacity
//! animation and an `::after` transform animation change their computed
//! values frame to frame; a `color` transition on `:hover::before` runs
//! (`rgb(191, 0, 64)` 50 ms into a 200 ms red-to-blue transition) and its
//! events reach the element with `pseudoElement: "::before"`;
//! `el.getAnimations()` leaves the pseudo-element's animations out and
//! `el.getAnimations({subtree: true})` and `document.getAnimations()` list
//! them, `effect.target` the element and `effect.pseudoElement` the
//! pseudo.
//!
//! Every frame of a paint-only pseudo animation takes LAT2's paint path
//! (no layout), and paints the display list a full relayout paints.

use super::*;
use crate::script_net_tests::serve_routes;
use rustkit_layout::DisplayCommand;

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

fn load_sized(page: &str, size: (u32, u32)) -> (Engine, EngineViewId, crate::script_net_tests::Server) {
    let server = serve_routes(vec![("/", "text/html", page.to_string())]);
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds {
            x: 0,
            y: 0,
            width: size.0,
            height: size.1,
        })
        .expect("view");
    let url = Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap();
    rt().block_on(engine.load_url(view, url)).expect("load_url");
    (engine, view, server)
}

fn load(page: &str) -> (Engine, EngineViewId, crate::script_net_tests::Server) {
    load_sized(page, (400, 300))
}

fn run(engine: &mut Engine, view: EngineViewId, script: &str) {
    engine.execute_script(view, script).unwrap();
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

fn animation_relayouts(engine: &Engine, view: EngineViewId) -> u32 {
    engine
        .relayout_stats(view)
        .expect("view")
        .by_first_cause
        .get("animation")
}

fn skipped(engine: &Engine, view: EngineViewId) -> u32 {
    engine.relayout_stats(view).expect("view").layouts_skipped
}

fn commands(engine: &Engine, view: EngineViewId) -> Vec<DisplayCommand> {
    engine.views[&view]
        .display_list
        .as_ref()
        .expect("a display list")
        .commands
        .clone()
}

fn list(engine: &Engine, view: EngineViewId) -> String {
    format!("{:?}", commands(engine, view))
}

fn opacities(engine: &Engine, view: EngineViewId) -> Vec<f32> {
    commands(engine, view)
        .into_iter()
        .filter_map(|c| match c {
            DisplayCommand::PushOpacity(o) => Some(o),
            _ => None,
        })
        .collect()
}

fn transforms(engine: &Engine, view: EngineViewId) -> Vec<[f32; 6]> {
    commands(engine, view)
        .into_iter()
        .filter_map(|c| match c {
            DisplayCommand::PushTransform { matrix, .. } => Some(matrix),
            _ => None,
        })
        .collect()
}

/// The colours the display list paints text in.
fn text_colours(engine: &Engine, view: EngineViewId) -> Vec<String> {
    commands(engine, view)
        .into_iter()
        .filter_map(|c| match c {
            DisplayCommand::Text { color, .. } => Some(format!("{color:?}")),
            _ => None,
        })
        .collect()
}

/// The display list a full relayout paints at the clock the view is at.
fn list_by_a_full_relayout(engine: &mut Engine, view: EngineViewId) -> String {
    let before = skipped(engine, view);
    engine.relayout(view).expect("relayout");
    assert_eq!(skipped(engine, view), before, "a relayout nobody named is a full one");
    list(engine, view)
}

/// A `::before` opacity animation: its painted opacity moves each frame,
/// each frame skips layout, and it is what a full relayout paints.
#[test]
fn a_before_opacity_animation_paints_each_frame_without_layout() {
    let (mut engine, view, _server) = load(
        "<!doctype html><style>body{margin:0}\
         #a::before{content:'';display:block;width:50px;height:50px;background:red;\
         animation:pulse 1s linear infinite}\
         @keyframes pulse{0%{opacity:1}50%{opacity:.2}100%{opacity:1}}\
         </style><div id=a></div><p>below</p>",
    );
    let (anim0, skip0) = (animation_relayouts(&engine, view), skipped(&engine, view));
    let mut last = opacities(&engine, view);
    for i in 0..8u32 {
        frame(&mut engine, view, 16);
        let now = opacities(&engine, view);
        assert_eq!(now.len(), 1, "frame {i}: the ::before's opacity is painted: {now:?}");
        assert_ne!(now, last, "frame {i}: the ::before's opacity moved with the clock");
        last = now;
        assert_eq!(animation_relayouts(&engine, view), anim0, "frame {i}: no animation relayout");
        assert_eq!(skipped(&engine, view), skip0 + i + 1, "frame {i}: the layout was skipped");
        let painted = list(&engine, view);
        assert_eq!(painted, list_by_a_full_relayout(&mut engine, view), "frame {i}");
    }
    // `getComputedStyle(el, '::before')` reads the frame painted.
    let read_opacity: f32 = read(&mut engine, view, "getComputedStyle(document.getElementById('a'),'::before').opacity")
        .parse()
        .unwrap();
    assert!((read_opacity - last[0]).abs() < 0.02, "read {read_opacity}, painted {last:?}");
}

/// An `::after` transform animation (a spinner).
#[test]
fn an_after_transform_animation_paints_each_frame_without_layout() {
    let (mut engine, view, _server) = load(
        "<!doctype html><style>body{margin:0}\
         #b::after{content:'';display:block;width:50px;height:50px;background:blue;\
         animation:spin 1s linear infinite}\
         @keyframes spin{from{transform:rotate(0deg)}to{transform:rotate(360deg)}}\
         </style><div id=b>text</div>",
    );
    let (anim0, skip0) = (animation_relayouts(&engine, view), skipped(&engine, view));
    let mut last = transforms(&engine, view);
    for i in 0..8u32 {
        frame(&mut engine, view, 16);
        let now = transforms(&engine, view);
        assert_eq!(now.len(), 1, "frame {i}: the ::after's transform is painted: {now:?}");
        assert_ne!(now, last, "frame {i}: the ::after's transform moved with the clock");
        last = now;
        assert_eq!(animation_relayouts(&engine, view), anim0, "frame {i}: no animation relayout");
        assert_eq!(skipped(&engine, view), skip0 + i + 1, "frame {i}: the layout was skipped");
        let painted = list(&engine, view);
        assert_eq!(painted, list_by_a_full_relayout(&mut engine, view), "frame {i}");
    }
}

/// A `color` transition on `:hover::before`: the hover starts it, its
/// frames are paint only, it ends on the hover colour, and its events
/// reach the element with `pseudoElement: "::before"` (Chromium 143).
#[test]
fn a_before_colour_transition_runs_on_hover() {
    let (mut engine, view, _server) = load(
        "<!doctype html><style>body{margin:0;font:16px sans-serif}\
         #c{width:100px;height:40px}\
         #c::before{content:'x';color:rgb(255,0,0);transition:color 200ms linear}\
         #c:hover::before{color:rgb(0,0,255)}\
         </style><div id=c></div>\
         <script>window.evs=[];['transitionrun','transitionstart','transitionend'].forEach(function(t){\
         document.addEventListener(t,function(e){evs.push(e.type+':'+e.target.id+':'+e.pseudoElement+':'+e.propertyName)})});\
         </script>",
    );
    let red = text_colours(&engine, view);
    assert_eq!(red.len(), 1, "{red:?}");
    crate::hover_restyle_tests::hover(&mut engine, view, (50.0, 20.0));
    let (anim0, skip0) = (animation_relayouts(&engine, view), skipped(&engine, view));
    let mut seen = vec![text_colours(&engine, view)];
    for i in 0..4u32 {
        frame(&mut engine, view, 16);
        seen.push(text_colours(&engine, view));
        assert_eq!(animation_relayouts(&engine, view), anim0, "frame {i}: no animation relayout");
        assert_eq!(skipped(&engine, view), skip0 + i + 1, "frame {i}: the layout was skipped");
        let painted = list(&engine, view);
        assert_eq!(painted, list_by_a_full_relayout(&mut engine, view), "frame {i}");
    }
    seen.dedup();
    assert!(seen.len() >= 4, "the ::before's colour moves frame to frame: {seen:?}");
    let mid = read(&mut engine, view, "getComputedStyle(document.getElementById('c'),'::before').color");
    assert_ne!(mid, "rgb(255, 0, 0)", "mid-transition");
    assert_ne!(mid, "rgb(0, 0, 255)", "mid-transition");
    for _ in 0..16 {
        frame(&mut engine, view, 16);
    }
    assert_eq!(
        read(&mut engine, view, "getComputedStyle(document.getElementById('c'),'::before').color"),
        "rgb(0, 0, 255)"
    );
    assert_eq!(
        read(&mut engine, view, "evs.join()"),
        "transitionrun:c:::before:color,transitionstart:c:::before:color,transitionend:c:::before:color"
    );
}

/// `getAnimations()`: the element's own list leaves its pseudo-elements'
/// animations out; `{subtree: true}` and the document's list them with
/// `effect.pseudoElement` set (Chromium 143).
#[test]
fn get_animations_lists_pseudo_element_animations_with_their_pseudo() {
    let (mut engine, view, _server) = load(
        "<!doctype html><style>body{margin:0}\
         #a::before{content:'';display:block;width:50px;height:50px;background:red;animation:pulse 1s linear infinite}\
         #b::after{content:'';display:block;width:50px;height:50px;background:blue;animation:spin 1s linear infinite}\
         @keyframes pulse{0%{opacity:1}50%{opacity:.2}100%{opacity:1}}\
         @keyframes spin{from{transform:rotate(0deg)}to{transform:rotate(360deg)}}\
         </style><div id=a></div><div id=b></div>\
         <script>function d(x){return x.animationName+'/'+x.constructor.name+'/'+x.effect.target.id+'/'+x.effect.pseudoElement}\
         var a=document.getElementById('a');</script>",
    );
    frame(&mut engine, view, 16);
    assert_eq!(read(&mut engine, view, "a.getAnimations().length"), "0");
    assert_eq!(
        read(&mut engine, view, "a.getAnimations({subtree:true}).map(d).join()"),
        "pulse/CSSAnimation/a/::before"
    );
    assert_eq!(
        read(&mut engine, view, "document.getAnimations().map(d).join()"),
        "pulse/CSSAnimation/a/::before,spin/CSSAnimation/b/::after"
    );
    assert_eq!(
        read(&mut engine, view, "document.body.getAnimations({subtree:true}).map(d).join()"),
        "pulse/CSSAnimation/a/::before,spin/CSSAnimation/b/::after"
    );
}

/// The RustKit chrome with a loading tab: `.tab.loading::before` runs
/// `tabLoading` (a transform), painted each frame, never laid out.
#[test]
fn the_chrome_tab_loading_bar_animates() {
    let (mut engine, view, _server) =
        load_sized(include_str!("../../hiwave-app/src/ui/chrome.html"), (1280, 120));
    // The chrome's own entrance animations run out first.
    for _ in 0..40 {
        frame(&mut engine, view, 16);
    }
    run(
        &mut engine,
        view,
        "var tab=document.querySelector('.tab')||document.getElementById('tabs').appendChild(document.createElement('div'));\
         tab.classList.add('tab','loading')",
    );
    frame(&mut engine, view, 16);
    assert_eq!(
        read(
            &mut engine,
            view,
            "tab.getAnimations({subtree:true}).filter(function(x){return x.effect.pseudoElement}).map(function(x){return x.animationName+x.effect.pseudoElement}).join()"
        ),
        "tabLoading::before"
    );
    let anim0 = animation_relayouts(&engine, view);
    let painted0 = engine.relayout_stats(view).expect("view").animation_frames_painted;
    let mut seen = Vec::new();
    for _ in 0..30 {
        frame(&mut engine, view, 16);
        seen.push(transforms(&engine, view));
    }
    seen.dedup();
    assert!(seen.len() >= 20, "the loading bar's transform moves frame to frame: {} distinct", seen.len());
    assert_eq!(animation_relayouts(&engine, view), anim0, "the loading bar never lays the chrome out");
    let painted = engine.relayout_stats(view).expect("view").animation_frames_painted - painted0;
    assert!(painted >= 25, "the loading bar is painted each frame: {painted}");
    let painted = list(&engine, view);
    assert_eq!(painted, list_by_a_full_relayout(&mut engine, view));
}
