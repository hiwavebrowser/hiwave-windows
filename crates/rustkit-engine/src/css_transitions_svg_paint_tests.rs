//! `fill` and `stroke` transitions on the elements of an inline `<svg>`
//! (A2 of the CSS-transitions brief, design #1292). Before this, an svg
//! element's paint jumped to its end value: svg content has no
//! `ComputedStyle`, so it never reached `Animations::on_style`.
//!
//! Every case is a pinned one: Chrome for Testing 148.0.7778.216 through
//! `tools/parity_oracle/css_transitions_svg_paint_log.mjs`, receipt
//! `docs/diagnostics/2026-11-17/css_transitions_svg_paint_cft148.json`,
//! which this file reads. `#s` is a 100x100 inline svg holding `#p`, an
//! 80x80 rect at (10, 10) with stroke-width 20; the transition is 100 ms
//! linear. Chrome's values were read paused and seeked; here the page's
//! virtual clock moves (the `.on` class lands with a style read, which
//! starts the transition at the clock as it stands, then `pump_live`
//! moves the clock in 5 ms steps). Colours may differ by one rounding step
//! a channel.

use super::*;
use crate::script_net_tests::serve_routes;

const RECEIPT: &str =
    include_str!("../../../docs/diagnostics/2026-11-17/css_transitions_svg_paint_cft148.json");

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

fn pump(engine: &mut Engine, view: EngineViewId, total: u64) {
    let rt = rt();
    let mut t = 0;
    while t < total {
        rt.block_on(engine.pump_live(view, 5));
        t += 5;
    }
}

/// The oracle's page: `css`, the svg, `attrs` on the rect, and every
/// transition event at `#s` and `#p` logged as `id:type:property`.
fn page(css: &str, attrs: &str) -> String {
    let attrs = attrs.replace('=', "=\"").replace(' ', "\" ");
    let attrs = if attrs.is_empty() {
        attrs
    } else {
        format!("{attrs}\"")
    };
    format!(
        "<!doctype html><style>body{{margin:0}}{css}</style>\
<svg id=s width=100 height=100><defs><linearGradient id=g><stop offset=0 stop-color=lime /></linearGradient></defs>\
<rect id=p x=10 y=10 width=80 height=80 stroke-width=20 {attrs} /></svg>\
<script>window.log=[];['s','p'].forEach(function(id){{var el=document.getElementById(id);\
['transitionrun','transitionstart','transitionend','transitioncancel'].forEach(function(t){{\
el.addEventListener(t,function(e){{if(e.target===e.currentTarget)log.push(id+':'+e.type.slice(10)+':'+e.propertyName)}})}})}});</script>"
    )
}

