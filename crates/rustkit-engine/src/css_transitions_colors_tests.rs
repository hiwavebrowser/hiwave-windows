//! Colour transitions beyond `color`, `background-color` and the border
//! colours (owner's hand test 2026-11-17, group A): `outline-color` and
//! `text-decoration-color` jumped to their end value. `currentcolor` is a
//! value of its own: it interpolates as the element's colour, and a
//! `currentcolor` that stays `currentcolor` while `color` jumps starts no
//! transition.
//!
//! Oracle: Chrome for Testing 148.0.7778.216 through
//! `tools/parity_oracle/css_transitions_colors_log.mjs`, receipt
//! `docs/diagnostics/2026-11-17/css_transitions_colors_cft148.json`: the
//! value at currentTime 50 of a 100 ms linear transition, the transitions
//! that start, and the events once finished. The decoration colour is set
//! by its longhand: the `text-decoration` shorthand's colour is not read by
//! the cascade yet (a separate gap, not an interpolation one).
//!
//! Sampling is on the page's virtual clock: `.on` lands and a style read
//! in the same script starts the transition at the clock as it stands,
//! then `pump_live` moves the clock exactly 50 ms. The midpoint of blue and
//! red is 127.5 a channel, so only rounding (one step) is tolerated.

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
            height: 200,
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

/// A page whose `#h` is styled by `css` under `transition: <transition>`,
/// logging every transition event at `#h` as `type:property`.
fn page(css: &str, transition: &str) -> String {
    format!(
        "<!doctype html><style>body{{margin:0}}#h{{font:16px sans-serif;transition:{transition}}}{css}</style>\
<div id=h>text <span id=c>child</span></div>\
<script>var h=document.getElementById('h');window.log=[];\
['transitionrun','transitionstart','transitionend','transitioncancel'].forEach(function(t){{\
h.addEventListener(t,function(e){{log.push(e.type.slice(10)+':'+e.propertyName)}})}});</script>"
    )
}

/// `.on` lands on `#h` with a style read (the transition starts at the
/// clock as it stands), and the clock moves exactly 50 ms.
fn toggle_and_reach_50(engine: &mut Engine, view: EngineViewId, prop: &str) {
    engine
        .execute_script(
            view,
            &format!("h.classList.add('on');getComputedStyle(h).getPropertyValue('{prop}')"),
        )
        .unwrap();
    pump(engine, view, 50, 10);
}

fn computed(engine: &mut Engine, view: EngineViewId, prop: &str) -> String {
    read(
        engine,
        view,
        &format!("getComputedStyle(h).getPropertyValue('{prop}')"),
    )
}

/// `prop` at 50 ms of the transition, and the transitions running on `#h`.
fn midway(css: &str, transition: &str, prop: &str) -> (String, String, Engine, EngineViewId) {
    let (mut engine, view, server) = load(&page(css, transition));
    toggle_and_reach_50(&mut engine, view, prop);
    let value = computed(&mut engine, view, prop);
    let names = read(
        &mut engine,
        view,
        "h.getAnimations().map(function(a){return a.transitionProperty}).join()",
    );
    drop(server);
    (value, names, engine, view)
}

/// The channels of `rgb(...)`/`rgba(...)` (alpha 1 when absent).
fn channels(colour: &str) -> [f64; 4] {
    let inner = colour
        .trim_start_matches("rgba(")
        .trim_start_matches("rgb(")
        .trim_end_matches(')');
    let parts: Vec<f64> = inner
        .split(',')
        .map(|c| {
            c.trim()
                .parse()
                .unwrap_or_else(|_| panic!("colour {colour:?}"))
        })
        .collect();
    [
        parts[0],
        parts[1],
        parts[2],
        parts.get(3).copied().unwrap_or(1.0),
    ]
}

