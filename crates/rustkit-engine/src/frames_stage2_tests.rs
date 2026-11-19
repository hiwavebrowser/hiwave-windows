//! Nested browsing contexts, Stage 2: a same-origin `<iframe>` loads
//! (docs/diagnostics/2026-11-11/nested-browsing-contexts-plan.md).
//!
//! A real loopback server and the engine's live loop, as
//! `inserted_script_live_tests`. The expected answers are Chrome for
//! Testing 148.0.7778.216's on the same pages (headless, recorded
//! 2026-11-12 with the parity oracle's pinned binary; quoted where asserted;
//! the cross-origin test's floor is a stated deviation).
//! With `EngineConfig::nested_contexts` off nothing here changes: the last
//! test pins that.

use super::*;
use crate::script_net_tests::serve_routes;
use std::time::{Duration, Instant};

struct Page {
    engine: Engine,
    view: EngineViewId,
    rt: tokio::runtime::Runtime,
    server: crate::script_net_tests::Server,
}

fn config(on: bool) -> EngineConfig {
    EngineConfig { nested_contexts: on, post_load_requests_live: true, ..EngineConfig::default() }
}

const CHILD: &str = "<!doctype html><html><head><title>child-title</title></head><body><div id=c>child text</div>\
<script>function childFn() { return 'child-fn-ok:' + document.title; } window.ranAt = (window.ranAt || 0) + 1; \
window.sawParent = (window.parent !== window) + ':' + (window.top === window.parent) + ':' + String(window.frameElement && window.frameElement.id);</script>\
<script src=/child.js></script></body></html>";

impl Page {
    fn load(on: bool, page: &str) -> Self {
        Self::load_with(config(on), page)
    }

    fn load_with(config: EngineConfig, page: &str) -> Self {
        let routes = vec![
            ("/", "text/html", page.to_string()),
            ("/child.html", "text/html", CHILD.to_string()),
            ("/other.html", "text/html", "<!doctype html><title>other-title</title><p id=o>other</p>".to_string()),
            ("/child.js", "text/javascript", "window.externalRan = true;".to_string()),
            ("/frame_load_child.html", "text/html", "<!doctype html><title>c</title><div id=stage></div>".to_string()),
            ("/noscript.html", "text/html", "<!doctype html><body><noscript><strong>x</strong></noscript><p>c</p></body>".to_string()),
            (
                "/styled.html",
                "text/html",
                "<!doctype html><html><head><link rel=stylesheet href=/styled.css></head><body><div id=stage></div></body></html>".to_string(),
            ),
            (
                "/late.html",
                "text/html",
                "<!doctype html><html><head><link rel=stylesheet href='/held.css'></head><body><div id=stage></div></body></html>".to_string(),
            ),
            // `/held...` answers after 300 ms (the test server's rule).
            ("/held.css", "text/css", "#stage { position: relative; }".to_string()),
            ("/styled.css", "text/css", "html { height: 100%; } body { height: 100%; margin: 0; } #stage { position: relative; width: 100%; height: 100%; }".to_string()),
        ];
        let server = serve_routes(routes);
        let mut engine = Engine::new(config).expect("engine");
        let view = engine
            .create_headless_view(Bounds { x: 0, y: 0, width: 800, height: 600 })
            .expect("view");
        let url = Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap();
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(engine.load_url(view, url)).expect("load_url");
        Page { engine, view, rt, server }
    }

    fn js(&mut self, script: &str) -> String {
        self.engine.execute_script(self.view, script).unwrap()
    }

    fn turns_until(&mut self, done: &str, within: Duration) {
        let started = Instant::now();
        while started.elapsed() < within {
            std::thread::sleep(Duration::from_millis(20));
            self.rt.block_on(self.engine.pump_live(self.view, 20));
            if self.js(done) == "Boolean(true)" {
                return;
            }
        }
    }

    /// Turn the page until `done` reads true, at most `max` times: whether
    /// it did. No clock is read.
    fn turns(&mut self, done: &str, max: usize) -> bool {
        for _ in 0..max {
            self.rt.block_on(self.engine.pump_live(self.view, 20));
            if self.js(done) == "Boolean(true)" {
                return true;
            }
        }
        false
    }

    fn served(&self, path: &str) -> usize {
        self.server.served(path).len()
    }
}

