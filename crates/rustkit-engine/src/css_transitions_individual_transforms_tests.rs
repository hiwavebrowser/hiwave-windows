//! Transitions of the individual transform properties (css-transforms-2
//! §5: `translate`, `rotate`, `scale`; owner's hand test 2026-11-17, group
//! B). They jumped to their end value. Tailwind v4 writes every
//! translate/rotate/scale utility through them, so `hover:translate-x-1`
//! with `transition` snapped.
//!
//! Oracle: Chrome for Testing 148.0.7778.216 through
//! `tools/parity_oracle/css_transitions_individual_transforms_log.mjs`,
//! receipt `docs/diagnostics/2026-11-17/css_transitions_individual_transforms_cft148.json`:
//! the value at currentTime 50 of a 100 ms linear transition, the
//! transitions that start, and the events once finished.
//!
//! Sampling is on the page's virtual clock: `.on` lands and a style read
//! in the same script starts the transition at the clock as it stands,
//! then `pump_live` moves the clock exactly 50 ms, so the values are
//! compared as Chrome serializes them.

use super::*;
use crate::script_net_tests::serve_routes;

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

fn load(page: &str) -> (Engine, EngineViewId, crate::script_net_tests::Server) {
    let server = serve_routes(vec![("/", "text/html", page.to_string())]);
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds {
            x: 0,
            y: 0,
            width: 200,
            height: 100,
        })
        .expect("view");
    let url = Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap();
    rt().block_on(engine.load_url(view, url)).expect("load_url");
    (engine, view, server)
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

/// Live turns of `step` ms of virtual time until `total` ms have passed.
fn pump(engine: &mut Engine, view: EngineViewId, total: u64, step: u64) {
    let rt = rt();
    let mut t = 0;
    while t < total {
        rt.block_on(engine.pump_live(view, step));
        t += step;
    }
}

/// `#h` (20 px square, red) styled by `css` under `transition: all 100ms
/// linear`, logging every transition event as `type:property`.
fn page(css: &str) -> String {
    format!(
        "<!doctype html><style>body{{margin:0}}\
#h{{width:20px;height:20px;background:rgb(255,0,0);transition:all 100ms linear}}{css}</style>\
<div id=h></div>\
<script>var h=document.getElementById('h');window.log=[];\
['transitionrun','transitionstart','transitionend','transitioncancel'].forEach(function(t){{\
h.addEventListener(t,function(e){{log.push(e.type.slice(10)+':'+e.propertyName)}})}});</script>"
    )
}

fn computed(engine: &mut Engine, view: EngineViewId, prop: &str) -> String {
    read(
        engine,
        view,
        &format!("getComputedStyle(h).getPropertyValue('{prop}')"),
    )
}

/// `.on` lands with a style read (the transition starts at the clock as
/// it stands), and the clock moves exactly 50 ms.
fn toggle_and_reach_50(engine: &mut Engine, view: EngineViewId, prop: &str) {
    engine
        .execute_script(
            view,
            &format!("h.classList.add('on');getComputedStyle(h).getPropertyValue('{prop}')"),
        )
        .unwrap();
    pump(engine, view, 50, 10);
}

/// `prop` at 50 ms, the transitions running on `#h`, then (after the end)
/// the events and `prop`'s end value.
fn run_case(css: &str, prop: &str) -> (String, String, String, String) {
    let (mut engine, view, _server) = load(&page(css));
    toggle_and_reach_50(&mut engine, view, prop);
    let mid = computed(&mut engine, view, prop);
    let names = read(
        &mut engine,
        view,
        "h.getAnimations().map(function(a){return a.transitionProperty}).join()",
    );
    pump(&mut engine, view, 100, 10);
    let events = read(&mut engine, view, "log.join()");
    let end = computed(&mut engine, view, prop);
    (mid, names, events, end)
}

/// Chrome 148's midpoint, transition and end for one case; the events are
/// run, start and end of `prop`, each once.
fn check(css: &str, prop: &str, chrome_mid: &str, chrome_end: &str) {
    let (mid, names, events, end) = run_case(css, prop);
    assert_eq!(mid, chrome_mid, "{css}: {prop} at 50 ms");
    assert_eq!(names, prop, "{css}: the transitions running");
    assert_eq!(
        events,
        format!("run:{prop},start:{prop},end:{prop}"),
        "{css}: the events"
    );
    assert_eq!(end, chrome_end, "{css}: {prop} at the end");
}

