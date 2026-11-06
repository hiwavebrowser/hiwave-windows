//! The Inspector's script lifecycle (dev-tools T3): one `script` record per
//! page script with how it was loaded, what became of it and how long each
//! stage took, and a `lifecycle` record for `DOMContentLoaded` and `load`
//! naming the scripts `load` waited for.

use super::*;
use crate::inspector_engine_tests::{records, runtime, serve, Route, TestServer};

fn engine(config: EngineConfig) -> (Engine, EngineViewId) {
    let mut config = config;
    config.inspector.buffer = true;
    let mut engine = Engine::new(config).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 300, height: 200 })
        .expect("view");
    (engine, view)
}

fn load(config: EngineConfig, routes: Vec<Route>) -> (Engine, EngineViewId, TestServer) {
    let server = serve(routes);
    let (mut engine, view) = engine(config);
    runtime().block_on(engine.load_url(view, server.url("/"))).expect("load_url");
    (engine, view, server)
}

/// The records as the JSON Lines sink writes them.
fn json(records: &[InspectorRecord]) -> Vec<serde_json::Value> {
    records.iter().map(|r| serde_json::to_value(r).expect("record serializes")).collect()
}

/// The `script` and `lifecycle` records, in order:
/// `script <kind> <path> <outcome>[ (<reason>)] held=<event> event=<event>`.
fn lifecycle_lines(records: &[InspectorRecord], port: u16) -> Vec<String> {
    let local = |s: &str| s.replace(&format!("http://127.0.0.1:{port}"), "");
    let text = |v: &serde_json::Value| v.as_str().unwrap_or("-").to_string();
    json(records)
        .iter()
        .filter_map(|r| match r["type"].as_str() {
            Some("script") => Some(format!(
                "script {} {} {}{} held={} event={}",
                text(&r["kind"]),
                local(&text(&r["script"])),
                text(&r["outcome"]),
                r["reason"].as_str().map(|why| format!(" ({why})")).unwrap_or_default(),
                text(&r["held"]),
                text(&r["event"]),
            )),
            Some("lifecycle") => Some(format!(
                "lifecycle {} held_by={:?} count={}",
                text(&r["event"]),
                r["held_by"]
                    .as_array()
                    .map(|a| a.iter().map(|s| local(&text(s))).collect::<Vec<_>>())
                    .unwrap_or_default(),
                r["held_by_count"]
            )),
            _ => None,
        })
        .collect()
}

fn script_record(records: &[InspectorRecord], path: &str) -> serde_json::Value {
    json(records)
        .into_iter()
        .find(|r| r["type"] == "script" && r["script"].as_str().is_some_and(|s| s.ends_with(path)))
        .unwrap_or_else(|| panic!("no script record for {path}"))
}

#[test]
fn every_load_path_script_has_one_record_with_its_kind_outcome_and_hold() {
    let page = r#"<html><head>
<script>var a = 1;</script>
<script src="/defer.js" defer></script>
<script src="/async.js" async></script>
<script type="module" src="/m.js"></script>
<script src="/missing.js"></script>
<script nomodule>var nm = 1;</script>
<script src="/huge.js"></script>
<script>throw new TypeError('top');</script>
<script>
var s = document.createElement('script');
s.src = '/ins.js';
document.head.appendChild(s);
</script>
</head><body>hi</body></html>"#;
    let huge = format!("var huge = true;\n{}", "// padding\n".repeat(MAX_PAGE_SCRIPT_BYTES / 11 + 1));
    let (engine, view, server) = load(
        EngineConfig::default(),
        vec![
            Route::html("/", page),
            Route::ok("/defer.js", "text/javascript", "var d = 1;"),
            Route::ok("/async.js", "text/javascript", "var b = 1;"),
            Route::ok("/m.js", "text/javascript", "window.m = 1;"),
            Route::ok("/huge.js", "text/javascript", huge),
            Route::ok("/ins.js", "text/javascript", "window.ins = 1;"),
        ],
    );
    assert_eq!(
        lifecycle_lines(&records(&engine, view), server.port),
        vec![
            // Scripts that will not run are recorded as the load sorts its
            // scripts, before any runs; the rest in the order they ran.
            "script parser /missing.js fetch_failed (HTTP 404) held=DOMContentLoaded event=-",
            "script parser inline#6 skipped (nomodule) held=DOMContentLoaded event=-",
            "script parser inline#1 ran held=DOMContentLoaded event=-",
            "script parser /huge.js skipped (too large to run inside the script budget) held=DOMContentLoaded event=-",
            "script parser inline#8 threw held=DOMContentLoaded event=-",
            "script parser inline#9 ran held=DOMContentLoaded event=-",
            "script defer /defer.js ran held=DOMContentLoaded event=-",
            "script module /m.js ran held=DOMContentLoaded event=load",
            "script async /async.js ran held=DOMContentLoaded event=-",
            "lifecycle DOMContentLoaded held_by=[] count=0",
            "script inserted /ins.js ran held=load event=load",
            "lifecycle load held_by=[\"/ins.js\"] count=1",
        ]
    );
}