/// The plan's table, row for row. Chrome 148 on this page:
/// contentWindow/contentDocument objects, the child ran (its scripts saw
/// a parent, `top === parent`, `frameElement` the iframe), `load` at the
/// element, then the parent reads the child's title, a node, a function,
/// its viewport (the iframe's CSS size) and its external script's global.
#[test]
fn a_same_origin_frame_loads_runs_and_is_reachable() {
    let page = "<!doctype html><body><script>
window.events = [];
var f = document.createElement('iframe');
f.id = 'f';
f.style.width = '320px'; f.style.height = '200px';
f.addEventListener('load', function () { events.push('listener'); });
f.onload = function () { events.push('onload'); };
f.src = '/child.html';
document.body.appendChild(f);
window.done = function () { return events.length >= 2; };
</script></body>";
    let mut page = Page::load(true, page);
    page.turns_until("window.done()", Duration::from_secs(5));
    assert_eq!(page.js("events.join()"), r#"String("listener,onload")"#);
    assert_eq!(
        page.js(
            "var w = f.contentWindow, d = f.contentDocument; \
             [typeof w, d === w.document, d.title, d.querySelector('#c').textContent, w.childFn(), \
              w.innerWidth + 'x' + w.innerHeight, w.ranAt, w.sawParent, w.externalRan, \
              w.location.pathname, d.getElementById('c').ownerDocument === d].join('|')"
        ),
        r#"String("object|true|child-title|child text|child-fn-ok:child-title|320x200|1|true:true:f|true|/child.html|true")"#
    );
    assert_eq!(page.served("/child.html"), 1);
}

/// Speedometer's runner sets `src` again on the same frame: a second load,
/// a new document, the old window's globals gone. Chrome 148: two `load`s,
/// `other-title`, `undefined`.
#[test]
fn setting_src_again_navigates_the_frame() {
    let page = "<!doctype html><body><iframe id=f></iframe><script>
window.loads = 0;
var f = document.getElementById('f');
f.onload = function () { loads++; if (loads === 1) { window.first = f.contentDocument.title; f.contentWindow.mark = 1; f.src = '/other.html'; } };
f.src = '/child.html';
</script></body>";
    let mut page = Page::load(true, page);
    page.turns_until("window.loads >= 2", Duration::from_secs(5));
    assert_eq!(
        page.js("[loads, first, f.contentDocument.title, typeof f.contentWindow.mark].join()"),
        r#"String("2,child-title,other-title,undefined")"#
    );
}

/// The initial about:blank's `load` (docs/diagnostics/2026-11-15/frame_load_*,
/// Chrome 148 in frame_load_cft148.json). Chrome fires it inside the
/// insertion, so a handler set after the insertion never hears it: a runner
/// that inserts the frame, then sets `onload`, then `src` (MotionMark's)
/// hears one `load`, the document's. A listener added before the insertion
/// hears the blank one, and the document's too when `src` follows.
#[test]
fn a_handler_set_after_the_insertion_does_not_hear_the_blank_load() {
    let page = "<!doctype html><body><div id=host></div><script>
window.seen = { late: [], early: [], both: [], capture: [] };
function see(list) { return function (ev) { var w = ev.target.contentWindow; list.push(w.location.pathname + ':' + typeof w.childFn); }; }
var host = document.getElementById('host');
var late = document.createElement('iframe');
host.appendChild(late);
late.onload = see(seen.late);
late.src = '/child.html';
var early = document.createElement('iframe');
early.addEventListener('load', see(seen.early));
host.appendChild(early);
var both = document.createElement('iframe');
both.addEventListener('load', see(seen.both));
host.appendChild(both);
both.src = '/child.html';
var box = document.createElement('div');
box.addEventListener('load', see(seen.capture), true);
host.appendChild(box);
box.appendChild(document.createElement('iframe'));
window.done = function () { return seen.late.length >= 1 && seen.early.length >= 1 && seen.both.length >= 2 && seen.capture.length >= 1; };
</script></body>";
    let mut page = Page::load(true, page);
    page.turns_until("window.done()", Duration::from_secs(5));
    // A few more turns: a second `load` at `late` would arrive by now.
    for _ in 0..5 {
        page.rt.block_on(page.engine.pump_live(page.view, 20));
    }
    assert_eq!(
        page.js("[seen.late.join(), seen.early.join(), seen.both.join(), seen.capture.join()].join(' | ')"),
        r#"String("/child.html:function | blank:undefined | blank:undefined,/child.html:function | blank:undefined")"#
    );
}

/// Frames in the markup and a handler replaced after the insertion: the
/// probe page itself, against the pinned Chrome's record of it
/// (docs/diagnostics/2026-11-15/frame_load_markup{.html,_cft148.json}).
/// Chrome fires a blank frame's `load` inside its insertion (the parser's
/// too), so it reaches only what was on the frame by then: the `onload`
/// attribute of a markup frame, a handler set before a script's insertion;
/// never a listener added or a handler put in its place afterwards.
#[test]
fn markup_frames_and_replaced_handlers_hear_the_loads_chrome_fires() {
    const PROBE: &str = include_str!("../../../docs/diagnostics/2026-11-15/frame_load_markup.html");
    const PIN: &str = include_str!("../../../docs/diagnostics/2026-11-15/frame_load_markup_cft148.json");
    let mut page = Page::load(true, PROBE);
    page.turns_until("out.cases.scanThenOnloadThenSrc.length >= 1 && out.cases.attrSrc.length >= 1", Duration::from_secs(5));
    for _ in 0..10 {
        page.rt.block_on(page.engine.pump_live(page.view, 20));
    }
    let got = page.js("JSON.stringify(out.cases)");
    let got: serde_json::Value = serde_json::from_str(got.trim_start_matches("String(").trim_end_matches(')')).ok()
        .and_then(|v: serde_json::Value| v.as_str().and_then(|s| serde_json::from_str(s).ok()))
        .unwrap_or_else(|| panic!("cases: {got}"));
    let pin: serde_json::Value = serde_json::from_str(PIN).unwrap();
    assert_eq!(pin["browser"], "148.0.7778.216");
    let mut want = pin["cases"].clone();
    // Known difference: `srcdoc` is not in this stage, so the frame shows
    // about:blank where Chrome shows its srcdoc document. One `load` each.
    assert_eq!(want["attrSrcdoc"], serde_json::json!(["srcdoc"]));
    want["attrSrcdoc"] = serde_json::json!(["blank"]);
    // Known difference: a listener removed between the insertion and the replay is not called.
    // Chrome fired the `load` inside the insertion, while the listener was still on.
    assert_eq!(want["removedAfterInsertion"], serde_json::json!(["blank"]));
    want["removedAfterInsertion"] = serde_json::json!([]);
    assert_eq!(got, want);
}

/// A frame's stylesheet that is still out at the subresource budget does
/// not hold the frame's `load` past the budget, and is applied when it
/// arrives, on the frame's live turns (as a page's late sheet is).
#[test]
fn a_frames_late_stylesheet_does_not_hold_its_load_and_lands_on_a_live_turn() {
    let page = "<!doctype html><body><script>
var f = document.createElement('iframe');
f.style.width = '400px'; f.style.height = '300px'; f.style.border = '0';
function position() { return f.contentWindow.getComputedStyle(f.contentDocument.getElementById('stage')).position; }
f.onload = function () { window.atLoad = position(); };
f.src = '/late.html';
document.body.appendChild(f);
</script></body>";
    let config = EngineConfig { subresource_budget_ms: 100, ..config(true) };
    let mut page = Page::load_with(config, page);
    page.turns_until("typeof window.atLoad === 'string'", Duration::from_secs(5));
    assert_eq!(page.js("window.atLoad"), r#"String("static")"#, "`load` waited for the sheet past the budget");
    page.turns_until("position() === 'relative'", Duration::from_secs(5));
    assert_eq!(page.js("position()"), r#"String("relative")"#);
    assert_eq!(page.served("/held.css"), 1);
}

/// A frame with a `sandbox` attribute keeps this stage's floor whatever
/// listened at its insertion (`frame_sandbox_tests`: no document, one
/// `load`): the quiet rule is for a blank document that is made, and never
/// takes the floor's `load` away. A handler set after the insertion hears
/// it. (Chrome fires this frame's blank `load` inside the insertion as for
/// any other, so there the late handler hears none: a difference of the
/// floor, kept as #1201 landed it.)
#[test]
fn a_sandboxed_frames_floor_load_is_not_made_quiet() {
    let page = "<!doctype html><body><script>
window.loads = 0;
var f = document.createElement('iframe');
f.setAttribute('sandbox', '');
document.body.appendChild(f);
f.onload = function () { loads++; };
</script></body>";
    let mut page = Page::load(true, page);
    page.turns_until("window.loads >= 1", Duration::from_secs(5));
    for _ in 0..5 {
        page.rt.block_on(page.engine.pump_live(page.view, 20));
    }
    assert_eq!(page.js("loads + '|' + String(f.contentDocument)"), r#"String("1|null")"#);
}

/// A frame's linked stylesheet is applied before `load` fires at its
/// element (docs/diagnostics/2026-11-15/frame_style_*; Chrome 148:
/// `400x300 relative`). MotionMark's runner builds each test's stage in the
/// frame's `load` handler from the size the frame's stylesheet gives it; an
/// unstyled stage is 0 high and the test holds nothing to draw.
#[test]
fn a_frames_stylesheet_is_applied_before_its_load() {
    let page = "<!doctype html><body><script>
var f = document.createElement('iframe');
f.style.width = '400px'; f.style.height = '300px'; f.style.border = '0';
f.onload = function () {
  var s = f.contentDocument.getElementById('stage'), r = s.getBoundingClientRect();
  window.atLoad = Math.round(r.width) + 'x' + Math.round(r.height) + ' ' + f.contentWindow.getComputedStyle(s).position;
};
f.src = '/styled.html';
document.body.appendChild(f);
</script></body>";
    let mut page = Page::load(true, page);
    page.turns_until("typeof window.atLoad === 'string'", Duration::from_secs(5));
    assert_eq!(page.js("window.atLoad"), r#"String("400x300 relative")"#);
    assert_eq!(page.served("/styled.css"), 1);
}

/// A cross-origin `src` is never requested in this stage (the plan's
/// floor): no document is reachable, and the element still hears `load`
/// so a page waiting on it goes on. (Chrome loads the document; the
/// floor is a stated deviation until cross-origin frames get their own
/// design.)
#[test]
fn a_cross_origin_frame_is_not_fetched_and_still_fires_load() {
    let page = "<!doctype html><body><script>
window.loads = 0;
var f = document.createElement('iframe');
f.onload = function () { loads++; };
f.src = 'http://localhost:9/elsewhere.html';
document.body.appendChild(f);
</script></body>";
    let mut page = Page::load(true, page);
    page.turns_until("window.loads >= 1", Duration::from_secs(5));
    assert_eq!(page.js("[loads, String(f.contentDocument), String(f.contentWindow)].join()"), r#"String("1,null,null")"#);
}

/// A frame in the parser's markup loads too (after the parent's own
/// `load` in this stage).
#[test]
fn a_frame_in_the_markup_loads() {
    let page = "<!doctype html><body><iframe id=f src=/child.html></iframe><script>
window.loaded = false;
document.getElementById('f').addEventListener('load', function () { loaded = true; });
</script></body>";
    let mut page = Page::load(true, page);
    page.turns_until("window.loaded", Duration::from_secs(5));
    assert_eq!(page.js("document.getElementById('f').contentDocument.title"), r#"String("child-title")"#);
}

/// A frame's document is parsed with scripting on, as its parent's is:
/// the contents of its `<noscript>` are one text node, not elements
/// (HTML §13.2.3.5). Chrome 148, a page and its same-origin frame with
/// `<noscript><strong>x</strong></noscript>`: `1|3|0` for each (one child,
/// a text node, no `strong`). The frame's was parsed with scripting off
/// (Speedometer's TodoMVC-Vue had an extra `strong` in its frame).
#[test]
fn a_frames_noscript_is_text() {
    let page = "<!doctype html><body><noscript><strong>top</strong></noscript><iframe id=f src=/noscript.html></iframe><script>
window.loaded = false;
document.getElementById('f').addEventListener('load', function () { loaded = true; });
function shape(doc) { var n = doc.querySelector('noscript'); return n.childNodes.length + '|' + n.firstChild.nodeType + '|' + doc.querySelectorAll('strong').length; }
</script></body>";
    let mut page = Page::load(true, page);
    page.turns_until("window.loaded", Duration::from_secs(5));
    assert_eq!(page.js("shape(document)"), r#"String("1|3|0")"#);
    assert_eq!(page.js("shape(document.getElementById('f').contentDocument)"), r#"String("1|3|0")"#, "the frame's noscript");
}

/// The documents of a frame tree are one event loop: their timers run in
/// the order they fall due, whichever document set them. Chrome 148 (probe
/// page: inside a page timer, the frame's `setTimeout(f, 0)`, then the
/// page's `setTimeout(g, 0)` and `setTimeout(h, 50)`): `frame0,page0,page50`.
/// Here a turn that caught the page's clock up (the loop had slept) ran
/// every page timer due in that span before any frame had a turn
/// (Speedometer's React-Stockcharts-SVG: the chart's deferred pan-end
/// `setState`, a frame timer, ran after the zoom and undid it).
#[test]
fn a_frame_timer_set_in_a_page_task_runs_before_the_pages_later_timers() {
    let page = "<!doctype html><body><iframe id=f src=/child.html></iframe><script>
window.order = []; window.ready = false;
document.getElementById('f').addEventListener('load', function () { ready = true; });
function arm() {
  var w = document.getElementById('f').contentWindow;
  setTimeout(function () {
    w.setTimeout(function () { order.push('frame0'); }, 0);
    setTimeout(function () { order.push('page0'); }, 0);
    setTimeout(function () { order.push('page50'); }, 50);
  }, 10);
}
</script></body>";
    let mut page = Page::load(true, page);
    page.turns_until("window.ready", Duration::from_secs(5));
    page.js("arm()");
    // One turn for 500 ms that passed (the loop slept: nothing was due
    // sooner), as the live loop gives an idle page.
    page.rt.block_on(page.engine.pump_live(page.view, 500));
    page.rt.block_on(page.engine.pump_live(page.view, 0));
    assert_eq!(page.js("order.join(',')"), r#"String("frame0,page0,page50")"#);
}

/// Timers of a frame tree that fall due together run in the order they
/// were set, whichever document set them; each runs its microtasks before
/// the next. Chrome 148 (probe page, in one page task: the page's 0 ms,
/// the frame's 0 ms, the page's 0 ms, each queueing a microtask):
/// `page0,page0-micro,frame0,frame0-micro,page0b,page0b-micro`.
#[test]
fn a_frame_trees_timers_due_together_run_in_the_order_set() {
    let page = "<!doctype html><body><iframe id=f src=/child.html></iframe><script>
window.order = []; window.ready = false;
document.getElementById('f').addEventListener('load', function () { ready = true; });
function arm() {
  var w = document.getElementById('f').contentWindow;
  setTimeout(function () {
    function t(name, win) { return function () { order.push(name); win.Promise.resolve().then(function () { order.push(name + '-micro'); }); }; }
    setTimeout(t('page0', window), 0);
    w.setTimeout(t('frame0', w), 0);
    setTimeout(t('page0b', window), 0);
  }, 10);
}
</script></body>";
    let mut page = Page::load(true, page);
    page.turns_until("window.ready", Duration::from_secs(5));
    page.js("arm()");
    page.rt.block_on(page.engine.pump_live(page.view, 500));
    page.rt.block_on(page.engine.pump_live(page.view, 0));
    assert_eq!(
        page.js("order.join(',')"),
        r#"String("page0,page0-micro,frame0,frame0-micro,page0b,page0b-micro")"#
    );
}

/// A removed frame is dropped: its window is no longer the element's.
/// Chrome 148: `contentWindow` and `contentDocument` are null once the
/// element is out of the document.
#[test]
fn a_removed_frame_is_dropped() {
    let page = "<!doctype html><body><iframe id=f src=/child.html></iframe><script>
window.loaded = false;
var f = document.getElementById('f');
f.addEventListener('load', function () { loaded = true; });
</script></body>";
    let mut page = Page::load(true, page);
    page.turns_until("window.loaded", Duration::from_secs(5));
    page.js("window.held = f.contentWindow; f.parentNode.removeChild(f);");
    page.turns_until("false", Duration::from_millis(100));
    assert_eq!(page.js("[String(f.contentWindow), String(f.contentDocument), held.document.title].join()"), r#"String("null,null,child-title")"#);
}

/// Off (the default), nothing changes: no request for the frame's
/// document, `contentWindow` null, no `load`.
#[test]
fn with_nested_contexts_off_a_frame_stays_empty() {
    let page = "<!doctype html><body><iframe id=f src=/child.html></iframe><script>
window.loads = 0; document.getElementById('f').onload = function () { loads++; };
</script></body>";
    let mut page = Page::load(false, page);
    page.turns_until("false", Duration::from_millis(300));
    assert_eq!(page.js("[loads, String(document.getElementById('f').contentWindow)].join()"), r#"String("0,null")"#);
    assert_eq!(page.served("/child.html"), 0);
}

// ── Stage 3: layout, paint, the frame's timers and network ─────────────────

/// The iframe is a replaced box: 300x150 by default, its width/height
/// attributes, author CSS, and a 2px inset border. Chrome 148 on this page
/// (body margin 0): a 0,0 304x154; b 304,70 204x84; c 0,158 324x204; each
/// frame's viewport is the content box (300x150, 200x80, 320x200), and the
/// border is 2px inset (painted solid here: not asserted).
#[test]
fn a_frame_is_a_replaced_box_with_a_2px_inset_border() {
    let page = "<!doctype html><body style='margin:0'><iframe id=a></iframe><iframe id=b width=200 height=80></iframe><br>\
<iframe id=c style='width:320px;height:200px'></iframe><script>
window.boxes = function () { return ['a','b','c'].map(function (id) { var e = document.getElementById(id), r = e.getBoundingClientRect(), s = getComputedStyle(e);
  return [r.x, r.y, r.width, r.height, s.borderTopWidth, e.contentWindow ? e.contentWindow.innerWidth + 'x' + e.contentWindow.innerHeight : 'none'].join(','); }).join(' | '); };
</script></body>";
    let mut page = Page::load(true, page);
    page.turns_until("['a','b','c'].every(function (id) { return document.getElementById(id).contentWindow; })", Duration::from_secs(5));
    assert_eq!(
        page.js("boxes()"),
        r#"String("0,0,304,154,2px,300x150 | 304,70,204,84,2px,200x80 | 0,158,324,204,2px,320x200")"#
    );
}

/// The frame's timers and its fetch() run, and what its script writes
/// reaches its layout and the parent's paint. Chrome 148: the child's
/// timer and fetch both answer, and its body's height read from the parent
/// is the new one in the same task.
#[test]
fn a_frames_timers_network_and_layout_run() {
    let page = "<!doctype html><body><iframe id=f src=/live.html style='width:200px;height:100px'></iframe><script>
window.loaded = false; document.getElementById('f').addEventListener('load', function () { loaded = true; });
</script></body>";
    let routes_page = page.to_string();
    let live = "<!doctype html><body style='margin:0'><div id=box style='height:10px;background:rgb(0,255,0)'></div><script>
window.log = [];
setTimeout(function () { log.push('timer'); }, 20);
requestAnimationFrame(function () { log.push('frame'); });
fetch('/data.txt').then(function (r) { return r.text(); }).then(function (t) { log.push('fetch:' + t); });
</script></body>";
    let server = serve_routes(vec![
        ("/", "text/html", routes_page),
        ("/live.html", "text/html", live.to_string()),
        ("/data.txt", "text/plain", "hello".to_string()),
    ]);
    let mut engine = Engine::new(config(true)).expect("engine");
    let view = engine.create_headless_view(Bounds { x: 0, y: 0, width: 800, height: 600 }).expect("view");
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    rt.block_on(engine.load_url(view, Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap())).unwrap();
    let mut page = Page { engine, view, rt, server };
    page.turns_until("window.loaded && document.getElementById('f').contentWindow.log.length >= 3", Duration::from_secs(8));
    assert_eq!(page.js("document.getElementById('f').contentWindow.log.slice().sort().join()"), r#"String("fetch:hello,frame,timer")"#);
    // A write in the frame, read across the boundary at once.
    assert_eq!(
        page.js("var d = document.getElementById('f').contentDocument; d.getElementById('box').style.height = '40px'; d.body.getBoundingClientRect().height"),
        "Number(40.0)"
    );
    // The frame's green box is in the parent's paint, inside the iframe.
    page.turns_until("false", Duration::from_millis(100));
    let list = page.engine.views[&page.view].doc.display_list.clone().expect("a display list");
    let green = list.commands.iter().any(|c| matches!(c,
        rustkit_layout::DisplayCommand::SolidColor(color, _) if color.r == 0 && color.g == 255 && color.b == 0));
    assert!(green, "the frame's paint is not in the parent's display list");
}

/// A loop that turns again only when the last turn asked for it (a timer
/// due, a request out), as the app's and parity-capture's do, still
/// finishes a frame navigation started from a `load` handler. Found with
/// Speedometer 3.1 on the integration branch: its runner sets `src` for
/// the next workload from the frame's `load` path, the page then had no
/// timer and nothing out, and the loop slept (149 s, until its window
/// ended) before the frame request was taken.
#[test]
fn a_frame_request_made_during_a_turn_asks_for_the_next_turn() {
    let page = "<!doctype html><body><iframe id=f></iframe><script>
window.loads = 0; var f = document.getElementById('f');
f.onload = function () { loads++; if (loads === 1) f.src = '/other.html'; };
f.src = '/child.html';
</script></body>";
    let mut page = Page::load(true, page);
    let mut turns = 0;
    loop {
        let turn = page.rt.block_on(page.engine.pump_live(page.view, 20));
        turns += 1;
        if turns >= 40 || (turn.next_timer_ms.is_none() && turn.in_flight == 0) {
            break;
        }
        std::thread::sleep(Duration::from_millis(turn.next_timer_ms.unwrap_or(10).min(20)));
    }
    assert_eq!(page.js("[loads, f.contentDocument.title].join()"), r#"String("2,other-title")"#, "after {turns} turns");
}

/// A frame's module scripts run, after its classic ones, with their
/// imports (Speedometer 3.1: the WebComponents and Angular workloads are
/// `<script type="module">`; without this the first found no elements and
/// the second waited for ever). Chrome 148: `classic,dep,dep-value,main` and the
/// custom element is upgraded.
#[test]
fn a_frames_module_scripts_run_with_their_imports() {
    let page = "<!doctype html><body><iframe id=f src=/mod.html></iframe><script>
window.loaded = false; document.getElementById('f').addEventListener('load', function () { loaded = true; });
</script></body>";
    let child = "<!doctype html><body><x-hello id=x></x-hello><script type=module src=/main.mjs></script>\
<script>window.order = ['classic'];</script></body>";
    let server = serve_routes(vec![
        ("/", "text/html", page.to_string()),
        ("/mod.html", "text/html", child.to_string()),
        ("/main.mjs", "text/javascript", "import { dep } from './dep.mjs'; order.push(dep, 'main'); \
            customElements.define('x-hello', class extends HTMLElement { connectedCallback() { this.textContent = 'upgraded'; } });".to_string()),
        ("/dep.mjs", "text/javascript", "order.push('dep'); export const dep = 'dep-value';".to_string()),
    ]);
    let mut engine = Engine::new(config(true)).expect("engine");
    let view = engine.create_headless_view(Bounds { x: 0, y: 0, width: 800, height: 600 }).expect("view");
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    rt.block_on(engine.load_url(view, Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap())).unwrap();
    let mut page = Page { engine, view, rt, server };
    page.turns_until("window.loaded", Duration::from_secs(8));
    assert_eq!(
        page.js("var w = document.getElementById('f').contentWindow; w.order.join() + '|' + w.document.getElementById('x').textContent"),
        r#"String("classic,dep,dep-value,main|upgraded")"#
    );
}

/// `querySelector` inside a shadow tree of a frame's document (Speedometer
/// 3.1 TodoMVC-WebComponents and Lit: the runner walks `todo-app` ->
/// shadow root -> `todo-topbar`, and got null inside the frame while the
/// same page top-level answered). The frame's bindings had no selector
/// matcher from the engine. Chrome 148: the elements are found.
#[test]
fn a_frames_script_finds_elements_inside_a_shadow_tree() {
    let page = "<!doctype html><body><iframe id=f src=/shadow.html></iframe><script>
window.loaded = false; document.getElementById('f').addEventListener('load', function () { loaded = true; });
</script></body>";
    let child = "<!doctype html><body><div id=h></div><script>
var r = document.getElementById('h').attachShadow({ mode: 'open' });
r.innerHTML = '<section class=app><x-top></x-top><span class=s>in</span></section>';
window.inside = [String(r.querySelector('x-top')), String(r.querySelector('.s')), r.querySelectorAll('*').length].join();
</script></body>";
    let server = serve_routes(vec![("/", "text/html", page.to_string()), ("/shadow.html", "text/html", child.to_string())]);
    let mut engine = Engine::new(config(true)).expect("engine");
    let view = engine.create_headless_view(Bounds { x: 0, y: 0, width: 800, height: 600 }).expect("view");
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    rt.block_on(engine.load_url(view, Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap())).unwrap();
    let mut page = Page { engine, view, rt, server };
    page.turns_until("window.loaded", Duration::from_secs(8));
    assert_eq!(
        page.js("var d = document.getElementById('f').contentDocument; document.getElementById('f').contentWindow.inside + '|' + String(d.getElementById('h').shadowRoot.querySelector('x-top'))"),
        r#"String("[object HTMLElement],[object HTMLSpanElement],3|[object HTMLElement]")"#
    );
}

/// A server for redirect chains: each route answers `(path, Ok(html))` with
/// the page or `(path, Err(location))` with a 302 to `location`. `{a}` and
/// `{b}` in a body or a location are this server's and `other`'s origins.
fn serve_with_redirects(routes: Vec<(&'static str, Result<String, String>)>) -> (u16, std::sync::Arc<std::sync::Mutex<Vec<(&'static str, Result<String, String>)>>>) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let routes = std::sync::Arc::new(std::sync::Mutex::new(routes));
    let shared = routes.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let routes = shared.clone();
            std::thread::spawn(move || {
                let mut request = Vec::new();
                let mut buf = [0u8; 2048];
                while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                    match stream.read(&mut buf) {
                        Ok(0) | Err(_) => return,
                        Ok(n) => request.extend_from_slice(&buf[..n]),
                    }
                }
                let head = String::from_utf8_lossy(&request).to_string();
                let path = head.split_whitespace().nth(1).unwrap_or("/").to_string();
                let answer = routes.lock().unwrap().iter().find(|(p, _)| *p == path).map(|(_, a)| a.clone());
                let reply = match answer {
                    Some(Ok(body)) => format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    ),
                    Some(Err(location)) => format!(
                        "HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    ),
                    None => "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string(),
                };
                let _ = stream.write_all(reply.as_bytes());
            });
        }
    });
    (port, routes)
}

/// Whether a frame is same-origin is decided on the response the document
/// comes from, after its redirects, not on the requested URL; and a chain
/// that leaves the page's origin is cross-origin even when it comes back.
/// Either way it gets the cross-origin floor: no document is reachable, the
/// frame's scripts do not run, and the element hears one `load`.
#[test]
fn a_frames_origin_is_its_responses_after_redirects() {
    let child = "<!doctype html><title>child</title><script>try { parent.childRan = (parent.childRan || 0) + 1; } catch (e) {}</script>".to_string();
    let (b, b_routes) = serve_with_redirects(vec![]);
    let (a, a_routes) = serve_with_redirects(vec![]);
    let (oa, ob) = (format!("http://127.0.0.1:{a}"), format!("http://127.0.0.1:{b}"));
    *b_routes.lock().unwrap() = vec![
        ("/child.html", Ok(child.clone())),
        ("/back", Err(format!("{oa}/child.html"))),
    ];
    let page = format!(
        "<!doctype html><body><script>
window.loads = {{}}; window.childRan = 0;
function frame(id, src) {{
  var f = document.createElement('iframe'); f.id = id;
  f.onload = function () {{ loads[id] = (loads[id] || 0) + 1; }};
  f.src = src; document.body.appendChild(f);
}}
frame('away', '/away');
frame('round', '/round');
frame('in', '{ob}/in');
frame('home', '/child.html');
</script></body>"
    );
    *a_routes.lock().unwrap() = vec![
        ("/", Ok(page)),
        ("/child.html", Ok(child.clone())),
        ("/away", Err(format!("{ob}/child.html"))),
        ("/round", Err(format!("{ob}/back"))),
    ];
    b_routes.lock().unwrap().push(("/in", Err(format!("{oa}/child.html"))));
    let mut engine = Engine::new(config(true)).expect("engine");
    let view = engine.create_headless_view(Bounds { x: 0, y: 0, width: 800, height: 600 }).expect("view");
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    rt.block_on(engine.load_url(view, Url::parse(&format!("{oa}/")).unwrap())).expect("load_url");
    let done = "['away','round','in','home'].every(function (k) { return loads[k] >= 1; })";
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(20));
        rt.block_on(engine.pump_live(view, 20));
        if engine.execute_script(view, done).unwrap() == "Boolean(true)" {
            break;
        }
    }
    let shape = "['away','round','in','home'].map(function (k) { var f = document.getElementById(k); \
        return k + ':' + loads[k] + ':' + (f.contentDocument ? f.contentDocument.title : 'null'); }).join(' ') + ' ran:' + childRan";
    assert_eq!(
        engine.execute_script(view, shape).unwrap(),
        r#"String("away:1:null round:1:null in:1:null home:1:child ran:1")"#,
        "only the frame whose whole chain is the page's origin is same-origin"
    );
}

/// The canceled flag is the frame tree's (#1106 merged with #1109): an
/// event made in one window and dispatched in another is cancelled by an
/// `on<type>` handler returning false or by `preventDefault()`, both ways.
/// Chrome 148 on docs/diagnostics/2026-11-13/cross_realm_cancel.html:
/// `A false true | B false true | C false true | D false true`.
#[test]
fn an_event_from_another_window_of_the_tree_is_cancelled_as_its_own() {
    let fixture = include_str!("../../../docs/diagnostics/2026-11-13/cross_realm_cancel.html");
    let mut page = Page::load(true, fixture);
    page.turns_until("typeof window.result === 'string'", Duration::from_secs(5));
    assert_eq!(page.js("window.result"), r#"String("A false true | B false true | C false true | D false true")"#);
}

/// A frame's response is parsed as markup only for an HTML type; other
/// types render as Chrome for Testing 148 renders them in a same-origin
/// frame (recorded 2026-11-13 on the probe page `ct.html`, the frame's
/// title and body markup, the port written as PORT):
///   ct.txt  -> `<pre style="word-wrap: break-word; white-space: pre-wrap;">plain &lt;b&gt;text&lt;/b&gt; line2 </pre>` (body text)
///   ct.json -> `<pre>{"a": 1, "b": "&lt;i&gt;x&lt;/i&gt;"}</pre><div class="json-formatter-container"></div>`
///   ct.js   -> the source as text in the same `<pre>`: it does not run
///   ct.css  -> `<pre ...>body { color: red }</pre>`
///   ct.png  -> title `ct.png (3×2)`, `<img style="display: block;-webkit-user-select: none;" src="http://127.0.0.1:PORT/ct.png">`
///   ct.bin  -> downloaded: no `load` at the frame, which keeps its initial document
/// (The image document's title has no size here: a stated deviation.)
#[test]
fn a_frames_response_is_markup_only_for_an_html_type() {
    use crate::script_net_tests::serve_byte_routes;
    let png: Vec<u8> = vec![
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 0x0d, 0x49, 0x48, 0x44, 0x52, 0, 0, 0, 3, 0, 0, 0, 2, 8, 2, 0, 0, 0,
        0x12, 0x16, 0xf1, 0x4d, 0, 0, 0, 0x0f, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xf8, 0xcf, 0x00, 0x42, 0x0c, 0x30, 0x02,
        0x00, 0x46, 0xd4, 0x0b, 0xf5, 0x3c, 0x47, 0x62, 0xe5, 0, 0, 0, 0, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];
    let page = "<!doctype html><body><script>
window.loads = {}; window.ranJs = 0;
['ct.txt','ct.json','ct.js','ct.css','ct.png','ct.bin'].forEach(function (k) {
  var f = document.createElement('iframe'); f.id = k; f.src = '/' + k;
  f.onload = function () { loads[k] = (loads[k] || 0) + 1; };
  document.body.appendChild(f);
});
function shape(k) {
  var d = document.getElementById(k).contentDocument;
  return d ? d.title + ' | ' + (d.body ? d.body.innerHTML : 'nobody').replace(/\\s+/g, ' ').replace(/127\\.0\\.0\\.1:\\d+/, '127.0.0.1:PORT') : 'null';
}
</script></body>";
    let server = serve_byte_routes(vec![
        ("/", "text/html", page.as_bytes().to_vec()),
        ("/ct.txt", "text/plain", b"plain <b>text</b>\nline2\n".to_vec()),
        ("/ct.json", "application/json", br#"{"a": 1, "b": "<i>x</i>"}"#.to_vec()),
        ("/ct.js", "text/javascript", b"window.parent.ranJs = 1;".to_vec()),
        ("/ct.css", "text/css", b"body { color: red }".to_vec()),
        ("/ct.png", "image/png", png),
        ("/ct.bin", "application/octet-stream", b"\x00\x01binary".to_vec()),
    ]);
    let mut engine = Engine::new(config(true)).expect("engine");
    let view = engine.create_headless_view(Bounds { x: 0, y: 0, width: 800, height: 600 }).expect("view");
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    rt.block_on(engine.load_url(view, Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap())).expect("load_url");
    let done = "['ct.txt','ct.json','ct.js','ct.css','ct.png'].every(function (k) { return loads[k] >= 1; })";
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(20));
        rt.block_on(engine.pump_live(view, 20));
        if engine.execute_script(view, done).unwrap() == "Boolean(true)" {
            break;
        }
    }
    // A little longer, for the download's frame to (not) hear `load`.
    for _ in 0..10 {
        std::thread::sleep(Duration::from_millis(20));
        rt.block_on(engine.pump_live(view, 20));
    }
    let js = |engine: &mut Engine, s: &str| engine.execute_script(view, s).unwrap();
    assert_eq!(js(&mut engine, "shape('ct.txt')"), r#"String(" | <pre style=\"word-wrap: break-word; white-space: pre-wrap;\">plain &lt;b&gt;text&lt;/b&gt; line2 </pre>")"#);
    assert_eq!(js(&mut engine, "shape('ct.json')"), r#"String(" | <pre>{\"a\": 1, \"b\": \"&lt;i&gt;x&lt;/i&gt;\"}</pre><div class=\"json-formatter-container\"></div>")"#);
    assert_eq!(js(&mut engine, "shape('ct.js') + ' ran:' + ranJs"), r#"String(" | <pre style=\"word-wrap: break-word; white-space: pre-wrap;\">window.parent.ranJs = 1;</pre> ran:0")"#);
    assert_eq!(js(&mut engine, "shape('ct.css')"), r#"String(" | <pre style=\"word-wrap: break-word; white-space: pre-wrap;\">body { color: red }</pre>")"#);
    assert_eq!(js(&mut engine, "shape('ct.png')"), r#"String("ct.png | <img style=\"display: block;-webkit-user-select: none;\" src=\"http://127.0.0.1:PORT/ct.png\">")"#);
    assert_eq!(js(&mut engine, "String(loads['ct.bin'] || 0)"), r#"String("0")"#, "a download is no document and no load");
}

/// How many documents of the page's frame tree have their workers asked
/// in the tree's timer turn (one entry a document).
fn documents_asked_for_workers(page: &Page) -> String {
    let bindings = page.engine.views.get(&page.view).and_then(|v| v.doc.bindings.as_ref()).expect("bindings");
    format!("{:?}", bindings.host_turn("return __rk.shared.timerQueue.pumpCount;", &[]).unwrap())
}

/// A dedicated worker started by a frame's document is heard through the
/// live loop: the frame's own turn takes no timers, and its worker's
/// message arrives in the tree's turn all the same (as in Chrome, where
/// a frame's worker is a worker like any other).
#[test]
fn a_frames_worker_is_heard_through_the_live_loop() {
    let page = "<!doctype html><body><iframe id=f src=/child.html></iframe><script>
window.loaded = false;
var f = document.getElementById('f');
f.addEventListener('load', function () { loaded = true; });
</script></body>";
    let mut page = Page::load(true, page);
    page.turns_until("window.loaded", Duration::from_secs(5));
    // The frame's own `Worker`: the worker is its document's.
    page.js(
        "var w = f.contentWindow; w.got = []; \
         w.worker = new w.Worker('data:text/javascript,postMessage(7)'); \
         w.worker.onmessage = function (e) { w.got.push(e.data); };",
    );
    page.turns_until("f.contentWindow.got.length > 0", Duration::from_secs(5));
    assert_eq!(page.js("f.contentWindow.got.join()"), r#"String("7")"#, "what the frame's worker sent");
}

/// A frame's document is asked for its workers while it is in the tree,
/// and no longer once the frame is removed.
#[test]
fn a_removed_frames_workers_are_no_longer_asked() {
    let page = "<!doctype html><body><iframe id=f src=/child.html></iframe><script>
window.loaded = false;
var f = document.getElementById('f');
f.addEventListener('load', function () { loaded = true; });
</script></body>";
    let mut page = Page::load(true, page);
    page.turns_until("window.loaded", Duration::from_secs(5));
    assert_eq!(documents_asked_for_workers(&page), "Number(2.0)", "the page and its frame");
    page.js("f.parentNode.removeChild(f);");
    page.turns_until("false", Duration::from_millis(100));
    assert_eq!(documents_asked_for_workers(&page), "Number(1.0)", "the page alone, its frame gone");
    // The page's own timers still run.
    page.js("window.ticked = false; setTimeout(function () { ticked = true; }, 0);");
    page.turns_until("window.ticked", Duration::from_secs(2));
    assert_eq!(page.js("String(window.ticked)"), r#"String("true")"#);
}

/// How far a frame's `performance.now()` moves across 40 ms of real time, and the page's.
fn clocks_across_40ms(config: EngineConfig) -> (f64, f64) {
    let page = "<!doctype html><body><iframe id=f src=/child.html></iframe><script>
window.loaded = false;
var f = document.getElementById('f');
f.addEventListener('load', function () { loaded = true; });
</script></body>";
    let mut page = Page::load_with(config, page);
    page.turns_until("window.loaded", Duration::from_secs(5));
    assert_eq!(page.js("String(typeof f.contentWindow.childFn)"), r#"String("function")"#, "the frame loaded");
    let read = |page: &mut Page| -> (f64, f64) {
        let v = page.js("f.contentWindow.performance.now() + ' ' + performance.now()");
        let v = v.trim_start_matches("String(\"").trim_end_matches("\")").to_string();
        let (frame, top) = v.split_once(' ').unwrap_or_else(|| panic!("{v}"));
        (frame.parse().unwrap(), top.parse().unwrap())
    };
    let a = read(&mut page);
    std::thread::sleep(Duration::from_millis(40));
    let b = read(&mut page);
    (b.0 - a.0, b.1 - a.1)
}

/// The page clock's capture switch is the frame tree's: a frame's document, which the engine
/// makes after the page's, reads a stepped clock under it and a flowing one without it, as the
/// page does (`clock_flag_tests`).
#[test]
fn the_capture_clock_is_the_frames_too() {
    let (frame, top) = clocks_across_40ms(EngineConfig { deterministic_page_clock: true, ..config(true) });
    assert!(frame < 5.0 && top < 5.0, "under the capture switch the frame moved {frame} ms and the page {top} ms across 40 ms");
    let (frame, top) = clocks_across_40ms(config(true));
    assert!(frame >= 35.0 && top >= 35.0, "without it the frame moved {frame} ms and the page {top} ms across 40 ms");
}

/// The blank load's replay and `window.onerror` in one dispatch: the
/// replayed `load` reaches only the handler the frame had at its insertion
/// (one set afterwards hears nothing), and what that handler throws
/// reaches `window.onerror` with its five arguments, `this` the Window,
/// where `true` back cancels the event.
#[test]
fn a_replayed_blank_load_reaches_its_recorded_handler_and_window_onerror_keeps_its_shape() {
    let page = "<!doctype html><body><script>
window.calls = []; window.heard = []; window.events = [];
window.onerror = function () { calls.push(arguments.length + ':' + (this === window) + ':' + arguments[0] + ':' + (arguments[4] instanceof Error)); return true; };
window.addEventListener('error', function (ev) { setTimeout(function () { events.push(ev.defaultPrevented); }, 0); });
var early = document.createElement('iframe');
early.onload = function () { heard.push('early'); throw new Error('from the handler'); };
document.body.appendChild(early);
var late = document.createElement('iframe');
document.body.appendChild(late);
late.onload = function () { heard.push('late'); };
</script></body>";
    let mut page = Page::load(true, page);
    // Counted in turns, not in time: the event's own timer has run within
    // 200 turns, and 10 more would show a second `load` or call.
    assert!(page.turns("window.events.length >= 1", 200), "the error event's timer never ran");
    page.turns("false", 10);
    assert_eq!(page.js("window.heard.join()"), r#"String("early")"#);
    assert_eq!(page.js("window.calls.join()"), r#"String("5:true:Uncaught Error: from the handler:true")"#);
    assert_eq!(page.js("window.events.join()"), r#"String("true")"#);
}
