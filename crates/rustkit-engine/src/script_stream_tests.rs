//! LAT17: a page's scripts run as they arrive, in document order.
//!
//! Before LAT17 the load fetched every external script before it ran any
//! (`fetch_page_scripts` collected them all). An inline script that comes
//! before a slow external one waited for it. On `i0a_nav` the first
//! script's `location.href` waited 1.5 s behind a later `<script src>`.
//! Pinned Chrome 148 left the page in 0.03 s.
//!
//! Now each classic script runs as soon as it and every script before it
//! are in. `defer` and module scripts run after them, in document order,
//! and `async` scripts after those, as before. On the content thread, a
//! script that asks to navigate ends the load at the next script boundary.
//! The window's navigation is then the only thing left to happen.

use std::time::{Duration, Instant};

use url::Url;

use crate::load_turns_tests::{html, js, serve, spin, Fixture, Load, Route, MAIN_DIV, SERVED};
use crate::{Engine, EngineConfig, ScriptOutcome};
use rustkit_viewhost::Bounds;

fn slow_js(body: &str, ms: u64) -> Route {
    ("text/javascript", body.as_bytes().to_vec(), ms)
}

/// Load `path` on the same-thread engine (no safe points). Returns the
/// engine and its view once the load is over.
fn load_engine(fixture: &Fixture, path: &str) -> (Engine, crate::EngineViewId) {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 400, height: 300 })
        .expect("view");
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    rt.block_on(engine.load_url(view, fixture.url(path))).expect("load_url");
    (engine, view)
}

fn read(engine: &mut Engine, view: crate::EngineViewId, script: &str) -> String {
    let answer = engine.execute_script(view, script).expect("script runs");
    // `String("...")`: the value's debug form.
    answer
        .strip_prefix("String(\"")
        .and_then(|rest| rest.strip_suffix("\")"))
        .map(str::to_string)
        .unwrap_or(answer)
}

// ---------------------------------------------------------------------
// Paths 1, 3: an inline script does not wait for a later external one.
// ---------------------------------------------------------------------

/// Inline A, a slow external B, inline C. A runs before B has arrived.
/// The order A, B, C is kept.
#[test]
fn an_inline_script_runs_before_a_later_slow_external_script_arrives() {
    let page = r#"<!doctype html><body>
<script>window.order = ['A']; window.tA = Date.now();</script>
<script src="/b.js"></script>
<script>order.push('C');</script>"#;
    let fixture = serve(vec![
        ("/page", html(page)),
        ("/b.js", slow_js("order.push('B'); window.tB = Date.now();", 800)),
    ]);
    let (mut engine, view) = load_engine(&fixture, "/page");
    assert_eq!(read(&mut engine, view, "order.join(',')"), "A,B,C");
    let gap: i64 = read(&mut engine, view, "String(tB - tA)").parse().expect("a number");
    assert!(gap >= 600, "A ran {gap} ms before B: it waited for B's fetch");
}

/// Across inline, external, `defer` and `async`. A slow `defer` script comes
/// first in the document, and every classic script still runs before it.
/// The classic scripts do not wait for it. Async runs last, as before
/// (stated: not when ready).
#[test]
fn classic_scripts_run_in_order_before_defer_and_async_and_do_not_wait_for_them() {
    let page = r#"<!doctype html><body>
<script defer src="/d.js"></script>
<script async src="/a.js"></script>
<script>window.order = ['i1']; window.t1 = Date.now();</script>
<script src="/e2.js"></script>
<script>order.push('i3');</script>"#;
    let fixture = serve(vec![
        ("/page", html(page)),
        ("/d.js", slow_js("order.push('defer'); window.tD = Date.now();", 800)),
        ("/a.js", js("order.push('async');")),
        ("/e2.js", slow_js("order.push('e2');", 200)),
    ]);
    let (mut engine, view) = load_engine(&fixture, "/page");
    assert_eq!(read(&mut engine, view, "order.join(',')"), "i1,e2,i3,defer,async");
    let gap: i64 = read(&mut engine, view, "String(tD - t1)").parse().expect("a number");
    assert!(gap >= 500, "the first inline ran {gap} ms before the slow defer arrived");
}