fn pixel(engine: &mut Engine, view: EngineViewId, x: u32, y: u32) -> [u8; 4] {
    let (w, _, rgba) = engine.frame_rgba(view).expect("frame");
    let i = ((y * w + x) * 4) as usize;
    [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
}

/// `rgb(...)` / `rgba(...)` channels, or `None` for anything else.
fn channels(colour: &str) -> Option<Vec<f64>> {
    let inner = colour
        .strip_prefix("rgba(")
        .or_else(|| colour.strip_prefix("rgb("))?
        .strip_suffix(')')?;
    inner.split(',').map(|c| c.trim().parse().ok()).collect()
}

/// `got` is Chrome's `want`: a colour to within one step a channel,
/// anything else exactly.
fn same_value(got: &str, want: &str) -> bool {
    match (channels(got), channels(want)) {
        (Some(g), Some(w)) => {
            g.len() == w.len() && g.iter().zip(&w).all(|(g, w)| (g - w).abs() <= 1.0)
        }
        _ => got == want,
    }
}

fn same_pixel(got: [u8; 4], want: &str) -> bool {
    let want: Vec<i32> = want.split(',').map(|c| c.parse().unwrap()).collect();
    got.iter()
        .zip(&want)
        .all(|(g, w)| (*g as i32 - w).abs() <= 1)
}

/// Run the receipt's case `label` and compare everything it pinned.
fn check(label: &str) {
    let receipt: serde_json::Value = serde_json::from_str(RECEIPT).unwrap();
    assert_eq!(receipt["chrome"], "148.0.7778.216");
    let case = receipt["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["label"] == label)
        .unwrap_or_else(|| panic!("no case {label:?} in the receipt"));
    let s = |key: &str| case[key].as_str().unwrap().to_string();
    let (toggle, read_at) = (s("toggle"), s("read"));
    let (target, prop) = read_at.split_once(' ').unwrap();
    let value =
        format!("getComputedStyle(document.getElementById('{target}')).getPropertyValue('{prop}')");
    let start = |engine: &mut Engine, view| {
        read(
            engine,
            view,
            &format!("(document.getElementById('{toggle}').classList.add('on'), {value})"),
        );
    };
    let mut failures = Vec::new();
    // The painted frame and `getAnimations()`, at 50 ms, from a page of
    // their own: the samples at 25, 50 and 75 ms come from a page that
    // only reads the value.
    {
        let (mut engine, view, _server) = load(&page(&s("css"), &s("attrs")));
        start(&mut engine, view);
        pump(&mut engine, view, 50);
        let names = read(
            &mut engine,
            view,
            "['s','p'].map(function(id){return document.getElementById(id).getAnimations()\
             .map(function(a){return id+':'+a.transitionProperty}).join()}).filter(String).join()",
        );
        let want: Vec<&str> = case["transitions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        if names != want.join(",") {
            failures.push(format!("transitions at 50 ms: {names:?}, Chrome {want:?}"));
        }
        for (spot, (x, y)) in [("centre", (50, 50)), ("stroke", (12, 50))] {
            let got = pixel(&mut engine, view, x, y);
            let want = case["painted_at_50"][spot].as_str().unwrap();
            if !same_pixel(got, want) {
                failures.push(format!("painted {spot} at 50 ms: {got:?}, Chrome {want}"));
            }
        }
    }
    let (mut engine, view, _server) = load(&page(&s("css"), &s("attrs")));
    start(&mut engine, view);
    for at in ["25", "50", "75"] {
        pump(&mut engine, view, 25);
        let got = read(&mut engine, view, &value);
        let want = case["at"][at].as_str().unwrap();
        if !same_value(&got, want) {
            failures.push(format!("{prop} at {at} ms: {got:?}, Chrome {want:?}"));
        }
    }
    pump(&mut engine, view, 100);
    let end = read(&mut engine, view, &value);
    if !same_value(&end, case["end"].as_str().unwrap()) {
        failures.push(format!("end value {end:?}, Chrome {}", case["end"]));
    }
    let events = read(&mut engine, view, "log.join()");
    let want: Vec<&str> = case["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    if events != want.join(",") {
        failures.push(format!("events {events:?}, Chrome {want:?}"));
    }
    assert!(failures.is_empty(), "{label}:\n  {}", failures.join("\n  "));
}

#[test]
fn fill_blue_to_red_interpolates() {
    check("fill blue->red");
}

#[test]
fn stroke_blue_to_red_interpolates() {
    check("stroke blue->red");
}

#[test]
fn a_named_fill_transition_interpolates() {
    check("fill named transition");
}

/// `none` and `url()` are not interpolable: the value jumps and no
/// transition or event happens.
#[test]
fn fill_none_to_red_is_discrete() {
    check("fill none->red");
}

#[test]
fn fill_red_to_none_is_discrete() {
    check("fill red->none");
}

#[test]
fn fill_url_to_red_is_discrete() {
    check("fill url->red");
}

/// The presentation attribute is the before-change value.
#[test]
fn a_presentation_attribute_is_the_before_value() {
    check("fill attr->css");
}

/// The transition is the svg element's; its rect inherits the moving
/// value.
#[test]
fn fill_on_the_svg_is_inherited_by_the_rect() {
    check("fill on svg, inherited by rect");
}

#[test]
fn fill_on_the_svg_reads_on_the_svg() {
    check("fill on svg read on svg");
}

/// Already right before this work (the `color` transition re-rasters the
/// svg): kept as a guard.
#[test]
fn a_currentcolor_fill_follows_a_moving_color() {
    check("fill currentcolor, color moves");
}

/// The hover shape: the class lands on the svg, the rule names the rect.
#[test]
fn a_class_on_the_svg_transitions_the_rect() {
    check("hover-like: class on svg, rule on rect");
}

/// A page whose svg names no transition runs no svg transition: the lane
/// stays empty while an HTML transition runs, so it asks for no frames of
/// its own.
#[test]
fn an_html_transition_leaves_the_svg_lane_empty() {
    let (mut engine, view, _server) = load(
        "<!doctype html><style>#h{color:blue;transition:color 100ms linear}#h.on{color:red}#p{fill:blue}</style>\
         <div id=h>x</div><svg width=10 height=10><rect id=p width=10 height=10 /></svg>",
    );
    read(
        &mut engine,
        view,
        "(h.classList.add('on'), getComputedStyle(h).color)",
    );
    for _ in 0..30 {
        pump(&mut engine, view, 5);
        let lay = engine.views[&view].lay.clone();
        assert!(!lay.animations.borrow().svg_moving(), "the svg lane moved");
    }
}

/// R1 on #1330 (a): each frame of a moving svg paint parses its svg under
/// a new key. The pass after evicts the keys it no longer uses, so the
/// cache stays flat over a long transition (400 frames here).
#[test]
fn a_long_svg_transition_keeps_the_svg_cache_flat() {
    let (mut engine, view, _server) = load(&page(
        "#p{fill:blue;transition:fill 2000ms linear}#p.on{fill:red}",
        "",
    ));
    read(
        &mut engine,
        view,
        "(document.getElementById('p').classList.add('on'), getComputedStyle(document.getElementById('p')).fill)",
    );
    let at_start = engine.lctx.svg_cache.borrow().len();
    let mut most = at_start;
    for _ in 0..40 {
        pump(&mut engine, view, 50);
        most = most.max(engine.lctx.svg_cache.borrow().len());
    }
    assert!(
        most <= at_start + 2,
        "svg_cache grew from {at_start} to {most} entries during the transition"
    );
    pump(&mut engine, view, 100);
    let end = read(
        &mut engine,
        view,
        "getComputedStyle(document.getElementById('p')).fill",
    );
    assert_eq!(end, "rgb(255, 0, 0)");
}

fn frames(engine: &Engine, view: EngineViewId) -> (u32, u32) {
    let stats = engine.relayout_stats(view).expect("stats");
    (stats.relayouts, stats.animation_frames_painted)
}

/// R1 on #1330 (b): a transition in its delay holds its start value, so
/// its frames take no build; the builds start with the value moving.
#[test]
fn an_svg_transition_in_its_delay_takes_no_build() {
    let (mut engine, view, _server) = load(&page(
        "#p{fill:blue;transition:fill 100ms linear 100ms}#p.on{fill:red}",
        "",
    ));
    let fill = "getComputedStyle(document.getElementById('p')).fill";
    read(
        &mut engine,
        view,
        &format!("(document.getElementById('p').classList.add('on'), {fill})"),
    );
    let (relayouts, _) = frames(&engine, view);
    for _ in 0..18 {
        pump(&mut engine, view, 5);
        let lay = engine.views[&view].lay.clone();
        assert!(!lay.animations.borrow().svg_moving(), "moving in the delay");
        assert!(lay.animations.borrow().active(), "the frames keep turning");
    }
    assert_eq!(
        frames(&engine, view).0,
        relayouts,
        "no relayout in the delay"
    );
    assert_eq!(
        read(&mut engine, view, fill),
        "rgb(0, 0, 255)",
        "the start value"
    );
    pump(&mut engine, view, 60);
    assert!(
        frames(&engine, view).0 > relayouts,
        "the moving value rebuilds"
    );
    let mid = read(&mut engine, view, fill);
    assert!(
        mid != "rgb(0, 0, 255)" && mid != "rgb(255, 0, 0)",
        "moving at 150 ms: {mid}"
    );
    pump(&mut engine, view, 100);
    assert_eq!(read(&mut engine, view, fill), "rgb(255, 0, 0)");
}

/// #1292: pages whose svg paint does not move schedule the same frames as
/// before. The same HTML transition, with and without an svg that names a
/// transition but never changes paint, takes the same relayouts and
/// painted frames on the deterministic clock.
#[test]
fn an_unmoving_svg_adds_no_frames_to_an_html_transition() {
    let count = |svg: &str| {
        let (mut engine, view, _server) = load(&format!(
            "<!doctype html><style>#h{{color:blue;transition:color 100ms linear}}#h.on{{color:red}}\
             #q{{fill:blue;transition:fill 100ms linear}}</style><div id=h>x</div>{svg}"
        ));
        read(
            &mut engine,
            view,
            "(h.classList.add('on'), getComputedStyle(h).color)",
        );
        let before = frames(&engine, view);
        pump(&mut engine, view, 150);
        let after = frames(&engine, view);
        (after.0 - before.0, after.1 - before.1)
    };
    let plain = count("");
    let with_svg = count("<svg width=10 height=10><rect id=q width=10 height=10 /></svg>");
    assert_eq!(
        with_svg, plain,
        "(relayouts, painted frames) with the svg and without"
    );
}

/// The svg cache keys `view`'s inline svgs paint under, as its last
/// build keyed them (the engine's paint map is that build's until another
/// view builds).
fn svg_keys(engine: &Engine, view: EngineViewId) -> Vec<String> {
    let document = engine.views[&view].document.clone().expect("document");
    document
        .get_elements_by_tag_name("svg")
        .iter()
        .map(|svg| engine.lctx.inline_svg_key_for(svg))
        .collect()
}

/// R1 on #1330 (delta): the svg cache is one per engine. An icon a page
/// added while its svg transition ran, and the frame on screen, stay
/// cached when another view relayouts: that view's pass evicts only its
/// own per-frame keys.
#[test]
fn another_views_relayout_keeps_this_views_svgs() {
    let (mut engine, a, _server) = load(&page(
        "#p{fill:blue;transition:fill 2000ms linear}#p.on{fill:red}",
        "",
    ));
    read(
        &mut engine,
        a,
        "(document.getElementById('p').classList.add('on'), getComputedStyle(document.getElementById('p')).fill)",
    );
    pump(&mut engine, a, 20);
    read(
        &mut engine,
        a,
        "(document.body.insertAdjacentHTML('beforeend', '<svg id=icon width=20 height=20><rect width=20 height=20 fill=lime /></svg>'), 1)",
    );
    pump(&mut engine, a, 20);
    let keys = svg_keys(&engine, a);
    assert_eq!(keys.len(), 2, "the moving svg and the icon: {keys:?}");

    let other_server = serve_routes(vec![(
        "/",
        "text/html",
        "<!doctype html><svg width=10 height=10><circle r=5 fill=navy /></svg>".to_string(),
    )]);
    let b = engine
        .create_headless_view(Bounds {
            x: 0,
            y: 0,
            width: 200,
            height: 200,
        })
        .expect("view");
    let url = Url::parse(&format!("http://127.0.0.1:{}/", other_server.port)).unwrap();
    rt().block_on(engine.load_url(b, url)).expect("load_url");
    engine.relayout(b).expect("relayout b");

    let cache = engine.lctx.svg_cache.borrow();
    let missing: Vec<&String> = keys.iter().filter(|k| !cache.contains_key(*k)).collect();
    assert!(
        missing.is_empty(),
        "view b's relayout evicted view a's svgs: {missing:?}"
    );
}

/// R1 on #1337: a view closed while its svg transition runs releases the frame it was
/// painting: the per-frame parse leaves the shared cache with it.
#[test]
fn closing_a_view_mid_transition_releases_its_svg_frames() {
    let (mut engine, a, _server) = load(&page(
        "#p{fill:blue;transition:fill 2000ms linear}#p.on{fill:red}",
        "",
    ));
    read(
        &mut engine,
        a,
        "(document.getElementById('p').classList.add('on'), getComputedStyle(document.getElementById('p')).fill)",
    );
    pump(&mut engine, a, 40);
    let moving = svg_keys(&engine, a);
    assert_eq!(moving.len(), 1);
    assert!(
        engine.lctx.svg_cache.borrow().contains_key(&moving[0]),
        "the frame is cached while it shows"
    );
    engine.destroy_view(a).expect("destroy");
    assert!(
        !engine.lctx.svg_cache.borrow().contains_key(&moving[0]),
        "the closed view's svg frame stayed in the shared cache"
    );
}
