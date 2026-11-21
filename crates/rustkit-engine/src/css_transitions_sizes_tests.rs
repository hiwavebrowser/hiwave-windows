//! Transitions of lengths and sizes (owner's hand test 2026-11-17, group
//! C): `line-height`, the four `border-*-radius` corners and
//! `background-size` jumped to their end value, and a length between two
//! units (`10px` to `50%`, `1rem` to `50%`, which Tailwind writes) started
//! no transition at all. Two units now interpolate as a `calc()` mix.
//!
//! Oracle: Chrome for Testing 148.0.7778.216 through
//! `tools/parity_oracle/css_transitions_sizes_log.mjs`, receipt
//! `docs/diagnostics/2026-11-17/css_transitions_sizes_cft148.json`: the
//! value at currentTime 50 of a 100 ms linear transition, the transitions
//! that start, and the events once finished.
//!
//! Sampling is on the page's virtual clock: `.on` lands and a style read
//! in the same script starts the transition at the clock as it stands,
//! then `pump_live` moves the clock exactly 50 ms.

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

const CORNERS: [&str; 4] = [
    "border-top-left-radius",
    "border-top-right-radius",
    "border-bottom-right-radius",
    "border-bottom-left-radius",
];

// ---------------------------------------------------- two units: calc()

#[test]
fn a_length_between_two_units_interpolates_as_calc() {
    check(
        "#h{width:100px}#h.on{width:50%}",
        "width",
        &["width"],
        "250px",
        "400px",
    );
    check(
        "#h{padding-left:10px}#h.on{padding-left:10%}",
        "padding-left",
        &["padding-left"],
        "45px",
        "80px",
    );
}

/// The case group B pinned as a jump: px or rem against % now runs.
#[test]
fn a_translate_between_two_units_interpolates_as_calc() {
    let translate = ["translate"];
    check(
        "#h{translate:10px}#h.on{translate:50%}",
        "translate",
        &translate,
        "calc(25% + 5px)",
        "50%",
    );
    check(
        "#h{translate:1rem}#h.on{translate:50%}",
        "translate",
        &translate,
        "calc(25% + 8px)",
        "50%",
    );
}

// ---------------------------------------------------------- line-height

#[test]
fn line_height_interpolates_lengths_and_numbers() {
    let lh = ["line-height"];
    check(
        "#h{line-height:20px}#h.on{line-height:40px}",
        "line-height",
        &lh,
        "30px",
        "40px",
    );
    check(
        "#h{line-height:1}#h.on{line-height:2}",
        "line-height",
        &lh,
        "24px",
        "32px",
    );
    check(
        "#h{line-height:100%}#h.on{line-height:200%}",
        "line-height",
        &lh,
        "24px",
        "32px",
    );
}

#[test]
fn line_height_normal_and_a_number_against_a_length_are_discrete() {
    check_discrete(
        "#h{line-height:1}#h.on{line-height:40px}",
        "line-height",
        "40px",
    );
    check_discrete(
        "#h{line-height:normal}#h.on{line-height:40px}",
        "line-height",
        "40px",
    );
}

// -------------------------------------------------------- border-radius

#[test]
fn border_radius_interpolates_every_corner() {
    check(
        "#h{border-radius:0px}#h.on{border-radius:20px}",
        "border-top-left-radius",
        &CORNERS,
        "10px",
        "20px",
    );
    check(
        "#h{border-radius:0px}#h.on{border-radius:10px 20px 30px 40px}",
        "border-radius",
        &CORNERS,
        "5px 10px 15px 20px",
        "10px 20px 30px 40px",
    );
}

#[test]
fn border_radius_interpolates_both_radii_and_percentages() {
    check(
        "#h{border-radius:10px / 20px}#h.on{border-radius:30px / 40px}",
        "border-top-left-radius",
        &CORNERS,
        "20px 30px",
        "30px 40px",
    );
    // Chrome 148 writes these as `25%` / `calc(25% + 5px)` / `50%`: a
    // percentage radius stays a percentage in getComputedStyle
    // (`radius_computed`, border-radius computed values).
    check(
        "#h{border-radius:0%}#h.on{border-radius:50%}",
        "border-top-left-radius",
        &CORNERS,
        "25%",
        "50%",
    );
    check(
        "#h{border-radius:10px}#h.on{border-radius:50%}",
        "border-top-left-radius",
        &CORNERS,
        "calc(25% + 5px)",
        "50%",
    );
}

#[test]
fn one_named_corner_transitions_alone() {
    check(
        "#h{border-top-right-radius:0px;transition:border-top-right-radius 100ms linear}\
         #h.on{border-top-right-radius:20px}",
        "border-top-right-radius",
        &["border-top-right-radius"],
        "10px",
        "20px",
    );
}

// ------------------------------------------------------ background-size

#[test]
fn background_size_interpolates_each_dimension() {
    let bs = ["background-size"];
    check(
        "#h{background-size:10px 20px}#h.on{background-size:30px 40px}",
        "background-size",
        &bs,
        "20px 30px",
        "30px 40px",
    );
    check(
        "#h{background-size:0% 0%}#h.on{background-size:100% 50%}",
        "background-size",
        &bs,
        "50% 25%",
        "100% 50%",
    );
    check(
        "#h{background-size:10px auto}#h.on{background-size:30px auto}",
        "background-size",
        &bs,
        "20px",
        "30px",
    );
}

#[test]
fn background_size_interpolates_per_layer() {
    check(
        "#h{background:linear-gradient(red,blue) no-repeat,linear-gradient(green,blue) no-repeat;\
         background-size:10px 10px,20px 20px}#h.on{background-size:30px 30px,40px 40px}",
        "background-size",
        &["background-size"],
        "20px 20px, 30px 30px",
        "30px 30px, 40px 40px",
    );
}

#[test]
fn background_size_keywords_are_discrete() {
    check_discrete(
        "#h{background-size:auto}#h.on{background-size:30px 40px}",
        "background-size",
        "30px 40px",
    );
    check_discrete(
        "#h{background-size:cover}#h.on{background-size:30px 40px}",
        "background-size",
        "30px 40px",
    );
}
