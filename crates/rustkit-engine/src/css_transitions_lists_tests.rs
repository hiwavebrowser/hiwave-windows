//! Transitions of the list properties (owner's hand test 2026-11-17,
//! group D): `box-shadow` and `filter` (and `backdrop-filter`, which takes
//! the same functions) jumped to their end value.
//!
//! box-shadow (css-backgrounds-3 §7.2): shadow by shadow, the shorter list
//! padded with transparent zero shadows; a pair whose `inset` differs is
//! not interpolable, so no transition starts. filter (filter-effects-1
//! §8): function by function when one list's functions are a prefix of
//! the other's, missing ones taken at their initial value; `none` is the
//! other list at its initial values; any other mismatch starts no
//! transition.
//!
//! The cascade kept every `box-shadow` declaration that matched, appended,
//! and read a comma list as one shadow: `#h{box-shadow:A}#h.on{box-shadow:B}`
//! painted both and `none` cleared nothing. A declaration now sets the
//! whole list (`none` empties it), which a transition needs to see a change
//! at all.
//!
//! Oracle: Chrome for Testing 148.0.7778.216 through
//! `tools/parity_oracle/css_transitions_lists_log.mjs`, receipt
//! `docs/diagnostics/2026-11-17/css_transitions_lists_cft148.json`: the
//! value at currentTime 50 of a 100 ms linear transition, the transitions
//! that start, and the events once finished.

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
            width: 800,
            height: 600,
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

fn pump(engine: &mut Engine, view: EngineViewId, total: u64, step: u64) {
    let rt = rt();
    let mut t = 0;
    while t < total {
        rt.block_on(engine.pump_live(view, step));
        t += step;
    }
}