/// A script that throws does not stop the ones after it, slow or not.
#[test]
fn a_script_that_throws_does_not_stop_the_later_scripts() {
    let page = r#"<!doctype html><body>
<script>window.order = ['i1']; throw new Error('boom');</script>
<script src="/e2.js"></script>
<script>order.push('i3');</script>"#;
    let fixture = serve(vec![("/page", html(page)), ("/e2.js", slow_js("order.push('e2');", 300))]);
    let (mut engine, view) = load_engine(&fixture, "/page");
    assert_eq!(read(&mut engine, view, "order.join(',')"), "i1,e2,i3");
    let log = engine.script_log(view).expect("a log").to_vec();
    let outcomes: Vec<(&str, bool)> = log
        .iter()
        .filter(|r| r.source.starts_with("inline#") || r.source.ends_with("/e2.js"))
        .map(|r| (r.source.as_str(), matches!(r.outcome, ScriptOutcome::Threw(_))))
        .collect();
    assert_eq!(outcomes.len(), 3, "{log:?}");
    assert!(outcomes[0].1, "the first script threw: {log:?}");
    assert!(!outcomes[1].1 && !outcomes[2].1, "{log:?}");
}

/// `document.write` in an early script: what it does is unchanged by when
/// the scripts run. Today the document is parsed before any script runs,
/// so the write does not reach the tree.
#[test]
fn document_write_in_an_early_script_is_unchanged() {
    let page = r#"<!doctype html><body><p id="main">main</p>
<script>window.order = ['i1']; document.write('<p id="written">w</p>');</script>
<script src="/e2.js"></script>
<script>order.push('i3:' + (document.getElementById('written') ? 'written' : 'none') + ':' + (document.getElementById('main') ? 'main' : 'gone'));</script>"#;
    let fixture = serve(vec![("/page", html(page)), ("/e2.js", slow_js("order.push('e2');", 300))]);
    let (mut engine, view) = load_engine(&fixture, "/page");
    assert_eq!(read(&mut engine, view, "order.join(',')"), "i1,e2,i3:none:main");
}

/// A classic script whose fetch fails is logged in its place in the
/// document, between the scripts around it.
#[test]
fn a_failed_fetch_is_logged_in_document_order() {
    let page = r#"<!doctype html><body>
<script>window.order = ['i1'];</script>
<script src="http://127.0.0.1:1/missing.js"></script>
<script>order.push('i3');</script>"#;
    // Nothing listens on port 1: the fetch fails at once.
    let fixture = serve(vec![("/page", html(page))]);
    let (mut engine, view) = load_engine(&fixture, "/page");
    assert_eq!(read(&mut engine, view, "order.join(',')"), "i1,i3");
    let log = engine.script_log(view).expect("a log").to_vec();
    let sources: Vec<&str> = log.iter().map(|r| r.source.as_str()).collect();
    let missing = sources.iter().position(|s| s.ends_with("/missing.js")).expect("the missing script is logged");
    let first = sources.iter().position(|s| *s == "inline#1").expect("inline#1");
    let third = sources.iter().position(|s| *s == "inline#3").expect("inline#3");
    assert!(first < missing && missing < third, "logged out of order: {sources:?}");
}

// ---------------------------------------------------------------------
// Overlap: the fetches are not serialized.
// ---------------------------------------------------------------------

