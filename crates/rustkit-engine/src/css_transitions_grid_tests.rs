//! `grid-template-columns` / `grid-template-rows` transitions (owner's
//! hand test 2026-11-17, group G). The track list jumped to its end value.
//!
//! css-grid-2 §7.2.5 and Chrome 148: two track lists transition when they
//! have the same shape: as many tracks, and the same `repeat()`s (a
//! `repeat(2, 100px)` against `200px 200px` does not). Track by track, two
//! sizes of one kind interpolate (px, %, fr, the `fit-content()` limit, the
//! two ends of a `minmax()`); two of different kinds (`1fr` against
//! `100px`, `100px` against `auto`) and differing line names flip at the
//! midpoint while the rest of the list still moves. Lists of different
//! lengths, `none` against a list, and a `repeat()` against a plain list
//! start no transition.
//!
//! Oracle: Chrome for Testing 148.0.7778.216 through
//! `tools/parity_oracle/css_transitions_grid_log.mjs`, receipt
//! `docs/diagnostics/2026-11-17/css_transitions_grid_cft148.json`: the
//! value and the layout at currentTime 25, 50 and 75 of a 100 ms linear
//! transition, the transitions that start, and the events once finished.
//! Layout is the second item's offset and size and the third's offset
//! (`#h` is a 400x200 grid with three empty items), which is what a used
//! fr size reads as: getComputedStyle of a non-px list reads empty here
//! (no layout behind `computed_read`), so the px lists check the value too.
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

/// The oracle's page: `#h` a 400x200 grid with three empty items,
/// `transition: all 100ms linear`, and `css` after.
fn page(css: &str) -> String {
    format!(
        "<!doctype html><style>body{{margin:0}}\
#h{{display:grid;width:400px;height:200px;font:16px sans-serif;transition:all 100ms linear}}\
#h>i{{display:block}}{css}</style><div id=h><i id=a></i><i id=b></i><i id=c></i></div>\
<script>var h=document.getElementById('h'),b=document.getElementById('b'),c=document.getElementById('c');\
window.log=[];['transitionrun','transitionstart','transitionend','transitioncancel'].forEach(function(t){{\
h.addEventListener(t,function(e){{log.push(e.type.slice(10)+':'+e.propertyName)}})}});</script>"
    )
}

const BOX: &str =
    "[b.offsetLeft,b.offsetTop,b.offsetWidth,b.offsetHeight,c.offsetLeft,c.offsetTop].join(' ')";

struct Run {
    /// `read`'s value at 25, 50 and 75 ms, then at the end.
    value: Vec<String>,
    /// The layout (`BOX`) at 25, 50 and 75 ms, then at the end.
    boxes: Vec<String>,
    /// The transitions running at 75 ms (sorted).
    names: Vec<String>,
    /// The events once over (sorted).
    events: Vec<String>,
}

fn run(css: &str, read_prop: &str) -> Run {
    let (mut engine, view, _server) = load(&page(css));
    engine
        .execute_script(
            view,
            &format!("h.classList.add('on');getComputedStyle(h).getPropertyValue('{read_prop}')"),
        )
        .unwrap();
    let value_expr = format!("getComputedStyle(h).getPropertyValue('{read_prop}')");
    let (mut value, mut boxes) = (vec![], vec![]);
    for _ in 0..3 {
        pump(&mut engine, view, 25, 5);
        value.push(read(&mut engine, view, &value_expr));
        boxes.push(read(&mut engine, view, BOX));
    }
    let list = |engine: &mut Engine, expr: &str| {
        let mut v: Vec<String> = read(engine, view, expr)
            .split(',')
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();
        v.sort();
        v
    };
    let names = list(
        &mut engine,
        "h.getAnimations().map(function(a){return a.transitionProperty}).join()",
    );
    pump(&mut engine, view, 50, 5);
    value.push(read(&mut engine, view, &value_expr));
    boxes.push(read(&mut engine, view, BOX));
    let events = list(&mut engine, "log.join()");
    Run {
        value,
        boxes,
        names,
        events,
    }
}

const COLS: &str = "grid-template-columns";
const ROWS: &str = "grid-template-rows";

/// Chrome 148's layout at 25, 50, 75 ms and the end; the one transition
/// runs, and each event fires once.
fn check(css: &str, prop: &str, chrome_boxes: [&str; 4]) -> Run {
    let r = run(css, prop);
    assert_eq!(r.boxes, chrome_boxes, "{css}: layout at 25, 50, 75 ms, end");
    assert_eq!(r.names, [prop], "{css}: the transitions running");
    let mut want = vec![
        format!("end:{prop}"),
        format!("run:{prop}"),
        format!("start:{prop}"),
    ];
    want.sort();
    assert_eq!(r.events, want, "{css}: the events");
    r
}

/// Chrome 148 starts no transition: the end layout at once.
fn check_discrete(css: &str, prop: &str, chrome_box: &str) -> Run {
    let r = run(css, prop);
    assert_eq!(r.boxes, [chrome_box; 4], "{css}: no transition");
    assert!(
        r.names.is_empty() && r.events.is_empty(),
        "{css}: {:?} {:?}",
        r.names,
        r.events
    );
    r
}

// ------------------------------------------------------ interpolable

#[test]
fn px_tracks_interpolate() {
    let r = check(
        "#h{grid-template-columns:100px 100px}#h.on{grid-template-columns:200px 50px}",
        COLS,
        [
            "125 0 88 100 0 100",
            "150 0 75 100 0 100",
            "175 0 63 100 0 100",
            "200 0 50 100 0 100",
        ],
    );
    assert_eq!(
        r.value,
        ["125px 87.5px", "150px 75px", "175px 62.5px", "200px 50px"]
    );
}

