//! Cooperative load cancel (threads plan M3b): a [`NavCanceller`] on
//! another thread cancels a load that the page thread is running, at
//! whatever point it has reached. Nothing of the cancelled load reaches the
//! document or the frame afterwards, and the next navigation loads
//! normally.

use super::*;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

/// 640 x 480 JPEG.
const JPEG: &[u8] =
    include_bytes!("../../../websuite/js-ladder/01-mdn/32-image-gallery/images/pic4.jpg");

/// The acceptance bound: a cancel ends the load this soon.
const CANCEL_BOUND: Duration = Duration::from_millis(200);

/// A loopback server. `/` and the other fixed routes answer at once;
/// `/held/...` waits until [`Gate::open`]; `/slow/...` after 100 ms. A path
/// ending `.jpg` is the JPEG, `.css` a sheet that colours the body.
struct Server {
    port: u16,
    gate: Arc<Gate>,
    /// Requests received for `/held/...` paths.
    held_requested: Arc<AtomicUsize>,
    /// Responses written for `/held/...` paths.
    held_answered: Arc<AtomicUsize>,
}

#[derive(Default)]
struct Gate {
    open: Mutex<bool>,
    opened: Condvar,
}

impl Gate {
    fn open(&self) {
        *self.open.lock().unwrap() = true;
        self.opened.notify_all();
    }

    fn wait(&self) {
        let mut open = self.open.lock().unwrap();
        while !*open {
            open = self.opened.wait(open).unwrap();
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        // Releases the connection threads still parked on the gate.
        self.gate.open();
    }
}

fn serve(pages: Vec<(&'static str, String)>) -> Server {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let gate = Arc::new(Gate::default());
    let held_requested = Arc::new(AtomicUsize::new(0));
    let held_answered = Arc::new(AtomicUsize::new(0));
    let pages = Arc::new(pages);
    let (server_gate, requested, answered) =
        (gate.clone(), held_requested.clone(), held_answered.clone());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let (gate, requested, answered, pages) = (
                server_gate.clone(),
                requested.clone(),
                answered.clone(),
                pages.clone(),
            );
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
                let held = path.starts_with("/held/");
                if held {
                    requested.fetch_add(1, Ordering::SeqCst);
                    gate.wait();
                } else if path.starts_with("/slow/") {
                    std::thread::sleep(Duration::from_millis(100));
                }
                let (content_type, body): (&str, Vec<u8>) =
                    if let Some((_, page)) = pages.iter().find(|(p, _)| *p == path) {
                        ("text/html", page.clone().into_bytes())
                    } else if path.ends_with(".jpg") {
                        ("image/jpeg", JPEG.to_vec())
                    } else if path.ends_with(".css") {
                        ("text/css", b"body { background: rgb(0, 128, 0); }".to_vec())
                    } else {
                        ("text/plain", path.clone().into_bytes())
                    };
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(&body);
                if held {
                    answered.fetch_add(1, Ordering::SeqCst);
                }
            });
        }
    });
    Server {
        port,
        gate,
        held_requested,
        held_answered,
    }
}

impl Server {
    fn url(&self, path: &str) -> Url {
        Url::parse(&format!("http://127.0.0.1:{}{path}", self.port)).unwrap()
    }
}

