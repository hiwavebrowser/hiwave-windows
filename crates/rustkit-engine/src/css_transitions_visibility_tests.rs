//! `visibility` transitions (owner's hand test 2026-11-17, group E). The
//! value jumped at once. css-transitions-1 / web-animations-1: visibility
//! interpolates as discrete, except that when one end is `visible` the
//! value is `visible` for the whole of the transition between the end
//! points (so a fade-out with `visibility: hidden` stays visible until it
//! ends, and a fade-in is visible from its first frame). Two values that
//! are both not `visible` start no transition (Chrome 148: `hidden` to
//! `collapse`).
//!
//! Oracle: Chrome for Testing 148.0.7778.216 through
//! `tools/parity_oracle/css_transitions_visibility_log.mjs`, receipt
//! `docs/diagnostics/2026-11-17/css_transitions_visibility_cft148.json`:
//! the value at currentTime 0, 1, 50 and 99 of a 100 ms linear
//! transition, the transitions that start, and the events once finished.
//!
//! Sampling is on the page's virtual clock: `.on` lands and a style read
//! in the same script starts the transition at the clock as it stands.

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

/// `visibility` at 0, 10, 50 and 90 ms of the transition, then the
/// transitions running at 90 ms, the events and the end value.
fn samples(css: &str) -> (Vec<String>, Vec<String>, Vec<String>, String) {
    let (mut engine, view, _server) = load(&page(css));
    engine
        .execute_script(
            view,
            "h.classList.add('on');getComputedStyle(h).getPropertyValue('visibility')",
        )
        .unwrap();
    let mut at = vec![computed(&mut engine, view, "visibility")];
    for step in [10, 40, 40] {
        pump(&mut engine, view, step, 10);
        at.push(computed(&mut engine, view, "visibility"));
    }
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
    pump(&mut engine, view, 50, 10);
    let mut events: Vec<String> = read(&mut engine, view, "log.join()")
        .split(',')
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    events.sort();
    let end = computed(&mut engine, view, "visibility");
    (at, names, events, end)
}

fn events_of(props: &[&str]) -> Vec<String> {
    let mut out: Vec<String> = props
        .iter()
        .flat_map(|p| [format!("end:{p}"), format!("run:{p}"), format!("start:{p}")])
        .collect();
    out.sort();
    out
}

/// Chrome 148: visible at 0, 1, 50 and 99 ms; hidden once over.
#[test]
fn visible_to_hidden_stays_visible_until_the_end() {
    let (at, names, events, end) = samples("#h{visibility:visible}#h.on{visibility:hidden}");
    assert_eq!(at, ["visible"; 4], "at 0, 10, 50, 90 ms");
    assert_eq!(names, ["visibility"]);
    assert_eq!(events, events_of(&["visibility"]));
    assert_eq!(end, "hidden");
}

/// Chrome 148: hidden at 0, visible from 1 ms on.
#[test]
fn hidden_to_visible_is_visible_from_the_first_moment() {
    let (at, names, events, end) = samples("#h{visibility:hidden}#h.on{visibility:visible}");
    assert_eq!(
        at,
        ["hidden", "visible", "visible", "visible"],
        "at 0, 10, 50, 90 ms"
    );
    assert_eq!(names, ["visibility"]);
    assert_eq!(events, events_of(&["visibility"]));
    assert_eq!(end, "visible");
}

#[test]
fn visible_to_collapse_stays_visible_until_the_end() {
    let (at, _, _, end) = samples("#h{visibility:visible}#h.on{visibility:collapse}");
    assert_eq!(at, ["visible"; 4]);
    assert_eq!(end, "collapse");
}

/// Neither end is visible: discrete, no transition (Chrome 148).
#[test]
fn hidden_to_collapse_starts_no_transition() {
    let (at, names, events, end) = samples("#h{visibility:hidden}#h.on{visibility:collapse}");
    assert_eq!(at, ["collapse"; 4]);
    assert!(
        names.is_empty() && events.is_empty(),
        "{names:?} {events:?}"
    );
    assert_eq!(end, "collapse");
}

/// The fade-out pattern: `opacity` and `visibility` together, both run,
/// and the element stays visible while it fades.
#[test]
fn a_fade_out_with_visibility_hidden_stays_visible_while_it_fades() {
    let (at, names, events, end) =
        samples("#h{visibility:visible;opacity:1}#h.on{visibility:hidden;opacity:0}");
    assert_eq!(at, ["visible"; 4]);
    assert_eq!(names, ["opacity", "visibility"]);
    assert_eq!(events, events_of(&["opacity", "visibility"]));
    assert_eq!(end, "hidden");
}

/// `transition: visibility` named alone.
#[test]
fn visibility_named_alone_transitions() {
    let (at, names, _, end) = samples(
        "#h{visibility:visible;transition:visibility 100ms linear}#h.on{visibility:hidden}",
    );
    assert_eq!(at, ["visible"; 4]);
    assert_eq!(names, ["visibility"]);
    assert_eq!(end, "hidden");
}

/// A child with no `visibility` of its own and no transition inherits the
/// parent's animated value (R1 on #1289). Chrome 148: the child reads the
/// parent's value at every sample (visible through a fade-out, visible
/// from the first moment of a fade-in), runs no transition of its own,
/// and its painted box follows.
#[test]
fn a_child_follows_its_parents_animated_visibility() {
    let page = |css: &str| {
        format!(
            "<!doctype html><style>body{{margin:0}}\
#h{{width:100px;height:50px;font:16px sans-serif;transition:all 100ms linear}}\
#c{{display:block;width:20px;height:20px;background:rgb(0,255,0)}}{css}</style>\
<div id=h><span id=c></span></div>\
<script>var h=document.getElementById('h');var c=document.getElementById('c');</script>"
        )
    };
    let green = |engine: &mut Engine, view: EngineViewId| {
        let (w, _, rgba) = engine.frame_rgba(view).expect("frame");
        let i = ((10 * w + 10) * 4) as usize;
        rgba[i] <= 5 && rgba[i + 1] >= 250 && rgba[i + 2] <= 5
    };
    for (css, want_at, want_end) in [
        (
            "#h{visibility:visible}#h.on{visibility:hidden}",
            ["visible", "visible", "visible", "visible"],
            "hidden",
        ),
        (
            "#h{visibility:hidden}#h.on{visibility:visible}",
            ["hidden", "visible", "visible", "visible"],
            "visible",
        ),
    ] {
        let (mut engine, view, _server) = load(&page(css));
        engine
            .execute_script(
                view,
                "h.classList.add('on');getComputedStyle(h).visibility;getComputedStyle(c).visibility",
            )
            .unwrap();
        let mut at = vec![read(&mut engine, view, "getComputedStyle(c).visibility")];
        let mut painted = vec![];
        for step in [10, 40, 40] {
            pump(&mut engine, view, step, 10);
            at.push(read(&mut engine, view, "getComputedStyle(c).visibility"));
            painted.push(green(&mut engine, view));
        }
        assert_eq!(at, want_at, "{css}: the child at 0, 10, 50, 90 ms");
        assert_eq!(
            painted, [true; 3],
            "{css}: the child painted at 10, 50, 90 ms"
        );
        assert_eq!(
            read(&mut engine, view, "c.getAnimations().length"),
            "0",
            "{css}"
        );
        pump(&mut engine, view, 50, 10);
        assert_eq!(
            read(&mut engine, view, "getComputedStyle(c).visibility"),
            want_end,
            "{css}: the child at the end"
        );
        assert_eq!(
            green(&mut engine, view),
            want_end == "visible",
            "{css}: painted at the end"
        );
    }
}