/// Chrome 148: rgb(128, 0, 128); 127.5 may round either way.
fn assert_purple(colour: &str, what: &str) {
    let [r, g, b, a] = channels(colour);
    assert!(
        (127.0..=128.0).contains(&r) && g == 0.0 && (127.0..=128.0).contains(&b) && a == 1.0,
        "{what} at 50 ms (Chrome: rgb(128, 0, 128)): {colour}"
    );
}

/// Chrome 148: rgba(255, 0, 0, 0.5).
fn assert_half_red(colour: &str, what: &str) {
    let [r, g, b, a] = channels(colour);
    assert!(
        r == 255.0 && g == 0.0 && b == 0.0 && (a - 0.5).abs() <= 0.005,
        "{what} at 50 ms (Chrome: rgba(255, 0, 0, 0.5)): {colour}"
    );
}

const LINEAR: &str = "all 100ms linear";

#[test]
fn outline_color_interpolates() {
    let (value, names, ..) = midway(
        "#h{outline:2px solid rgb(0,0,255)}#h.on{outline-color:rgb(255,0,0)}",
        LINEAR,
        "outline-color",
    );
    assert_purple(&value, "outline-color");
    assert_eq!(names, "outline-color");
}

#[test]
fn outline_color_from_currentcolor_interpolates_from_the_colour() {
    let (value, _, ..) = midway(
        "#h{color:rgb(0,0,255);outline:2px solid}#h.on{outline-color:rgb(255,0,0)}",
        LINEAR,
        "outline-color",
    );
    assert_purple(&value, "outline-color from currentcolor");
}

#[test]
fn outline_color_to_currentcolor_interpolates_to_the_colour() {
    let (value, _, ..) = midway(
        "#h{color:rgb(0,0,255);outline:2px solid rgb(255,0,0)}#h.on{outline-color:currentcolor}",
        LINEAR,
        "outline-color",
    );
    assert_purple(&value, "outline-color to currentcolor");
}

/// `currentcolor` before and after is no change, whatever `color` did:
/// the outline follows the colour at once and no transition runs.
#[test]
fn a_currentcolor_outline_starts_no_transition_when_the_colour_jumps() {
    let (value, names, mut engine, view) = midway(
        "#h{color:rgb(0,0,255);outline:2px solid}#h.on{color:rgb(255,0,0)}",
        "outline-color 100ms linear",
        "outline-color",
    );
    assert_eq!(value, "rgb(255, 0, 0)");
    assert_eq!(names, "");
    pump(&mut engine, view, 100, 10);
    assert_eq!(read(&mut engine, view, "log.join()"), "", "no events");
}

#[test]
fn outline_color_from_transparent_interpolates_premultiplied() {
    let (value, _, ..) = midway(
        "#h{outline:2px solid transparent}#h.on{outline-color:rgb(255,0,0)}",
        LINEAR,
        "outline-color",
    );
    assert_half_red(&value, "outline-color from transparent");
}

/// `transition: outline` names the shorthand, which covers its colour.
#[test]
fn the_outline_shorthand_covers_outline_color() {
    let (value, names, ..) = midway(
        "#h{outline:2px solid rgb(0,0,255)}#h.on{outline-color:rgb(255,0,0)}",
        "outline 100ms linear",
        "outline-color",
    );
    assert_purple(&value, "outline-color under `transition: outline`");
    assert_eq!(names, "outline-color");
}

/// Chrome 148: run, start and end, each once, named `outline-color`.
#[test]
fn an_outline_color_transition_fires_run_start_and_end() {
    let (_, _, mut engine, view) = midway(
        "#h{outline:2px solid rgb(0,0,255)}#h.on{outline-color:rgb(255,0,0)}",
        LINEAR,
        "outline-color",
    );
    assert_eq!(
        read(&mut engine, view, "log.join()"),
        "run:outline-color,start:outline-color"
    );
    pump(&mut engine, view, 100, 10);
    assert_eq!(
        read(&mut engine, view, "log.join()"),
        "run:outline-color,start:outline-color,end:outline-color"
    );
    assert_eq!(computed(&mut engine, view, "outline-color"), "rgb(255, 0, 0)");
}

