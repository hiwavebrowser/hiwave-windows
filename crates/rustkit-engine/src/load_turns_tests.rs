//! LAT15: a page load serves the window's commands between its phases.
//!
//! Before LAT15 the content thread ran a navigation as one future and read
//! no command until it ended: a click on text already on screen waited for
//! every image and script (cnn 27 s in the 2026-11-08 hand test). The load
//! now lends the engine to the endpoint at safe points (after the first
//! layout, every tick of the image wait, between page scripts), where
//! input, resizes, renders, evals and devtools reads are applied. A
//! navigation, Stop, Close, reload or live turn is never applied there: it
//! and everything after it wait for the load, in order.
//!
//! Every page here is served on loopback by [`serve`], with the content
//! thread built as the app builds it ([`app_page`]). `measure_slow_load`
//! prints the LAT15 figures and asserts nothing (`--ignored --nocapture`).

use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rustkit_viewhost::Bounds;
use tokio::sync::oneshot;
use url::Url;

use crate::content_thread::ContentThread;
use crate::{EngineBuilder, LoadState, PageEvent, PageInput};

/// 640 x 480 JPEG.
const JPEG: &[u8] =
    include_bytes!("../../../websuite/js-ladder/01-mdn/32-image-gallery/images/pic4.jpg");

/// The main text's box colour: the first frame that has it shows the text.
pub(crate) const MAIN: (u8, u8, u8) = (0, 160, 0);

/// How soon a command sent while the load waits is answered.
pub(crate) const SERVED: Duration = Duration::from_millis(500);

/// One route: content type, body, how long the server waits before
/// answering.
pub(crate) type Route = (&'static str, Vec<u8>, u64);

pub(crate) struct Fixture {
    port: u16,
    pub(crate) start: Instant,
    /// (path, ms since `start`) for every request.
    seen: Arc<Mutex<Vec<(String, u128)>>>,
}

impl Fixture {
    pub(crate) fn url(&self, path: &str) -> Url {
        Url::parse(&format!("http://127.0.0.1:{}{path}", self.port)).unwrap()
    }

    pub(crate) fn requests(&self, path: &str) -> Vec<u128> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .filter(|(p, _)| p == path)
            .map(|(_, at)| *at)
            .collect()
    }
}

pub(crate) fn serve(routes: Vec<(&'static str, Route)>) -> Fixture {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let start = Instant::now();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let seen_s = seen.clone();
    let routes = Arc::new(routes);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let (seen, routes) = (seen_s.clone(), routes.clone());
            std::thread::spawn(move || {
                let mut request = Vec::new();
                let mut buf = [0u8; 2048];
                loop {
                    if request.windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                    match stream.read(&mut buf) {
                        Ok(0) | Err(_) => return,
                        Ok(n) => request.extend_from_slice(&buf[..n]),
                    }
                }
                let head = String::from_utf8_lossy(&request).to_string();
                let path = head.split_whitespace().nth(1).unwrap_or("/").to_string();
                seen.lock().unwrap().push((path.clone(), start.elapsed().as_millis()));
                let (ty, body, delay) = routes
                    .iter()
                    .find(|(p, _)| *p == path)
                    .map(|(_, route)| route.clone())
                    .unwrap_or(("text/plain", b"ok".to_vec(), 0));
                std::thread::sleep(Duration::from_millis(delay));
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: {ty}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(&body);
            });
        }
    });
    Fixture { port, start, seen }
}

pub(crate) fn html(body: &str) -> Route {
    ("text/html", body.as_bytes().to_vec(), 0)
}

pub(crate) fn js(body: &str) -> Route {
    ("text/javascript", body.as_bytes().to_vec(), 0)
}

fn slow_image(ms: u64) -> Route {
    ("image/jpeg", JPEG.to_vec(), ms)
}

/// JavaScript that runs for `ms` of wall clock. Each turn of the loop does
/// some real work: an empty loop reaches the loop limit
/// (`script_loop_iteration_limit`, 10 M iterations) in about a second and
/// is stopped there.
pub(crate) fn spin(ms: u64) -> String {
    format!("(function () {{ var t = Date.now(); while (Date.now() - t < {ms}) {{ JSON.stringify({{ a: [1, 2, 3], b: 'x' }}); }} }})();")
}

/// The app's content builder, as far as the engine has it.
fn app_page() -> ContentThread {
    ContentThread::spawn(
        EngineBuilder::new()
            .javascript_enabled(true)
            .script_budget_ms(15_000)
            .interrupt_scripts_at_budget(true)
            .live_turn_budget_ms(5_000)
            .follows_script_navigation(true)
            .post_load_requests_live(true),
        Bounds { x: 0, y: 0, width: 800, height: 600 },
    )
    .expect("content thread spawns")
}