/// Waits until the server holds `count` requests: the load is then
/// waiting on them, past its first layout (seconds in a debug build, and
/// a cancel cannot end a synchronous layout early).
fn wait_until_held(requested: &AtomicUsize, count: usize) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while requested.load(Ordering::SeqCst) < count {
        assert!(
            Instant::now() < deadline,
            "the load never asked for its subresources"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    std::thread::sleep(Duration::from_millis(20));
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

/// The app's script settings, with budgets far past the test so that only
/// the cancel can stop a spin.
fn config() -> EngineConfig {
    EngineConfig {
        interrupt_scripts_at_budget: true,
        script_budget_ms: 60_000,
        live_turn_budget_ms: 60_000,
        script_loop_iteration_limit: u64::MAX / 2,
        ..EngineConfig::default()
    }
}

fn engine_and_view() -> (Engine, EngineViewId) {
    let mut engine = Engine::new(config()).expect("engine");
    let view = engine
        .create_headless_view(Bounds {
            x: 0,
            y: 0,
            width: 400,
            height: 300,
        })
        .expect("view");
    (engine, view)
}

/// A test whose cancel fails would spin for good: end the process instead.
fn hang_guard() -> Arc<AtomicBool> {
    let done = Arc::new(AtomicBool::new(false));
    let watched = done.clone();
    std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            if watched.load(Ordering::SeqCst) {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        eprintln!("load_cancel_tests: a cancelled load was still running after 30 s");
        std::process::abort();
    });
    done
}

/// Sets the load phase hook for the life of the guard.
struct PhaseHook;

impl PhaseHook {
    fn set(hook: impl FnMut(&str) + 'static) -> Self {
        nav_cancel::LOAD_PHASE_HOOK.with(|h| *h.borrow_mut() = Some(Box::new(hook)));
        PhaseHook
    }
}

impl Drop for PhaseHook {
    fn drop(&mut self) {
        nav_cancel::LOAD_PHASE_HOOK.with(|h| *h.borrow_mut() = None);
    }
}

fn events(rx: &mut mpsc::UnboundedReceiver<EngineEvent>) -> Vec<EngineEvent> {
    std::iter::from_fn(|| rx.try_recv().ok()).collect()
}

fn page_loaded(events: &[EngineEvent], url: &Url) -> bool {
    events
        .iter()
        .any(|e| matches!(e, EngineEvent::PageLoaded { url: u, .. } if u == url))
}

fn cancelled(events: &[EngineEvent]) -> bool {
    events
        .iter()
        .any(|e| matches!(e, EngineEvent::NavigationFailed { error, .. } if error == "cancelled"))
}

fn sources(engine: &Engine, view: EngineViewId) -> Vec<(String, ScriptOutcome)> {
    engine
        .script_log(view)
        .unwrap()
        .iter()
        .map(|r| (r.source.clone(), r.outcome.clone()))
        .collect()
}

/// The page after a cancelled load: a plain one with a script that runs.
const NEXT: &str = "<html><head><title>next</title></head><body><p>next</p>\
<img src=/next.jpg><script>document.title = 'next ran';</script></body></html>";

/// The next navigation after a cancel loads normally: its script runs, its
/// image arrives, and it finishes.
fn assert_next_navigation_loads(
    engine: &mut Engine,
    view: EngineViewId,
    server: &Server,
    rx: &mut mpsc::UnboundedReceiver<EngineEvent>,
    rt: &tokio::runtime::Runtime,
) {
    let next = server.url("/next");
    rt.block_on(engine.load_url(view, next.clone()))
        .expect("the next navigation loads");
    let seen = events(rx);
    assert!(page_loaded(&seen, &next), "{seen:?}");
    assert!(!cancelled(&seen), "{seen:?}");
    assert_eq!(engine.get_url(view), Some(next));
    assert!(
        sources(engine, view)
            .iter()
            .any(|(s, o)| s == "inline#1" && *o == ScriptOutcome::Ran),
        "{:?}",
        sources(engine, view)
    );
    assert_eq!(
        engine.execute_script(view, "document.title").unwrap(),
        "String(\"next ran\")",
        "the cancel's stop of all script ended with its document"
    );
    assert!(engine.is_image_cached(&server.url("/next.jpg")));
}

/// Where the acceptance test's cancel lands.
#[derive(Clone, Copy, Debug)]
enum Point {
    /// While the twenty subresources are held by the server.
    SubresourcesHeld,
    /// While the spinning script runs, the subresources in.
    Spinning,
}

/// The plan's acceptance for M3b: a load with a spinning script and twenty
/// slow subresources is cancelled within 200 ms, and the next navigation
/// then loads normally. A load's scripts run after its subresources here,
/// so the spin and the twenty fetches are never in flight together: the
/// cancel is sent at each point in turn.
fn cancel_a_spinning_load_with_twenty_slow_subresources(point: Point) {
    let prefix = match point {
        Point::SubresourcesHeld => "/held",
        Point::Spinning => "/slow",
    };
    let mut page = String::from("<html><body><p>spin</p>");
    for i in 0..20 {
        page.push_str(&format!("<img src={prefix}/{i}.jpg width=10 height=10>"));
    }
    page.push_str("<script>window.spun = true; while (true) {}</script>");
    page.push_str("<script>document.title = 'after the spin';</script></body></html>");
    let server = serve(vec![("/", page), ("/next", NEXT.to_string())]);
    let (mut engine, view) = engine_and_view();
    let mut rx = engine.take_event_receiver().unwrap();
    let rt = runtime();
    let done = hang_guard();

    // The script is about to run: the canceller waits for that, then for
    // the spin to be under way.
    let script_started = Arc::new(AtomicBool::new(false));
    let started = script_started.clone();
    let _hook = PhaseHook::set(move |phase| {
        if phase == "script:0" {
            started.store(true, Ordering::SeqCst);
        }
    });
    let canceller = engine.nav_canceller(view).unwrap();
    let requested = server.held_requested.clone();
    let canceller_thread = std::thread::spawn(move || {
        match point {
            // Eight at a time (the engine's image concurrency): eight held
            // by the server, the other twelve queued behind them.
            Point::SubresourcesHeld => wait_until_held(&requested, 8),
            Point::Spinning => {
                while !script_started.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(5));
                }
                std::thread::sleep(Duration::from_millis(200));
            }
        }
        let loading = canceller.loading_nav();
        assert!(loading != 0, "the load is still in progress at the cancel");
        assert!(
            canceller.cancel_current(),
            "a load in progress was cancelled"
        );
        Instant::now()
    });

    rt.block_on(engine.load_url(view, server.url("/")))
        .expect("a cancelled load is not an error");
    let returned = Instant::now();
    let sent = canceller_thread.join().expect("canceller");
    let took = returned.saturating_duration_since(sent);
    assert!(
        took < CANCEL_BOUND,
        "{point:?}: the load ended {took:?} after the cancel"
    );

    let seen = events(&mut rx);
    assert!(cancelled(&seen), "{point:?}: {seen:?}");
    assert!(!page_loaded(&seen, &server.url("/")), "{point:?}: {seen:?}");
    let log = sources(&engine, view);
    assert!(
        !log.iter().any(|(s, _)| s == "inline#2"),
        "{point:?}: no script after the cancel ran: {log:?}"
    );
    match point {
        Point::SubresourcesHeld => {
            assert!(log.is_empty(), "the spin never started: {log:?}");
            assert_eq!(server.held_answered.load(Ordering::SeqCst), 0);
        }
        Point::Spinning => {
            assert!(
                log.iter().any(|(s, _)| s == "inline#1"),
                "the spin was stopped, not skipped: {log:?}"
            );
        }
    }
    drop(_hook);
    assert_next_navigation_loads(&mut engine, view, &server, &mut rx, &rt);
    done.store(true, Ordering::SeqCst);
}

