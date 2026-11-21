//! Each path that lays a view out again records why (`relayout_cause`):
//! the load, its sheets, fonts and images, script writes and what ran the
//! script (a scroll, a timer, an input event, an observer), a resize, a
//! hover. Diagnostics only: these tests read `Engine::relayout_stats` and
//! change nothing that is laid out.

use super::*;
use crate::script_net_tests::serve_routes;

fn engine_and_view() -> (Engine, EngineViewId) {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds {
            x: 0,
            y: 0,
            width: 400,
            height: 200,
        })
        .expect("view");
    (engine, view)
}

fn load(
    routes: Vec<(&'static str, &'static str, String)>,
) -> (
    Engine,
    EngineViewId,
    crate::script_net_tests::Server,
    tokio::runtime::Runtime,
) {
    let server = serve_routes(routes);
    let (mut engine, view) = engine_and_view();
    let url = Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(engine.load_url(view, url)).expect("load_url");
    (engine, view, server, rt)
}

fn stats(engine: &Engine, view: EngineViewId) -> RelayoutStats {
    engine.relayout_stats(view).expect("view").clone()
}

fn first(engine: &Engine, view: EngineViewId, kind: &str) -> u32 {
    stats(engine, view).by_first_cause.get(kind)
}

fn last(engine: &Engine, view: EngineViewId) -> Option<RelayoutCause> {
    stats(engine, view).last
}

/// A target to write to, 1000 px of spacer under a 200 px viewport, then a
/// box below the fold.
const TALL: &str = "<html><head><style>body{margin:0} #spacer{height:1000px} #below{height:50px}</style></head><body>\
    <div id=t></div><div id=spacer></div><div id=below></div><script>SCRIPT</script></body></html>";

fn tall(script: &str) -> Vec<(&'static str, &'static str, String)> {
    vec![("/", "text/html", TALL.replace("SCRIPT", script))]
}

#[test]
fn load_html_is_an_initial_load() {
    let (mut engine, view) = engine_and_view();
    engine.load_html(view, "<p>hello</p>").expect("load_html");
    let s = stats(&engine, view);
    assert_eq!(s.relayouts, 1);
    assert_eq!(s.last, Some(RelayoutCause::InitialLoad));
    assert_eq!(s.by_first_cause.to_string(), "{initial_load:1}");
}

#[test]
fn a_relayout_nobody_named_is_unattributed() {
    let (mut engine, view) = engine_and_view();
    engine.load_html(view, "<p>hello</p>").expect("load_html");
    engine.relayout(view).expect("relayout");
    assert_eq!(
        last(&engine, view),
        Some(RelayoutCause::Other("unattributed"))
    );
    engine
        .relayout_for(view, RelayoutCause::Input)
        .expect("relayout");
    assert_eq!(last(&engine, view), Some(RelayoutCause::Input));
    assert_eq!(stats(&engine, view).relayouts, 3);
}

#[test]
fn a_script_write_names_its_kind_and_node() {
    let (mut engine, view) = engine_and_view();
    engine
        .load_html(view, "<div id=a></div><p>x</p>")
        .expect("load_html");
    engine
        .execute_script(
            view,
            "var a = document.getElementById('a'); a.setAttribute('data-x', '1'); \
             a.style.width = '10px'; a.appendChild(document.createElement('span'));",
        )
        .expect("script");
    assert_eq!(
        last(&engine, view),
        Some(RelayoutCause::ScriptMutation {
            kind: MutationKind::Attribute,
            node: "div#a".into()
        })
    );
    let s = stats(&engine, view);
    assert_eq!(s.by_cause.get("script_mutation"), 3);
    assert_eq!(
        s.mutations.to_string(),
        "{attribute:1,child_list:1,style:1}"
    );

    // A script that writes nothing lays nothing out and records nothing.
    let before = stats(&engine, view).relayouts;
    engine.execute_script(view, "1 + 1").expect("script");
    assert_eq!(stats(&engine, view).relayouts, before);
}