#[test]
fn text_decoration_color_interpolates() {
    let (value, names, mut engine, view) = midway(
        "#h{text-decoration:underline;text-decoration-color:rgb(0,0,255)}#h.on{text-decoration-color:rgb(255,0,0)}",
        LINEAR,
        "text-decoration-color",
    );
    assert_purple(&value, "text-decoration-color");
    assert_eq!(names, "text-decoration-color");
    pump(&mut engine, view, 100, 10);
    assert_eq!(
        computed(&mut engine, view, "text-decoration-color"),
        "rgb(255, 0, 0)",
        "the end value"
    );
}

#[test]
fn text_decoration_color_from_transparent_interpolates_premultiplied() {
    let (value, _, ..) = midway(
        "#h{text-decoration:underline;text-decoration-color:transparent}#h.on{text-decoration-color:rgb(255,0,0)}",
        LINEAR,
        "text-decoration-color",
    );
    assert_half_red(&value, "text-decoration-color from transparent");
}

#[test]
fn the_text_decoration_shorthand_covers_its_colour() {
    let (value, names, ..) = midway(
        "#h{text-decoration:underline;text-decoration-color:rgb(0,0,255)}#h.on{text-decoration-color:rgb(255,0,0)}",
        "text-decoration 100ms linear",
        "text-decoration-color",
    );
    assert_purple(
        &value,
        "text-decoration-color under `transition: text-decoration`",
    );
    assert_eq!(names, "text-decoration-color");
}

/// Chrome 148: run, start and end, each once, named `text-decoration-color`.
#[test]
fn a_text_decoration_color_transition_fires_run_start_and_end() {
    let (_, _, mut engine, view) = midway(
        "#h{text-decoration:underline;text-decoration-color:rgb(0,0,255)}#h.on{text-decoration-color:rgb(255,0,0)}",
        LINEAR,
        "text-decoration-color",
    );
    pump(&mut engine, view, 100, 10);
    assert_eq!(
        read(&mut engine, view, "log.join()"),
        "run:text-decoration-color,start:text-decoration-color,end:text-decoration-color"
    );
}

/// The pixel at (x, y) of the view's frame as painted now.
fn pixel(engine: &mut Engine, view: EngineViewId, x: u32, y: u32) -> [u8; 4] {
    let (w, _, rgba) = engine.frame_rgba(view).expect("frame");
    let i = ((y * w + x) * 4) as usize;
    [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
}

fn assert_pixel(got: [u8; 4], want: [u8; 3], what: &str) {
    let close = got[..3]
        .iter()
        .zip(want)
        .all(|(g, w)| (*g as i32 - w as i32).abs() <= 1);
    assert!(close && got[3] == 255, "{what}: painted {got:?}, want {want:?}");
}

/// `outline-color` is a paint-only property (the LAT2 frame path): the
/// painted outline, not only `getComputedStyle`, is blue, then the
/// midpoint at 50 ms, then red. A pixel inside the 10 px outline band,
/// left of the box.
#[test]
fn the_painted_outline_moves_with_the_transition() {
    let (mut engine, view, _server) = load(&page(
        "#h{position:absolute;left:40px;top:40px;width:60px;height:60px;outline:10px solid rgb(0,0,255)}\
         #h.on{outline-color:rgb(255,0,0)}",
        "outline-color 100ms linear",
    ));
    assert_pixel(pixel(&mut engine, view, 35, 70), [0, 0, 255], "before");
    toggle_and_reach_50(&mut engine, view, "outline-color");
    let mid = pixel(&mut engine, view, 35, 70);
    assert!(
        (127..=128).contains(&mid[0]) && mid[1] <= 1 && (127..=128).contains(&mid[2]),
        "the outline at 50 ms (Chrome: rgb(128, 0, 128)): painted {mid:?}"
    );
    pump(&mut engine, view, 100, 10);
    assert_pixel(pixel(&mut engine, view, 35, 70), [255, 0, 0], "after");
}