#[test]
fn a_load_held_on_twenty_slow_subresources_is_cancelled_within_200_ms() {
    cancel_a_spinning_load_with_twenty_slow_subresources(Point::SubresourcesHeld);
}

#[test]
fn a_load_spinning_in_its_script_is_cancelled_within_200_ms() {
    cancel_a_spinning_load_with_twenty_slow_subresources(Point::Spinning);
}

#[test]
fn a_cancel_during_the_parse_leaves_the_shown_document_in_place() {
    let first = "<html><head><title>first</title></head><body><p>first</p></body></html>";
    let second = "<html><head><title>second</title></head><body><p>second</p>\
<script>document.title = 'second ran';</script></body></html>";
    let server = serve(vec![
        ("/first", first.to_string()),
        ("/second", second.to_string()),
        ("/next", NEXT.to_string()),
    ]);
    let (mut engine, view) = engine_and_view();
    let mut rx = engine.take_event_receiver().unwrap();
    let rt = runtime();
    rt.block_on(engine.load_url(view, server.url("/first")))
        .unwrap();
    let first_document = engine.views[&view].document.clone().unwrap();
    let layouts = engine.relayout_stats(view).unwrap().clone();
    let _ = events(&mut rx);

    let canceller = engine.nav_canceller(view).unwrap();
    let hook = PhaseHook::set(move |phase| {
        if phase == "parse" {
            assert!(canceller.cancel_current());
        }
    });
    rt.block_on(engine.load_url(view, server.url("/second")))
        .unwrap();
    drop(hook);

    let seen = events(&mut rx);
    assert!(cancelled(&seen), "{seen:?}");
    assert!(!page_loaded(&seen, &server.url("/second")), "{seen:?}");
    let shown = engine.views[&view].document.clone().unwrap();
    assert!(
        Rc::ptr_eq(&shown, &first_document),
        "the parsed document was not installed"
    );
    assert_eq!(engine.get_url(view), Some(server.url("/first")));
    assert_eq!(engine.get_title(view).as_deref(), Some("first"));
    assert_eq!(
        engine.relayout_stats(view).unwrap(),
        &layouts,
        "nothing of the cancelled load was laid out"
    );
    assert_next_navigation_loads(&mut engine, view, &server, &mut rx, &rt);
}