/// Ten external scripts, the first one slow. Every request is out before
/// the slow one answers. A slow head does not hold back the 9th and 10th
/// (today's `buffered(8)` did).
#[test]
fn a_slow_first_script_does_not_hold_back_the_other_fetches() {
    let mut page = String::from("<!doctype html><body><script>window.order = [];</script>");
    let mut routes: Vec<(&'static str, Route)> = Vec::new();
    const PATHS: [&str; 10] =
        ["/e1.js", "/e2.js", "/e3.js", "/e4.js", "/e5.js", "/e6.js", "/e7.js", "/e8.js", "/e9.js", "/e10.js"];
    for (i, path) in PATHS.iter().enumerate() {
        page.push_str(&format!(r#"<script src="{path}"></script>"#));
        let body = format!("order.push({});", i + 1);
        routes.push((path, slow_js(&body, if i == 0 { 1_000 } else { 0 })));
    }
    routes.push(("/page", ("text/html", page.into_bytes(), 0)));
    let fixture = serve(routes);
    let (mut engine, view) = load_engine(&fixture, "/page");
    assert_eq!(read(&mut engine, view, "order.join(',')"), "1,2,3,4,5,6,7,8,9,10");
    let first = fixture.requests("/e1.js")[0];
    let last = fixture.requests("/e10.js")[0];
    assert!(last < first + 800, "the 10th fetch started {} ms after the slow first", last - first);
}

// ---------------------------------------------------------------------
// Content thread: the i0a_nav chain, and the wait for a slow script.
// ---------------------------------------------------------------------

/// The i0a_nav fixture without the real window. Each page asks for the next
/// one in its first script, before a script the server holds for 1.5 s.
/// The window (played by the harness) follows. Each page is left without
/// waiting for its slow script. The last page finishes.
#[test]
fn each_page_of_the_i0a_chain_is_left_before_its_slow_script_arrives() {
    let page = |n: u32| -> String {
        format!(
            r#"<!doctype html><body><p>Visit {n} of 5.</p>
<script>fetch('/beacon?start-{n}'); {nav}</script>
<script src="/slow-{n}.js"></script>
<script>fetch('/beacon?after-{n}'); {last}</script>"#,
            nav = if n < 5 { format!("location.href = '/nav?n={}';", n + 1) } else { String::new() },
            last = if n == 5 { "fetch('/beacon?last');" } else { "" },
        )
    };
    const NAV: [&str; 5] = ["/nav?n=1", "/nav?n=2", "/nav?n=3", "/nav?n=4", "/nav?n=5"];
    const SLOW: [&str; 5] = ["/slow-1.js", "/slow-2.js", "/slow-3.js", "/slow-4.js", "/slow-5.js"];
    let mut routes: Vec<(&'static str, Route)> = Vec::new();
    for n in 1..=5u32 {
        routes.push((NAV[n as usize - 1], ("text/html", page(n).into_bytes(), 0)));
        routes.push((SLOW[n as usize - 1], slow_js("", 1_500)));
    }
    let fixture = serve(routes);
    let mut load = Load::start(&fixture, "/nav?n=1");
    load.follow = true;
    assert!(
        load.until(Duration::from_secs(30), |_| !fixture.requests("/beacon?last").is_empty()),
        "the last page finished; pages requested at {:?}",
        NAV.iter().map(|p| fixture.requests(p)).collect::<Vec<_>>()
    );
    let asked: Vec<u128> = NAV.iter().map(|p| fixture.requests(p)[0]).collect();
    let stays: Vec<u128> = asked.windows(2).map(|w| w[1] - w[0]).collect();
    eprintln!("LAT17 i0a chain: page requested at {asked:?} ms; stays {stays:?} ms");
    if std::env::var_os("LAT17_TRACE").is_some() {
        for (at, event) in &load.events {
            let ms = at.duration_since(fixture.start).as_millis();
            let line = format!("{event:?}");
            if !line.starts_with("Commit") {
                eprintln!("  {ms} ms {}", &line[..line.len().min(160)]);
            }
        }
        eprintln!("  beacons start {:?}", (1..=5).map(|n| fixture.requests(&format!("/beacon?start-{n}"))).collect::<Vec<_>>());
    }
    // A debug build spends most of a page's stay building its document and
    // bindings before any script can run. What this row decides is the
    // part after that: from the moment the page asked for its slow script
    // (its scripts were found) to the moment the next page was requested.
    // Before LAT17 that was the slow script's 1.5 s and more.
    let left: Vec<u128> = (0..4).map(|i| asked[i + 1] - fixture.requests(SLOW[i])[0]).collect();
    eprintln!("LAT17 i0a chain: next page requested {left:?} ms after the slow script was");
    assert!(left.iter().all(|ms| *ms < 500), "a page waited for its slow script before it was left: {left:?} ms");
    for (n, nav) in NAV.iter().enumerate() {
        assert_eq!(fixture.requests(nav).len(), 1, "page {} requested once", n + 1);
    }
    // Each page's first script ran, and the request it made before asking
    // to leave reached the server, as in Chrome (the real window's "five
    // visits" assertion reads these beacons).
    for n in 1..=5 {
        let start = format!("/beacon?start-{n}");
        assert!(
            load.until(Duration::from_secs(5), |_| !fixture.requests(&start).is_empty()),
            "page {n}'s first request never reached the server"
        );
    }
    // A page that was left runs none of its later scripts.
    for n in 1..=4 {
        let after = format!("/beacon?after-{n}");
        assert!(fixture.requests(&after).is_empty(), "page {n} ran a script after it asked to leave");
    }
}

/// While the load waits for a slow script, a click on text already shown
/// is answered, and its listener runs before that script. No page timer
/// runs early.
#[test]
fn a_click_while_a_slow_script_is_fetched_is_answered_before_it_runs() {
    let page = format!(
        r#"<!doctype html><body style="margin:0">{MAIN_DIV}
<script>window.order = ['s1']; setTimeout(function () {{ order.push('timer'); }}, 0);
document.getElementById('main').addEventListener('click', function () {{ order.push('click:' + document.readyState); }});</script>
<script src="/slow.js"></script>"#
    );
    let fixture = serve(vec![("/page", html(&page)), ("/slow.js", slow_js("order.push('s2');", 2_000))]);
    let mut load = Load::start(&fixture, "/page");
    load.wait_main();
    std::thread::sleep(Duration::from_millis(200));
    let waited = load.click(200.0, 100.0, Duration::from_secs(20)).expect("the click is answered");
    assert!(load.completed(load.nav).is_none(), "the load was still waiting for its script");
    assert!(waited < SERVED, "the click waited {waited:?} for the script");
    load.wait_complete();
    let (order, _) = load.eval("order.slice(0, 3).join(',')", Duration::from_secs(5));
    assert_eq!(order.as_deref(), Some("String(\"s1,click:loading,s2\")"));
}

/// Stop while the load waits for a slow script: the script never runs.
#[test]
fn stop_while_a_slow_script_is_fetched_runs_nothing_more() {
    let page = format!(
        r#"<!doctype html><body style="margin:0">{MAIN_DIV}
<script>window.order = ['s1'];</script>
<script src="/slow.js"></script>
<script>order.push('s3');</script>"#
    );
    let fixture = serve(vec![("/page", html(&page)), ("/slow.js", slow_js("order.push('s2');", 1_500))]);
    let mut load = Load::start(&fixture, "/page");
    load.wait_main();
    let started = Instant::now();
    // Wait until s1 has run (it does not wait for the slow script).
    let mut ran = false;
    while started.elapsed() < Duration::from_millis(1_000) {
        let (order, _) = load.eval("String(window.order)", Duration::from_secs(5));
        if order.as_deref() == Some("String(\"s1\")") {
            ran = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(ran, "s1 ran while the slow script was out");
    load.page.stop();
    std::thread::sleep(Duration::from_millis(2_000));
    let (order, _) = load.eval("String(window.order)", Duration::from_secs(5));
    assert_eq!(order.as_deref(), Some("String(\"s1\")"));
    let _ = spin;
    let _ = Url::parse;
}

// ---------------------------------------------------------------------
// Prometheus's GO (11-09), rulings 1 and 2: the leave path's edges.
// ---------------------------------------------------------------------

/// `DOMContentLoaded` fires only after every `defer` and module script,
/// however late the `defer` script arrives.
#[test]
fn dom_content_loaded_waits_for_every_defer_and_module_script() {
    let page = r#"<!doctype html><body>
<script>window.order = ['i1']; document.addEventListener('DOMContentLoaded', function () { order.push('dcl'); });</script>
<script defer src="/d.js"></script>
<script type="module">order.push('module');</script>
<script>order.push('i2');</script>"#;
    let fixture = serve(vec![("/page", html(page)), ("/d.js", slow_js("order.push('defer');", 600))]);
    let (mut engine, view) = load_engine(&fixture, "/page");
    assert_eq!(read(&mut engine, view, "order.join(',')"), "i1,i2,defer,module,dcl");
}

/// A same-document navigation (`location.href = '#x'`) is not a reason to
/// end the load: the later scripts run and the load completes.
#[test]
fn a_fragment_assignment_does_not_end_the_load() {
    let page = r#"<!doctype html><body><p id="x">x</p>
<script>location.href = '#x';</script>
<script src="/slow.js"></script>
<script>fetch('/beacon?after');</script>"#;
    let fixture = serve(vec![("/page", html(page)), ("/slow.js", slow_js("", 800))]);
    let mut load = Load::start(&fixture, "/page");
    load.follow = true;
    load.wait_complete();
    assert!(
        load.until(Duration::from_secs(5), |_| !fixture.requests("/beacon?after").is_empty()),
        "the script after the slow one ran"
    );
    assert_eq!(fixture.requests("/page").len(), 1, "the page was not loaded again: {:?}", load.followed);
}

/// A navigation asked from a promise reaction of a script is caught at the
/// same boundary: the slow script after it never runs.
#[test]
fn a_navigation_from_a_promise_reaction_ends_the_load_at_the_next_script() {
    let page = r#"<!doctype html><body><p>leaving</p>
<script>Promise.resolve().then(function () { location.href = '/next'; });</script>
<script src="/slow.js"></script>
<script>fetch('/beacon?after');</script>"#;
    let fixture = serve(vec![
        ("/page", html(page)),
        ("/slow.js", slow_js("fetch('/beacon?slow-ran');", 1_500)),
        ("/next", html("<p>next</p>")),
    ]);
    let mut load = Load::start(&fixture, "/page");
    load.follow = true;
    assert!(load.until(Duration::from_secs(10), |_| !fixture.requests("/next").is_empty()), "/next requested");
    let slow = fixture.requests("/slow.js")[0];
    let next = fixture.requests("/next")[0];
    assert!(next < slow + 1_000, "/next requested {} ms after the slow script", next - slow);
    std::thread::sleep(Duration::from_millis(2_000));
    load.pump();
    assert!(fixture.requests("/beacon?slow-ran").is_empty() && fixture.requests("/beacon?after").is_empty());
    assert_eq!(fixture.requests("/next").len(), 1);
}

/// A page with lifecycle listeners, left while its slow script is out:
/// `how` is the page's own script (`script`) or a link the window clicks
/// (`click`). Returns the beacons the old page sent and the times `/next`
/// was requested.
fn leave_during_slow_fetch(how: &str) -> (Vec<&'static str>, Vec<u128>) {
    let leave = if how == "script" { "location.href = '/next';" } else { "" };
    let page = format!(
        r#"<!doctype html><body style="margin:0">
<a id="link" href="/next" style="display:block;width:400px;height:200px;background:rgb(0,160,0)">next</a>
<script>
document.addEventListener('DOMContentLoaded', function () {{ fetch('/beacon?dcl'); }});
window.addEventListener('load', function () {{ fetch('/beacon?load'); }});
window.addEventListener('pagehide', function () {{ fetch('/beacon?pagehide'); }});
window.addEventListener('unload', function () {{ fetch('/beacon?unload'); }});
{leave}
</script>
<script src="/slow.js"></script>"#
    );
    let fixture = serve(vec![
        ("/page", html(&page)),
        ("/slow.js", slow_js("fetch('/beacon?slow-ran');", 2_000)),
        ("/next", html("<p>next</p>")),
    ]);
    let mut load = Load::start(&fixture, "/page");
    load.follow = true;
    if how == "click" {
        load.wait_main_color((0, 160, 0));
        std::thread::sleep(Duration::from_millis(100));
        assert!(load.click(200.0, 100.0, Duration::from_secs(20)).is_some(), "the click is answered");
    }
    assert!(load.until(Duration::from_secs(10), |_| !fixture.requests("/next").is_empty()), "/next requested");
    // Past the slow script's arrival, had the old load kept going.
    std::thread::sleep(Duration::from_millis(2_500));
    load.pump();
    let _ = load.eval("0", Duration::from_secs(5));
    let sent = ["/beacon?dcl", "/beacon?load", "/beacon?pagehide", "/beacon?unload", "/beacon?slow-ran"]
        .into_iter()
        .filter(|b| !fixture.requests(b).is_empty())
        .collect();
    (sent, fixture.requests("/next"))
}

/// The page that asked to leave fires what a page left by a served click
/// fires (LAT15 row 13): no DOMContentLoaded, no `load`, and (no
/// pagehide/unload in RustKit yet) nothing else. Its pending script never
/// runs, and the window navigates once.
#[test]
fn a_page_left_by_its_script_fires_what_a_page_left_by_a_click_fires() {
    let (by_script, next_script) = leave_during_slow_fetch("script");
    let (by_click, next_click) = leave_during_slow_fetch("click");
    assert_eq!(by_script, by_click, "the two ways out differ");
    assert!(by_script.is_empty(), "the page that was left sent {by_script:?}");
    assert_eq!(next_script.len(), 1, "navigated once (script)");
    assert_eq!(next_click.len(), 1, "navigated once (click)");
}

// ---------------------------------------------------------------------
// Prometheus R1 (review 5828788579), nits 1, 2 and 4.
// ---------------------------------------------------------------------

/// Nit 1: a page thread that does not follow script navigation mid-load
/// (`follows_script_navigation` false) is not left at a safe point: its
/// scripts all run and the load completes, and the navigation is
/// reported after it, as before LAT17.
#[test]
fn without_follows_a_script_navigation_does_not_end_the_load() {
    let page = r#"<!doctype html><body><p>staying</p>
<script>location.href = '/next';</script>
<script src="/slow.js"></script>
<script>fetch('/beacon?s3');</script>"#;
    let fixture = serve(vec![
        ("/page", html(page)),
        ("/slow.js", slow_js("", 800)),
        ("/next", html("<p>next</p>")),
    ]);
    let thread = crate::content_thread::ContentThread::spawn(
        crate::EngineBuilder::new()
            .javascript_enabled(true)
            .script_budget_ms(15_000)
            .interrupt_scripts_at_budget(true)
            .live_turn_budget_ms(5_000)
            .follows_script_navigation(false)
            .post_load_requests_live(true),
        Bounds { x: 0, y: 0, width: 800, height: 600 },
    )
    .expect("content thread spawns");
    let mut load = Load::start_on(thread, &fixture, "/page");
    load.follow = true;
    load.wait_complete();
    assert!(
        load.until(Duration::from_secs(5), |_| !fixture.requests("/beacon?s3").is_empty()),
        "the last script ran"
    );
}

/// Nit 2: a script navigation the window does not take (nothing reports
/// it here) is offered at one boundary only. A request a later script
/// makes is answered before DOMContentLoaded, as on any load.
#[test]
fn a_navigation_that_is_not_taken_is_offered_once() {
    let page = r#"<!doctype html><body>
<script>window.order = ['s1']; document.addEventListener('DOMContentLoaded', function () { order.push('dcl'); }); location.href = '/next';</script>
<script src="/slow.js"></script>
<script>fetch('/b').then(function () { order.push('b'); });</script>"#;
    let fixture = serve(vec![("/page", html(page)), ("/slow.js", slow_js("order.push('s2');", 300)), ("/b", js("b"))]);
    let mut engine = Engine::new(EngineConfig { follows_script_navigation: true, ..EngineConfig::default() }).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 400, height: 300 })
        .expect("view");
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    // The window that never takes the navigation: it serves nothing.
    let mut serve = |_: &mut Engine| crate::Served::default();
    let lend: &mut crate::ServeFn<'_> = &mut serve;
    rt.block_on(engine.load_url_serving(view, fixture.url("/page"), None, Some(lend))).expect("load");
    assert_eq!(read(&mut engine, view, "order.join(',')"), "s1,s2,b,dcl");
    // Only the requests queued at the one offer (none here) moved to the
    // live loop; the later script's request was the load's own.
    let live = engine.views.get(&view).map(|v| v.live_requests.len());
    assert_eq!(live, Some(0), "requests were moved to the live loop at later boundaries");
}

/// Nit 4: requests a leaving page left out, held by the server, do not
/// take the next page's places: the next page's own request goes out at
/// once. (The policy, its slots and budget are per page.)
#[test]
fn a_leaving_pages_held_requests_do_not_delay_the_next_pages_request() {
    let first = r#"<!doctype html><body><p>leaving</p>
<script>for (var i = 0; i < 10; i++) { fetch('/held?' + i); } location.href = '/next';</script>
<script src="/slow.js"></script>"#;
    let next = r#"<!doctype html><body><p>next</p><script>fetch('/beacon?next');</script>"#;
    let mut routes: Vec<(&'static str, Route)> = vec![
        ("/page", html(first)),
        ("/slow.js", slow_js("", 1_500)),
        ("/next", html(next)),
    ];
    const HELD: [&str; 10] =
        ["/held?0", "/held?1", "/held?2", "/held?3", "/held?4", "/held?5", "/held?6", "/held?7", "/held?8", "/held?9"];
    for path in HELD {
        routes.push((path, ("text/plain", b"held".to_vec(), 5_000)));
    }
    let fixture = serve(routes);
    let mut load = Load::start(&fixture, "/page");
    load.follow = true;
    assert!(load.until(Duration::from_secs(10), |_| !fixture.requests("/next").is_empty()), "/next requested");
    let next_at = fixture.requests("/next")[0];
    assert!(
        load.until(Duration::from_secs(4), |_| !fixture.requests("/beacon?next").is_empty()),
        "the next page's request waited behind the old page's held ones"
    );
    let beacon = fixture.requests("/beacon?next")[0];
    let held = HELD.iter().filter(|p| !fixture.requests(p).is_empty()).count();
    eprintln!("LAT17 nit 4: {held} held requests out; next page's request {} ms after it", beacon - next_at);
    assert!(held >= 6, "the old page's requests went out ({held})");
}