fn has_color(commit: &crate::FrameCommit, rgb: (u8, u8, u8)) -> bool {
    let Some(list) = commit.list.as_ref() else { return false };
    list.commands.iter().any(|cmd| {
        matches!(cmd, rustkit_layout::DisplayCommand::SolidColor(c, _) if (c.r, c.g, c.b) == rgb)
    })
}

fn wait<T>(rx: &mut oneshot::Receiver<T>, budget: Duration) -> Option<T> {
    let deadline = Instant::now() + budget;
    loop {
        match rx.try_recv() {
            Ok(v) => return Some(v),
            Err(oneshot::error::TryRecvError::Closed) => return None,
            Err(_) if Instant::now() >= deadline => return None,
            Err(_) => std::thread::sleep(Duration::from_millis(1)),
        }
    }
}

/// A page thread on one navigation, with the events it has published.
pub(crate) struct Load {
    pub(crate) page: ContentThread,
    pub(crate) nav: u64,
    started: Instant,
    pub(crate) events: Vec<(Instant, PageEvent)>,
    /// Play the window: load what the page asks to navigate to.
    pub(crate) follow: bool,
    pub(crate) followed: Vec<String>,
}

impl Load {
    pub(crate) fn start(fixture: &Fixture, path: &str) -> Self {
        Self::start_on(app_page(), fixture, path)
    }

    /// [`Load::start`] on a page thread built some other way.
    pub(crate) fn start_on(page: ContentThread, fixture: &Fixture, path: &str) -> Self {
        let mut boot = page.eval("0");
        assert!(wait(&mut boot, Duration::from_secs(30)).is_some(), "the page thread boots");
        let _ = page.poll();
        let started = Instant::now();
        let nav = page.navigate(fixture.url(path));
        Load { page, nav, started, events: Vec::new(), follow: false, followed: Vec::new() }
    }

    pub(crate) fn pump(&mut self) {
        for event in self.page.poll() {
            if let (true, PageEvent::NavigateRequested { url, .. }) = (self.follow, &event) {
                self.followed.push(url.clone());
                if let Ok(url) = Url::parse(url) {
                    self.page.navigate(url);
                }
            }
            self.events.push((Instant::now(), event));
        }
    }