/// The oracle's page: `#h` 100x50, 16px text, a gradient background that
/// does not repeat, `transition: all 100ms linear`, and `css` after.
fn page(css: &str) -> String {
    format!(
        "<!doctype html><style>body{{margin:0}}\
#h{{width:100px;height:50px;font:16px sans-serif;background:linear-gradient(red,blue) no-repeat;\
transition:all 100ms linear}}{css}</style><div id=h>text</div>\
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

/// `read` at 50 ms; the transitions running (sorted); after the end, the
/// events (sorted) and `read`'s end value.
fn run_case(css: &str, read_prop: &str) -> (String, Vec<String>, Vec<String>, String) {
    let (mut engine, view, _server) = load(&page(css));
    engine
        .execute_script(
            view,
            &format!("h.classList.add('on');getComputedStyle(h).getPropertyValue('{read_prop}')"),
        )
        .unwrap();
    pump(&mut engine, view, 50, 10);
    let mid = computed(&mut engine, view, read_prop);
    let mut names: Vec<String> = read(
        &mut engine,
        view,
        "h.getAnimations().map(function(a){return a.transitionProperty}).join()",
    )
    .split(',')
    .filter(|s| !s.is_empty())
    .map(str::to_string)
    .collect();
    names.sort();
    pump(&mut engine, view, 100, 10);
    let mut events: Vec<String> = read(&mut engine, view, "log.join()")
        .split(',')
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    events.sort();
    let end = computed(&mut engine, view, read_prop);
    (mid, names, events, end)
}

/// Chrome 148's midpoint and end; `props` transition, each firing run,
/// start and end once.
fn check(css: &str, read_prop: &str, props: &[&str], chrome_mid: &str, chrome_end: &str) {
    let (mid, names, events, end) = run_case(css, read_prop);
    assert_eq!(mid, chrome_mid, "{css}: {read_prop} at 50 ms");
    let mut want: Vec<String> = props.iter().map(|p| p.to_string()).collect();
    want.sort();
    assert_eq!(names, want, "{css}: the transitions running");
    let mut want_events: Vec<String> = props
        .iter()
        .flat_map(|p| [format!("end:{p}"), format!("run:{p}"), format!("start:{p}")])
        .collect();
    want_events.sort();
    assert_eq!(events, want_events, "{css}: the events");
    assert_eq!(end, chrome_end, "{css}: {read_prop} at the end");
}

/// Chrome 148 starts no transition: the value is the end value at once.
fn check_discrete(css: &str, read_prop: &str, chrome_end: &str) {
    let (mid, names, events, end) = run_case(css, read_prop);
    assert_eq!(
        (mid.as_str(), names.len(), events.len()),
        (chrome_end, 0, 0),
        "{css}: no transition"
    );
    assert_eq!(end, chrome_end, "{css}");
}

const BS: [&str; 1] = ["box-shadow"];
const FILTER: [&str; 1] = ["filter"];

// ------------------------------------------------- box-shadow, cascade

/// A later declaration replaces the list; `none` empties it; a comma list
/// is that many shadows (Chrome 148 reads them in that order).
#[test]
fn a_box_shadow_declaration_sets_the_whole_list() {
    let (end, ..) = run_case_end(
        "#h{box-shadow:1px 1px 0px 0px rgb(0,0,255)}#h.on{box-shadow:2px 2px 0px 0px rgb(255,0,0)}",
    );
    assert_eq!(end, "rgb(255, 0, 0) 2px 2px 0px 0px");
    let (end, ..) =
        run_case_end("#h{box-shadow:1px 1px 0px 0px rgb(0,0,255)}#h.on{box-shadow:none}");
    assert_eq!(end, "none");
    let (end, ..) =
        run_case_end("#h.on{box-shadow:1px 1px 0px 0px rgb(255,0,0),2px 2px 0px 0px rgb(0,0,255)}");
    assert_eq!(
        end,
        "rgb(255, 0, 0) 1px 1px 0px 0px, rgb(0, 0, 255) 2px 2px 0px 0px"
    );
}

/// The end value of `box-shadow` after the toggle, with no transition.
fn run_case_end(css: &str) -> (String, ()) {
    let css = format!("{css}#h{{transition:none}}");
    let (mid, _, _, end) = run_case(&css, "box-shadow");
    assert_eq!(mid, end, "{css}: no transition");
    (end, ())
}

/// The CSS-wide keywords replace the list too (R1 on #1290): `initial`,
/// `unset` and `revert` (the UA sheet has no shadow) are `none`; `inherit`
/// is the parent's whole list. Each starts a transition (Chrome 148).
#[test]
fn box_shadow_css_wide_keywords_replace_the_list() {
    let from = "#h{box-shadow:10px 20px 30px 4px rgb(255,0,0)}";
    for keyword in ["initial", "unset", "revert", "revert-layer"] {
        check(
            &format!("{from}#h.on{{box-shadow:{keyword}}}"),
            "box-shadow",
            &BS,
            "rgba(255, 0, 0, 0.5) 5px 10px 15px 2px",
            "none",
        );
    }
    check(
        "body{box-shadow:10px 20px 30px 4px rgb(255,0,0),2px 2px 0px 0px rgb(0,0,255)}\
         #h.on{box-shadow:inherit}",
        "box-shadow",
        &BS,
        "rgba(255, 0, 0, 0.5) 5px 10px 15px 2px, rgba(0, 0, 255, 0.5) 1px 1px 0px 0px",
        "rgb(255, 0, 0) 10px 20px 30px 4px, rgb(0, 0, 255) 2px 2px 0px 0px",
    );
    check(
        &format!("body{{box-shadow:none}}{from}#h.on{{box-shadow:inherit}}"),
        "box-shadow",
        &BS,
        "rgba(255, 0, 0, 0.5) 5px 10px 15px 2px",
        "none",
    );
}

/// `revert-layer` inside a layer rolls back to the earlier layer's shadow,
/// or to none when no earlier layer sets one (R1 on #1290, Chrome 148).
#[test]
fn box_shadow_revert_layer_inside_a_layer() {
    check(
        "@layer a{#h.on{box-shadow:10px 20px 30px 4px rgb(255,0,0)}}\
         @layer b{#h{box-shadow:0px 0px 0px 0px rgb(0,0,255)}#h.on{box-shadow:revert-layer}}",
        "box-shadow",
        &BS,
        "rgb(128, 0, 128) 5px 10px 15px 2px",
        "rgb(255, 0, 0) 10px 20px 30px 4px",
    );
    check(
        "@layer a{#h{color:red}}\
         @layer b{#h{box-shadow:10px 20px 30px 4px rgb(255,0,0)}#h.on{box-shadow:revert-layer}}",
        "box-shadow",
        &BS,
        "rgba(255, 0, 0, 0.5) 5px 10px 15px 2px",
        "none",
    );
}

// --------------------------------------------- box-shadow, transitions

#[test]
fn box_shadow_interpolates_lengths_and_colour() {
    check(
        "#h{box-shadow:0px 0px 0px 0px rgb(0,0,255)}#h.on{box-shadow:10px 20px 30px 4px rgb(255,0,0)}",
        "box-shadow",
        &BS,
        "rgb(128, 0, 128) 5px 10px 15px 2px",
        "rgb(255, 0, 0) 10px 20px 30px 4px",
    );
    check(
        "#h{box-shadow:inset 0px 0px 0px 0px rgb(0,0,255)}#h.on{box-shadow:inset 10px 10px 10px 2px rgb(255,0,0)}",
        "box-shadow",
        &BS,
        "rgb(128, 0, 128) 5px 5px 5px 1px inset",
        "rgb(255, 0, 0) 10px 10px 10px 2px inset",
    );
}

/// `none` is a transparent zero shadow of the other's kind.
#[test]
fn box_shadow_from_and_to_none() {
    check(
        "#h{box-shadow:none}#h.on{box-shadow:10px 20px 30px 4px rgb(255,0,0)}",
        "box-shadow",
        &BS,
        "rgba(255, 0, 0, 0.5) 5px 10px 15px 2px",
        "rgb(255, 0, 0) 10px 20px 30px 4px",
    );
    check(
        "#h{box-shadow:10px 20px 30px 4px rgb(255,0,0)}#h.on{box-shadow:none}",
        "box-shadow",
        &BS,
        "rgba(255, 0, 0, 0.5) 5px 10px 15px 2px",
        "none",
    );
}

/// The shorter list is padded; the Tailwind ring (a spread) moves.
#[test]
fn box_shadow_lists_of_different_lengths_pad_the_shorter() {
    check(
        "#h{box-shadow:10px 10px 0px 0px rgb(255,0,0)}\
         #h.on{box-shadow:20px 20px 0px 0px rgb(255,0,0),4px 4px 0px 0px rgb(0,0,255)}",
        "box-shadow",
        &BS,
        "rgb(255, 0, 0) 15px 15px 0px 0px, rgba(0, 0, 255, 0.5) 2px 2px 0px 0px",
        "rgb(255, 0, 0) 20px 20px 0px 0px, rgb(0, 0, 255) 4px 4px 0px 0px",
    );
    check(
        "#h{box-shadow:0 0 0 0px rgba(59,130,246,0.5)}#h.on{box-shadow:0 0 0 4px rgba(59,130,246,0.5)}",
        "box-shadow",
        &BS,
        "rgba(59, 130, 246, 0.5) 0px 0px 0px 2px",
        "rgba(59, 130, 246, 0.5) 0px 0px 0px 4px",
    );
}

/// An outer shadow against an inset one: no transition (Chrome 148).
#[test]
fn box_shadow_inset_against_outer_is_discrete() {
    check_discrete(
        "#h{box-shadow:0px 0px 0px 0px rgb(0,0,255)}#h.on{box-shadow:inset 10px 10px 10px 2px rgb(255,0,0)}",
        "box-shadow",
        "rgb(255, 0, 0) 10px 10px 10px 2px inset",
    );
}

/// The painted shadow moves: 10 px of solid red to the right of the box
/// at the end, half of it at 50 ms.
#[test]
fn the_painted_shadow_moves_with_the_transition() {
    let page = "<!doctype html><style>body{margin:0}\
#h{position:absolute;left:20px;top:20px;width:40px;height:40px;background:rgb(0,0,255);\
box-shadow:0px 0px 0px 0px rgb(255,0,0);transition:box-shadow 100ms linear}\
#h.on{box-shadow:20px 0px 0px 0px rgb(255,0,0)}</style><div id=h></div>\
<script>var h=document.getElementById('h');</script>";
    let (mut engine, view, _server) = load(page);
    let red_at = |engine: &mut Engine, x: u32| {
        let (w, _, rgba) = engine.frame_rgba(view).expect("frame");
        let i = ((40 * w + x) * 4) as usize;
        rgba[i] >= 250 && rgba[i + 1] <= 5 && rgba[i + 2] <= 5
    };
    assert!(
        !red_at(&mut engine, 65),
        "before: no shadow right of the box"
    );
    engine
        .execute_script(view, "h.classList.add('on');getComputedStyle(h).boxShadow")
        .unwrap();
    pump(&mut engine, view, 50, 10);
    assert!(
        red_at(&mut engine, 65),
        "at 50 ms: the shadow reaches 60..70"
    );
    assert!(!red_at(&mut engine, 75), "at 50 ms: not past 70");
    pump(&mut engine, view, 100, 10);
    assert!(red_at(&mut engine, 75), "after: the shadow reaches 60..80");
}

// ------------------------------------------------------------- filter

#[test]
fn filter_interpolates_matching_functions() {
    check(
        "#h{filter:blur(0px)}#h.on{filter:blur(10px)}",
        "filter",
        &FILTER,
        "blur(5px)",
        "blur(10px)",
    );
    check(
        "#h{filter:brightness(1)}#h.on{filter:brightness(2)}",
        "filter",
        &FILTER,
        "brightness(1.5)",
        "brightness(2)",
    );
    check(
        "#h{filter:blur(0px) brightness(1)}#h.on{filter:blur(10px) brightness(2)}",
        "filter",
        &FILTER,
        "blur(5px) brightness(1.5)",
        "blur(10px) brightness(2)",
    );
    check(
        "#h{filter:hue-rotate(0deg)}#h.on{filter:hue-rotate(180deg)}",
        "filter",
        &FILTER,
        "hue-rotate(90deg)",
        "hue-rotate(180deg)",
    );
    check(
        "#h{filter:opacity(1) saturate(1) contrast(1) invert(0) sepia(0)}\
         #h.on{filter:opacity(0) saturate(2) contrast(2) invert(1) sepia(1)}",
        "filter",
        &FILTER,
        "opacity(0.5) saturate(1.5) contrast(1.5) invert(0.5) sepia(0.5)",
        "opacity(0) saturate(2) contrast(2) invert(1) sepia(1)",
    );
    check(
        "#h{filter:drop-shadow(0px 0px 0px rgb(0,0,255))}#h.on{filter:drop-shadow(10px 10px 4px rgb(255,0,0))}",
        "filter",
        &FILTER,
        "drop-shadow(rgb(128, 0, 128) 5px 5px 2px)",
        "drop-shadow(rgb(255, 0, 0) 10px 10px 4px)",
    );
}

/// `none` and a shorter list take the missing functions at their initial
/// value.
#[test]
fn filter_none_and_a_prefix_list_take_initial_values() {
    check(
        "#h{filter:none}#h.on{filter:blur(10px)}",
        "filter",
        &FILTER,
        "blur(5px)",
        "blur(10px)",
    );
    check(
        "#h{filter:blur(10px)}#h.on{filter:none}",
        "filter",
        &FILTER,
        "blur(5px)",
        "none",
    );
    check(
        "#h{filter:none}#h.on{filter:grayscale(1)}",
        "filter",
        &FILTER,
        "grayscale(0.5)",
        "grayscale(1)",
    );
    check(
        "#h{filter:blur(0px)}#h.on{filter:blur(10px) brightness(2)}",
        "filter",
        &FILTER,
        "blur(5px) brightness(1.5)",
        "blur(10px) brightness(2)",
    );
}

#[test]
fn filter_lists_that_do_not_match_are_discrete() {
    check_discrete(
        "#h{filter:blur(10px)}#h.on{filter:grayscale(1)}",
        "filter",
        "grayscale(1)",
    );
}

#[test]
fn backdrop_filter_interpolates() {
    check(
        "#h{backdrop-filter:blur(0px)}#h.on{backdrop-filter:blur(10px)}",
        "backdrop-filter",
        &["backdrop-filter"],
        "blur(5px)",
        "blur(10px)",
    );
}