/// #799 (T8) review: `load_html` runs inline scripts now, so a navigation
/// away during them must stop it as on the URL path: the rest of the
/// scripts and both lifecycle events do not run on the abandoned document,
/// and it fails as cancelled.
#[test]
fn a_cancel_between_two_scripts_of_an_html_load_runs_neither_the_second_nor_the_lifecycle() {
    let page = "<html><body><p id=p>two</p>\
<script>var p = document.getElementById('p'); p.setAttribute('data-step', 'one');\
document.addEventListener('DOMContentLoaded', function () { p.setAttribute('data-dcl', 'fired'); });\
window.addEventListener('load', function () { p.setAttribute('data-load', 'fired'); });</script>\
<script>document.getElementById('p').setAttribute('data-step', 'two');</script>\
</body></html>";
    let (mut engine, view) = engine_and_view();
    let mut rx = engine.take_event_receiver().unwrap();
    let canceller = engine.nav_canceller(view).unwrap();
    let hook = PhaseHook::set(move |phase| {
        if phase == "html-script:1" {
            assert!(canceller.cancel_current());
        }
    });
    engine.load_html(view, page).unwrap();
    drop(hook);

    assert_eq!(sources(&engine, view), vec![("inline#1".to_string(), ScriptOutcome::Ran)]);
    let p = engine.views[&view].document.as_ref().unwrap().get_element_by_id("p").unwrap();
    assert_eq!(p.get_attribute("data-step"), Some("one"), "the second script did not run");
    assert_eq!(p.get_attribute("data-dcl"), None, "DOMContentLoaded fired on the abandoned document");
    assert_eq!(p.get_attribute("data-load"), None, "load fired on the abandoned document");
    let seen = events(&mut rx);
    assert!(cancelled(&seen), "{seen:?}");
    assert!(!seen.iter().any(|e| matches!(e, EngineEvent::PageLoaded { .. })), "{seen:?}");
}

#[test]
fn a_cancel_between_two_scripts_runs_neither_the_second_nor_the_lifecycle() {
    let page = "<html><body><p id=p>two</p>\
<script>document.getElementById('p').setAttribute('data-step', 'one');</script>\
<script>document.getElementById('p').setAttribute('data-step', 'two');</script>\
<script>window.addEventListener('load', function () { \
document.getElementById('p').setAttribute('data-step', 'loaded'); });</script>\
</body></html>";
    let server = serve(vec![("/", page.to_string()), ("/next", NEXT.to_string())]);
    let (mut engine, view) = engine_and_view();
    let mut rx = engine.take_event_receiver().unwrap();
    let rt = runtime();
    let canceller = engine.nav_canceller(view).unwrap();
    let hook = PhaseHook::set(move |phase| {
        if phase == "script:1" {
            assert!(canceller.cancel_current());
        }
    });
    rt.block_on(engine.load_url(view, server.url("/"))).unwrap();
    drop(hook);

    let log = sources(&engine, view);
    assert_eq!(
        log,
        vec![("inline#1".to_string(), ScriptOutcome::Ran)],
        "the first script ran, nothing after it"
    );
    let p = engine.views[&view]
        .document
        .as_ref()
        .unwrap()
        .get_element_by_id("p")
        .unwrap();
    assert_eq!(
        p.get_attribute("data-step"),
        Some("one"),
        "the second script and `load` did not run"
    );
    let seen = events(&mut rx);
    assert!(cancelled(&seen), "{seen:?}");
    assert!(!page_loaded(&seen, &server.url("/")), "{seen:?}");
    assert_next_navigation_loads(&mut engine, view, &server, &mut rx, &rt);
}