    pub(crate) fn until(&mut self, budget: Duration, mut done: impl FnMut(&Self) -> bool) -> bool {
        let deadline = Instant::now() + budget;
        loop {
            self.pump();
            if done(self) {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn first_frame_with(&self, rgb: (u8, u8, u8)) -> Option<Instant> {
        self.events.iter().find_map(|(at, event)| match event {
            PageEvent::Commit(commit) if has_color(commit, rgb) => Some(*at),
            _ => None,
        })
    }

    fn last_frame(&self) -> Option<&crate::FrameCommit> {
        self.events.iter().rev().find_map(|(_, event)| match event {
            PageEvent::Commit(commit) => Some(commit),
            _ => None,
        })
    }

    pub(crate) fn completed(&self, nav: u64) -> Option<Instant> {
        self.events.iter().find_map(|(at, event)| match event {
            PageEvent::LoadState { state: LoadState::Complete, nav_id } if *nav_id == nav => Some(*at),
            _ => None,
        })
    }

    pub(crate) fn wait_main_color(&mut self, rgb: (u8, u8, u8)) -> Instant {
        assert!(self.until(Duration::from_secs(20), |l| l.first_frame_with(rgb).is_some()), "the colour is shown");
        self.first_frame_with(rgb).unwrap()
    }

    pub(crate) fn wait_main(&mut self) -> Instant {
        assert!(self.until(Duration::from_secs(20), |l| l.first_frame_with(MAIN).is_some()), "the main text is shown");
        self.first_frame_with(MAIN).unwrap()
    }

    pub(crate) fn wait_complete(&mut self) -> Instant {
        let nav = self.nav;
        assert!(self.until(Duration::from_secs(30), |l| l.completed(nav).is_some()), "the load completes");
        self.completed(nav).unwrap()
    }

    /// Evaluate `script`; its answer and how long it took.
    pub(crate) fn eval(&mut self, script: &str, budget: Duration) -> (Option<String>, Duration) {
        let sent = Instant::now();
        let mut rx = self.page.eval(script);
        let deadline = sent + budget;
        loop {
            self.pump();
            match rx.try_recv() {
                Ok(answer) => return (answer.ok(), sent.elapsed()),
                Err(oneshot::error::TryRecvError::Closed) => return (None, sent.elapsed()),
                Err(_) if Instant::now() >= deadline => return (None, sent.elapsed()),
                Err(_) => std::thread::sleep(Duration::from_millis(1)),
            }
        }
    }

    pub(crate) fn click(&mut self, x: f32, y: f32, budget: Duration) -> Option<Duration> {
        let sent = Instant::now();
        let (reply, mut rx) = oneshot::channel();
        self.page.input(PageInput::Click { x, y, reply });
        let deadline = sent + budget;
        loop {
            self.pump();
            match rx.try_recv() {
                Ok(_) => return Some(sent.elapsed()),
                Err(oneshot::error::TryRecvError::Closed) => return None,
                Err(_) if Instant::now() >= deadline => return None,
                Err(_) => std::thread::sleep(Duration::from_millis(1)),
            }
        }
    }
}

impl Drop for Load {
    fn drop(&mut self) {
        self.page.shutdown();
    }
}

pub(crate) const MAIN_DIV: &str = r#"<div id="main" style="width:400px;height:200px;background:rgb(0,160,0)">Main text</div>"#;

// ---------------------------------------------------------------------
// Path 6 / safe point (b): the image wait.
// ---------------------------------------------------------------------

/// A click on text already shown is answered while an image is still
/// arriving, and its handler runs before the page's scripts.
#[test]
fn a_click_during_the_image_wait_is_answered_before_the_load_ends() {
    let page = format!(
        r#"<!doctype html><body style="margin:0">
<div id="main" style="width:400px;height:200px;background:rgb(0,160,0)" onclick="window.clicks = (window.clicks || 0) + 1">Main text</div>
<img src="/slow.jpg" width="100" height="100">
<script>window.clicksSeenByScript = window.clicks || 0;</script>
<script>window.addEventListener('load', function () {{ fetch('/beacon?load'); }});</script>"#
    );
    let fixture = serve(vec![("/page", html(&page)), ("/slow.jpg", slow_image(3_000))]);
    let mut load = Load::start(&fixture, "/page");
    load.wait_main();
    std::thread::sleep(Duration::from_millis(100));
    let waited = load.click(200.0, 100.0, Duration::from_secs(20)).expect("the click is answered");
    assert!(load.completed(load.nav).is_none(), "the load was still waiting for its image when the click was answered");
    assert!(waited < SERVED, "the click waited {waited:?} for the image");
    load.wait_complete();
    let (seen, _) = load.eval("String(window.clicksSeenByScript) + ',' + String(window.clicks)", Duration::from_secs(5));
    assert_eq!(seen.as_deref(), Some("String(\"1,1\")"), "the handler ran once, before the page's script");
    assert!(load.until(Duration::from_secs(5), |_| !fixture.requests("/beacon?load").is_empty()), "`load` fired");
    let image = fixture.requests("/slow.jpg")[0];
    let fired = fixture.requests("/beacon?load")[0];
    assert!(fired >= image + 3_000, "`load` fired at {fired} ms, before the image arrived ({image} + 3000 ms)");
}

/// A resize sent while the image arrives is laid out at once: script sees
/// the new size before the load ends.
#[test]
fn a_resize_during_the_image_wait_is_applied_before_the_load_ends() {
    let page = format!(r#"<!doctype html><body style="margin:0">{MAIN_DIV}<img src="/slow.jpg" width="100" height="100">"#);
    let fixture = serve(vec![("/page", html(&page)), ("/slow.jpg", slow_image(3_000))]);
    let mut load = Load::start(&fixture, "/page");
    load.wait_main();
    load.page.resize(Bounds { x: 0, y: 0, width: 640, height: 480 }, 1.0);
    // The next safe point lays the new size out, as a live turn's start
    // does; an eval in the same batch as the resize reads the old one on
    // a live turn too.
    std::thread::sleep(Duration::from_millis(100));
    let (width, waited) = load.eval("String(window.innerWidth)", Duration::from_secs(20));
    assert!(load.completed(load.nav).is_none(), "the load was still waiting for its image");
    assert!(waited < SERVED, "the resize and the read waited {waited:?}");
    assert_eq!(width.as_deref(), Some("String(\"640\")"));
}

/// (C) The app's evals (zoom, find, autofill, the Inspector) can now run
/// while the document is `loading`. What one writes is kept by the load.
#[test]
fn an_eval_during_the_image_wait_sees_a_loading_document_and_its_write_is_kept() {
    let page = format!(r#"<!doctype html><body style="margin:0">{MAIN_DIV}<img src="/slow.jpg" width="100" height="100">"#);
    let fixture = serve(vec![("/page", html(&page)), ("/slow.jpg", slow_image(3_000))]);
    let mut load = Load::start(&fixture, "/page");
    load.wait_main();
    let (state, waited) = load.eval("document.readyState", Duration::from_secs(20));
    assert!(waited < SERVED, "the eval waited {waited:?}");
    assert_eq!(state.as_deref(), Some("String(\"loading\")"));
    // The app's zoom script, as `UserEvent::ZoomIn` sends it.
    let _ = load.eval("document.body.style.zoom = '150%';", Duration::from_secs(20));
    load.wait_complete();
    let (zoom, _) = load.eval("document.body.style.zoom", Duration::from_secs(5));
    assert_eq!(zoom.as_deref(), Some("String(\"150%\")"));
}

/// The style memo spans the sheets layout and the images layout. A class a
/// served click sets between them is in the frame after the image.
#[test]
fn a_class_a_served_click_sets_is_in_the_frame_after_the_image() {
    let page = r#"<!doctype html><style>#main { width: 400px; height: 200px; background: rgb(0,160,0) } .x #main { background: rgb(0,0,200) }</style><body style="margin:0">
<div id="main" onclick="document.body.className = 'x'">Main text</div>
<img src="/slow.jpg" width="100" height="100">"#;
    let fixture = serve(vec![("/page", html(page)), ("/slow.jpg", slow_image(2_000))]);
    let mut load = Load::start(&fixture, "/page");
    load.wait_main();
    assert!(load.click(200.0, 100.0, Duration::from_secs(20)).is_some());
    load.wait_complete();
    load.until(Duration::from_secs(3), |l| l.last_frame().is_some_and(|f| has_color(f, (0, 0, 200))));
    let (class, _) = load.eval("document.body.className", Duration::from_secs(5));
    assert_eq!(class.as_deref(), Some("String(\"x\")"), "the click set the class");
    let frame = load.last_frame().expect("a frame");
    assert!(has_color(frame, (0, 0, 200)), "the clicked class is drawn");
    assert!(!has_color(frame, MAIN), "the old background is gone");
}

// ---------------------------------------------------------------------
// Path 1 / (A): what is never applied at a safe point.
// ---------------------------------------------------------------------

/// A link clicked while the image arrives navigates exactly once, and the
/// page being left fires no `load`.
#[test]
fn a_link_clicked_during_the_image_wait_navigates_once_and_the_old_load_fires_no_load() {
    let page = r#"<!doctype html><body style="margin:0">
<a id="link" href="/next" style="display:block;width:400px;height:200px;background:rgb(0,160,0)">next</a>
<img src="/slow.jpg" width="100" height="100">
<script>window.addEventListener('load', function () { fetch('/beacon?old-load'); });</script>"#;
    let fixture = serve(vec![
        ("/page", html(page)),
        ("/slow.jpg", slow_image(3_000)),
        ("/next", html("<p>next</p>")),
    ]);
    let mut load = Load::start(&fixture, "/page");
    load.follow = true;
    load.wait_main();
    let clicked = fixture.start.elapsed().as_millis();
    assert!(load.click(200.0, 100.0, Duration::from_secs(20)).is_some());
    assert!(load.until(Duration::from_secs(10), |_| !fixture.requests("/next").is_empty()), "the link is followed");
    let next_at = fixture.requests("/next")[0];
    assert!(next_at < clicked + 1_000, "the link was followed {} ms after the click", next_at - clicked);
    // Past the image's arrival and the old page's `load`, had it fired.
    std::thread::sleep(Duration::from_millis(3_500));
    load.pump();
    let _ = load.eval("0", Duration::from_secs(5));
    assert_eq!(fixture.requests("/next").len(), 1, "followed once: {:?}", load.followed);
    assert!(fixture.requests("/beacon?old-load").is_empty(), "the page that was left fired `load`");
    let (path, _) = load.eval("location.pathname", Duration::from_secs(5));
    assert_eq!(path.as_deref(), Some("String(\"/next\")"));
}

/// A navigation sent while the load waits is not applied at a safe point:
/// the commands behind it wait for it, in order.
#[test]
fn a_navigation_queued_mid_load_keeps_its_place() {
    let page = format!(r#"<!doctype html><body style="margin:0">{MAIN_DIV}<img src="/slow.jpg" width="100" height="100">"#);
    let fixture = serve(vec![
        ("/page", html(&page)),
        ("/slow.jpg", slow_image(3_000)),
        ("/second", html("<p>second</p>")),
    ]);
    let mut load = Load::start(&fixture, "/page");
    load.wait_main();
    let mut before = load.page.eval("location.pathname");
    load.page.navigate(fixture.url("/second"));
    let mut after = load.page.eval("location.pathname");
    let before = wait(&mut before, Duration::from_secs(20)).and_then(|r| r.ok());
    let after = wait(&mut after, Duration::from_secs(20)).and_then(|r| r.ok());
    // Ahead of the navigation: the first page, or refused as the page being
    // left. Never the second page.
    assert_ne!(before.as_deref(), Some("String(\"/second\")"), "the eval ahead of the navigation ran after it");
    assert_eq!(after.as_deref(), Some("String(\"/second\")"), "the eval behind it ran on the second");
}

// ---------------------------------------------------------------------
// Path 8 / safe point (c), (B), row 10: between page scripts.
// ---------------------------------------------------------------------

fn script_page() -> Vec<(&'static str, Route)> {
    let head = r#"<!doctype html><body style="margin:0">
<script>window.order = [];
document.addEventListener('DOMContentLoaded', function () { order.push('dcl'); });
window.addEventListener('load', function () { order.push('load'); });</script>
<div id="main" style="width:400px;height:200px;background:rgb(0,160,0)">Main text</div>
<script src="/s1.js"></script><script src="/s2.js"></script><script src="/s3.js"></script>"#;
    let s1 = format!(
        "{}\norder.push('s1');\nsetTimeout(function () {{ order.push('timer'); }}, 0);\n\
         document.getElementById('main').addEventListener('click', function () {{\n\
           order.push('click:' + document.readyState);\n\
           Promise.resolve().then(function () {{ order.push('micro'); }});\n\
         }});",
        spin(1_500)
    );
    let s2 = format!("order.push('s2:' + (document.currentScript ? document.currentScript.src.split('/').pop() : 'none'));\n{}", spin(300));
    vec![
        ("/page", html(head)),
        ("/s1.js", js(&s1)),
        ("/s2.js", js(&s2)),
        ("/s3.js", js("order.push('s3');")),
    ]
}

/// A click that arrives while s1 runs is dispatched before s2 starts, with
/// its microtasks, and no timer runs early. Script order, `currentScript`,
/// DOMContentLoaded and `load` are as before.
#[test]
fn a_click_during_a_script_runs_before_the_next_script_with_its_microtasks() {
    let fixture = serve(script_page());
    let mut load = Load::start(&fixture, "/page");
    let shown = load.wait_main();
    // s1 runs for 1.5 s from about when the main text is shown.
    let since = shown.elapsed();
    if since < Duration::from_millis(400) {
        std::thread::sleep(Duration::from_millis(400) - since);
    }
    let waited = load.click(200.0, 100.0, Duration::from_secs(20)).expect("the click is answered");
    assert!(load.completed(load.nav).is_none(), "answered before the load ended");
    assert!(waited < Duration::from_millis(1_500), "the click waited {waited:?}: only for s1");
    load.wait_complete();
    let (order, _) = load.eval("order.join(',')", Duration::from_secs(5));
    let order = order.unwrap_or_default();
    let order = order.trim_start_matches("String(\"").trim_end_matches("\")");
    let at = |name: &str| order.split(',').position(|o| o == name).unwrap_or_else(|| panic!("{name} missing: {order}"));
    assert!(order.starts_with("s1,click:loading,micro,s2:s2.js,s3,dcl,load"), "{order}");
    assert!(at("timer") > at("s3"), "a timer ran before the scripts were done: {order}");
}

// ---------------------------------------------------------------------
// Path 5 / safe point (a): the first layout, before any script.
// ---------------------------------------------------------------------

/// With no sheet the first frame is laid out before the scripts; a command
/// sent then is answered before they run.
#[test]
fn an_eval_after_the_first_frame_runs_before_the_scripts() {
    let page = format!(
        r#"<!doctype html><body style="margin:0">{MAIN_DIV}
<img src="/slow.jpg" width="100" height="100">
<script>window.ran = true;</script>"#
    );
    let fixture = serve(vec![("/page", html(&page)), ("/slow.jpg", slow_image(2_000))]);
    let mut load = Load::start(&fixture, "/page");
    load.wait_main();
    let (ran, waited) = load.eval("String(window.ran)", Duration::from_secs(20));
    assert!(waited < SERVED, "the eval waited {waited:?}");
    assert_eq!(ran.as_deref(), Some("String(\"undefined\")"), "the page's script had not run");
}

// ---------------------------------------------------------------------
// The LAT15 figures.
// ---------------------------------------------------------------------

#[derive(Debug, Default)]
struct Figures {
    html_ms: Option<u128>,
    first_main_ms: Option<u128>,
    complete_ms: Option<u128>,
    /// Longest wait for a no-op command (an `Eval` of `0`), ms.
    longest_ms: u128,
    /// (sent at, waited) for the click, ms.
    click: Option<(u128, u128)>,
    /// (sent at, the page thread's next answer) after Stop, ms.
    stop: Option<(u128, u128)>,
    /// Longest wait of another tab's no-op command during the load, ms.
    other_tab_ms: u128,
    /// When each page script started, ms.
    scripts_ms: Option<String>,
}

const SHEET_MS: u64 = 2_000;
const IMAGE_MS: u64 = 4_000;
const SCRIPT_MS: u64 = 2_000;

fn figures_fixture() -> Fixture {
    let page = format!(
        r#"<!doctype html><html><head><link rel="stylesheet" href="/slow.css"></head>
<body style="margin:0">{MAIN_DIV}
<img src="/slow.jpg" width="100" height="100">
<script src="/s1.js"></script><script src="/s2.js"></script><script src="/s3.js"></script>
</body></html>"#
    );
    serve(vec![
        ("/page", html(&page)),
        ("/slow.css", ("text/css", b"body { color: rgb(0,0,0) }".to_vec(), SHEET_MS)),
        ("/slow.jpg", slow_image(IMAGE_MS)),
        ("/s1.js", js(&format!("window.s1 = Date.now(); {} window.e1 = Date.now();", spin(SCRIPT_MS)))),
        ("/s2.js", js(&format!("window.s2 = Date.now(); {} window.e2 = Date.now();", spin(SCRIPT_MS)))),
        ("/s3.js", js(&format!("window.s3 = Date.now(); {} window.e3 = Date.now();", spin(SCRIPT_MS)))),
    ])
}

fn figures(click_at: Option<u64>, stop_at: Option<u64>) -> Figures {
    let fixture = figures_fixture();
    let other = app_page();
    let mut loaded = other.load_html("<p>other tab</p>");
    assert!(wait(&mut loaded, Duration::from_secs(30)).is_some(), "other tab loads");
    let mut load = Load::start(&fixture, "/page");
    let t0 = load.started;
    let t0_epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis()
        .saturating_sub(t0.elapsed().as_millis());
    let t0_server = fixture.start.elapsed().as_millis();
    let ms = |at: Instant| at.duration_since(t0).as_millis();

    let mut out = Figures::default();
    let mut ping: Option<(Instant, oneshot::Receiver<Result<String, String>>)> = None;
    let mut other_ping: Option<(Instant, oneshot::Receiver<Result<String, String>>)> = None;
    let mut click: Option<(Instant, oneshot::Receiver<crate::ClickOutcome>)> = None;
    let mut stop_sent: Option<Instant> = None;
    let mut clicked = false;
    let deadline = t0 + Duration::from_secs(40);
    while Instant::now() < deadline {
        load.pump();
        let now = Instant::now();
        out.first_main_ms = load.first_frame_with(MAIN).map(ms);
        out.complete_ms = load.completed(load.nav).map(ms);
        match ping.as_mut() {
            None => ping = Some((now, load.page.eval("0"))),
            Some((sent, rx)) => {
                if rx.try_recv().is_ok() {
                    out.longest_ms = out.longest_ms.max(now.duration_since(*sent).as_millis());
                    if let Some(at) = stop_sent {
                        if *sent >= at && out.stop.is_none() {
                            out.stop = Some((ms(at), now.duration_since(at).as_millis()));
                        }
                    }
                    ping = None;
                }
            }
        }
        match other_ping.as_mut() {
            None if out.complete_ms.is_none() => other_ping = Some((now, other.eval("0"))),
            None => {}
            Some((sent, rx)) => {
                if rx.try_recv().is_ok() {
                    out.other_tab_ms = out.other_tab_ms.max(now.duration_since(*sent).as_millis());
                    other_ping = None;
                }
            }
        }
        if let (Some(at), false) = (click_at, clicked) {
            if ms(now) >= at as u128 {
                let (reply, rx) = oneshot::channel();
                load.page.input(PageInput::Click { x: 200.0, y: 100.0, reply });
                click = Some((now, rx));
                clicked = true;
            }
        }
        if let Some((sent, rx)) = click.as_mut() {
            if rx.try_recv().is_ok() {
                out.click = Some((ms(*sent), now.duration_since(*sent).as_millis()));
                click = None;
            }
        }
        if let (Some(at), None) = (stop_at, stop_sent) {
            if ms(now) >= at as u128 {
                load.page.stop();
                stop_sent = Some(now);
                ping = None;
            }
        }
        if click.is_none() && (out.complete_ms.is_some() || stop_sent.is_some_and(|s| s.elapsed() > Duration::from_secs(3))) {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    out.html_ms = fixture.requests("/page").first().map(|at| at.saturating_sub(t0_server));
    if out.complete_ms.is_some() {
        let (started, _) = load.eval(&format!("[window.s1, window.e1, window.s2, window.e2, window.s3, window.e3].map(function (t) {{ return t - {t0_epoch}; }}).join('/')"), Duration::from_secs(5));
        out.scripts_ms = started;
    }
    other.shutdown();
    out
}

#[test]
#[ignore = "measurement: run with --ignored --nocapture"]
fn measure_slow_load() {
    let plain = figures(Some(3_000), None);
    println!("LAT15 load+click: {plain:?}");
    let stopped = figures(None, Some(4_500));
    println!("LAT15 load+stop: {stopped:?}");
}

// ---------------------------------------------------------------------
// Risks: document.write, Stop.
// ---------------------------------------------------------------------

/// A listener served between scripts calls `document.write` while the
/// document is `loading`. The document survives and the load finishes in
/// order. (Chrome 148 differs: a write from an event listener while the
/// parser is blocked opens a new document; here the parser has already
/// finished and the write is dropped, as it is after the load.)
#[test]
fn a_document_write_from_a_served_listener_while_loading_does_not_destroy_the_document() {
    let mut routes = script_page();
    let s1 = format!(
        "{}\norder.push('s1');\n\
         document.getElementById('main').addEventListener('click', function () {{\n\
           order.push('click:' + document.readyState);\n\
           document.write('<p id=\"written\">w</p>');\n\
           order.push('wrote');\n\
         }});",
        spin(1_500)
    );
    routes[1] = ("/s1.js", js(&s1));
    let fixture = serve(routes);
    let mut load = Load::start(&fixture, "/page");
    let shown = load.wait_main();
    let since = shown.elapsed();
    if since < Duration::from_millis(400) {
        std::thread::sleep(Duration::from_millis(400) - since);
    }
    assert!(load.click(200.0, 100.0, Duration::from_secs(20)).is_some());
    let finished = load.until(Duration::from_secs(15), |l| l.completed(l.nav).is_some());
    let (state, _) = load.eval(
        "[String(!!document.getElementById('main')), String(!!document.getElementById('written')), (window.order || []).join('|'), document.readyState].join(',')",
        Duration::from_secs(5),
    );
    assert!(finished, "the load finished");
    assert_eq!(
        state.as_deref(),
        Some("String(\"true,false,s1|click:loading|wrote|s2:s2.js|s3|dcl|load,complete\")"),
        "the document, its scripts and its lifecycle survive the write"
    );
}

/// Stop after a served click ends the load: no `load`, no completion.
#[test]
fn stop_after_a_served_click_ends_the_load() {
    let page = r#"<!doctype html><body style="margin:0">
<div id="main" style="width:400px;height:200px;background:rgb(0,160,0)" onclick="window.clicks = (window.clicks || 0) + 1">Main text</div>
<img src="/slow.jpg" width="100" height="100">
<script>window.addEventListener('load', function () { fetch('/beacon?load'); });</script>"#;
    let fixture = serve(vec![("/page", html(page)), ("/slow.jpg", slow_image(2_000))]);
    let mut load = Load::start(&fixture, "/page");
    load.wait_main();
    assert!(load.click(200.0, 100.0, Duration::from_secs(20)).expect("answered") < SERVED);
    load.page.stop();
    let (_, waited) = load.eval("0", Duration::from_secs(20));
    assert!(waited < SERVED, "the page answered {waited:?} after Stop");
    std::thread::sleep(Duration::from_millis(2_500));
    load.pump();
    assert!(load.completed(load.nav).is_none(), "the stopped load completed");
    assert!(fixture.requests("/beacon?load").is_empty(), "the stopped load fired `load`");
    let (clicks, _) = load.eval("String(window.clicks)", Duration::from_secs(5));
    assert_eq!(clicks.as_deref(), Some("String(\"1\")"));
    // R1 (1): the stopped document does not read `loading` for good.
    let (state, _) = load.eval("document.readyState", Duration::from_secs(5));
    assert_eq!(state.as_deref(), Some("String(\"complete\")"), "a stopped document reads `complete`, as in Chrome");
}

/// R1 (2): what a listener served between scripts logs keeps its place in
/// the script log, after the script that ran before it and before the next.
#[test]
fn a_served_listener_record_keeps_its_place_in_the_script_log() {
    let mut routes = script_page();
    let s1 = format!(
        "{}\norder.push('s1'); window.s1done = true;\n\
         document.getElementById('main').addEventListener('click', function () {{ throw new Error('served'); }});",
        spin(200)
    );
    routes[1] = ("/s1.js", js(&s1));
    let fixture = serve(routes);
    let mut engine = EngineBuilder::new()
        .javascript_enabled(true)
        .script_budget_ms(15_000)
        .interrupt_scripts_at_budget(true)
        .post_load_requests_live(true)
        .build()
        .expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 800, height: 600 })
        .expect("view");
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let mut clicked = false;
    // Plays the endpoint: at the first safe point after s1, a click.
    let mut serve = |engine: &mut crate::Engine| {
        let mut served = crate::Served::default();
        if !clicked && engine.execute_script(view, "String(window.s1done)").is_ok_and(|v| v.contains("true")) {
            engine.click_at_point(view, 200.0, 100.0);
            clicked = true;
            served.ran = true;
        }
        served
    };
    let loaded = rt.block_on(engine.load_url_serving(view, fixture.url("/page"), None, Some(&mut serve)));
    assert!(loaded.is_ok(), "{loaded:?}");
    assert!(clicked, "no safe point came after s1");
    let sources: Vec<String> = engine
        .script_log(view)
        .unwrap_or_default()
        .iter()
        .map(|record| record.source.clone())
        .collect();
    let at = |needle: &str| {
        sources
            .iter()
            .position(|source| source.contains(needle))
            .unwrap_or_else(|| panic!("{needle} not in {sources:?}"))
    };
    assert!(at("s1.js") < at("event:click") && at("event:click") < at("s2.js"), "{sources:?}");
}

// ---------------------------------------------------------------------
// R1 ruling on the app's evals that write the DOM: pinned, not deferred.
// ---------------------------------------------------------------------

/// The app's find-in-page script, run while an image arrives and before a
/// page script that replaces the text (as a hydrating page does). In
/// RustKit content the script stops at `Range.surroundContents`, which is
/// not callable here, after it has added its `<style>`; it does so after
/// the load too (a separate gap). The pin: the find is answered at once,
/// wraps no text, and the page's own script and load are untouched.
#[test]
fn find_in_page_mid_load_then_a_hydrating_script() {
    const FIND: &str = include_str!("../../hiwave-app/src/ui/find_in_page.js");
    let find = |query: &str| {
        format!(
            "{FIND}\n(function () {{ var r = window.__hiwaveFind.run({{ query: '{query}', case_sensitive: false, direction: 'reset' }}); return String(r.count); }})()"
        )
    };
    let page = r#"<!doctype html><body style="margin:0">
<div id="main" style="width:400px;height:200px;background:rgb(0,160,0)">Main text hello</div>
<img src="/slow.jpg" width="100" height="100">
<script>window.sawMark = document.getElementById('main').innerHTML.indexOf('zen-find-match') >= 0;
document.getElementById('main').textContent = 'hydrated hello world';</script>"#;
    let fixture = serve(vec![("/page", html(page)), ("/slow.jpg", slow_image(2_000))]);
    let mut load = Load::start(&fixture, "/page");
    load.wait_main();
    let mut rx = load.page.eval(&find("hello"));
    let sent = Instant::now();
    let answer = wait(&mut rx, Duration::from_secs(20));
    let waited = sent.elapsed();
    assert!(waited < SERVED, "find waited {waited:?}");
    // macOS's find script wraps matches with Range.surroundContents, which RustKit lacks, so
    // there the find errors. The Windows copy of find_in_page.js wraps with createTextNode and
    // insertBefore (no surroundContents), so it answers with the count instead.
    let expect_error = !cfg!(windows);
    assert!(
        answer.as_ref().is_some_and(|r| if expect_error {
            r.as_ref().is_err_and(|e| e.contains("surroundContents") || e.contains("not a callable"))
        } else {
            r.as_ref().is_ok_and(|v| v == "String(\"1\")")
        }),
        "find mid-load: {answer:?}"
    );
    load.wait_complete();
    let (state, _) = load.eval(
        "String(window.sawMark) + ',' + document.getElementById('main').textContent + ',' + document.readyState",
        Duration::from_secs(5),
    );
    assert_eq!(state.as_deref(), Some("String(\"false,hydrated hello world,complete\")"));
}

/// The app's autofill scripts mid-load. On macOS the autofill helper is
/// not injected into RustKit content (main.rs injects it off macOS only),
/// so the trigger is a no-op and the credentials message reaches only a
/// page listener. Neither touches the form or the load.
#[test]
fn autofill_scripts_mid_load_leave_the_form_and_the_load_alone() {
    let page = r#"<!doctype html><body style="margin:0">
<div id="main" style="width:400px;height:200px;background:rgb(0,160,0)">Main text</div>
<form><input id="user" name="user" value="typed"></form>
<img src="/slow.jpg" width="100" height="100">
<script>window.ran = true;</script>"#;
    let fixture = serve(vec![("/page", html(page)), ("/slow.jpg", slow_image(2_000))]);
    let mut load = Load::start(&fixture, "/page");
    load.wait_main();
    // `UserEvent::TriggerAutofill` and `SendAutofillCredentials`, verbatim.
    let (_, waited) = load.eval("if(window.hiwaveAutofill) { hiwaveAutofill.trigger(); }", Duration::from_secs(20));
    assert!(waited < SERVED, "the trigger waited {waited:?}");
    let _ = load.eval(
        "window.postMessage({ type: 'hiwave_autofill_credentials', credentials: [{\"username\":\"u\",\"password\":\"p\"}] }, '*');",
        Duration::from_secs(20),
    );
    load.wait_complete();
    let (state, _) = load.eval(
        "String(window.ran) + ',' + document.getElementById('user').value + ',' + document.readyState",
        Duration::from_secs(5),
    );
    assert_eq!(state.as_deref(), Some("String(\"true,typed,complete\")"));
}