#[test]
fn a_script_the_budget_stops_is_stopped_and_the_next_is_over_budget() {
    let page = "<html><head><script>var spun = 0; var row = new Array(1500).fill(0);
row.forEach(function () { row.forEach(function () { spun++; if (spun % 4096 === 0) { document.getElementById('p'); } }); });</script>
<script>var after = true;</script></head><body><p id=\"p\">alive</p></body></html>";
    let config = EngineConfig {
        script_budget_ms: 200,
        interrupt_scripts_at_budget: true,
        ..EngineConfig::default()
    };
    let (engine, view, server) = load(config, vec![Route::html("/", page)]);
    let lines = lifecycle_lines(&records(&engine, view), server.port);
    assert_eq!(
        &lines[..2],
        &[
            "script parser inline#1 stopped (script budget) held=DOMContentLoaded event=-",
            "script parser inline#2 over_budget (script budget spent before it started) held=DOMContentLoaded event=-",
        ],
        "{lines:#?}"
    );
}

#[test]
fn stage_times_say_where_a_script_spent_its_time() {
    let page = r#"<html><head>
<script>var a = 1;</script>
<script src="/slow.js"></script>
<script>
var s = document.createElement('script');
s.src = '/ins.js';
document.head.appendChild(s);
</script>
</head><body>hi</body></html>"#;
    let (engine, view, _server) = load(
        EngineConfig::default(),
        vec![
            Route::html("/", page),
            Route::ok("/slow.js", "text/javascript", "var slow = 1;").delay(300),
            Route::ok("/ins.js", "text/javascript", "window.ins = 1;").delay(300),
        ],
    );
    let records = records(&engine, view);
    let inline = script_record(&records, "inline#1");
    assert!(inline["queued_ms"].is_null() && inline["fetch_ms"].is_null(), "inline: nothing fetched: {inline}");
    assert!(inline["bytes"].as_u64().is_some_and(|n| n > 0), "{inline}");
    let slow = script_record(&records, "/slow.js");
    assert!(slow["queued_ms"].is_null(), "a parser script is not queued by page script: {slow}");
    assert!(slow["fetch_ms"].as_f64().is_some_and(|ms| ms >= 250.0), "{slow}");
    assert!(slow["waited_ms"].is_f64(), "{slow}");
    let inserted = script_record(&records, "/ins.js");
    assert!(inserted["queued_ms"].is_f64(), "{inserted}");
    assert!(inserted["fetch_ms"].as_f64().is_some_and(|ms| ms >= 250.0), "{inserted}");
    assert!(inserted["waited_ms"].is_f64(), "{inserted}");
}

#[test]
fn with_nobody_listening_no_script_record_is_built() {
    let server = serve(vec![Route::html("/", "<html><body><script>var a = 1;</script></body></html>")]);
    // No ring (the tab ring on would be a listener).
    let config = EngineConfig { inspector: InspectorConfig::default(), ..EngineConfig::default() };
    let mut engine = Engine::new(config).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 300, height: 200 })
        .expect("view");
    runtime().block_on(engine.load_url(view, server.url("/"))).expect("load_url");
    let inspector = engine.inspector(view).unwrap();
    assert!(!inspector.is_active());
    assert!(inspector.snapshot_since(0).records.is_empty());
}

/// #797 R2 (1): a `src` that is not a URL (a port past 65535) cannot be
/// reduced to origin and path, so it is recorded as `unparseable#n`, never
/// as its text: the query carries a session.
#[test]
fn an_unparseable_src_is_recorded_by_label_not_by_its_text() {
    let page = r#"<html><head>
<script src="https://cdn.example:99999/a.js?session=SECRET"></script>
<script>
var s = document.createElement('script');
s.src = 'https://cdn.example:99999/b.js?session=SECRET';
document.head.appendChild(s);
</script>
</head><body>hi</body></html>"#;
    let (engine, view, server) = load(EngineConfig::default(), vec![Route::html("/", page)]);
    let records = records(&engine, view);
    let all = serde_json::to_string(&json(&records)).unwrap();
    assert!(!all.contains("SECRET"), "{all}");
    let log = format!("{:?}", engine.script_log(view).unwrap());
    assert!(!log.contains("SECRET"), "{log}");
    let lines = lifecycle_lines(&records, server.port);
    assert!(
        lines.contains(&"script parser unparseable#1 skipped (unparseable src) held=DOMContentLoaded event=-".to_string()),
        "{lines:#?}"
    );
}