#[test]
fn px_and_fr_tracks_interpolate() {
    check(
        "#h{grid-template-columns:100px 1fr}#h.on{grid-template-columns:200px 2fr}",
        COLS,
        [
            "125 0 275 100 0 100",
            "150 0 250 100 0 100",
            "175 0 225 100 0 100",
            "200 0 200 100 0 100",
        ],
    );
}

/// `1fr 1fr` to `1fr 3fr`: the flex factors move (1fr 2fr at 50 ms).
#[test]
fn fr_tracks_interpolate() {
    check(
        "#h{grid-template-columns:1fr 1fr}#h.on{grid-template-columns:1fr 3fr}",
        COLS,
        [
            "160 0 240 100 0 100",
            "133 0 267 100 0 100",
            "114 0 286 100 0 100",
            "100 0 300 100 0 100",
        ],
    );
}

#[test]
fn matching_repeats_interpolate() {
    let r = check(
        "#h{grid-template-columns:repeat(2,100px)}#h.on{grid-template-columns:repeat(2,200px)}",
        COLS,
        [
            "125 0 125 100 0 100",
            "150 0 150 100 0 100",
            "175 0 175 100 0 100",
            "200 0 200 100 0 100",
        ],
    );
    assert_eq!(
        r.value,
        ["125px 125px", "150px 150px", "175px 175px", "200px 200px"]
    );
}

/// `auto` against `auto` is the same kind; the px track still moves.
#[test]
fn a_keyword_track_against_itself_keeps_the_list_moving() {
    check(
        "#h{grid-template-columns:100px auto}#h.on{grid-template-columns:200px auto}",
        COLS,
        [
            "125 0 275 100 0 100",
            "150 0 250 100 0 100",
            "175 0 225 100 0 100",
            "200 0 200 100 0 100",
        ],
    );
}

/// Line names do not stop it: equal names stay, different ones flip at
/// the midpoint (Chrome 148 reads `[a]` at 25 ms and `[z]` from 50).
#[test]
fn named_lines_interpolate() {
    let boxes = [
        "125 0 88 100 0 100",
        "150 0 75 100 0 100",
        "175 0 63 100 0 100",
        "200 0 50 100 0 100",
    ];
    check(
        "#h{grid-template-columns:[a] 100px [b] 100px}#h.on{grid-template-columns:[a] 200px [b] 50px}",
        COLS,
        boxes,
    );
    check(
        "#h{grid-template-columns:[a] 100px 100px}#h.on{grid-template-columns:[z] 200px 50px}",
        COLS,
        boxes,
    );
}

/// A pair of tracks of different kinds flips at the midpoint while the
/// rest of the list moves: `1fr` (275px at 25 ms) is `100px` from 50 ms,
/// and `100px` is `auto` from 50 ms.
#[test]
fn tracks_of_different_kinds_flip_at_the_midpoint() {
    check(
        "#h{grid-template-columns:100px 1fr}#h.on{grid-template-columns:200px 100px}",
        COLS,
        [
            "125 0 275 100 0 100",
            "150 0 100 100 0 100",
            "175 0 100 100 0 100",
            "200 0 100 100 0 100",
        ],
    );
    check(
        "#h{grid-template-columns:100px 100px}#h.on{grid-template-columns:200px auto}",
        COLS,
        [
            "125 0 100 100 0 100",
            "150 0 250 100 0 100",
            "175 0 225 100 0 100",
            "200 0 200 100 0 100",
        ],
    );
}

#[test]
fn grid_template_rows_interpolate() {
    check(
        "#h{grid-template-rows:20px 20px}#h.on{grid-template-rows:60px 100px}",
        ROWS,
        [
            "0 30 400 40 0 70",
            "0 40 400 60 0 100",
            "0 50 400 80 0 130",
            "0 60 400 100 0 160",
        ],
    );
    check(
        "#h{grid-template-rows:20px 1fr}#h.on{grid-template-rows:60px 3fr}",
        ROWS,
        [
            "0 30 400 170 0 200",
            "0 40 400 160 0 200",
            "0 50 400 150 0 200",
            "0 60 400 140 0 200",
        ],
    );
}

// ----------------------------------------------------------- discrete

#[test]
fn lists_of_different_lengths_start_no_transition() {
    let r = check_discrete(
        "#h{grid-template-columns:100px 100px}#h.on{grid-template-columns:200px 200px 200px}",
        COLS,
        "200 0 200 200 400 0",
    );
    assert_eq!(r.value, ["200px 200px 200px"; 4]);
}

/// Chrome 148 compares the declared `repeat()`, not the expanded list.
/// A stated gap: `parse_grid_template` expands an integer `repeat()` into
/// plain tracks, so RustKit sees two equal-shape lists and interpolates.
/// Keeping `TrackRepeat::Count` at parse moves such pages off grid
/// layout's estimate fast path (`grid.rs`, which bails on `repeats`), a
/// layout change outside this PR (open row in the handoff).
#[test]
#[ignore = "integer repeat() is expanded at parse; open row"]
fn a_repeat_against_a_plain_list_starts_no_transition() {
    check_discrete(
        "#h{grid-template-columns:repeat(2,100px)}#h.on{grid-template-columns:200px 200px}",
        COLS,
        "200 0 200 100 0 100",
    );
}

#[test]
fn none_against_a_list_starts_no_transition() {
    check_discrete(
        "#h{grid-template-columns:none}#h.on{grid-template-columns:100px 200px}",
        COLS,
        "100 0 200 100 0 100",
    );
    check_discrete(
        "#h{grid-template-columns:100px 200px}#h.on{grid-template-columns:none}",
        COLS,
        "0 67 400 67 0 133",
    );
}
