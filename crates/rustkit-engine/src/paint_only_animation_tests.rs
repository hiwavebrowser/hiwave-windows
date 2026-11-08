//! Paint-only animation frames (LAT2, = S18). Every frame of a running
//! CSS animation, transition or Web Animation used to lay the whole view
//! out (`relayout cause=animation`, `would_path=full`): a page with one
//! spinner rebuilt and laid out every box 60 times a second (cnn 354 ms a
//! frame, the RustKit chrome's loading bar ~40 full relayouts a second).
//!
//! A frame whose changed values are all paint (`opacity`, `transform`,
//! `color`, `background-color`, the border colours) now restyles the
//! animated elements from the style they cascaded to at the last build,
//! copies the new values onto the laid-out boxes and builds the display
//! list from them: no build of the page, no layout, no stylesheet
//! extraction. A frame that changes anything layout reads (`width`,
//! margins, `font-size`, ...) is a full relayout, as before.
//!
//! Every test that takes the short path compares the frame it painted
//! with what a full relayout paints at the same clock.

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
    load_sized(page, (800, 600))
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

/// Full relayouts whose first cause was an animation frame.
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

fn relayouts(engine: &Engine, view: EngineViewId) -> u32 {
    engine.relayout_stats(view).expect("view").relayouts
}

fn list(engine: &Engine, view: EngineViewId) -> String {
    format!(
        "{:?}",
        engine.views[&view]
            .display_list
            .as_ref()
            .expect("a display list")
            .commands
    )
}

