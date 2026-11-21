//! Transitions of registered custom properties (group F of the CSS
//! transitions brief, CSS Properties and Values API Level 1 §6). An
//! `@property` rule (or `CSS.registerProperty()`) gives a custom property a
//! syntax: its computed value is then typed (`red` computes to
//! `rgb(255, 0, 0)`, `1em` to px), and a transition naming it interpolates.
//! An unregistered custom property stays discrete: no transition, no
//! events, and no `CSSTransition.--x` in the Inspector's unsupported list
//! (Tailwind v4's `--tw-gradient-from` on x.com reported one per name).
//!
//! Oracle: Chrome for Testing 148.0.7778.216 through
//! `tools/parity_oracle/css_transitions_property_log.mjs`, receipt
//! `docs/diagnostics/2026-11-20/css_transitions_property_cft148.json`:
//! the value at currentTime 0, 1, 50 and 99 of a 100 ms linear
//! transition, the transitions that start, the events once finished, and
//! the painted pixel at 50 ms. Here the samples are 0, 50 and 110 ms on the
//! page's virtual clock.

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

/// The oracle's page: `#h` 100x50 holding `#c` 50x20, `css` after, and
/// `script` before the elements.
fn page(css: &str, script: &str) -> String {
    format!(
        "<!doctype html><style>body{{margin:0}}#h{{width:100px;height:50px}}#c{{width:50px;height:20px}}{css}</style>\
<script>{script}</script><div id=h><div id=c></div></div>\
<script>var h=document.getElementById('h');var c=document.getElementById('c');window.log=[];\
['transitionrun','transitionstart','transitionend','transitioncancel'].forEach(function(t){{\
h.addEventListener(t,function(e){{if(e.target===h)log.push(e.type.slice(10)+':'+e.propertyName)}})}});</script>"
    )
}

fn reg(name: &str, syntax: &str, inherits: bool, initial: &str) -> String {
    format!("@property {name}{{syntax:'{syntax}';inherits:{inherits};initial-value:{initial}}}")
}

fn transition(prop: &str) -> String {
    format!("transition:{prop} 100ms linear")
}

/// What one case shows.
#[derive(Debug)]
struct Run {
    before: String,
    /// At 0 and 50 ms.
    at: [String; 2],
    /// `transitionProperty` of the running transitions at 50 ms, by element.
    transitions: Vec<String>,
    events: Vec<String>,
    end: String,
    /// The painted pixels at 50 ms: x 2, 50 and 97 of row 40.
    painted: [[u8; 4]; 3],
}