/// #797 review: a panel listening when a script held `load`, closed before
/// the next navigation and opened again, reads the next document's `load`
/// as held by that document's scripts only.
#[test]
fn a_detached_navigation_forgets_what_held_the_last_load() {
    let inspector = Inspector::new(&InspectorConfig::default());
    let ended = |source: &str| {
        inspector.script_ended(inspector::ScriptEnd {
            source,
            kind: "async",
            outcome: &ScriptOutcome::Ran,
            started: true,
            bytes: 1,
            queued: None,
            fetch: None,
            waited: None,
            ran_ms: 0,
            event: None,
        })
    };
    inspector.navigation_started(None);
    inspector.lifecycle("DOMContentLoaded");
    inspector.set_buffer_enabled(true);
    ended("inline#1");
    inspector.set_buffer_enabled(false);
    inspector.navigation_started(None);
    inspector.set_buffer_enabled(true);
    inspector.lifecycle("DOMContentLoaded");
    inspector.lifecycle("load");
    let records = json(&inspector.snapshot_since(0).records);
    let load = records.iter().find(|r| r["type"] == "lifecycle" && r["event"] == "load").expect("load record");
    assert_eq!(load["held_by"], serde_json::json!([]), "{records:#?}");
    assert_eq!(load["held_by_count"], 0, "{records:#?}");
}

/// #797 R2 (2), load path: a navigation cancel between two scripts gives
/// the rest one `cancelled` record each, and no lifecycle event fires.
#[test]
fn a_cancel_between_scripts_records_the_rest_as_cancelled() {
    let page = r#"<html><head>
<script>var one = 1;</script>
<script>var two = 2;</script>
<script type="module" src="/m.js"></script>
</head><body>hi</body></html>"#;
    let server = serve(vec![Route::html("/", page), Route::ok("/m.js", "text/javascript", "window.m = 1;")]);
    let (mut engine, view) = engine(EngineConfig::default());
    let canceller = engine.nav_canceller(view).unwrap();
    nav_cancel::LOAD_PHASE_HOOK.with(|h| {
        *h.borrow_mut() = Some(Box::new(move |phase: &str| {
            if phase == "script:1" {
                assert!(canceller.cancel_current());
            }
        }))
    });
    runtime().block_on(engine.load_url(view, server.url("/"))).unwrap();
    nav_cancel::LOAD_PHASE_HOOK.with(|h| *h.borrow_mut() = None);
    assert_eq!(
        lifecycle_lines(&records(&engine, view), server.port),
        vec![
            "script parser inline#1 ran held=DOMContentLoaded event=-",
            "script parser inline#2 cancelled (navigation or view closed) held=- event=-",
            "script module /m.js cancelled (navigation or view closed) held=- event=-",
        ]
    );
}

/// #797 R2 (2), inserted path: a script whose fetch is still out when the
/// page navigates away gets one `cancelled` record, not none.
#[test]
fn an_inserted_script_still_fetching_at_navigation_is_cancelled() {
    let page = r#"<html><body><p>first</p><script>
setTimeout(function () {
  var s = document.createElement('script');
  s.src = '/slow.js';
  document.head.appendChild(s);
}, 0);
</script></body></html>"#;
    let server = serve(vec![
        Route::html("/", page),
        Route::ok("/slow.js", "text/javascript", "window.slow = 1;").delay(2000),
        Route::html("/next", "<html><body>next</body></html>"),
    ]);
    let (mut engine, view) = engine(EngineConfig { script_budget_ms: 200, ..EngineConfig::default() });
    let rt = runtime();
    rt.block_on(engine.load_url(view, server.url("/"))).expect("first load");
    rt.block_on(engine.load_url(view, server.url("/next"))).expect("second load");
    let slow: Vec<_> = json(&records(&engine, view))
        .into_iter()
        .filter(|r| r["type"] == "script" && r["script"].as_str().is_some_and(|s| s.ends_with("/slow.js")))
        .collect();
    assert_eq!(slow.len(), 1, "{slow:#?}");
    assert_eq!(slow[0]["outcome"], "cancelled", "{slow:#?}");
    assert_eq!(slow[0]["kind"], "inserted", "{slow:#?}");
    assert!(slow[0]["held"].is_null() && slow[0]["queued_ms"].is_f64(), "{slow:#?}");
}

/// Prometheus #797 R1 (1) with R2 (3): the lifecycle flags are kept with
/// nobody listening, so a panel attached after `DOMContentLoaded` reads a
/// later script as ending before `load`, not before `DOMContentLoaded`.
#[test]
fn a_panel_attached_after_dom_content_loaded_reads_held_load() {
    let inspector = Inspector::new(&InspectorConfig::default());
    assert!(!inspector.is_active());
    inspector.navigation_started(None);
    inspector.lifecycle("DOMContentLoaded");
    inspector.set_buffer_enabled(true);
    inspector.script_ended(inspector::ScriptEnd {
        source: "inline#3",
        kind: "parser",
        outcome: &ScriptOutcome::Ran,
        started: true,
        bytes: 1,
        queued: None,
        fetch: None,
        waited: None,
        ran_ms: 0,
        event: None,
    });
    let records = json(&inspector.snapshot_since(0).records);
    let script = records.iter().find(|r| r["type"] == "script").expect("script record");
    assert_eq!(script["held"], "load", "{records:#?}");
}