fn transforms(engine: &Engine, view: EngineViewId) -> Vec<[f32; 6]> {
    engine.views[&view]
        .display_list
        .as_ref()
        .expect("a display list")
        .commands
        .iter()
        .filter_map(|c| match c {
            DisplayCommand::PushTransform { matrix, .. } => Some(*matrix),
            _ => None,
        })
        .collect()
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

fn fills(engine: &Engine, view: EngineViewId) -> Vec<String> {
    engine.views[&view]
        .display_list
        .as_ref()
        .expect("a display list")
        .commands
        .iter()
        .filter_map(|c| match c {
            DisplayCommand::SolidColor(color, _) => Some(format!("{color:?}")),
            _ => None,
        })
        .collect()
}

/// The display list a full relayout paints at the clock the view is at.
/// The relayout is not an animation frame: it moves no counter the tests
/// read but `relayouts`.
fn list_by_a_full_relayout(engine: &mut Engine, view: EngineViewId) -> String {
    let before = skipped(engine, view);
    engine.relayout(view).expect("relayout");
    assert_eq!(skipped(engine, view), before, "a relayout nobody named is a full one");
    list(engine, view)
}

/// A spinner (`transform`) and a pulse (`opacity`): each frame skips
/// layout, the display list moves with the clock, and it is the list a
/// full relayout paints at that clock.
#[test]
fn a_transform_and_opacity_animation_paints_each_frame_without_layout() {
    let (mut engine, view, _server) = load(
        "<!doctype html><style>body{margin:0}\
         #s{width:50px;height:50px;background:red;animation:spin 1s linear infinite}\
         #p{width:50px;height:50px;background:blue;animation:pulse 1s linear infinite}\
         @keyframes spin{from{transform:rotate(0deg)}to{transform:rotate(360deg)}}\
         @keyframes pulse{0%{opacity:1}50%{opacity:.2}100%{opacity:1}}\
         </style><div id=s></div><div id=p>text</div><p>a paragraph below</p>",
    );
    let (anim0, skip0) = (animation_relayouts(&engine, view), skipped(&engine, view));
    let mut last = (transforms(&engine, view), opacities(&engine, view));
    const N: u32 = 8;
    for i in 0..N {
        frame(&mut engine, view, 16);
        let now = (transforms(&engine, view), opacities(&engine, view));
        assert_eq!(now.0.len(), 1, "frame {i}: the spinner's transform is painted: {:?}", now.0);
        assert_eq!(now.1.len(), 1, "frame {i}: the pulse's opacity is painted: {:?}", now.1);
        assert_ne!(now.0, last.0, "frame {i}: the transform moved with the clock");
        assert_ne!(now.1, last.1, "frame {i}: the opacity moved with the clock");
        last = now;
        // Checked before the full relayout below, which is not a frame.
        assert_eq!(animation_relayouts(&engine, view), anim0, "frame {i}: no animation relayout");
        assert_eq!(skipped(&engine, view), skip0 + i + 1, "frame {i}: the layout was skipped");
        let painted = list(&engine, view);
        assert_eq!(painted, list_by_a_full_relayout(&mut engine, view), "frame {i}");
    }
}

/// An animation of a property layout reads keeps the full path.
#[test]
fn a_width_animation_still_relays_out() {
    let (mut engine, view, _server) = load(
        "<!doctype html><style>body{margin:0}\
         #w{height:20px;background:red;animation:grow 1s linear infinite}\
         @keyframes grow{from{width:10px}to{width:400px}}\
         </style><div id=w></div>",
    );
    let (anim0, skip0) = (animation_relayouts(&engine, view), skipped(&engine, view));
    const N: u32 = 5;
    for _ in 0..N {
        frame(&mut engine, view, 16);
    }
    assert_eq!(animation_relayouts(&engine, view), anim0 + N, "each width frame is laid out");
    assert_eq!(skipped(&engine, view), skip0, "no width frame skips layout");
}

/// One animation of a paint and a layout property on one element: the
/// frame lays out (the width moves).
#[test]
fn a_mixed_paint_and_layout_animation_relays_out() {
    let (mut engine, view, _server) = load(
        "<!doctype html><style>body{margin:0}\
         #w{height:20px;background:red;animation:mix 1s linear infinite}\
         @keyframes mix{from{width:10px;opacity:1}to{width:400px;opacity:.1}}\
         </style><div id=w></div>",
    );
    let anim0 = animation_relayouts(&engine, view);
    for _ in 0..3 {
        frame(&mut engine, view, 16);
    }
    assert_eq!(animation_relayouts(&engine, view), anim0 + 3);
}

/// A `background-color` transition: the class change lays out (it is a
/// script write), then every frame of the transition is paint only, and
/// it ends on the new colour with one `transitionend`.
#[test]
fn a_background_color_transition_takes_the_paint_path() {
    let (mut engine, view, _server) = load(
        "<!doctype html><style>body{margin:0}\
         #t{width:100px;height:100px;background-color:rgb(255,0,0);transition:background-color 200ms linear}\
         #t.on{background-color:rgb(0,0,255)}\
         </style><div id=t></div>\
         <script>var t=document.getElementById('t');window.log=[];\
         t.addEventListener('transitionend',function(e){log.push(e.propertyName)});</script>",
    );
    run(&mut engine, view, "t.classList.add('on')");
    let (anim0, skip0) = (animation_relayouts(&engine, view), skipped(&engine, view));
    let mut last = fills(&engine, view);
    for i in 0..6u32 {
        frame(&mut engine, view, 16);
        let now = fills(&engine, view);
        assert_ne!(now, last, "frame {i}: the background moved");
        last = now;
        assert_eq!(animation_relayouts(&engine, view), anim0, "frame {i}");
        assert_eq!(skipped(&engine, view), skip0 + i + 1, "frame {i}");
        let painted = list(&engine, view);
        assert_eq!(painted, list_by_a_full_relayout(&mut engine, view), "frame {i}");
    }
    for _ in 0..12 {
        frame(&mut engine, view, 16);
    }
    assert_eq!(read(&mut engine, view, "log.join()"), "background-color");
    assert_eq!(
        read(&mut engine, view, "getComputedStyle(t).backgroundColor"),
        "rgb(0, 0, 255)"
    );
    assert_eq!(animation_relayouts(&engine, view), anim0, "the transition never laid out");
    let painted = list(&engine, view);
    assert_eq!(painted, list_by_a_full_relayout(&mut engine, view), "at rest");
}

/// A `color` animation: the text runs inherit the colour, and a child
/// element that sets its own colour keeps it.
#[test]
fn a_color_animation_repaints_the_text_it_colours() {
    let (mut engine, view, _server) = load(
        "<!doctype html><style>body{margin:0;font:16px sans-serif}\
         #c{animation:hue 1s linear infinite}\
         #c b{color:rgb(0,128,0)}\
         @keyframes hue{from{color:rgb(255,0,0)}to{color:rgb(0,0,255)}}\
         </style><div id=c>some text <b>bold own colour</b> more</div>",
    );
    let skip0 = skipped(&engine, view);
    for i in 0..4u32 {
        frame(&mut engine, view, 16);
        assert_eq!(skipped(&engine, view), skip0 + i + 1, "frame {i}");
        let painted = list(&engine, view);
        assert_eq!(painted, list_by_a_full_relayout(&mut engine, view), "frame {i}");
    }
}

/// `getComputedStyle` reads the frame that was painted, not the frame of
/// the last layout.
#[test]
fn a_computed_read_sees_the_painted_frame() {
    let (mut engine, view, _server) = load(
        "<!doctype html><style>body{margin:0}\
         #p{width:50px;height:50px;background:blue;animation:fade 1s linear forwards}\
         @keyframes fade{from{opacity:1}to{opacity:0}}\
         </style><div id=p></div><script>var p=document.getElementById('p');</script>",
    );
    let first: f64 = read(&mut engine, view, "getComputedStyle(p).opacity").parse().unwrap();
    for _ in 0..10 {
        frame(&mut engine, view, 16);
    }
    let later: f64 = read(&mut engine, view, "getComputedStyle(p).opacity").parse().unwrap();
    assert!(later < first - 0.1, "opacity {first} -> {later} after 160 ms of a 1 s fade");
    let painted = opacities(&engine, view);
    assert_eq!(painted.len(), 1);
    assert!((painted[0] as f64 - later).abs() < 0.02, "painted {painted:?}, read {later}");
}

/// `Element.animate()` of `opacity` and `transform`: the layer write each
/// frame is paint only.
#[test]
fn a_web_animation_of_opacity_takes_the_paint_path() {
    let (mut engine, view, _server) = load(
        "<!doctype html><style>body{margin:0}\
         #a{width:50px;height:50px;background:green}\
         </style><div id=a></div><script>var a=document.getElementById('a');</script>",
    );
    run(
        &mut engine,
        view,
        "a.animate([{opacity:1,transform:'translateX(0px)'},{opacity:0.2,transform:'translateX(100px)'}],{duration:1000,iterations:Infinity})",
    );
    // The first frame writes the layer for the first time: a layout.
    frame(&mut engine, view, 16);
    frame(&mut engine, view, 16);
    let (mut relaid, skip0) = (relayouts(&engine, view), skipped(&engine, view));
    let mut last = opacities(&engine, view);
    for i in 0..5u32 {
        frame(&mut engine, view, 16);
        let now = opacities(&engine, view);
        assert_ne!(now, last, "frame {i}: the opacity moved");
        last = now;
        assert_eq!(relayouts(&engine, view), relaid, "frame {i}: no relayout");
        assert_eq!(skipped(&engine, view), skip0 + i + 1, "frame {i}");
        let painted = list(&engine, view);
        assert_eq!(painted, list_by_a_full_relayout(&mut engine, view), "frame {i}");
        // The full relayout above is not a frame; it moves `relayouts`.
        relaid = relayouts(&engine, view);
    }
}

/// A page with a running Web Animation (`#a`, opacity) and a box `#b`
/// 50px wide. Once the animation frames take the paint path, `write`
/// runs in a `requestAnimationFrame` callback, the same turn as a frame's
/// layer write; after a few more frames `expr` is read. Whatever the
/// write dirtied must reach the layout: the paint path is taken only
/// when the layer write is all that changed (#941 R1 HOLD).
fn write_in_a_web_animation_frame(head: &str, body: &str, write: &str, expr: &str) -> String {
    let (mut engine, view, _server) = load(&format!(
        "<!doctype html><style>body{{margin:0}}\
         #a{{width:50px;height:50px;background:green}}\
         #b{{width:50px;height:20px;background:blue}}{head}\
         </style><div id=a></div>{body}\
         <script>var a=document.getElementById('a'),b=document.getElementById('b');</script>"
    ));
    run(
        &mut engine,
        view,
        "a.animate([{opacity:1},{opacity:0.2}],{duration:1000,iterations:Infinity})",
    );
    for _ in 0..4 {
        frame(&mut engine, view, 16);
    }
    let skip0 = skipped(&engine, view);
    frame(&mut engine, view, 16);
    if animation_repaint_enabled() {
        assert_eq!(skipped(&engine, view), skip0 + 1, "the animation frames take the paint path");
    }
    run(&mut engine, view, &format!("requestAnimationFrame(function(){{{write}}})"));
    for _ in 0..4 {
        frame(&mut engine, view, 16);
    }
    let value = read(&mut engine, view, expr);
    let painted = list(&engine, view);
    assert_eq!(painted, list_by_a_full_relayout(&mut engine, view), "the painted frame is the laid-out one");
    value
}

/// `document.adoptedStyleSheets` marks the page dirty with no mutation
/// record.
#[test]
fn an_adopted_sheet_in_a_web_animation_frame_reaches_the_layout() {
    let width = write_in_a_web_animation_frame(
        "",
        "<div id=b></div>",
        "var s=new CSSStyleSheet();s.replaceSync('#b{width:123px}');document.adoptedStyleSheets=[s];",
        "b.offsetWidth+','+getComputedStyle(b).width",
    );
    assert_eq!(width, "123,123px");
}

/// `customElements.define` upgrades `:defined` with no mutation record.
#[test]
fn a_custom_element_definition_in_a_web_animation_frame_reaches_the_layout() {
    let width = write_in_a_web_animation_frame(
        "x-y{display:block;width:50px;height:20px}x-y:defined{width:123px}",
        "<x-y id=c></x-y>",
        "customElements.define('x-y',class extends HTMLElement{});",
        "document.getElementById('c').offsetWidth",
    );
    assert_eq!(width, "123");
}

/// `attachShadow` takes the host's children out of the rendering (no
/// slot) with no mutation record.
#[test]
fn an_attach_shadow_in_a_web_animation_frame_reaches_the_layout() {
    let width = write_in_a_web_animation_frame(
        "#h{display:inline-block}",
        "<div id=b></div><div id=h><div style=\"width:123px;height:10px\"></div></div>",
        "document.getElementById('h').attachShadow({mode:'open'});",
        "document.getElementById('h').offsetWidth",
    );
    assert_eq!(width, "0");
}

/// A CSS animation's `finish()` from script (the engine applies it at its
/// next build) with no mutation record.
#[test]
fn a_css_animation_control_in_a_web_animation_frame_reaches_the_layout() {
    let width = write_in_a_web_animation_frame(
        "#b{animation:g 1s linear paused both}@keyframes g{from{width:50px}to{width:123px}}",
        "<div id=b></div>",
        "b.getAnimations()[0].finish();",
        "b.offsetWidth",
    );
    assert_eq!(width, "123");
}

/// The control: an inline style write has a record and always lays out.
#[test]
fn an_inline_style_write_in_a_web_animation_frame_reaches_the_layout() {
    let width = write_in_a_web_animation_frame(
        "",
        "<div id=b></div>",
        "b.style.width='123px';",
        "b.offsetWidth",
    );
    assert_eq!(width, "123");
}

/// The reduced cnn/amazon page: 500 boxes of text and one spinning
/// element. After the first frame no animation frame lays out, and a
/// frame costs at most 2 ms (a fifth of a relayout on a debug build).
#[test]
fn a_spinner_on_a_500_box_page_never_relays_out_and_is_cheap() {
    let mut page = String::from(
        "<!doctype html><style>body{margin:0;font:14px sans-serif}\
         .row{display:flex;gap:4px}.row div{flex:1;padding:2px;border:1px solid #ccc;background:#f8f8f8}\
         #spin{position:absolute;top:10px;right:10px;width:24px;height:24px;border:3px solid #08f;border-radius:50%;\
         animation:spin .8s linear infinite}\
         @keyframes spin{to{transform:rotate(360deg)}}</style><div id=spin></div>",
    );
    for row in 0..100 {
        page.push_str("<div class=row>");
        for cell in 0..5 {
            page.push_str(&format!("<div>cell {row}.{cell} with a few words</div>"));
        }
        page.push_str("</div>");
    }
    let (mut engine, view, _server) = load_sized(&page, (1024, 768));
    frame(&mut engine, view, 16);
    let anim0 = animation_relayouts(&engine, view);
    const N: u32 = 30;
    let started = std::time::Instant::now();
    for _ in 0..N {
        frame(&mut engine, view, 16);
    }
    let per_frame = started.elapsed().as_secs_f64() * 1000.0 / N as f64;
    eprintln!("LAT2 reduced page: {per_frame:.2} ms per animation frame (pump included)");
    assert_eq!(animation_relayouts(&engine, view), anim0, "no frame after the first lays out");
    let full = {
        let started = std::time::Instant::now();
        for _ in 0..5 {
            engine.relayout(view).expect("relayout");
        }
        started.elapsed().as_secs_f64() * 1000.0 / 5.0
    };
    eprintln!("LAT2 reduced page: {full:.2} ms per full relayout");
    let stats = engine.relayout_stats(view).expect("view");
    let painted_ms = stats.animation_paint_ms / stats.animation_frames_painted.max(1) as f64;
    eprintln!(
        "LAT2 reduced page: {painted_ms:.2} ms per painted frame over {} frames",
        stats.animation_frames_painted
    );
    assert!(stats.animation_frames_painted >= N, "{stats:?}");
    // 2 ms on an optimised build. A debug build (the test profile) runs
    // the display list several times slower, so there the frame must cost
    // at most a fifth of the relayout it replaces.
    if cfg!(debug_assertions) {
        assert!(painted_ms * 5.0 <= full, "a paint-only frame took {painted_ms:.2} ms, a relayout {full:.2} ms");
    } else {
        assert!(painted_ms <= 2.0, "a paint-only frame took {painted_ms:.2} ms");
    }
}

/// The RustKit chrome with its loading bar on (`.loading-indicator.active`)
/// and a loading tab: its animations never lay the chrome out. Once the
/// load is over and the bar is off, nothing animates.
#[test]
fn the_chrome_loading_animations_never_relay_out() {
    let (mut engine, view, _server) =
        load_sized(include_str!("../../hiwave-app/src/ui/chrome.html"), (1280, 120));
    // Entrance animations of the chrome's own boot run out first.
    for _ in 0..40 {
        frame(&mut engine, view, 16);
    }
    run(
        &mut engine,
        view,
        "document.getElementById('loadingIndicator').classList.add('active');\
         var tab=document.querySelector('.tab')||document.body.appendChild(document.createElement('div'));\
         tab.classList.add('tab','loading')",
    );
    let anim0 = animation_relayouts(&engine, view);
    let painted0 = engine.relayout_stats(view).expect("view").animation_frames_painted;
    for _ in 0..60 {
        frame(&mut engine, view, 16);
    }
    assert_eq!(animation_relayouts(&engine, view), anim0, "the loading bar never lays the chrome out");
    let painted = engine.relayout_stats(view).expect("view").animation_frames_painted - painted0;
    assert!(painted >= 50, "the loading bar is painted each frame: {painted}");
    run(
        &mut engine,
        view,
        "document.getElementById('loadingIndicator').classList.remove('active');\
         document.querySelectorAll('.tab.loading').forEach(function(t){t.classList.remove('loading')})",
    );
    for _ in 0..60 {
        frame(&mut engine, view, 16);
    }
    let anims = engine.views[&view].lay.animations.borrow();
    assert!(!anims.active(), "nothing animates once no page is loading");
}