fn run(css: &str, script: &str, read_id: &str, prop: &str) -> Run {
    let (mut engine, view, _server) = load(&page(css, script));
    let value = format!("getComputedStyle({read_id}).getPropertyValue('{prop}')");
    let before = read(&mut engine, view, &value);
    engine
        .execute_script(view, &format!("h.classList.add('on');{value}"))
        .unwrap();
    let at0 = read(&mut engine, view, &value);
    pump(&mut engine, view, 50, 10);
    let at50 = read(&mut engine, view, &value);
    let (w, _, rgba) = engine.frame_rgba(view).expect("frame");
    let px = |x: u32, y: u32| {
        let i = ((y * w + x) * 4) as usize;
        [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
    };
    let painted = [px(2, 40), px(50, 40), px(97, 40)];
    let transitions: Vec<String> = read(
        &mut engine,
        view,
        "document.getAnimations().map(function(a){return a.effect.target.id+':'+a.transitionProperty}).join()",
    )
    .split(',')
    .filter(|s| !s.is_empty())
    .map(str::to_string)
    .collect();
    pump(&mut engine, view, 60, 10);
    let mut events: Vec<String> = read(&mut engine, view, "log.join()")
        .split(',')
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    events.sort();
    let end = read(&mut engine, view, &value);
    Run {
        before,
        at: [at0, at50],
        transitions,
        events,
        end,
        painted,
    }
}

fn events_of(props: &[&str]) -> Vec<String> {
    let mut out: Vec<String> = props
        .iter()
        .flat_map(|p| [format!("end:{p}"), format!("run:{p}"), format!("start:{p}")])
        .collect();
    out.sort();
    out
}

/// One registered property `--x` on `#h`, transitioned from `from` to `to`:
/// before, at 0 ms, at 50 ms, the end.
fn typed(syntax: &str, initial: &str, from: &str, to: &str, extra: &str) -> Run {
    run(
        &format!(
            "{}#h{{{extra}--x:{from};{}}}#h.on{{--x:{to}}}",
            reg("--x", syntax, false, initial),
            transition("--x")
        ),
        "",
        "h",
        "--x",
    )
}

fn assert_runs(r: &Run, want: [&str; 4], what: &str) {
    assert_eq!(
        (
            r.before.as_str(),
            r.at[0].as_str(),
            r.at[1].as_str(),
            r.end.as_str()
        ),
        (want[0], want[1], want[2], want[3]),
        "{what}: before, 0 ms, 50 ms, end ({r:?})"
    );
    assert_eq!(r.transitions, ["h:--x"], "{what}: the running transitions");
    assert_eq!(r.events, events_of(&["--x"]), "{what}: the events");
}

#[test]
fn a_registered_color_interpolates() {
    let r = typed("<color>", "black", "rgb(0,0,255)", "red", "");
    assert_runs(
        &r,
        [
            "rgb(0, 0, 255)",
            "rgb(0, 0, 255)",
            "rgb(128, 0, 128)",
            "rgb(255, 0, 0)",
        ],
        "<color>",
    );
}

#[test]
fn a_registered_length_interpolates() {
    let r = typed("<length>", "0px", "10px", "110px", "");
    assert_runs(&r, ["10px", "10px", "60px", "110px"], "<length>");
}

/// `1em` computes to px against the element's font size (Chrome: 10px).
#[test]
fn a_registered_length_in_em_computes_to_px() {
    let r = typed("<length>", "0px", "1em", "30px", "font-size:10px;");
    assert_runs(&r, ["10px", "10px", "20px", "30px"], "<length> em");
}

/// A length to a percentage: a `calc()` at every point between, its first
/// point too (Chrome: `calc(0% + 10px)` at 0, `calc(25% + 5px)` at 50).
#[test]
fn a_registered_length_percentage_interpolates_as_calc() {
    let r = typed("<length-percentage>", "0px", "10px", "50%", "");
    assert_runs(
        &r,
        ["10px", "calc(0% + 10px)", "calc(25% + 5px)", "50%"],
        "<length-percentage>",
    );
}

#[test]
fn registered_numbers_percentages_and_angles_interpolate() {
    assert_runs(
        &typed("<number>", "0", "0", "10", ""),
        ["0", "0", "5", "10"],
        "<number>",
    );
    assert_runs(
        &typed("<integer>", "0", "0", "10", ""),
        ["0", "0", "5", "10"],
        "<integer>",
    );
    assert_runs(
        &typed("<percentage>", "0%", "0%", "100%", ""),
        ["0%", "0%", "50%", "100%"],
        "<percentage>",
    );
    assert_runs(
        &typed("<angle>", "0deg", "0deg", "1turn", ""),
        ["0deg", "0deg", "180deg", "360deg"],
        "<angle>",
    );
}

#[test]
fn registered_lists_interpolate_item_by_item() {
    assert_runs(
        &typed("<length>+", "0px", "0px 0px", "10px 20px", ""),
        ["0px 0px", "0px 0px", "5px 10px", "10px 20px"],
        "<length>+",
    );
    assert_runs(
        &typed("<color>#", "black", "blue, red", "red, blue", ""),
        [
            "rgb(0, 0, 255), rgb(255, 0, 0)",
            "rgb(0, 0, 255), rgb(255, 0, 0)",
            "rgb(128, 0, 128), rgb(128, 0, 128)",
            "rgb(255, 0, 0), rgb(0, 0, 255)",
        ],
        "<color>#",
    );
}

/// `*` is a token stream: discrete, no transition (Chrome 148).
#[test]
fn a_universal_syntax_is_discrete() {
    let r = typed("*", "0px", "0px", "100px", "");
    assert_eq!(
        (r.at[0].as_str(), r.at[1].as_str(), r.end.as_str()),
        ("100px", "100px", "100px")
    );
    assert!(r.transitions.is_empty() && r.events.is_empty(), "{r:?}");
}

/// The computed value of a registered property with no transition: a
/// keyword colour computes to `rgb()`, an undeclared one is the initial
/// value, an invalid declaration falls back to the initial value.
#[test]
fn registered_computed_values() {
    let value = |css: &str| {
        let (mut engine, view, _server) = load(&page(css, ""));
        read(
            &mut engine,
            view,
            "getComputedStyle(h).getPropertyValue('--x')",
        )
    };
    assert_eq!(
        value(&format!(
            "{}#h{{--x:red}}",
            reg("--x", "<color>", false, "black")
        )),
        "rgb(255, 0, 0)"
    );
    assert_eq!(
        value(&reg("--x", "<color>", false, "rgb(1, 2, 3)")),
        "rgb(1, 2, 3)"
    );
    assert_eq!(
        value(&format!(
            "{}#h{{--x:red}}",
            reg("--x", "<length>", false, "7px")
        )),
        "7px"
    );
}

/// `inherits: true`: the child follows the parent's animated value.
#[test]
fn an_inherited_registered_property_reaches_the_child_as_it_moves() {
    let r = run(
        &format!(
            "{}#h{{--x:0px;{}}}#h.on{{--x:100px}}",
            reg("--x", "<length>", true, "0px"),
            transition("--x")
        ),
        "",
        "c",
        "--x",
    );
    assert_eq!(
        (
            r.before.as_str(),
            r.at[0].as_str(),
            r.at[1].as_str(),
            r.end.as_str()
        ),
        ("0px", "0px", "50px", "100px"),
        "{r:?}"
    );
}

/// `inherits: false`: the child has the initial value throughout.
#[test]
fn a_non_inherited_registered_property_is_the_initial_value_on_the_child() {
    let r = run(
        &format!(
            "{}#h{{--x:0px;{}}}#h.on{{--x:100px}}",
            reg("--x", "<length>", false, "5px"),
            transition("--x")
        ),
        "",
        "c",
        "--x",
    );
    assert_eq!(
        (
            r.before.as_str(),
            r.at[0].as_str(),
            r.at[1].as_str(),
            r.end.as_str()
        ),
        ("5px", "5px", "5px", "5px"),
        "{r:?}"
    );
    assert_eq!(r.transitions, ["h:--x"]);
}

/// An unregistered custom property is discrete: it jumps, no transition
/// runs and no event fires, named or under `all` (Chrome 148).
#[test]
fn an_unregistered_custom_property_jumps() {
    for t in ["--u", "all"] {
        let r = run(
            &format!("#h{{--u:0px;{}}}#h.on{{--u:100px}}", transition(t)),
            "",
            "h",
            "--u",
        );
        assert_eq!(
            (
                r.before.as_str(),
                r.at[0].as_str(),
                r.at[1].as_str(),
                r.end.as_str()
            ),
            ("0px", "100px", "100px", "100px"),
            "transition: {t}"
        );
        assert!(
            r.transitions.is_empty() && r.events.is_empty(),
            "transition: {t}: {r:?}"
        );
    }
}

/// `transition: all` does not cover a registered custom property either
/// (Chrome 148 starts none).
#[test]
fn transition_all_does_not_cover_a_registered_property() {
    let r = run(
        &format!(
            "{}#h{{--x:0px;{}}}#h.on{{--x:100px}}",
            reg("--x", "<length>", false, "0px"),
            transition("all")
        ),
        "",
        "h",
        "--x",
    );
    assert_eq!(
        (r.at[0].as_str(), r.at[1].as_str()),
        ("100px", "100px"),
        "{r:?}"
    );
    assert!(r.transitions.is_empty() && r.events.is_empty(), "{r:?}");
}

fn assert_pixel(got: [u8; 4], want: [u8; 3], tolerance: i32, what: &str) {
    let close = got[..3]
        .iter()
        .zip(want)
        .all(|(g, w)| (*g as i32 - w as i32).abs() <= tolerance);
    assert!(
        close && got[3] == 255,
        "{what}: painted {got:?}, want {want:?}"
    );
}

/// A property that reads the registered one moves with it: its computed
/// value and its paint (Chrome: rgb(128, 0, 128) at 50 ms).
#[test]
fn background_color_through_var_follows_the_registered_color() {
    let r = run(
        &format!(
            "{}#h{{--x:rgb(0,0,255);background-color:var(--x);{}}}#h.on{{--x:rgb(255,0,0)}}",
            reg("--x", "<color>", false, "black"),
            transition("--x")
        ),
        "",
        "h",
        "background-color",
    );
    assert_eq!(
        (r.at[0].as_str(), r.at[1].as_str(), r.end.as_str()),
        ("rgb(0, 0, 255)", "rgb(128, 0, 128)", "rgb(255, 0, 0)"),
        "{r:?}"
    );
    assert_pixel(r.painted[1], [128, 0, 128], 1, "background at 50 ms");
}

/// Layout reads it too (Chrome: 50px at 50 ms).
#[test]
fn width_through_var_follows_the_registered_length() {
    let r = run(
        &format!(
            "{}#h{{--x:20px;width:var(--x);{}}}#h.on{{--x:80px}}",
            reg("--x", "<length>", false, "0px"),
            transition("--x")
        ),
        "",
        "h",
        "width",
    );
    assert_eq!(
        (r.before.as_str(), r.at[1].as_str(), r.end.as_str()),
        ("20px", "50px", "80px"),
        "{r:?}"
    );
}

/// The gradient case: both stops read one registered colour, so the box
/// paints one colour, rgb(128, 0, 128) at 50 ms in Chrome.
#[test]
fn a_gradient_of_a_registered_color_paints_the_midpoint() {
    let r = run(
        &format!(
            "{}#h{{--x:rgb(0,0,255);background-image:linear-gradient(var(--x),var(--x));{}}}\
             #h.on{{--x:rgb(255,0,0)}}",
            reg("--x", "<color>", false, "black"),
            transition("--x")
        ),
        "",
        "h",
        "--x",
    );
    assert_eq!(r.at[1], "rgb(128, 0, 128)", "{r:?}");
    for (p, what) in r.painted.iter().zip(["left", "centre", "right"]) {
        assert_pixel(*p, [128, 0, 128], 1, what);
    }
}

/// Tailwind v4's gradient: `--tw-gradient-from/to` registered `<color>`,
/// `--tw-gradient-stops` a `*` built from them, `background-image` reading
/// the stops. Chrome 148 at 50 ms: from is rgb(128, 0, 128), to
/// rgb(128, 128, 0); painted 124,3,128 / 63,64,128 / 3,125,128 at
/// x 2 / 50 / 97.
#[test]
fn tailwind_gradient_stops_transition() {
    let tw = "@property --tw-gradient-from{syntax:\"<color>\";inherits:false;initial-value:#0000}\
              @property --tw-gradient-to{syntax:\"<color>\";inherits:false;initial-value:#0000}\
              @property --tw-gradient-position{syntax:\"*\";inherits:false}\
              @property --tw-gradient-stops{syntax:\"*\";inherits:false}";
    let r = run(
        &format!(
            "{tw}#h{{--tw-gradient-position:to right;--tw-gradient-from:rgb(0,0,255);--tw-gradient-to:rgb(0,0,255);\
             --tw-gradient-stops:var(--tw-gradient-position),var(--tw-gradient-from) 0%,var(--tw-gradient-to) 100%;\
             background-image:linear-gradient(var(--tw-gradient-stops));\
             transition:--tw-gradient-from 100ms linear,--tw-gradient-to 100ms linear}}\
             #h.on{{--tw-gradient-from:rgb(255,0,0);--tw-gradient-to:rgb(0,255,0)}}"
        ),
        "",
        "h",
        "--tw-gradient-from",
    );
    assert_eq!(
        (r.at[0].as_str(), r.at[1].as_str(), r.end.as_str()),
        ("rgb(0, 0, 255)", "rgb(128, 0, 128)", "rgb(255, 0, 0)"),
        "{r:?}"
    );
    let mut names = r.transitions.clone();
    names.sort();
    assert_eq!(names, ["h:--tw-gradient-from", "h:--tw-gradient-to"]);
    assert_eq!(
        r.events,
        events_of(&["--tw-gradient-from", "--tw-gradient-to"])
    );
    assert_pixel(r.painted[0], [124, 3, 128], 3, "left");
    assert_pixel(r.painted[1], [63, 64, 128], 3, "centre");
    assert_pixel(r.painted[2], [3, 125, 128], 3, "right");
}

/// `CSS.registerProperty()` from script registers as `@property` does.
#[test]
fn css_register_property_registers_from_script() {
    let r = run(
        &format!(
            "#h{{--r:rgb(0,0,255);{}}}#h.on{{--r:rgb(255,0,0)}}",
            transition("--r")
        ),
        "CSS.registerProperty({name:'--r',syntax:'<color>',inherits:false,initialValue:'black'})",
        "h",
        "--r",
    );
    assert_eq!(
        (r.before.as_str(), r.at[1].as_str(), r.end.as_str()),
        ("rgb(0, 0, 255)", "rgb(128, 0, 128)", "rgb(255, 0, 0)"),
        "{r:?}"
    );
    assert_eq!(r.transitions, ["h:--r"]);
}

/// Its errors, as Chrome 148 throws them.
#[test]
fn css_register_property_throws_as_chrome_does() {
    let (mut engine, view, _server) = load(&page("", ""));
    let got = read(
        &mut engine,
        view,
        "(function(){function t(d){try{CSS.registerProperty(d);return 'ok'}catch(e){return e.name}}\
         return [t({name:'--twice',syntax:'<length>',inherits:false,initialValue:'0px'}),\
         t({name:'--twice',syntax:'<length>',inherits:false,initialValue:'0px'}),\
         t({name:'x',syntax:'<length>',inherits:false,initialValue:'0px'}),\
         t({name:'--noinit',syntax:'<length>',inherits:false}),\
         t({name:'--bad',syntax:'<length>',inherits:false,initialValue:'red'}),\
         t({name:'--any',syntax:'*',inherits:false})].join()})()",
    );
    assert_eq!(
        got,
        "ok,InvalidModificationError,SyntaxError,SyntaxError,SyntaxError,ok"
    );
}