/// #655: a control `.value` write must land under `form_value` in
/// `RelayoutStats.mutations`, not as a generic attribute/style write — W7-C
/// attribution depends on the engine rollup matching the bindings tally.
#[test]
fn a_form_value_write_is_counted_under_form_value() {
    let (mut engine, view) = engine_and_view();
    engine
        .load_html(view, r#"<input id="i" value="a"><input id="c" type="checkbox">"#)
        .expect("load_html");
    engine
        .execute_script(view, "document.getElementById('i').value = 'typed';")
        .expect("script");
    assert_eq!(
        last(&engine, view),
        Some(RelayoutCause::ScriptMutation {
            kind: MutationKind::FormValue,
            node: "input#i".into()
        })
    );
    let s = stats(&engine, view);
    assert!(
        s.mutations.get("form_value") >= 1,
        "form_value missing from mutations: {}",
        s.mutations
    );
    assert_eq!(s.by_cause.get("script_mutation"), 1);

    engine
        .execute_script(view, "document.getElementById('c').checked = true;")
        .expect("checked");
    assert_eq!(
        last(&engine, view),
        Some(RelayoutCause::ScriptMutation {
            kind: MutationKind::FormValue,
            node: "input#c".into()
        })
    );
    assert!(
        stats(&engine, view).mutations.get("form_value") >= 2,
        "checked write must also tally as form_value: {}",
        stats(&engine, view).mutations
    );
}

#[test]
fn a_scroll_listener_write_is_a_scroll() {
    let (mut engine, view, _server, _rt) = load(tall(
        "window.addEventListener('scroll', function () { \
           document.getElementById('t').textContent = 'y=' + scrollY; });",
    ));
    let before = first(&engine, view, "scroll");
    assert!(engine.scroll_view(view, 0.0, -50.0).expect("scroll"));
    assert_eq!(first(&engine, view, "scroll"), before + 1);
    assert_eq!(last(&engine, view), Some(RelayoutCause::Scroll));
    assert!(stats(&engine, view).mutations.get("text") >= 1);
}

#[test]
fn a_scroll_with_no_write_lays_nothing_out() {
    let (mut engine, view, _server, _rt) = load(tall(""));
    let before = stats(&engine, view).relayouts;
    assert!(engine.scroll_view(view, 0.0, -50.0).expect("scroll"));
    assert_eq!(stats(&engine, view).relayouts, before);
}

#[test]
fn an_observer_callback_write_is_counted_under_its_trigger() {
    let (mut engine, view, _server, _rt) = load(tall(
        "new IntersectionObserver(function (es) { es.forEach(function (e) { \
           if (e.isIntersecting) document.getElementById('t').setAttribute('data-seen', '1'); \
         }); }).observe(document.getElementById('below'));",
    ));
    let before = stats(&engine, view).by_cause.get("observer_callback");
    // `#below` scrolls into view: the observer reports it and its callback writes.
    assert!(engine.scroll_view(view, 0.0, -900.0).expect("scroll"));
    let s = stats(&engine, view);
    assert!(s.by_cause.get("observer_callback") > before, "{s:?}");
    assert_eq!(
        s.last,
        Some(RelayoutCause::Scroll),
        "the scroll ran the observer"
    );
}

#[test]
fn a_timer_write_in_a_live_turn_is_a_timer() {
    let (mut engine, view, _server, rt) = load(tall(
        // Past the load's 5 s timer horizon: only a live turn runs it.
        "setTimeout(function () { document.getElementById('t').textContent = 'later'; }, 8000);",
    ));
    let before = first(&engine, view, "timer");
    rt.block_on(engine.pump_live(view, 10_000));
    assert_eq!(first(&engine, view, "timer"), before + 1);
    assert_eq!(last(&engine, view), Some(RelayoutCause::Timer));
}

#[test]
fn a_live_resize_is_a_resize() {
    let (mut engine, view, _server, rt) = load(tall(""));
    engine
        .set_view_bounds(
            view,
            Bounds {
                x: 0,
                y: 0,
                width: 500,
                height: 200,
            },
        )
        .expect("bounds");
    rt.block_on(engine.pump_live(view, 0));
    assert_eq!(first(&engine, view, "resize"), 1);
    engine
        .resize_view(
            view,
            Bounds {
                x: 0,
                y: 0,
                width: 300,
                height: 200,
            },
        )
        .expect("resize");
    assert_eq!(first(&engine, view, "resize"), 2);
    assert_eq!(last(&engine, view), Some(RelayoutCause::Resize));
}

#[test]
fn a_hover_a_sheet_reads_is_hover_or_focus() {
    let (mut engine, view) = engine_and_view();
    engine
        .load_html(
            view,
            "<style>body{margin:0} #h{height:50px} #h:hover{height:80px}</style><div id=h></div>",
        )
        .expect("load_html");
    let before = first(&engine, view, "hover_or_focus");
    engine.mouse_move_at_point(view, 10.0, 10.0);
    assert_eq!(first(&engine, view, "hover_or_focus"), before + 1);
    assert_eq!(last(&engine, view), Some(RelayoutCause::HoverOrFocus));
}

#[test]
fn a_click_listener_write_is_input() {
    let (mut engine, view, _server, _rt) = load(vec![(
        "/",
        "text/html",
        "<html><body style='margin:0'><div id=b style='height:50px'></div><script>\
         document.getElementById('b').addEventListener('click', function () { \
           this.textContent = 'clicked'; });</script></body></html>"
            .to_string(),
    )]);
    let before = first(&engine, view, "input");
    engine.click_at_point(view, 10.0, 10.0);
    assert!(
        first(&engine, view, "input") > before,
        "{:?}",
        stats(&engine, view)
    );
}

#[test]
fn a_load_with_a_stylesheet_records_the_initial_load_and_the_sheet() {
    let (engine, view, _server, _rt) = load(vec![
        (
            "/",
            "text/html",
            "<html><head><link rel=stylesheet href=/s.css></head><body><p>x</p></body></html>"
                .to_string(),
        ),
        ("/s.css", "text/css", "p { margin: 3px }".to_string()),
    ]);
    let s = stats(&engine, view);
    // The sheet is render-blocking: the first layout waits for it, so one
    // relayout is both the initial load and the sheet.
    assert_eq!(s.by_first_cause.get("initial_load"), 1, "{s:?}");
    assert_eq!(s.by_cause.get("stylesheet_loaded"), 1, "{s:?}");
}

#[test]
fn a_load_with_an_image_records_the_image() {
    let svg = "<svg xmlns='http://www.w3.org/2000/svg' width='20' height='20'>\
               <rect width='20' height='20' fill='red'/></svg>";
    let (engine, view, _server, _rt) = load(vec![
        (
            "/",
            "text/html",
            "<html><body><img src=/i.svg></body></html>".to_string(),
        ),
        ("/i.svg", "image/svg+xml", svg.to_string()),
    ]);
    let s = stats(&engine, view);
    assert_eq!(s.by_cause.get("image_loaded"), 1, "{s:?}");
    assert_eq!(
        s.last,
        Some(RelayoutCause::ImageLoaded { count: 1 }),
        "{s:?}"
    );
}

#[test]
fn a_new_document_starts_its_own_counts() {
    let (mut engine, view) = engine_and_view();
    engine
        .load_html(view, "<div id=a></div>")
        .expect("load_html");
    engine
        .execute_script(view, "document.getElementById('a').textContent = 'x';")
        .expect("script");
    assert_eq!(stats(&engine, view).relayouts, 2);
    engine.load_html(view, "<p>next</p>").expect("load_html");
    let s = stats(&engine, view);
    assert_eq!(s.relayouts, 1, "{s:?}");
    assert_eq!(s.by_first_cause.to_string(), "{initial_load:1}");
    assert!(s.mutations.is_empty());
}

/// Collects what a `fmt` subscriber writes, for reading the log lines back.
#[derive(Clone, Default)]
struct Captured(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for Captured {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Log lines that contain `needle` while `run` ran on this thread.
pub(crate) fn capture_lines(needle: &str, run: impl FnOnce()) -> Vec<String> {
    let captured = Captured::default();
    let writer = captured.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_max_level(tracing::Level::INFO)
        .with_writer(move || writer.clone())
        .finish();
    tracing::subscriber::with_default(subscriber, || {
        // A callsite first reached on another test's thread, with no
        // subscriber there, caches "never" and skips this one (#967 R1):
        // recompute every callsite's interest under this subscriber.
        tracing::callsite::rebuild_interest_cache();
        run()
    });
    let log = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
    log.lines()
        .filter(|line| line.contains(needle))
        .map(str::to_string)
        .collect()
}

/// The `relayout cause=` lines logged while `run` ran on this thread.
pub(crate) fn relayout_lines(run: impl FnOnce()) -> Vec<String> {
    capture_lines("relayout cause=", run)
}

/// The number after `name=` in a log line.
pub(crate) fn field_ms(line: &str, name: &str) -> Option<f64> {
    let at = line.find(&format!(" {name}="))? + name.len() + 2;
    line[at..].split_whitespace().next()?.parse().ok()
}

#[test]
fn every_relayout_line_carries_the_five_stage_times() {
    let (mut engine, view) = engine_and_view();
    let lines = relayout_lines(|| {
        engine
            .load_html(view, "<style>p{color:red}</style><p>hello</p>")
            .expect("load_html");
        engine.relayout(view).expect("relayout");
    });
    assert_eq!(lines.len(), 2, "one line per relayout: {lines:#?}");
    for line in &lines {
        let took = field_ms(line, "took_ms").expect("took_ms");
        let mut sum = 0.0;
        for stage in [
            "build_ms",
            "layout_ms",
            "display_list_ms",
            "snapshot_ms",
            "render_ms",
        ] {
            let ms = field_ms(line, stage).unwrap_or_else(|| panic!("{stage} missing: {line}"));
            assert!(ms >= 0.0, "{stage} in {line}");
            sum += ms;
        }
        // Each is rounded to 0.1 ms: five roundings, and the stages run
        // inside the relayout that took_ms times.
        assert!(sum <= took + 0.3, "stages sum past took_ms: {line}");
    }
}

/// #655: navigation summaries must go through `summary_url`, not the raw
/// document URL — query/tokens and userinfo must not land in the log.
#[test]
fn a_navigation_summary_scrubs_the_outgoing_url() {
    let (mut engine, view) = engine_and_view();
    engine
        .load_html(view, "<div id=a></div>")
        .expect("load_html");
    // A URL a real navigation would store: credentials and a token in the
    // query. The summary path must scrub both before logging.
    engine.views.get_mut(&view).expect("view").url = Some(
        Url::parse("https://user:pass@example.com/secret?token=leak-me#frag").unwrap(),
    );
    engine
        .execute_script(view, "document.getElementById('a').textContent = 'x';")
        .expect("script");

    let lines = capture_lines("relayout summary url=", || {
        engine.load_html(view, "<p>next</p>").expect("navigate away");
    });
    assert_eq!(lines.len(), 1, "one summary for the outgoing document: {lines:#?}");
    let line = &lines[0];
    assert!(
        line.contains("relayout summary url=https://example.com/secret "),
        "origin+path only: {line}"
    );
    for leak in ["token=", "leak-me", "user:", "pass@", "#frag"] {
        assert!(
            !line.contains(leak),
            "summary must not contain {leak:?}: {line}"
        );
    }
}

/// LAT1: the hold summary line covers every entry by name and carries no
/// page content; the engine's drop logs the last visit's.
#[test]
fn the_hold_summary_line_names_entries_and_milliseconds_only() {
    // Read through `take_last_summary` (this thread's last line), not the
    // log capture, so another test's tracing state cannot hide it. The
    // summary is the content thread's, so the engine runs on one.
    let line = on_page_thread(|| {
        crate::wgate::flush_hold_summary("drop");
        let _ = crate::wgate::take_last_summary();
        let (mut engine, view) = engine_and_view();
        engine
            .load_html(view, "<p id=secretid>private words</p>")
            .expect("load_html");
        for _ in 0..3 {
            engine
                .execute_script(view, "document.getElementById('secretid').textContent")
                .unwrap();
        }
        drop(engine);
        crate::wgate::take_last_summary()
    });
    let line = line.expect("the drop logs the visit's summary");
    assert!(line.contains(" at=drop"), "{line}");
    assert!(line.contains("load_html:1:"), "{line}");
    assert!(line.contains("script:3:"), "{line}");
    for field in ["p50_ms", "p95_ms", "max_ms"] {
        assert!(field_ms(&line, field).is_some(), "{field}: {line}");
    }
    for leak in ["secretid", "private", "words"] {
        assert!(!line.contains(leak), "{leak}: {line}");
    }
}

/// Runs `f` on a thread named as the content thread is (`page`).
fn on_page_thread<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::Builder::new()
        .name("page".into())
        .stack_size(64 * 1024 * 1024)
        .spawn(f)
        .expect("spawn")
        .join()
        .expect("join")
}

/// LAT1b: the hold summary is one line per content visit, tagged with the
/// view and the host (hostname only), so a log groups it by site: a
/// navigation ends the first visit's line and the drop the second's, each
/// with that visit's entries only.
#[test]
fn the_hold_summary_is_one_line_per_content_visit_with_its_view_and_host() {
    let server = serve_routes(vec![("/secretpath?q=1", "text/html", "<p>two</p>".to_string())]);
    let port = server.port;
    let (view, first, second) = on_page_thread(move || {
        crate::wgate::flush_hold_summary("drop");
        let _ = crate::wgate::take_last_summary();
        let (mut engine, view) = engine_and_view();
        engine.load_html(view, "<p>one</p>").expect("load_html");
        for _ in 0..2 {
            engine.execute_script(view, "1").expect("script");
        }
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let url = Url::parse(&format!("http://127.0.0.1:{port}/secretpath?q=1")).unwrap();
        rt.block_on(engine.load_url(view, url)).expect("load_url");
        let first = crate::wgate::take_last_summary();
        engine.execute_script(view, "2").expect("script");
        drop(engine);
        let second = crate::wgate::take_last_summary();
        (view.raw(), first, second)
    });
    let first = first.expect("the navigation ends the first visit");
    assert!(
        first.starts_with(&format!("Content thread holds view={view} host=- n=3 ")),
        "{first}"
    );
    assert!(first.contains("load_html:1:") && first.contains("script:2:"), "{first}");
    assert!(first.ends_with(" at=navigation"), "{first}");
    let second = second.expect("the drop ends the second visit");
    assert!(
        second.starts_with(&format!("Content thread holds view={view} host=127.0.0.1 n=2 ")),
        "{second}"
    );
    assert!(second.contains("load:1:") && second.contains("script:1:"), "{second}");
    assert!(second.ends_with(" at=drop"), "{second}");
    for leak in ["secretpath", "q=1", "http"] {
        assert!(!second.contains(leak), "{leak}: {second}");
    }
}

/// LAT1b: entries on the window thread (a headless capture's load and
/// render, the flag-off page) are no content visit: the drop logs no hold
/// summary for them, so a log's metric is never the whole-load hold.
#[test]
fn the_window_thread_logs_no_hold_summary() {
    crate::wgate::flush_hold_summary("drop");
    let _ = crate::wgate::take_last_summary();
    let (mut engine, view) = engine_and_view();
    engine.load_html(view, "<p>one</p>").expect("load_html");
    engine.execute_script(view, "1").expect("script");
    drop(engine);
    assert_eq!(crate::wgate::take_last_summary(), None);
}

/// LAT1: the relayout line names the sheet and custom-property extraction
/// (`extract_ms`), which `build_ms` does not include.
#[test]
fn the_relayout_line_carries_the_sheet_extraction_time() {
    let vars: String = (0..3000).map(|i| format!("--v{i}:{i}px;")).collect();
    let page = format!("<style>:root{{{vars}}} p{{width:var(--v7)}}</style><p>x</p>");
    let (mut engine, view) = engine_and_view();
    let lines = relayout_lines(|| {
        engine.load_html(view, &page).expect("load_html");
    });
    let line = lines.last().expect("a relayout line");
    let extract = field_ms(line, "extract_ms").expect("extract_ms");
    let took = field_ms(line, "took_ms").expect("took_ms");
    assert!(extract > 0.0, "3,000 custom properties take time to extract: {line}");
    assert!(extract <= took, "{line}");
}