/// Turn the live loop a while, as the app does after a load.
fn turn_live(engine: &mut Engine, view: EngineViewId, rt: &tokio::runtime::Runtime) {
    for _ in 0..20 {
        rt.block_on(engine.pump_live(view, 10));
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn a_subresource_answered_after_the_cancel_changes_nothing() {
    let page = "<html><head><link rel=stylesheet href=/held/a.css></head>\
<body><p>late</p><img src=/held/b.jpg></body></html>";
    let server = serve(vec![("/", page.to_string()), ("/next", NEXT.to_string())]);
    let (mut engine, view) = engine_and_view();
    let rt = runtime();
    let canceller = engine.nav_canceller(view).unwrap();
    let requested = server.held_requested.clone();
    let cancel = std::thread::spawn(move || {
        // The sheet first; the image is asked for once sheets are in.
        wait_until_held(&requested, 1);
        assert!(canceller.cancel_current());
    });
    rt.block_on(engine.load_url(view, server.url("/"))).unwrap();
    cancel.join().unwrap();
    let layouts = engine.relayout_stats(view).unwrap().clone();
    let frame = engine.views[&view].frame_generation;

    // Now the server answers both.
    server.gate.open();
    let deadline = Instant::now() + Duration::from_secs(5);
    while server.held_answered.load(Ordering::SeqCst) < server.held_requested.load(Ordering::SeqCst)
    {
        assert!(Instant::now() < deadline, "the server never answered");
        std::thread::sleep(Duration::from_millis(5));
    }
    turn_live(&mut engine, view, &rt);

    assert!(!engine.is_image_cached(&server.url("/held/b.jpg")));
    assert!(
        engine.views[&view].external_stylesheets.is_empty(),
        "the late sheet was not applied"
    );
    assert_eq!(
        engine.relayout_stats(view).unwrap(),
        &layouts,
        "nothing was laid out again"
    );
    assert_eq!(
        engine.views[&view].frame_generation, frame,
        "no frame came of it"
    );
    assert_eq!(
        engine.image_manager().decode_stats().pooled,
        0,
        "nothing was decoded"
    );
}

#[test]
fn a_decode_result_for_the_cancelled_navigation_is_dropped() {
    let page = "<html><body><img src=/held/c.jpg></body></html>";
    let server = serve(vec![("/", page.to_string()), ("/next", NEXT.to_string())]);
    let (mut engine, view) = engine_and_view();
    let rt = runtime();
    let before = engine.decode_nav(view).nav_id;
    let canceller = engine.nav_canceller(view).unwrap();
    let requested = server.held_requested.clone();
    let cancel = std::thread::spawn(move || {
        wait_until_held(&requested, 1);
        assert!(canceller.cancel_current());
    });
    rt.block_on(engine.load_url(view, server.url("/"))).unwrap();
    cancel.join().unwrap();

    // The key the load's decodes carried (begin_nav moved it on by one)
    // is no longer current once the page thread has seen the cancel.
    let load_key = rustkit_image::DecodeNav {
        view: view.0,
        nav_id: before + 1,
    };
    assert_ne!(engine.decode_nav(view), load_key);
    let image = server.url("/held/c.jpg");
    let result = rt.block_on(engine.image_manager().decode_fetched(
        &image,
        Some("image/jpeg"),
        JPEG.to_vec(),
        load_key,
    ));
    assert!(
        matches!(result, Err(rustkit_image::ImageError::Superseded)),
        "{:?}",
        result.map(|_| ())
    );
    assert!(
        !engine.is_image_cached(&image),
        "a dropped decode is never cached"
    );
    assert_eq!(engine.image_manager().decode_stats().superseded, 1);
    // A decode under the view's key now is kept.
    let current = engine.decode_nav(view);
    let kept = rt.block_on(engine.image_manager().decode_fetched(
        &image,
        Some("image/jpeg"),
        JPEG.to_vec(),
        current,
    ));
    assert!(kept.is_ok());
}

#[test]
fn a_cancel_for_a_finished_navigation_changes_nothing() {
    let page = "<html><body><p>done</p><script>window.n = 1;</script></body></html>";
    let server = serve(vec![("/", page.to_string())]);
    let (mut engine, view) = engine_and_view();
    let rt = runtime();
    rt.block_on(engine.load_url(view, server.url("/"))).unwrap();
    let nav = engine.nav_id(view).unwrap();
    assert!(nav > 0);
    let canceller = engine.nav_canceller(view).unwrap();
    assert!(!canceller.cancel_nav(nav), "no load in progress");
    assert_eq!(
        engine.execute_script(view, "window.n + 1").unwrap(),
        "Number(2.0)",
        "the loaded page's script still runs"
    );
}

#[test]
fn each_navigation_gets_the_next_nav_id_and_stop_does_not() {
    let server = serve(vec![("/", "<p>x</p>".to_string())]);
    let (mut engine, view) = engine_and_view();
    let rt = runtime();
    assert_eq!(engine.nav_id(view), Some(0));
    rt.block_on(engine.load_url(view, server.url("/"))).unwrap();
    assert_eq!(engine.nav_id(view), Some(1));
    engine.stop(view);
    assert_eq!(engine.nav_id(view), Some(1));
    engine.load_html(view, "<p>y</p>").unwrap();
    assert_eq!(engine.nav_id(view), Some(2));
    assert_eq!(
        engine.nav_canceller(view).unwrap().loading_nav(),
        0,
        "no load left in progress"
    );
}

/// The view's display list, to compare frames by.
fn frame_of(engine: &Engine, view: EngineViewId) -> String {
    format!("{:?}", engine.views[&view].display_list)
}

#[test]
fn a_cancel_during_a_render_blocking_sheet_lays_out_the_committed_document() {
    let first = "<html><head><title>first</title></head><body><p>first</p></body></html>";
    let second = "<html><head><title>second</title><link rel=stylesheet href=/held/a.css>\
</head><body><p>second</p></body></html>";
    let server = serve(vec![
        ("/first", first.to_string()),
        ("/second", second.to_string()),
        ("/next", NEXT.to_string()),
    ]);
    let (mut engine, view) = engine_and_view();
    let mut rx = engine.take_event_receiver().unwrap();
    let rt = runtime();
    rt.block_on(engine.load_url(view, server.url("/first")))
        .unwrap();
    let first_frame = frame_of(&engine, view);
    let _ = events(&mut rx);

    let canceller = engine.nav_canceller(view).unwrap();
    let requested = server.held_requested.clone();
    let cancel = std::thread::spawn(move || {
        wait_until_held(&requested, 1);
        assert!(canceller.cancel_current());
    });
    rt.block_on(engine.load_url(view, server.url("/second")))
        .unwrap();
    cancel.join().unwrap();

    let seen = events(&mut rx);
    assert!(cancelled(&seen), "{seen:?}");
    // The document committed before the sheet was asked for: it stays,
    // and the frame is of it, laid out without the sheet.
    assert_eq!(engine.get_url(view), Some(server.url("/second")));
    assert_eq!(engine.get_title(view).as_deref(), Some("second"));
    assert!(!engine.views[&view].initial_layout_deferred);
    assert!(
        engine.views[&view].external_stylesheets.is_empty(),
        "the held sheet was not applied"
    );
    assert_ne!(
        frame_of(&engine, view),
        first_frame,
        "the frame is the committed document's, not the previous page's"
    );
    assert_next_navigation_loads(&mut engine, view, &server, &mut rx, &rt);
}

#[test]
fn a_cancel_before_the_commit_leaves_the_shown_pages_script_running() {
    let first = "<html><head><title>first</title></head><body><p>first</p>\
<script>function f() { return 1; } window.n = 1;</script></body></html>";
    let server = serve(vec![
        ("/first", first.to_string()),
        ("/next", NEXT.to_string()),
    ]);
    let (mut engine, view) = engine_and_view();
    let mut rx = engine.take_event_receiver().unwrap();
    let rt = runtime();
    rt.block_on(engine.load_url(view, server.url("/first")))
        .unwrap();
    assert_eq!(
        engine.execute_script(view, "f() + window.n").unwrap(),
        "Number(2.0)"
    );
    let _ = events(&mut rx);

    // The document itself is held: the cancel lands before the commit.
    let canceller = engine.nav_canceller(view).unwrap();
    let requested = server.held_requested.clone();
    let cancel = std::thread::spawn(move || {
        wait_until_held(&requested, 1);
        assert!(canceller.cancel_current());
    });
    rt.block_on(engine.load_url(view, server.url("/held/second")))
        .unwrap();
    cancel.join().unwrap();

    let seen = events(&mut rx);
    assert!(cancelled(&seen), "{seen:?}");
    assert_eq!(engine.get_url(view), Some(server.url("/first")));
    assert_eq!(
        engine.execute_script(view, "f() + window.n").unwrap(),
        "Number(2.0)",
        "the shown page's script still runs"
    );
    assert_next_navigation_loads(&mut engine, view, &server, &mut rx, &rt);
}