#[test]
fn translate_from_none_interpolates_from_zero() {
    check(
        "#h.on{translate:100px 50px}",
        "translate",
        "50px 25px",
        "100px 50px",
    );
}

#[test]
fn translate_interpolates_each_length() {
    check(
        "#h{translate:10px 20px}#h.on{translate:30px 60px}",
        "translate",
        "20px 40px",
        "30px 60px",
    );
    check(
        "#h{translate:0px}#h.on{translate:100px}",
        "translate",
        "50px",
        "100px",
    );
    check(
        "#h{translate:0%}#h.on{translate:100%}",
        "translate",
        "50%",
        "100%",
    );
}

#[test]
fn translate_to_none_interpolates_to_zero() {
    check(
        "#h{translate:100px}#h.on{translate:none}",
        "translate",
        "50px",
        "none",
    );
}

#[test]
fn rotate_from_none_interpolates_from_zero() {
    check("#h.on{rotate:90deg}", "rotate", "45deg", "90deg");
}

/// No turn is taken off: 45deg to 405deg passes 225deg.
#[test]
fn rotate_interpolates_the_angle_whole() {
    check(
        "#h{rotate:45deg}#h.on{rotate:405deg}",
        "rotate",
        "225deg",
        "405deg",
    );
    check(
        "#h{rotate:0deg}#h.on{rotate:1turn}",
        "rotate",
        "180deg",
        "360deg",
    );
}

#[test]
fn rotate_about_z_and_to_none_interpolate() {
    check(
        "#h{rotate:z 0deg}#h.on{rotate:z 90deg}",
        "rotate",
        "45deg",
        "90deg",
    );
    check(
        "#h{rotate:90deg}#h.on{rotate:none}",
        "rotate",
        "45deg",
        "none",
    );
}

#[test]
fn scale_interpolates_each_factor_from_one() {
    check("#h.on{scale:2}", "scale", "1.5", "2");
    check("#h{scale:1 2}#h.on{scale:3 4}", "scale", "2 3", "3 4");
    check("#h{scale:50%}#h.on{scale:150%}", "scale", "1", "1.5");
}

fn red_at(engine: &mut Engine, view: EngineViewId, x: u32, y: u32) -> bool {
    let (w, _, rgba) = engine.frame_rgba(view).expect("frame");
    let i = ((y * w + x) * 4) as usize;
    rgba[i] >= 250 && rgba[i + 1] <= 5 && rgba[i + 2] <= 5
}

/// The individual transforms are paint-only (the LAT2 frame path): the
/// painted square, not only `getComputedStyle`, is at 50 px at 50 ms of
/// a 0 to 100 px `translate`.
#[test]
fn the_painted_box_moves_with_a_translate_transition() {
    let (mut engine, view, _server) = load(&page("#h.on{translate:100px}"));
    assert!(red_at(&mut engine, view, 10, 10), "before: at 0..20");
    assert!(!red_at(&mut engine, view, 60, 10));
    toggle_and_reach_50(&mut engine, view, "translate");
    assert!(red_at(&mut engine, view, 60, 10), "at 50 ms: at 50..70");
    assert!(!red_at(&mut engine, view, 10, 10), "at 50 ms: left 0..20");
    assert!(!red_at(&mut engine, view, 110, 10));
    pump(&mut engine, view, 100, 10);
    assert!(red_at(&mut engine, view, 110, 10), "after: at 100..120");
    assert!(!red_at(&mut engine, view, 60, 10));
}

/// One value against two: a missing `translate` y is `0px` and a missing
/// `scale` y is the x (Chrome 148: 20px 20px, 1.5 2 both ways).
#[test]
fn one_value_interpolates_against_two() {
    check(
        "#h{translate:10px}#h.on{translate:30px 40px}",
        "translate",
        "20px 20px",
        "30px 40px",
    );
    check(
        "#h{translate:30px 40px}#h.on{translate:10px}",
        "translate",
        "20px 20px",
        "10px",
    );
    check("#h{scale:1}#h.on{scale:2 3}", "scale", "1.5 2", "2 3");
    check("#h{scale:2 3}#h.on{scale:1}", "scale", "1.5 2", "1");
}

// `a_mixed_unit_translate_jumps_today` (group B) pinned px/rem against %
// as a jump; group C interpolates it as a calc() mix:
// `css_transitions_sizes_tests::a_translate_between_two_units_interpolates_as_calc`.
