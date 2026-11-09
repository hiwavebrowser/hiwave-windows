//! The content thread (threads plan M4, second PR).
//!
//! `HIWAVE_CONTENT_THREAD=1` runs the page on one dedicated thread. The
//! window sends [`PageCmd`] and presents [`PageEvent`]s; it does not wait
//! for a load, a script or a fetch. The flag is off unless that variable
//! is set, so the same-thread drain stays the default until a Mac has
//! accepted the threaded window.
//!
//! The engine is constructed on the content thread. The presenter is not:
//! a page engine publishes [`crate::FrameCommit`]s and the window draws
//! them. Per-tab threads are M6; this is the one shared content thread.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use rustkit_image::ImageManager;
use rustkit_js::ViewControl;
use rustkit_viewhost::Bounds;
use tokio::sync::oneshot;
use url::Url;

use crate::nav_cancel::NavCanceller;
use crate::page_protocol::{adopt, coalesce};
use crate::devtools_service::{
    ask_console as devtools_ask_console, ask_network as devtools_ask_network, watch as devtools_watch, DevtoolsCount, DevtoolsCounters,
};
use crate::inspector::Inspector;
use crate::{DevtoolsAnswer, DevtoolsQuery, EngineBuilder, EngineViewId, PageCmd, PageEvent, PageInput};

/// `HIWAVE_CONTENT_THREAD=1` (or true/on/yes). Anything else, including an
/// unset variable, leaves the page on the window thread.
pub fn content_thread_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| {
        matches!(
            std::env::var("HIWAVE_CONTENT_THREAD").ok().as_deref(),
            Some("1" | "true" | "on" | "yes")
        )
    })
}

/// Native stack requested for the content thread.
///
/// `HIWAVE_CONTENT_STACK_MB`, default 64, clamped to 8..=512. A requested
/// size is not a measurement of how deep a host re-entry can go.
///
/// A 4,000-deep `__rk_reenter` climb on this stack used 25,864,544 bytes in
/// a debug build (6,466 bytes/frame, 38.5% of 64 MiB) and 4,355,104 bytes in
/// release (1,089 bytes/frame, 6.5%). The host re-entry cap stays 1,024.
/// That cap also binds the flag-off page, which still runs on the window
/// thread's stack, and 1,024 debug frames are already about 6.6 MiB.
pub fn content_stack_bytes() -> usize {
    let mb = std::env::var("HIWAVE_CONTENT_STACK_MB")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(64);
    (mb.clamp(8, 512) as usize).saturating_mul(1024 * 1024)
}

#[derive(Debug)]
pub enum ContentThreadError {
    Spawn(String),
    Engine(String),
}

#[derive(Clone)]
struct Boot {
    control: Arc<ViewControl>,
    canceller: NavCanceller,
    images: Arc<ImageManager>,
    /// The view's Inspector, for reads the page need not answer.
    inspector: Arc<Inspector>,
    devtools_counters: Arc<DevtoolsCounters>,
    #[allow(dead_code)]
    view: EngineViewId,
    thread: std::thread::ThreadId,
}

/// The window's handle on the content thread.
///
/// Dropping it signals Stop and Close and returns. It does not join the
/// thread: a caller that is the window must not wait out page work.
pub struct ContentThread {
    cmd: CmdSender,
    events: Receiver<PageEvent>,
    boot_rx: Mutex<Option<Receiver<Result<Boot, String>>>>,
    boot: Mutex<Option<Result<Boot, String>>>,
    nav_seq: Arc<AtomicU64>,
    join: Mutex<Option<JoinHandle<()>>>,
    exited: Arc<AtomicBool>,
    stack_bytes: usize,
}

/// The window's end of the page's command channel. Every command also
/// wakes the content thread, which otherwise sleeps until its page's next
/// timer (up to 50 ms) before it looks at the channel again (CH3.A9: the
/// RustKit chrome's Cmd+T waited out that sleep).
struct CmdSender {
    tx: Sender<PageCmd>,
    wake: Arc<tokio::sync::Notify>,
}

impl CmdSender {
    fn send(&self, cmd: PageCmd) -> Result<(), mpsc::SendError<PageCmd>> {
        let sent = self.tx.send(cmd);
        // A permit is kept when the thread is not waiting yet, so a
        // command sent during a turn still ends the sleep after it.
        self.wake.notify_one();
        sent
    }
}

impl ContentThread {
    pub fn spawn(builder: EngineBuilder, bounds: Bounds) -> Result<Self, ContentThreadError> {
        Self::spawn_with_stack(builder, bounds, content_stack_bytes())
    }

    pub fn spawn_with_stack(
        builder: EngineBuilder,
        bounds: Bounds,
        stack_bytes: usize,
    ) -> Result<Self, ContentThreadError> {
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let cmd_wake = Arc::new(tokio::sync::Notify::new());
        let cmd_wake_for_thread = Arc::clone(&cmd_wake);
        let (ev_tx, ev_rx) = mpsc::channel();
        let (boot_tx, boot_rx) = mpsc::channel();
        let nav_seq = Arc::new(AtomicU64::new(0));
        let nav_for_thread = Arc::clone(&nav_seq);
        let exited = Arc::new(AtomicBool::new(false));
        let exited_for_thread = Arc::clone(&exited);
        // pthread on Linux accepts a 1-byte request and the thread then
        // aborts the process. Refuse anything under the usual minimum
        // before spawn, and still surface a real spawn error.
        if stack_bytes < 16 * 1024 {
            return Err(ContentThreadError::Spawn(format!(
                "stack of {stack_bytes} bytes is below the 16 KiB minimum"
            )));
        }
        let join = std::thread::Builder::new()
            .name("page".into())
            .stack_size(stack_bytes)
            .spawn(move || {
                let built = (|| -> Result<(), String> {
                    let mut engine = builder
                        .build_page(nav_for_thread)
                        .map_err(|e| e.to_string())?;
                    let view = engine.create_page_view(bounds).map_err(|e| e.to_string())?;
                    let images = engine.image_manager();
                    let control = engine
                        .view_control(view)
                        .ok_or_else(|| "view has no control".to_string())?;
                    let canceller = engine
                        .nav_canceller(view)
                        .ok_or_else(|| "view has no canceller".to_string())?;
                    let inspector = engine
                        .inspector(view)
                        .ok_or_else(|| "view has no inspector".to_string())?;
                    let mut endpoint = adopt(engine, view, cmd_rx, ev_tx);
                    endpoint.set_settle_keys(true);
                    endpoint.set_wake_window(true);
                    endpoint.set_command_wake(cmd_wake_for_thread);
                    let _ = boot_tx.send(Ok(Boot {
                        control,
                        canceller,
                        images,
                        inspector,
                        devtools_counters: endpoint.devtools_counters(),
                        view,
                        thread: std::thread::current().id(),
                    }));
                    let runtime = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .map_err(|e| e.to_string())?;
                    runtime.block_on(endpoint.run());
                    Ok(())
                })();
                if let Err(error) = built {
                    let _ = boot_tx.send(Err(error));
                }
                exited_for_thread.store(true, Ordering::Release);
            })
            .map_err(|e| ContentThreadError::Spawn(e.to_string()))?;
        Ok(Self {
            cmd: CmdSender { tx: cmd_tx, wake: cmd_wake },
            events: ev_rx,
            boot_rx: Mutex::new(Some(boot_rx)),
            boot: Mutex::new(None),
            nav_seq,
            join: Mutex::new(Some(join)),
            exited,
            stack_bytes,
        })
    }

    pub fn stack_bytes(&self) -> usize {
        self.stack_bytes
    }

    pub fn exited(&self) -> bool {
        self.exited.load(Ordering::Acquire)
    }

    /// The thread the page was constructed on, once that construction has
    /// finished. Never blocks.
    pub fn page_thread(&self) -> Option<std::thread::ThreadId> {
        self.boot()
            .and_then(|boot| boot.ok())
            .map(|boot| boot.thread)
    }

    pub fn images(&self) -> Option<Arc<ImageManager>> {
        self.boot()
            .and_then(|boot| boot.ok())
            .map(|boot| boot.images)
    }

    fn boot(&self) -> Option<Result<Boot, String>> {
        let mut cached = self.boot.lock().unwrap_or_else(|e| e.into_inner());
        if cached.is_none() {
            let rx = self.boot_rx.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(rx) = rx.as_ref() {
                if let Ok(message) = rx.try_recv() {
                    *cached = Some(message);
                }
            }
        }
        cached.clone()
    }

    fn signal_stop(&self) {
        if let Some(Ok(boot)) = self.boot() {
            boot.control.stop_all();
            boot.canceller.cancel_current();
        }
    }

    /// Allocate the next navigation id, cancel the one before it, and stop
    /// a script that is running now. The command is queued after those
    /// signals. Returns without waiting for the load.
    pub fn navigate(&self, url: Url) -> u64 {
        let nav_id = self.nav_seq.fetch_add(1, Ordering::AcqRel) + 1;
        if let Some(Ok(boot)) = self.boot() {
            if nav_id > 1 {
                boot.canceller.cancel_nav(nav_id - 1);
            }
            boot.control.stop_all();
        }
        let _ = self.cmd.send(PageCmd::Navigate { url, nav_id });
        nav_id
    }

    /// Interrupt a running script and cancel the load, then queue Stop.
    /// Returns without waiting for the page to drain the command.
    pub fn stop(&self) {
        self.signal_stop();
        let (reply, _rx) = oneshot::channel();
        let _ = self.cmd.send(PageCmd::Stop { reply });
    }

    /// Signal interrupt and cancel, queue Close, and return. Does not join.
    pub fn shutdown(&self) {
        self.signal_stop();
        let _ = self.cmd.send(PageCmd::Close);
    }

    pub fn poll(&self) -> Vec<PageEvent> {
        let mut out = Vec::new();
        while let Ok(event) = self.events.try_recv() {
            out.push(event);
        }
        out
    }

    pub fn eval(&self, script: &str) -> oneshot::Receiver<Result<String, String>> {
        let (reply, rx) = oneshot::channel();
        let _ = self.cmd.send(PageCmd::Eval {
            script: script.to_string(),
            reply,
        });
        rx
    }

    /// The Inspector console's input line: cancelled, not run, if the page
    /// reaches it at or past `deadline` (T6c).
    pub fn devtools_eval(&self, script: &str, deadline: Instant) -> oneshot::Receiver<Result<String, String>> {
        let (reply, rx) = oneshot::channel();
        let _ = self.cmd.send(PageCmd::DevtoolsEval {
            script: script.to_string(),
            deadline,
            reply,
        });
        rx
    }

    /// The newest navigation id this handle allocated (by `navigate`,
    /// `load_html` or a reload). Only the window allocates ids, so right
    /// after one of those calls this is the id it used.
    pub fn last_nav_id(&self) -> u64 {
        self.nav_seq.load(Ordering::Acquire)
    }

    pub fn load_html(&self, html: &str) -> oneshot::Receiver<Result<(), String>> {
        let (reply, rx) = oneshot::channel();
        // Allocated here, not when the page later runs the command. A
        // navigation queued behind this one then has the newer id, and a
        // default document that lost the race does not replace it.
        let nav_id = self.nav_seq.fetch_add(1, Ordering::AcqRel) + 1;
        let newest = self.nav_seq.load(Ordering::Acquire);
        if newest == nav_id {
            if let Some(Ok(boot)) = self.boot() {
                if nav_id > 1 {
                    boot.canceller.cancel_nav(nav_id - 1);
                }
            }
        }
        let _ = self.cmd.send(PageCmd::LoadHtml {
            html: html.to_string(),
            nav_id,
            reply,
        });
        rx
    }

    pub fn resize(&self, bounds: Bounds, scale: f64) {
        let (reply, _rx) = oneshot::channel();
        let _ = self.cmd.send(PageCmd::Resize {
            bounds,
            scale,
            reply,
        });
    }

    pub fn input(&self, input: PageInput) {
        let _ = self.cmd.send(PageCmd::Input(input));
    }

    /// CH3.A7: the app owns this view (see [`crate::Engine::set_app_owned`]).
    /// Queued like any command, so it applies to every document loaded
    /// after it.
    pub fn set_app_owned(&self, owned: bool) {
        let _ = self.cmd.send(PageCmd::SetAppOwned { owned });
    }

    pub fn set_host_reentry(&self, limit: usize) {
        let _ = self.cmd.send(PageCmd::SetHostReentry { limit });
    }

    pub fn install_probe(&self) -> oneshot::Receiver<Result<(), String>> {
        let (reply, rx) = oneshot::channel();
        let _ = self.cmd.send(PageCmd::InstallReentryProbe { reply });
        rx
    }

    pub fn scroll(&self, dx: f32, dy: f32) {
        let (reply, _rx) = oneshot::channel();
        self.input(PageInput::Scroll { dx, dy, reply });
    }

    /// Queue a command. Does not signal cancel and does not wait.
    pub fn enqueue(&self, cmd: PageCmd) {
        let _ = self.cmd.send(cmd);
    }

    /// Ask a developer-tools question. Console reads are answered by the
    /// devtools thread from the view's Inspector, without the page (T6a);
    /// the rest are page commands. Before the page has booted every
    /// question is a page command, answered once it runs.
    pub fn devtools_ask(&self, query: DevtoolsQuery) -> oneshot::Receiver<DevtoolsAnswer> {
        if let (DevtoolsQuery::Console { since }, Some(Ok(boot))) = (&query, self.boot()) {
            return devtools_ask_console(boot.inspector, *since, Some(boot.devtools_counters));
        }
        if let (DevtoolsQuery::Network, Some(Ok(boot))) = (&query, self.boot()) {
            return devtools_ask_network(boot.inspector);
        }
        // An open panel is woken when the view records, not on a clock (T6b).
        if let (DevtoolsQuery::Attach(on), Some(Ok(boot))) = (&query, self.boot()) {
            devtools_watch(&boot.inspector, &boot.devtools_counters, *on);
        }
        let (reply, rx) = oneshot::channel();
        let _ = self.cmd.send(PageCmd::Devtools { query, reply });
        rx
    }

    /// The view's Inspector, once the page has booted. Read its
    /// [`Inspector::watermark`] to know whether there is anything new.
    pub fn devtools_inspector(&self) -> Option<Arc<Inspector>> {
        self.boot().and_then(|boot| boot.ok()).map(|boot| boot.inspector)
    }

    /// What devtools has cost the page so far; zero before it has booted.
    pub fn devtools_counters(&self) -> DevtoolsCount {
        self.boot()
            .and_then(|boot| boot.ok())
            .map(|boot| boot.devtools_counters.read())
            .unwrap_or_default()
    }
}

impl Drop for ContentThread {
    fn drop(&mut self) {
        self.shutdown();
        // Detach. Joining here would wait out whatever the page is doing.
        let _ = self.join.lock().unwrap_or_else(|e| e.into_inner()).take();
    }
}

/// Events whose navigation is older than `accepted` are a late result.
#[cfg(test)]
pub fn reject_stale(events: Vec<PageEvent>, accepted: u64) -> Vec<PageEvent> {
    events
        .into_iter()
        .filter(|event| match event {
            PageEvent::Title { nav_id, .. }
            | PageEvent::Url { nav_id, .. }
            | PageEvent::LoadState { nav_id, .. }
            | PageEvent::NavigateRequested { nav_id, .. } => *nav_id == 0 || *nav_id >= accepted,
            PageEvent::Commit(commit) => {
                commit.nav_generation == 0 || commit.nav_generation >= accepted
            }
            _ => true,
        })
        .collect()
}

#[cfg(test)]
fn wait_recv<T>(rx: &mut oneshot::Receiver<T>, budget: Duration) -> Option<T> {
    let deadline = Instant::now() + budget;
    loop {
        match rx.try_recv() {
            Ok(value) => return Some(value),
            Err(oneshot::error::TryRecvError::Closed) => return None,
            Err(oneshot::error::TryRecvError::Empty) => {
                if Instant::now() >= deadline {
                    return None;
                }
                std::thread::sleep(Duration::from_millis(2));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{reject_stale, wait_recv, *};
    use crate::{EngineBuilder, LoadState, PageCmd, PageInput};

    fn page() -> ContentThread {
        ContentThread::spawn(
            EngineBuilder::new()
                .script_budget_ms(5_000)
                .interrupt_scripts_at_budget(true),
            Bounds {
                x: 0,
                y: 0,
                width: 800,
                height: 600,
            },
        )
        .expect("content thread spawns")
    }

    #[test]
    fn a_too_small_stack_fails_the_spawn() {
        let error = ContentThread::spawn_with_stack(
            EngineBuilder::new(),
            Bounds {
                x: 0,
                y: 0,
                width: 1,
                height: 1,
            },
            1,
        );
        assert!(error.is_err(), "spawn reported success for a 1-byte stack");
    }

    #[test]
    fn the_page_is_built_on_the_content_thread_and_events_stay_ordered() {
        let page = page();
        let caller = std::thread::current().id();
        let started = Instant::now();
        let mut loaded = page.load_html("<p id=\"t\">hi</p>");
        let mut text = page.eval("document.getElementById('t').textContent");
        assert!(
            started.elapsed() < Duration::from_millis(50),
            "queueing the load and the eval waited on the page"
        );
        let load = wait_recv(&mut loaded, Duration::from_secs(10)).expect("load reply");
        assert!(load.is_ok(), "{load:?}");
        let text = wait_recv(&mut text, Duration::from_secs(10))
            .expect("eval reply")
            .expect("eval");
        assert!(text.contains("hi"), "{text}");
        let owner = page.page_thread().expect("boot");
        assert_ne!(owner, caller, "the page ran on the caller");
        let _ = page.poll();
    }

    #[test]
    fn load_html_runs_the_documents_inline_scripts() {
        // Red on develop ff6ad36a (dev-tools lane, 2026-10-25): the app
        // loads New Tab, About and the report page (Chart.js inline) with
        // `load_html`, which reaches `Engine::load_html_as`. That set up the
        // bindings and laid out but ran no `<script>`.
        let page = page();
        let mut loaded = page.load_html(
            "<p id=\"t\">before</p><script>document.getElementById('t').textContent = 'ran';</script>",
        );
        let load = wait_recv(&mut loaded, Duration::from_secs(10)).expect("load reply");
        assert!(load.is_ok(), "{load:?}");
        let mut text = page.eval("document.getElementById('t').textContent");
        let text = wait_recv(&mut text, Duration::from_secs(10))
            .expect("eval reply")
            .expect("eval");
        assert!(text.contains("ran"), "the inline script did not run: {text}");
    }

    #[test]
    fn load_html_fires_domcontentloaded_and_load_after_its_scripts() {
        // The report page draws its charts from a `DOMContentLoaded`
        // listener, so the lifecycle events matter as much as the scripts.
        let page = page();
        let mut loaded = page.load_html(
            "<p id=\"t\"></p><script>\
             var seen = [document.readyState];\
             document.addEventListener('DOMContentLoaded', function () { seen.push('dcl:' + document.readyState); });\
             window.addEventListener('load', function () { seen.push('load:' + document.readyState); });\
             </script><script>seen.push('second');</script>",
        );
        let load = wait_recv(&mut loaded, Duration::from_secs(10)).expect("load reply");
        assert!(load.is_ok(), "{load:?}");
        let mut seen = page.eval("seen.join(',')");
        let seen = wait_recv(&mut seen, Duration::from_secs(10))
            .expect("eval reply")
            .expect("eval");
        assert!(
            seen.contains("loading,second,dcl:interactive,load:complete"),
            "scripts in order, then DOMContentLoaded, then load: {seen}"
        );
    }

    #[test]
    fn about_page_script_runs_and_sets_the_version_line() {
        // about.html (New Tab / About in RustKit content mode) exposes
        // `hiwaveAbout.setVersion`, which the shell calls with the version.
        let page = page();
        let mut loaded = page.load_html(include_str!("../../hiwave-app/src/ui/about.html"));
        let load = wait_recv(&mut loaded, Duration::from_secs(10)).expect("load reply");
        assert!(load.is_ok(), "{load:?}");
        let mut version = page.eval(
            "window.hiwaveAbout.setVersion('v9.8.7'); document.getElementById('versionBadge').textContent",
        );
        let version = wait_recv(&mut version, Duration::from_secs(10))
            .expect("eval reply")
            .expect("the about page's script defined hiwaveAbout");
        assert!(version.contains("v9.8.7"), "version line: {version}");
    }

    #[test]
    fn report_page_runs_inline_chart_js_and_its_dom_content_loaded_listener() {
        // Prometheus's #799 nit 1: the report page as the shell loads it
        // (`load_report_page`): Chart.js (200 KB) inlined in a `<script>`,
        // then the page's script, which does its work from DOMContentLoaded.
        let html = include_str!("../../hiwave-app/src/ui/report.html").replace(
            "<!-- Chart.js is injected by Rust before page load -->",
            &format!("<script>{}</script>", include_str!("../../hiwave-app/src/ui/chart.umd.min.js")),
        );
        let page = page();
        let mut loaded = page.load_html(&html);
        let load = wait_recv(&mut loaded, Duration::from_secs(20)).expect("load reply");
        assert!(load.is_ok(), "{load:?}");
        // CH3.A7: the content view is not app-owned, so the page has no
        // `window.ipc`: its DOMContentLoaded `loadReport` fails at once and
        // says so (before, it waited forever on an IPC nothing read).
        let mut seen = page.eval(
            "typeof Chart + ',' + typeof loadReport + ',' + document.getElementById('loading').style.display \
             + ',' + /Failed to load analytics data/.test(document.getElementById('error-container').textContent)",
        );
        let seen = wait_recv(&mut seen, Duration::from_secs(10))
            .expect("eval reply")
            .expect("eval");
        assert!(seen.contains("function,function,none,true"), "Chart.js, the page script, DOMContentLoaded: {seen}");
    }

    #[test]
    fn stop_during_a_script_returns_and_a_later_eval_runs() {
        let page = page();
        let mut loaded = page.load_html("<p id=\"t\">x</p>");
        assert!(wait_recv(&mut loaded, Duration::from_secs(10))
            .unwrap()
            .is_ok());
        let mut spinning = page.eval("while (true) { document.getElementById('t'); }");
        std::thread::sleep(Duration::from_millis(30));
        let started = Instant::now();
        page.stop();
        assert!(
            started.elapsed() < Duration::from_millis(50),
            "stop waited for the script"
        );
        let stopped = wait_recv(&mut spinning, Duration::from_secs(5));
        assert!(stopped.is_some(), "the spin never unwound");
        let mut after = page.eval("1 + 1");
        let value = wait_recv(&mut after, Duration::from_secs(5))
            .expect("runtime recovered")
            .expect("eval");
        assert!(value.contains('2'), "{value}");
    }

    #[test]
    fn a_navigation_that_fails_leaves_the_page_on_screen_running() {
        // Independent R1 on #730, BLOCKER: `navigate` raises ALL so the old
        // document's script stops, and only a commit lowered it. A fetch
        // that never commits (refused here) left the page on screen dead.
        let page = page();
        let mut loaded = page.load_html("<p id=\"t\">x</p>");
        assert!(wait_recv(&mut loaded, Duration::from_secs(10)).unwrap().is_ok());
        let nav_id = page.navigate(Url::parse("http://127.0.0.1:1/").unwrap());
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut failed = false;
        while Instant::now() < deadline && !failed {
            failed = page.poll().into_iter().any(|e| {
                matches!(e, PageEvent::LoadState { state: LoadState::Failed { .. }, nav_id: n } if n == nav_id)
            });
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(failed, "the window never heard that navigation {nav_id} failed");
        let mut after = page.eval("document.getElementById('t').textContent + (1 + 1)");
        let value = wait_recv(&mut after, Duration::from_secs(5))
            .expect("eval reply")
            .expect("the old document's script is still stopped after the failed navigation");
        assert!(value.contains("x2"), "{value}");
    }

    fn ask_devtools(page: &ContentThread, query: crate::DevtoolsQuery) -> crate::DevtoolsAnswer {
        let (reply, mut rx) = oneshot::channel();
        page.enqueue(PageCmd::Devtools { query, reply });
        wait_recv(&mut rx, Duration::from_secs(5)).expect("the page answered the Inspector")
    }

    fn find_by_id(node: &crate::DevtoolsNode, id: &str) -> Option<usize> {
        if node.id.as_deref() == Some(id) {
            return Some(node.node_id);
        }
        node.children.iter().find_map(|child| find_by_id(child, id))
    }

    #[test]
    fn the_inspector_is_answered_on_the_content_thread() {
        // #764 re-cut: the Inspector queried the engine on the window
        // thread, which has no engine once the page runs on its own thread
        // (the shipped default). Every question goes through the protocol.
        use crate::{DevtoolsAnswer, DevtoolsQuery};
        let page = page();
        let mut loaded = page.load_html(
            "<body style='margin:0'><div id=box class='a b' style='width:100px;height:50px;padding:5px'>hi</div></body>",
        );
        assert!(wait_recv(&mut loaded, Duration::from_secs(10)).unwrap().is_ok());
        assert_eq!(ask_devtools(&page, DevtoolsQuery::Attach(true)), DevtoolsAnswer::Done);
        let DevtoolsAnswer::Tree(Some(tree)) = ask_devtools(&page, DevtoolsQuery::DomTree { root: None, reveal: None })
        else {
            panic!("no DOM tree");
        };
        let node = find_by_id(&tree, "box").expect("the tree has #box");
        let DevtoolsAnswer::Element(Some(element)) = ask_devtools(&page, DevtoolsQuery::Element(node)) else {
            panic!("no element details");
        };
        assert_eq!(element.tag, "div");
        assert_eq!(element.classes, vec!["a".to_string(), "b".to_string()]);
        let rect = element.rect.expect("the element has a box");
        assert_eq!((rect.width, rect.height), (110.0, 60.0));
        assert_eq!(
            ask_devtools(&page, DevtoolsQuery::NodeAt { x: 20.0, y: 20.0 }),
            DevtoolsAnswer::Node(Some(node))
        );
        let _ = page.poll();
        assert_eq!(ask_devtools(&page, DevtoolsQuery::Highlight(Some(node))), DevtoolsAnswer::Done);
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut outlined = false;
        while Instant::now() < deadline && !outlined {
            outlined = page.poll().into_iter().any(|e| matches!(e, PageEvent::Commit(_)));
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(outlined, "the outline was never drawn into a new frame");
        let mut logged = page.eval("console.log('from the page'); 1");
        assert!(wait_recv(&mut logged, Duration::from_secs(5)).is_some());
        let DevtoolsAnswer::Console(console) = ask_devtools(&page, DevtoolsQuery::Console { since: 0 }) else {
            panic!("no console");
        };
        // The Inspector records a value's shape, not the page's text
        // (inspector.rs): the line is there, as `<string len=13>`.
        assert!(
            console.entries.iter().any(|e| e.level == "log" && e.message == "<string len=13>"),
            "{:?}",
            console.entries
        );
    }

    #[test]
    fn an_open_panel_on_an_idle_page_costs_the_page_no_devtools_commands() {
        // T6a (T6-design.md): every console read the open panel made was a
        // page command, four a second on an idle page (#785's 250 ms
        // CONSOLE_POLL). The reads now come from the view's Inspector on
        // the devtools thread, and the console still updates.
        use crate::{DevtoolsAnswer, DevtoolsQuery};
        let page = page();
        let mut loaded = page.load_html("<p id=t>idle</p>");
        assert!(wait_recv(&mut loaded, Duration::from_secs(10)).unwrap().is_ok());
        assert_eq!(ask_devtools(&page, DevtoolsQuery::Attach(true)), DevtoolsAnswer::Done);
        let before = page.devtools_counters();
        let (mut since, mut lines) = (0, Vec::new());
        let start = Instant::now();
        let mut logged_late = false;
        while start.elapsed() < Duration::from_secs(2) {
            if !logged_late && start.elapsed() >= Duration::from_secs(1) {
                logged_late = true;
                let mut done = page.eval("console.log('late'); 1");
                assert!(wait_recv(&mut done, Duration::from_secs(5)).is_some());
            }
            // The panel's clock (devtools_panel.rs, CONSOLE_POLL).
            let mut rx = page.devtools_ask(DevtoolsQuery::Console { since });
            let Some(DevtoolsAnswer::Console(console)) = wait_recv(&mut rx, Duration::from_secs(5)) else {
                panic!("the console read was not answered");
            };
            since = console.next_seq;
            lines.extend(console.entries.into_iter().map(|e| e.message));
            std::thread::sleep(Duration::from_millis(250));
        }
        let cost = page.devtools_counters();
        assert_eq!(
            cost.page_commands - before.page_commands,
            0,
            "devtools page commands in 2 s on an idle page with the panel open"
        );
        assert!(lines.iter().any(|m| m == "<string len=4>"), "the late line never reached the panel: {lines:?}");
    }

    /// The panel's console read, through the routing the window uses.
    fn read_console(page: &ContentThread, since: u64, budget: Duration) -> Option<crate::DevtoolsConsole> {
        let mut rx = page.devtools_ask(crate::DevtoolsQuery::Console { since });
        match wait_recv(&mut rx, budget) {
            Some(crate::DevtoolsAnswer::Console(console)) => Some(console),
            _ => None,
        }
    }

    #[test]
    fn a_console_read_is_answered_while_the_page_thread_is_blocked() {
        // T6b (T6-design.md): the panel stays usable when the page is hung.
        // The page thread sits in a 3 s host-side wait; the console read
        // is answered from the view's Inspector meanwhile.
        use crate::{DevtoolsAnswer, DevtoolsQuery};
        let page = page();
        let mut loaded = page.load_html("<p>x</p>");
        assert!(wait_recv(&mut loaded, Duration::from_secs(10)).unwrap().is_ok());
        let mut attached = page.devtools_ask(DevtoolsQuery::Attach(true));
        assert_eq!(wait_recv(&mut attached, Duration::from_secs(5)), Some(DevtoolsAnswer::Done));
        let mut logged = page.eval("console.log('before'); 1");
        assert!(wait_recv(&mut logged, Duration::from_secs(5)).is_some());
        page.enqueue(PageCmd::Block(3_000));
        // Queued behind the block: no answer until it ends.
        let mut behind = page.eval("1");
        std::thread::sleep(Duration::from_millis(100));
        let asked = Instant::now();
        let console = read_console(&page, 0, Duration::from_secs(1)).expect("no console answer while the page was blocked");
        assert!(asked.elapsed() < Duration::from_secs(1), "the read waited for the page");
        assert!(
            matches!(behind.try_recv(), Err(oneshot::error::TryRecvError::Empty)),
            "the page was not in its block when the read was served"
        );
        assert!(
            console.entries.iter().any(|e| e.message == "<string len=6>"),
            "the line logged before the block is missing: {:?}",
            console.entries
        );
    }

    #[test]
    fn a_devtools_eval_reached_past_its_deadline_is_cancelled_not_run_late() {
        // T6c, R1 condition 5: the page is busy (a 1.5 s host-side block)
        // when the panel's console line arrives with a 300 ms deadline. The
        // page reaches it late and must not run it; one in time still runs.
        let page = page();
        let mut loaded = page.load_html("<p>x</p>");
        assert!(wait_recv(&mut loaded, Duration::from_secs(10)).unwrap().is_ok());
        page.enqueue(PageCmd::Block(1_500));
        let mut late = page.devtools_eval("window.ranLate = 1; 'ran'", Instant::now() + Duration::from_millis(300));
        let answer = wait_recv(&mut late, Duration::from_secs(5)).expect("no answer to the late eval");
        assert_eq!(answer, Err(crate::EVAL_CANCELLED.to_string()));
        let mut check = page.devtools_eval("String(window.ranLate)", Instant::now() + Duration::from_secs(5));
        let ran = wait_recv(&mut check, Duration::from_secs(5)).expect("no answer").expect("eval");
        assert!(ran.contains("undefined"), "the cancelled eval ran anyway: {ran}");
    }

    #[test]
    fn a_page_thread_panic_holding_the_inspector_lock_does_not_blank_the_panel() {
        // T6 R1 condition 2: the page thread panics while it holds the
        // view's Inspector lock, with the panel open. The panel still reads
        // what was recorded, and is told the page crashed.
        use crate::{DevtoolsAnswer, DevtoolsQuery};
        let page = page();
        let mut loaded = page.load_html("<p>x</p>");
        assert!(wait_recv(&mut loaded, Duration::from_secs(10)).unwrap().is_ok());
        let mut attached = page.devtools_ask(DevtoolsQuery::Attach(true));
        assert_eq!(wait_recv(&mut attached, Duration::from_secs(5)), Some(DevtoolsAnswer::Done));
        let mut logged = page.eval("console.log('before'); 1");
        assert!(wait_recv(&mut logged, Duration::from_secs(5)).is_some());
        let inspector = page.devtools_inspector().expect("booted");
        assert!(!inspector.page_crashed());
        page.enqueue(PageCmd::PanicHoldingInspector);
        let deadline = Instant::now() + Duration::from_secs(5);
        while !inspector.page_crashed() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(inspector.page_crashed(), "the page thread never panicked");
        let console = read_console(&page, 0, Duration::from_secs(2)).expect("no console answer after the crash");
        assert!(console.page_crashed, "the answer does not say the page crashed");
        assert!(
            console.entries.iter().any(|e| e.message == "<string len=6>"),
            "the panel lost what was recorded before the crash: {:?}",
            console.entries
        );
    }

    #[test]
    fn an_open_panel_is_woken_when_the_view_records_and_not_otherwise() {
        // T6b: no CONSOLE_POLL. With the panel watching, an idle page posts
        // no window wakes; a console line posts one, and the next is not
        // posted until the panel has read.
        use crate::{DevtoolsAnswer, DevtoolsQuery};
        let page = page();
        let mut loaded = page.load_html("<p>idle</p>");
        assert!(wait_recv(&mut loaded, Duration::from_secs(10)).unwrap().is_ok());
        let mut attached = page.devtools_ask(DevtoolsQuery::Attach(true));
        assert_eq!(wait_recv(&mut attached, Duration::from_secs(5)), Some(DevtoolsAnswer::Done));
        // What the page recorded before the panel opened: read it once.
        std::thread::sleep(Duration::from_millis(200));
        let inspector = page.devtools_inspector().expect("booted");
        let since = read_console(&page, 0, Duration::from_secs(5)).expect("first read").next_seq;
        std::thread::sleep(Duration::from_millis(200));
        let idle_from = page.devtools_counters();
        std::thread::sleep(Duration::from_secs(1));
        let idle = page.devtools_counters();
        assert_eq!(idle.window_wakes - idle_from.window_wakes, 0, "window wakes in 1 s on an idle page");
        assert_eq!(idle.page_commands - idle_from.page_commands, 0);
        // Three lines, no read: one wake.
        let mut done = page.eval("console.log('a'); console.log('b'); console.log('c'); 1");
        assert!(wait_recv(&mut done, Duration::from_secs(5)).is_some());
        let deadline = Instant::now() + Duration::from_secs(2);
        while page.devtools_counters().window_wakes == idle.window_wakes && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        std::thread::sleep(Duration::from_millis(200));
        let woken = page.devtools_counters();
        assert_eq!(woken.window_wakes - idle.window_wakes, 1, "one wake for unread lines");
        assert!(inspector.watermark() > since);
        // The panel reads (its answer wakes the window once more), then a
        // new line wakes it again.
        let console = read_console(&page, since, Duration::from_secs(5)).expect("read");
        assert_eq!(console.entries.len(), 3, "{:?}", console.entries);
        std::thread::sleep(Duration::from_millis(200));
        let read = page.devtools_counters();
        let mut done = page.eval("console.log('d'); 1");
        assert!(wait_recv(&mut done, Duration::from_secs(5)).is_some());
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(page.devtools_counters().window_wakes - read.window_wakes, 1, "a new line after a read wakes");
        assert_eq!(page.devtools_counters().page_commands, idle.page_commands, "no console read reached the page");
    }

    #[test]
    fn a_panel_attached_while_the_page_boots_is_still_watched() {
        // #803 review (2): before `boot()` returns the window cannot reach
        // the Inspector, so it made no watch, and an idle page never woke
        // the panel. The page now watches when it handles the attach.
        use crate::DevtoolsQuery;
        let page = page();
        let before_boot = page.boot().is_none();
        let _attached = page.devtools_ask(DevtoolsQuery::Attach(true));
        assert!(before_boot, "the page booted before the attach: this test checks nothing");
        let mut loaded = page.load_html("<p>idle</p>");
        assert!(wait_recv(&mut loaded, Duration::from_secs(10)).unwrap().is_ok());
        std::thread::sleep(Duration::from_millis(300));
        let since = read_console(&page, 0, Duration::from_secs(5)).expect("first read").next_seq;
        std::thread::sleep(Duration::from_millis(200));
        let from = page.devtools_counters().window_wakes;
        let mut done = page.eval("console.log('late'); 1");
        assert!(wait_recv(&mut done, Duration::from_secs(5)).is_some());
        let deadline = Instant::now() + Duration::from_secs(3);
        while page.devtools_counters().window_wakes == from && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(page.devtools_counters().window_wakes > from, "a line after boot never woke the panel");
        let console = read_console(&page, since, Duration::from_secs(5)).expect("read");
        assert!(console.entries.iter().any(|e| e.message == "<string len=4>"), "{:?}", console.entries);
    }

    #[test]
    fn with_the_ring_off_a_closed_panel_drops_its_session() {
        // #803 review nit: with the tab ring off nothing reads a closed
        // panel's records, so they are not kept for the life of the view.
        use crate::{DevtoolsAnswer, DevtoolsQuery};
        let page = page();
        let mut loaded = page.load_html("<p>one</p>");
        assert!(wait_recv(&mut loaded, Duration::from_secs(10)).unwrap().is_ok());
        let mut attached = page.devtools_ask(DevtoolsQuery::Attach(true));
        assert_eq!(wait_recv(&mut attached, Duration::from_secs(5)), Some(DevtoolsAnswer::Done));
        let mut logged = page.eval("console.log('one'); 1");
        assert!(wait_recv(&mut logged, Duration::from_secs(5)).is_some());
        let inspector = page.devtools_inspector().expect("booted");
        assert!(!inspector.snapshot_since(0).records.is_empty());
        let mut detached = page.devtools_ask(DevtoolsQuery::Attach(false));
        assert_eq!(wait_recv(&mut detached, Duration::from_secs(5)), Some(DevtoolsAnswer::Done));
        assert!(inspector.snapshot_since(0).records.is_empty(), "the closed panel's records were kept");
    }

    /// A page with the tab ring on (`HIWAVE_DEVTOOLS_RING=1`).
    fn page_with_ring() -> ContentThread {
        let mut config = EngineBuilder::new()
            .script_budget_ms(5_000)
            .interrupt_scripts_at_budget(true)
            .config()
            .clone();
        config.inspector.buffer = true;
        ContentThread::spawn(
            EngineBuilder::new().with_config(config),
            Bounds {
                x: 0,
                y: 0,
                width: 800,
                height: 600,
            },
        )
        .expect("content thread spawns")
    }

    #[test]
    fn the_panel_reopens_with_history_marked_by_document() {
        // T6b: with the tab ring on, closing the panel does not forget, and
        // records carry their document, so a panel opened after a navigation
        // shows both pages' lines and can tell them apart.
        use crate::{DevtoolsAnswer, DevtoolsQuery};
        let page = page_with_ring();
        let mut attached = page.devtools_ask(DevtoolsQuery::Attach(true));
        let _ = wait_recv(&mut attached, Duration::from_secs(5));
        let mut loaded = page.load_html("<p>one</p>");
        assert!(wait_recv(&mut loaded, Duration::from_secs(10)).unwrap().is_ok());
        let mut logged = page.eval("console.log('one'); 1");
        assert!(wait_recv(&mut logged, Duration::from_secs(5)).is_some());
        let mut detached = page.devtools_ask(DevtoolsQuery::Attach(false));
        assert_eq!(wait_recv(&mut detached, Duration::from_secs(5)), Some(DevtoolsAnswer::Done));
        let mut loaded = page.load_html("<p>two</p>");
        assert!(wait_recv(&mut loaded, Duration::from_secs(10)).unwrap().is_ok());
        let mut logged = page.eval("console.log('second'); 1");
        assert!(wait_recv(&mut logged, Duration::from_secs(5)).is_some());
        let mut attached = page.devtools_ask(DevtoolsQuery::Attach(true));
        assert_eq!(wait_recv(&mut attached, Duration::from_secs(5)), Some(DevtoolsAnswer::Done));
        let console = read_console(&page, 0, Duration::from_secs(5)).expect("read");
        let doc_of = |len: usize| {
            let shape = format!("<string len={len}>");
            console.entries.iter().find(|e| e.message == shape).map(|e| e.doc)
        };
        let (first, second) = (doc_of(3), doc_of(6));
        assert!(first.is_some(), "the first page's line was forgotten: {:?}", console.entries);
        assert!(second.is_some(), "the second page's line is missing: {:?}", console.entries);
        assert!(second > first, "documents not told apart: {first:?} vs {second:?}");
    }

    /// Events until `nav_id` fails or `budget` runs out.
    fn events_until_failed(page: &ContentThread, nav_id: u64, budget: Duration) -> Vec<PageEvent> {
        let deadline = Instant::now() + budget;
        let mut events = Vec::new();
        while Instant::now() < deadline {
            events.extend(page.poll());
            let failed = events.iter().any(|e| {
                matches!(e, PageEvent::LoadState { state: LoadState::Failed { .. }, nav_id: n } if *n == nav_id)
            });
            if failed {
                std::thread::sleep(Duration::from_millis(100));
                events.extend(page.poll());
                return events;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        events
    }

    fn wait_complete(page: &ContentThread, nav_id: u64) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            let done = page.poll().into_iter().any(|e| {
                matches!(e, PageEvent::LoadState { state: LoadState::Complete, nav_id: n } if n == nav_id)
            });
            if done {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("navigation {nav_id} never completed");
    }

    #[test]
    fn a_failed_navigation_puts_the_shown_documents_url_back() {
        // #730 follow-up (c): the address bar showed the URL that failed
        // over the document still on screen.
        let page = page();
        let shown = "data:text/html,<p id=t>shown</p>";
        let first = page.navigate(Url::parse(shown).unwrap());
        wait_complete(&page, first);
        let nav_id = page.navigate(Url::parse("http://127.0.0.1:1/").unwrap());
        let events = events_until_failed(&page, nav_id, Duration::from_secs(20));
        let last_url = events.iter().rev().find_map(|e| match e {
            PageEvent::Url { url, nav_id: n } if *n == nav_id => Some(url.to_string()),
            _ => None,
        });
        let shown_url = Url::parse(shown).unwrap().to_string();
        assert_eq!(
            last_url.as_deref(),
            Some(shown_url.as_str()),
            "the window was left showing the failed URL: {events:?}"
        );
    }

    /// A local server answering every request with `head` and an empty body.
    fn serve_empty(head: &'static str) -> u16 {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for stream in listener.incoming().take(8) {
                let Ok(mut stream) = stream else { continue };
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf);
                let _ = stream.write_all(
                    format!("{head}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").as_bytes(),
                );
            }
        });
        port
    }

    fn events_for(page: &ContentThread, nav_id: u64, budget: Duration) -> Vec<PageEvent> {
        let deadline = Instant::now() + budget;
        let mut events = Vec::new();
        while Instant::now() < deadline {
            events.extend(page.poll().into_iter().filter(|e| match e {
                PageEvent::LoadState { nav_id: n, .. } | PageEvent::Url { nav_id: n, .. } => *n == nav_id,
                _ => false,
            }));
            std::thread::sleep(Duration::from_millis(20));
        }
        events
    }

    fn failed_count(events: &[PageEvent]) -> usize {
        events
            .iter()
            .filter(|e| matches!(e, PageEvent::LoadState { state: LoadState::Failed { .. }, .. }))
            .count()
    }

    #[test]
    fn a_204_navigation_leaves_the_page_as_it_is_and_reports_no_failure() {
        // #730 follow-up (e), Prometheus R1 NIT: an Ok load that commits
        // nothing was reported Failed. HTML aborts a 204/205 navigation; the
        // document on screen stays, running, and the load just ends.
        let port = serve_empty("HTTP/1.1 204 No Content");
        let page = page();
        let shown = "data:text/html,<p id=t>shown</p>";
        let first = page.navigate(Url::parse(shown).unwrap());
        wait_complete(&page, first);
        let nav_id = page.navigate(Url::parse(&format!("http://127.0.0.1:{port}/ping")).unwrap());
        let events = events_for(&page, nav_id, Duration::from_secs(3));
        assert_eq!(failed_count(&events), 0, "a 204 was reported as a failure: {events:?}");
        assert!(
            events.iter().any(|e| matches!(e, PageEvent::LoadState { state: LoadState::Complete, .. })),
            "the window was never told the 204 navigation ended: {events:?}"
        );
        let mut text = page.eval("document.getElementById('t') ? document.getElementById('t').textContent : 'replaced'");
        let text = wait_recv(&mut text, Duration::from_secs(5)).expect("eval reply").expect("eval");
        assert!(text.contains("shown"), "the 204 replaced the document on screen: {text}");
    }

    #[test]
    fn a_205_navigation_leaves_the_page_as_it_is_and_reports_no_failure() {
        // #730 follow-up (e) / #780: HTML aborts 204 *and* 205 the same way.
        // Only 204 was pinned; a wrong status match would wipe the shown
        // document or report Failed on Reset Content.
        let port = serve_empty("HTTP/1.1 205 Reset Content");
        let page = page();
        let shown = "data:text/html,<p id=t>shown</p>";
        let first = page.navigate(Url::parse(shown).unwrap());
        wait_complete(&page, first);
        let nav_id = page.navigate(Url::parse(&format!("http://127.0.0.1:{port}/ping")).unwrap());
        let events = events_for(&page, nav_id, Duration::from_secs(3));
        assert_eq!(failed_count(&events), 0, "a 205 was reported as a failure: {events:?}");
        assert!(
            events.iter().any(|e| matches!(e, PageEvent::LoadState { state: LoadState::Complete, .. })),
            "the window was never told the 205 navigation ended: {events:?}"
        );
        let mut text = page.eval("document.getElementById('t') ? document.getElementById('t').textContent : 'replaced'");
        let text = wait_recv(&mut text, Duration::from_secs(5)).expect("eval reply").expect("eval");
        assert!(text.contains("shown"), "the 205 replaced the document on screen: {text}");
    }

    /// Content-thread Stop cancels the in-flight load first (`signal_stop`),
    /// so the page reports `Failed { "cancelled" }` — not `"stopped"` (that
    /// string is for `Engine::stop` on a yielded load_tail). The shown
    /// document must stay and the held navigation must not Complete.
    #[test]
    fn stop_during_a_held_navigation_fails_as_cancelled_and_keeps_the_shown_page() {
        let (port, release, seen) = serve(|path, release_flag| {
            if path == "/slow" {
                let start = Instant::now();
                while !release_flag.load(Ordering::Acquire)
                    && start.elapsed() < Duration::from_secs(8)
                {
                    std::thread::sleep(Duration::from_millis(5));
                }
                return (200, "text/html", b"<p id=t>slow</p>".to_vec());
            }
            (200, "text/html", b"<p id=t>shown</p>".to_vec())
        });
        let page = page();
        let first = page.navigate(Url::parse(&format!("http://127.0.0.1:{port}/")).unwrap());
        wait_complete(&page, first);

        let nav_id = page.navigate(Url::parse(&format!("http://127.0.0.1:{port}/slow")).unwrap());
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if seen
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .iter()
                .any(|p| p == "/slow")
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            seen.lock()
                .unwrap_or_else(|e| e.into_inner())
                .iter()
                .any(|p| p == "/slow"),
            "the held navigation never reached the server"
        );

        page.stop();
        release.store(true, Ordering::Release);

        let events = events_for(&page, nav_id, Duration::from_secs(3));
        assert_eq!(failed_count(&events), 1, "{events:?}");
        assert!(
            events.iter().any(|e| matches!(
                e,
                PageEvent::LoadState {
                    state: LoadState::Failed { error },
                    ..
                } if error == "cancelled"
            )),
            "content-thread Stop must fail the held load as cancelled: {events:?}"
        );
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, PageEvent::LoadState { state: LoadState::Complete, .. })),
            "the held navigation must not complete after Stop: {events:?}"
        );
        let mut text = page.eval(
            "document.getElementById('t') ? document.getElementById('t').textContent : 'gone'",
        );
        let text = wait_recv(&mut text, Duration::from_secs(5))
            .expect("eval reply")
            .expect("eval");
        assert!(
            text.contains("shown"),
            "Stop must leave the previous document on screen: {text}"
        );
    }

    #[test]
    fn a_failed_navigation_is_reported_failed_once() {
        // #730 follow-up (e): an empty error response failed in the engine
        // (one Failed) and again in end_uncommitted_nav (a second).
        let port = serve_empty("HTTP/1.1 500 Internal Server Error");
        let page = page();
        let first = page.navigate(Url::parse("data:text/html,<p>shown</p>").unwrap());
        wait_complete(&page, first);
        let nav_id = page.navigate(Url::parse(&format!("http://127.0.0.1:{port}/")).unwrap());
        let events = events_for(&page, nav_id, Duration::from_secs(3));
        assert_eq!(failed_count(&events), 1, "{events:?}");
    }

    /// The newest `Focused` report among the events seen within `budget`.
    fn last_focused(page: &ContentThread, budget: Duration) -> Option<bool> {
        let deadline = Instant::now() + budget;
        let mut last = None;
        while Instant::now() < deadline {
            for event in page.poll() {
                if let PageEvent::Focused(focused) = event {
                    last = Some(focused);
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        last
    }

    #[test]
    fn the_window_hears_focus_a_script_moved_and_a_new_document_has_none() {
        // #730 delta R1 NIT: the window answers "is a field focused" from
        // the page's last Focused report. A script's `focus()` sent none, so
        // the first key after it went to the window, and a new document
        // kept the old document's answer.
        let page = page();
        let mut loaded = page.load_html("<input id=\"i\"><p>x</p>");
        assert!(wait_recv(&mut loaded, Duration::from_secs(10)).unwrap().is_ok());
        let _ = page.poll();
        let mut focused = page.eval("document.getElementById('i').focus(); 1");
        assert!(wait_recv(&mut focused, Duration::from_secs(5)).is_some());
        assert_eq!(
            last_focused(&page, Duration::from_millis(300)),
            Some(true),
            "a script focused the field and the window was not told"
        );
        let mut timer = page.eval("document.getElementById('i').blur(); setTimeout(function () { document.getElementById('i').focus(); }, 20); 1");
        assert!(wait_recv(&mut timer, Duration::from_secs(5)).is_some());
        assert_eq!(
            last_focused(&page, Duration::from_millis(500)),
            Some(true),
            "a timer focused the field and the window was not told"
        );
        let mut next = page.load_html("<p>next</p>");
        assert!(wait_recv(&mut next, Duration::from_secs(10)).unwrap().is_ok());
        assert_eq!(
            last_focused(&page, Duration::from_millis(300)),
            Some(false),
            "the new document has no focused field and the window was not told"
        );
    }

    /// The newest `TextFocused` report among the events seen within `budget`.
    fn last_text_focused(page: &ContentThread, budget: Duration) -> Option<bool> {
        let deadline = Instant::now() + budget;
        let mut last = None;
        while Instant::now() < deadline {
            for event in page.poll() {
                if let PageEvent::TextFocused(text) = event {
                    last = Some(text);
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        last
    }

    #[test]
    fn the_window_hears_whether_the_focused_element_is_a_text_field() {
        // CH2.E1 R1: the window sends Cmd+A/C/V/X/Z and Cmd+arrows to the
        // page only for a focused text field, not a focused <select>.
        let page = page();
        let mut loaded = page.load_html("<input id=\"i\"><select id=\"s\"><option>a</option></select>");
        assert!(wait_recv(&mut loaded, Duration::from_secs(10)).unwrap().is_ok());
        let _ = page.poll();
        let mut focused = page.eval("document.getElementById('i').focus(); 1");
        assert!(wait_recv(&mut focused, Duration::from_secs(5)).is_some());
        assert_eq!(last_text_focused(&page, Duration::from_millis(300)), Some(true), "input");
        let mut select = page.eval("document.getElementById('s').focus(); 1");
        assert!(wait_recv(&mut select, Duration::from_secs(5)).is_some());
        assert_eq!(last_text_focused(&page, Duration::from_millis(300)), Some(false), "select");
    }

    /// The `Ipc` payloads seen within `budget`.
    fn ipc_payloads(page: &ContentThread, budget: Duration) -> Vec<String> {
        let deadline = Instant::now() + budget;
        let mut seen = Vec::new();
        while Instant::now() < deadline {
            for event in page.poll() {
                if let PageEvent::Ipc { payload, .. } = event {
                    seen.push(payload);
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        seen
    }

    /// CH3.A7: a web page on the content thread has no app bridge
    /// (Chromium 143: both `undefined`), and nothing it does reaches the
    /// window as IPC.
    #[test]
    fn a_web_page_on_the_content_thread_has_no_app_bridge() {
        let page = page();
        let mut loaded = page.load_html("<p>web</p>");
        assert!(wait_recv(&mut loaded, Duration::from_secs(10)).unwrap().is_ok());
        let _ = page.poll();
        let mut probe = page.eval(
            "[typeof window.ipc, typeof window.hiwaveChrome, typeof __rk_ipc_post].join(',')",
        );
        let got = wait_recv(&mut probe, Duration::from_secs(5)).expect("eval answered");
        // The answer is the value's Debug form (`String("...")`).
        assert!(
            got.as_deref().is_ok_and(|v| v.contains("\"undefined,undefined,undefined\"")),
            "{got:?}"
        );
        let mut posted = page.eval(
            "try { window.ipc.postMessage('{\"cmd\":\"navigate\"}'); } catch (e) {} 1",
        );
        assert!(wait_recv(&mut posted, Duration::from_secs(5)).is_some());
        assert!(ipc_payloads(&page, Duration::from_millis(300)).is_empty());
    }

    #[test]
    fn the_window_is_sent_what_the_page_posts_on_window_ipc() {
        // CH2.E9: `window.ipc.postMessage` filled a queue only a poll of
        // the engine could read, and nothing polled it: the chrome's
        // messages from a RustKit view went nowhere. The chrome is an
        // app-owned view (CH3.A7): web content has no `window.ipc`.
        let page = page();
        page.set_app_owned(true);
        let mut loaded = page.load_html("<p>chrome</p>");
        assert!(wait_recv(&mut loaded, Duration::from_secs(10)).unwrap().is_ok());
        let _ = page.poll();
        let mut posted = page.eval(r#"window.ipc.postMessage('{"cmd":"navigate","url":"https://example.com/"}'); 1"#);
        assert!(wait_recv(&mut posted, Duration::from_secs(5)).is_some());
        assert_eq!(
            ipc_payloads(&page, Duration::from_millis(300)),
            vec![r#"{"cmd":"navigate","url":"https://example.com/"}"#.to_string()],
            "a message from a script was not sent to the window"
        );
        let mut timer = page.eval("setTimeout(function () { window.ipc.postMessage('later'); }, 20); 1");
        assert!(wait_recv(&mut timer, Duration::from_secs(5)).is_some());
        assert_eq!(
            ipc_payloads(&page, Duration::from_millis(500)),
            vec!["later".to_string()],
            "a message from a timer was not sent to the window"
        );
    }

    #[test]
    fn a_superseded_navigation_drops_the_late_result() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();
        let (release_tx, release_rx) = mpsc::channel::<()>();
        std::thread::spawn(move || {
            if let Ok((sock, _)) = listener.accept() {
                let _ = release_rx.recv_timeout(Duration::from_secs(5));
                drop(sock);
            }
        });
        let page = page();
        let first = page.navigate(Url::parse(&format!("http://127.0.0.1:{port}/")).unwrap());
        std::thread::sleep(Duration::from_millis(50));
        let started = Instant::now();
        let second = page.navigate(Url::parse("data:text/html,<title>second</title>").unwrap());
        assert!(
            started.elapsed() < Duration::from_millis(50),
            "the second navigate waited"
        );
        assert!(second > first);
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut saw_second = false;
        let mut events = Vec::new();
        while Instant::now() < deadline && !saw_second {
            events.extend(page.poll());
            saw_second = reject_stale(events.clone(), second).iter().any(
                |event| matches!(event, PageEvent::Title { text, .. } if text.contains("second")),
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = release_tx.send(());
        assert!(
            saw_second,
            "the second document never committed: {events:?}"
        );
        let kept = reject_stale(events, second);
        assert!(
            kept.iter().all(|event| match event {
                PageEvent::LoadState {
                    state: LoadState::Complete,
                    nav_id,
                } => *nav_id == second,
                PageEvent::Title { text, .. } => text.contains("second"),
                _ => true,
            }),
            "a stale result survived: {kept:?}"
        );
    }

    #[test]
    fn timers_run_while_the_page_is_idle() {
        let page = page();
        let mut loaded = page.load_html("<p id=\"t\"></p>");
        assert!(wait_recv(&mut loaded, Duration::from_secs(10))
            .unwrap()
            .is_ok());
        let mut arm = page.eval("setTimeout(function () { document.getElementById('t').textContent = 'tick'; }, 20); 'armed'");
        assert!(wait_recv(&mut arm, Duration::from_secs(5))
            .unwrap()
            .unwrap()
            .contains("armed"));
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut text = String::new();
        while Instant::now() < deadline {
            let mut probe = page.eval("document.getElementById('t').textContent");
            if let Some(Ok(value)) = wait_recv(&mut probe, Duration::from_millis(200)) {
                text = value;
                if text.contains("tick") {
                    break;
                }
            }
        }
        assert!(
            text.contains("tick"),
            "the idle loop did not run the timer: {text}"
        );
    }

    #[test]
    fn shutdown_does_not_wait_for_a_spinning_script() {
        let page = page();
        let mut loaded = page.load_html("<p id=\"t\">x</p>");
        assert!(wait_recv(&mut loaded, Duration::from_secs(10))
            .unwrap()
            .is_ok());
        let _spinning = page.eval("while (true) { document.getElementById('t'); }");
        std::thread::sleep(Duration::from_millis(30));
        let started = Instant::now();
        page.shutdown();
        assert!(
            started.elapsed() < Duration::from_millis(50),
            "shutdown joined the page"
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        while !page.exited() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(page.exited(), "the page ignored the interrupt");
    }

    #[test]
    fn a_scroll_commit_is_delivered() {
        let page = page();
        let mut loaded = page.load_html("<div style=\"height:4000px\">tall</div>");
        assert!(wait_recv(&mut loaded, Duration::from_secs(10))
            .unwrap()
            .is_ok());
        // Let the first layout commit land, then scroll.
        std::thread::sleep(Duration::from_millis(100));
        let _ = page.poll();
        page.scroll(0.0, -200.0);
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut scrolled = false;
        while Instant::now() < deadline && !scrolled {
            for event in page.poll() {
                if let PageEvent::Commit(commit) = event {
                    if commit.scroll_offset.1 > 0.0 {
                        scrolled = true;
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(scrolled, "no scroll commit arrived");
    }

    #[test]
    fn four_thousand_host_reentries_fit_the_configured_stack_and_the_limit_recovers() {
        let page = ContentThread::spawn_with_stack(
            EngineBuilder::new(),
            Bounds {
                x: 0,
                y: 0,
                width: 800,
                height: 600,
            },
            content_stack_bytes(),
        )
        .expect("spawn");
        page.set_host_reentry(8_000);
        let mut loaded = page.load_html("<p id=\"t\"></p>");
        assert!(wait_recv(&mut loaded, Duration::from_secs(10))
            .unwrap()
            .is_ok());
        let mut probe = page.install_probe();
        assert!(wait_recv(&mut probe, Duration::from_secs(5))
            .unwrap()
            .is_ok());
        let script = "function __rk_step(n) { if (n <= 0) return 0; return __rk_reenter(n - 1); } String(__rk_reenter(4000))";
        let mut climbed = page.eval(script);
        let value = wait_recv(&mut climbed, Duration::from_secs(30))
            .expect("climb reply")
            .expect("climb");
        assert!(value.contains('0'), "{value}");
        let used = rustkit_js::reentry_stack_used();
        let stack = page.stack_bytes();
        eprintln!(
            "host re-entry: {used} bytes for 4000 frames on a {stack} byte stack ({:.1} bytes/frame, {:.1}% of the stack)",
            used as f64 / 4000.0,
            (used as f64 / stack as f64) * 100.0
        );
        assert!(used > 0, "the probe did not record a stack");
        assert!(used < stack, "4000 frames used {used} of {stack}");

        page.set_host_reentry(8);
        let mut reloaded = page.load_html("<p id=\"t\"></p>");
        assert!(wait_recv(&mut reloaded, Duration::from_secs(10))
            .unwrap()
            .is_ok());
        let mut probe = page.install_probe();
        assert!(wait_recv(&mut probe, Duration::from_secs(5))
            .unwrap()
            .is_ok());
        let mut over = page.eval(script);
        let over = wait_recv(&mut over, Duration::from_secs(10)).expect("over-limit reply");
        assert!(
            over.is_err(),
            "a 4000 climb under a cap of 8 succeeded: {over:?}"
        );
        let mut recovered = page.eval("1 + 1");
        let recovered = wait_recv(&mut recovered, Duration::from_secs(5))
            .expect("recovery")
            .expect("eval");
        assert!(recovered.contains('2'), "{recovered}");
    }

    #[test]
    fn coalesce_collapses_adjacent_resizes_and_keeps_the_click() {
        let (a, _) = oneshot::channel();
        let (b, _) = oneshot::channel();
        let (c, _) = oneshot::channel();
        let bounds = |height| Bounds {
            x: 0,
            y: 0,
            width: 100,
            height,
        };
        let batch = vec![
            PageCmd::Resize {
                bounds: bounds(10),
                scale: 1.0,
                reply: a,
            },
            PageCmd::Resize {
                bounds: bounds(20),
                scale: 1.0,
                reply: b,
            },
            PageCmd::Input(PageInput::MouseDown { x: 1.0, y: 2.0, clicks: 1, shift: false }),
            PageCmd::Resize {
                bounds: bounds(30),
                scale: 2.0,
                reply: c,
            },
        ];
        let out = coalesce(batch);
        assert_eq!(out.len(), 3, "the click was reordered or a resize survived");
        assert!(matches!(out[0], PageCmd::Resize { bounds, .. } if bounds.height == 20));
        assert!(matches!(
            out[1],
            PageCmd::Input(PageInput::MouseDown { .. })
        ));
        assert!(
            matches!(out[2], PageCmd::Resize { bounds, scale, .. } if bounds.height == 30 && scale == 2.0)
        );
    }

    /// Z row S2 (Pete's hand test 11-03, reddit): a navigation typed while
    /// the pointer moved waited 5.5 s behind eleven queued moves of the page
    /// being left, each a ~430 ms hover restyle there, and their 288
    /// listener calls were then cut by the navigation. Chromium dispatches
    /// one move per frame from the latest position (coalesced pointer
    /// events). A move replaces an earlier one when only input marks and
    /// renders sit between; a press, key or click is never passed. (A
    /// navigation is not passed either; since S2's reopening the moves
    /// ahead of one are dropped outright, the test below.)
    #[test]
    fn coalesce_keeps_only_the_latest_of_adjacent_moves() {
        let mv = |x: f32| PageCmd::Input(PageInput::MouseMove { x, y: 0.0 });
        let note = |seq| PageCmd::NoteInput { at: Instant::now(), seq, kind: crate::InputClass::Move };
        let batch = vec![
            note(1),
            mv(1.0),
            note(2),
            mv(2.0),
            PageCmd::Render { every: false },
            note(3),
            mv(3.0),
            PageCmd::Input(PageInput::MouseDown { x: 3.0, y: 0.0, clicks: 1, shift: false }),
        ];
        let out = coalesce(batch);
        let moves: Vec<f32> = out
            .iter()
            .filter_map(|c| match c {
                PageCmd::Input(PageInput::MouseMove { x, .. }) => Some(*x),
                _ => None,
            })
            .collect();
        assert_eq!(moves, vec![3.0], "one move, from the latest position");
        assert_eq!(
            out.iter().filter(|c| matches!(c, PageCmd::NoteInput { .. })).count(),
            3,
            "every input mark is kept for the latency figure"
        );
        assert!(matches!(out.last(), Some(PageCmd::Input(PageInput::MouseDown { .. }))), "the press stays last");

        // A press between moves keeps both: the page hears where it went down.
        let batch = vec![mv(1.0), PageCmd::Input(PageInput::MouseDown { x: 1.0, y: 0.0, clicks: 1, shift: false }), mv(2.0)];
        let out = coalesce(batch);
        assert_eq!(out.len(), 3, "a move jumped the press");
        assert!(matches!(out[0], PageCmd::Input(PageInput::MouseMove { x, .. }) if x == 1.0));
        assert!(matches!(out[1], PageCmd::Input(PageInput::MouseDown { .. })));
    }

    /// Z row S2, reopened (Pete's hand test 11-04, reddit): a load held the
    /// page thread for 11 s while the pointer moved; the user closed the
    /// tab; the thread then dispatched thirty queued moves, leaves, presses
    /// and clicks to the document being left (a ~100 ms hover restyle each,
    /// every listener refused by the navigation's interrupt word and logged
    /// as a budget stop, 678 lines) before it loaded the new page, three
    /// seconds late. Pointer input queued ahead of a navigation is for a
    /// document the user has left: it is dropped, a queued click answered
    /// with nothing. Input marks stay for the latency figure, and input
    /// behind the navigation is kept.
    #[test]
    fn coalesce_drops_pointer_input_queued_ahead_of_a_navigation() {
        let mv = |x: f32| PageCmd::Input(PageInput::MouseMove { x, y: 0.0 });
        let note = |seq| PageCmd::NoteInput { at: Instant::now(), seq, kind: crate::InputClass::Move };
        let (click_reply, mut clicked) = oneshot::channel();
        let (html_reply, _) = oneshot::channel();
        let batch = vec![
            note(1),
            mv(1.0),
            PageCmd::Input(PageInput::MouseLeave),
            note(2),
            PageCmd::Input(PageInput::MouseDown { x: 2.0, y: 0.0, clicks: 1, shift: false }),
            mv(2.0),
            PageCmd::Input(PageInput::Click { x: 2.0, y: 0.0, reply: click_reply }),
            PageCmd::LoadHtml { html: "<p>next</p>".into(), nav_id: 9, reply: html_reply },
            mv(3.0),
        ];
        let out = coalesce(batch);
        let kinds: Vec<&str> = out
            .iter()
            .map(|c| match c {
                PageCmd::NoteInput { .. } => "note",
                PageCmd::Input(PageInput::MouseMove { .. }) => "move",
                PageCmd::Input(PageInput::MouseLeave) => "leave",
                PageCmd::Input(PageInput::MouseDown { .. }) => "down",
                PageCmd::Input(PageInput::Click { .. }) => "click",
                PageCmd::LoadHtml { .. } => "load_html",
                _ => "other",
            })
            .collect();
        assert_eq!(kinds, vec!["note", "note", "load_html", "move"], "{kinds:?}");
        assert!(matches!(out[2], PageCmd::LoadHtml { nav_id: 9, .. }));
        assert!(matches!(out[3], PageCmd::Input(PageInput::MouseMove { x, .. }) if x == 3.0));
        let outcome = clicked.try_recv().expect("the dropped click is answered");
        assert!(outcome.navigate.is_none() && outcome.focused.is_none(), "{outcome:?}");

        // A navigation command is what the pointer input waits behind; a
        // Stop keeps the document and its input.
        let (stop_reply, _) = oneshot::channel();
        let out = coalesce(vec![mv(1.0), PageCmd::Stop { reply: stop_reply }]);
        assert_eq!(out.len(), 2, "a Stop dropped the move");
    }

    fn poll_until(page: &ContentThread, budget: Duration, mut done: impl FnMut(&[PageEvent]) -> bool) -> Vec<PageEvent> {
        let deadline = Instant::now() + budget;
        let mut events = Vec::new();
        while Instant::now() < deadline {
            events.extend(page.poll());
            if done(&events) {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        events.extend(page.poll());
        events
    }

    fn solid_colors(commit: &crate::FrameCommit) -> Vec<(u8, u8, u8)> {
        let Some(list) = commit.list.as_ref() else {
            return Vec::new();
        };
        list.commands
            .iter()
            .filter_map(|cmd| match cmd {
                rustkit_layout::DisplayCommand::SolidColor(color, _) => Some((color.r, color.g, color.b)),
                _ => None,
            })
            .collect()
    }

    fn has_color(commit: &crate::FrameCommit, r: u8, g: u8, b: u8) -> bool {
        solid_colors(commit).contains(&(r, g, b))
    }

    /// One loopback server. `release` is the flag a held response waits on.
    /// Every request path is appended to the returned log.
    fn serve(
        handler: impl Fn(&str, &AtomicBool) -> (u16, &'static str, Vec<u8>) + Send + Sync + 'static,
    ) -> (u16, Arc<AtomicBool>, Arc<Mutex<Vec<String>>>) {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let release = Arc::new(AtomicBool::new(false));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let release_for_server = Arc::clone(&release);
        let seen_for_server = Arc::clone(&seen);
        let handler = Arc::new(handler);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let release_for_server = Arc::clone(&release_for_server);
                let seen_for_server = Arc::clone(&seen_for_server);
                let handler = Arc::clone(&handler);
                std::thread::spawn(move || {
                    let mut request = Vec::new();
                    let mut buf = [0u8; 2048];
                    let header_end = loop {
                        if let Some(i) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                            break i + 4;
                        }
                        match stream.read(&mut buf) {
                            Ok(0) | Err(_) => return,
                            Ok(n) => request.extend_from_slice(&buf[..n]),
                        }
                    };
                    let head = String::from_utf8_lossy(&request[..header_end]);
                    let path = head.split_whitespace().nth(1).unwrap_or("/").to_string();
                    seen_for_server.lock().unwrap_or_else(|e| e.into_inner()).push(path.clone());
                    let (status, content_type, body) = handler(&path, &release_for_server);
                    let status_text = if status == 200 { "200 OK" } else { "404 Not Found" };
                    let _ = write!(
                        stream,
                        "HTTP/1.1 {status_text}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(&body);
                });
            }
        });
        (port, release, seen)
    }

    #[test]
    fn script_navigation_is_reported_with_the_documents_nav_id() {
        let page = page();
        let mut loaded = page.load_html(
            r#"<a id="l" href="https://example.com/clicked">go</a><p id="t">x</p>"#,
        );
        assert!(wait_recv(&mut loaded, Duration::from_secs(10)).unwrap().is_ok());
        let events = poll_until(&page, Duration::from_secs(5), |events| {
            events.iter().any(|event| matches!(event, PageEvent::Commit(commit) if commit.nav_generation > 0))
        });
        let nav = events.iter().find_map(|event| match event {
            PageEvent::Commit(commit) if commit.nav_generation > 0 => Some(commit.nav_generation),
            _ => None,
        }).expect("document nav");

        let cases = [
            ("location.href = 'https://example.com/assigned'", "assigned", false),
            ("location.replace('https://example.com/replaced')", "replaced", true),
            ("document.getElementById('l').click()", "clicked", false),
        ];
        for (script, needle, replace) in cases {
            let _ = page.poll();
            let mut eval = page.eval(&format!("{script}; 'set'"));
            assert!(
                wait_recv(&mut eval, Duration::from_secs(5)).unwrap().unwrap().contains("set"),
                "{script}"
            );
            let events = poll_until(&page, Duration::from_secs(3), |events| {
                events.iter().any(|event| matches!(event, PageEvent::NavigateRequested { url, .. } if url.contains(needle)))
            });
            let found = events.iter().find_map(|event| match event {
                PageEvent::NavigateRequested {
                    url,
                    replace: flag,
                    nav_id,
                    from_user,
                } if url.contains(needle) => Some((url.clone(), *flag, *nav_id, *from_user)),
                _ => None,
            });
            let Some((url, flag, nav_id, from_user)) = found else {
                panic!("{script} produced no navigation: {events:?}");
            };
            assert_eq!(flag, replace, "{url}");
            assert!(!from_user, "{url} was the user's click");
            assert_eq!(nav_id, nav, "{url} was not tagged with the document");
            // Id 0 is untagged and is kept. A request from this document is
            // stale once a newer navigation has been accepted.
            let dropped = reject_stale(
                vec![PageEvent::NavigateRequested {
                    url: url.clone(),
                    replace: flag,
                    nav_id: nav,
                    from_user: false,
                }],
                nav + 1,
            );
            assert!(dropped.is_empty(), "an older navigation request would be followed");
        }
    }

    #[test]
    fn a_queued_default_document_does_not_replace_a_newer_navigation() {
        let page = page();
        let mut loaded = page.load_html("<title>base</title><p id=\"t\">x</p>");
        assert!(wait_recv(&mut loaded, Duration::from_secs(10)).unwrap().is_ok());
        let mut spinning = page.eval("while (true) { document.getElementById('t'); }");
        std::thread::sleep(Duration::from_millis(40));
        let mut default_load = page.load_html("<title>default</title><p>new tab</p>");
        let restored = page.navigate(Url::parse("data:text/html,<title>restored</title>").unwrap());
        let _ = wait_recv(&mut spinning, Duration::from_secs(5));
        let _ = wait_recv(&mut default_load, Duration::from_secs(5));
        let events = poll_until(&page, Duration::from_secs(10), |events| {
            reject_stale(events.to_vec(), restored).iter().any(|event| {
                matches!(event, PageEvent::Title { text, .. } if text == "restored")
            })
        });
        std::thread::sleep(Duration::from_millis(200));
        let mut events = events;
        events.extend(page.poll());
        let kept = reject_stale(events, restored);
        assert!(
            kept.iter().any(|event| matches!(event, PageEvent::Title { text, .. } if text == "restored")),
            "the restore never committed: {kept:?}"
        );
        assert!(
            kept.iter().all(|event| match event {
                PageEvent::Title { text, .. } => text == "restored" || text == "base",
                _ => true,
            }),
            "the default document replaced the restore: {kept:?}"
        );
    }

    #[test]
    fn the_first_commit_is_the_laid_out_frame() {
        let page = page();
        let mut loaded = page.load_html(
            r#"<html><body style="margin:0"><div style="height:30px;margin:6px 20px;background:#ff0000"></div></body></html>"#,
        );
        assert!(wait_recv(&mut loaded, Duration::from_secs(10)).unwrap().is_ok());
        let events = poll_until(&page, Duration::from_secs(5), |events| {
            events.iter().any(|event| matches!(event, PageEvent::Commit(_)))
        });
        let commits: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                PageEvent::Commit(commit) => Some(commit),
                _ => None,
            })
            .collect();
        assert!(!commits.is_empty(), "no frame was published: {events:?}");
        let first = commits[0];
        assert!(first.generation > 0, "generation {}", first.generation);
        assert!(first.nav_generation > 0, "nav {}", first.nav_generation);
        assert!(
            first.list.as_ref().is_some_and(|list| !list.commands.is_empty()),
            "the first frame had no display list"
        );
        assert!(
            has_color(first, 255, 0, 0),
            "the red band was not in the first frame: {:?}",
            solid_colors(first)
        );
    }

    #[test]
    fn a_frame_is_published_between_timer_tasks() {
        let page = page();
        let mut loaded = page.load_html(
            r#"<div id="t" style="width:80px;height:40px;background:#cccccc"></div>"#,
        );
        assert!(wait_recv(&mut loaded, Duration::from_secs(10)).unwrap().is_ok());
        let _ = poll_until(&page, Duration::from_secs(3), |events| {
            events.iter().any(|event| matches!(event, PageEvent::Commit(_)))
        });
        let _ = page.poll();
        let mut arm = page.eval(
            r#"setTimeout(function () {
                document.getElementById('t').style.background = '#00ff00';
                var end = Date.now() + 40;
                while (Date.now() < end) {}
            }, 0);
            setTimeout(function () {
                document.getElementById('t').style.background = '#0000ff';
            }, 0);
            'armed'"#,
        );
        assert!(wait_recv(&mut arm, Duration::from_secs(5)).unwrap().unwrap().contains("armed"));
        let mut saw_green = false;
        let mut blue_first = false;
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline && !saw_green {
            for event in page.poll() {
                let PageEvent::Commit(commit) = event else { continue };
                let green = has_color(&commit, 0, 255, 0);
                let blue = has_color(&commit, 0, 0, 255);
                if blue && !saw_green {
                    blue_first = true;
                }
                if green {
                    saw_green = true;
                }
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(saw_green, "no frame was published between the timer tasks");
        assert!(!blue_first, "the later timer was painted before that frame");
    }

    /// The laid-out frame is published while the load is still waiting on a
    /// script, not only when the load returns.
    #[test]
    fn a_commit_is_published_while_the_load_is_still_waiting() {
        let (port, release, _seen) = serve(|path, release_flag| {
            if path == "/hold.js" {
                let start = Instant::now();
                while !release_flag.load(Ordering::Acquire) && start.elapsed() < Duration::from_secs(8) {
                    std::thread::sleep(Duration::from_millis(5));
                }
                return (200, "text/javascript", b"/* held */".to_vec());
            }
            (
                200,
                "text/html",
                br#"<html><body style="margin:0"><div style="height:30px;margin:6px 20px;background:#ff0000"></div><script src="/hold.js"></script></body></html>"#.to_vec(),
            )
        });
        let page = page();
        let _nav = page.navigate(Url::parse(&format!("http://127.0.0.1:{port}/")).unwrap());
        let events = poll_until(&page, Duration::from_secs(8), |events| {
            events.iter().any(|event| matches!(event, PageEvent::Commit(commit) if has_color(commit, 255, 0, 0)))
        });
        let commit = events.iter().find_map(|event| match event {
            PageEvent::Commit(commit) if has_color(commit, 255, 0, 0) => Some(commit),
            _ => None,
        });
        let Some(commit) = commit else {
            release.store(true, Ordering::Release);
            panic!("no laid-out frame while the load was waiting: {events:?}");
        };
        assert!(
            commit.list.as_ref().is_some_and(|list| !list.commands.is_empty()),
            "the frame during the load had no display list"
        );
        assert!(
            !events.iter().any(|event| {
                matches!(event, PageEvent::LoadState { state: LoadState::Complete, .. })
            }),
            "the frame arrived only when the load finished: {events:?}"
        );
        release.store(true, Ordering::Release);
        let finished = poll_until(&page, Duration::from_secs(8), |events| {
            events.iter().any(|event| {
                matches!(event, PageEvent::LoadState { state: LoadState::Complete, .. })
            })
        });
        assert!(
            finished.iter().any(|event| {
                matches!(event, PageEvent::LoadState { state: LoadState::Complete, .. })
            }),
            "the load never finished after the script was released"
        );
    }

    #[test]
    fn the_commit_carries_the_canvas_colour() {
        let page = page();
        let mut loaded = page.load_html(
            r#"<html><body style="margin:0;background:#112233"><p>x</p></body></html>"#,
        );
        assert!(wait_recv(&mut loaded, Duration::from_secs(10)).unwrap().is_ok());
        let events = poll_until(&page, Duration::from_secs(5), |events| {
            events.iter().any(|event| matches!(event, PageEvent::Commit(_)))
        });
        let commit = events.iter().find_map(|event| match event {
            PageEvent::Commit(commit) => Some(commit),
            _ => None,
        });
        let Some(commit) = commit else {
            panic!("no frame: {events:?}");
        };
        let canvas = commit.canvas;
        assert!((canvas[0] - 17.0 / 255.0).abs() < 0.02, "{canvas:?}");
        assert!((canvas[1] - 34.0 / 255.0).abs() < 0.02, "{canvas:?}");
        assert!((canvas[2] - 51.0 / 255.0).abs() < 0.02, "{canvas:?}");
        assert!((canvas[3] - 1.0).abs() < 0.02, "{canvas:?}");
    }

    /// The panel's network read, through the routing the window uses.
    fn read_network(page: &ContentThread, budget: Duration) -> Option<crate::DevtoolsNetwork> {
        let mut rx = page.devtools_ask(crate::DevtoolsQuery::Network);
        match wait_recv(&mut rx, budget) {
            Some(crate::DevtoolsAnswer::Network(network)) => Some(network),
            _ => None,
        }
    }

    #[test]
    fn a_fetch_response_is_recorded_while_the_page_thread_is_blocked() {
        // T7 acceptance (T7-design.md test 1): a fetch started before a
        // host-side block gets its response record while the page is still
        // blocked, read through the devtools service. On develop the request
        // is polled only by the page thread, so nothing is read or recorded
        // until the block ends.
        let (port, _release, _seen) = serve(|path, _release| {
            if path.starts_with("/slow") {
                std::thread::sleep(Duration::from_millis(200));
                return (200, "text/plain", b"ok".to_vec());
            }
            (200, "text/html", b"<p id=\"t\">ready</p>".to_vec())
        });
        let page = page();
        let _nav = page.navigate(Url::parse(&format!("http://127.0.0.1:{port}/")).unwrap());
        let loaded = poll_until(&page, Duration::from_secs(8), |events| {
            events.iter().any(|event| matches!(event, PageEvent::LoadState { state: LoadState::Complete, .. }))
        });
        assert!(
            loaded.iter().any(|event| matches!(event, PageEvent::LoadState { state: LoadState::Complete, .. })),
            "the page never finished loading"
        );
        let mut attached = page.devtools_ask(crate::DevtoolsQuery::Attach(true));
        assert_eq!(wait_recv(&mut attached, Duration::from_secs(5)), Some(crate::DevtoolsAnswer::Done));
        let mut started = page.eval("fetch('/slow?token=SECRET'); 'started'");
        assert!(wait_recv(&mut started, Duration::from_secs(5)).unwrap().unwrap().contains("started"));
        let slow = |network: &crate::DevtoolsNetwork| {
            network.requests.iter().find(|r| r.url.ends_with("/slow")).cloned()
        };
        // The request record is made at request start, on the page.
        let deadline = Instant::now() + Duration::from_secs(2);
        let (start_doc, request) = loop {
            let network = read_network(&page, Duration::from_secs(1)).expect("no network answer");
            if let Some(request) = slow(&network) {
                break (network.doc, request);
            }
            assert!(Instant::now() < deadline, "the fetch never started: {network:?}");
            std::thread::sleep(Duration::from_millis(5));
        };
        assert_eq!(request.state, "pending", "the fetch ended before the block: {request:?}");
        assert!(!request.url.contains("SECRET"), "{request:?}");

        page.enqueue(PageCmd::Block(1_500));
        let mut behind = page.eval("1");
        let blocked_at = Instant::now();
        let mut answered = None;
        while blocked_at.elapsed() < Duration::from_millis(1_200) {
            let network = read_network(&page, Duration::from_millis(500)).expect("no network answer while blocked");
            if let Some(r) = slow(&network).filter(|r| r.state != "pending") {
                answered = Some((network.doc, r));
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            matches!(behind.try_recv(), Err(oneshot::error::TryRecvError::Empty)),
            "the page was not in its block when the record was read"
        );
        let (doc, response) = answered.expect("no response record while the page thread was blocked");
        assert_eq!(response.state, "response", "{response:?}");
        assert_eq!(response.status, Some(200));
        assert_eq!(doc, start_doc, "the record is not in the document the fetch started in");
        assert!(wait_recv(&mut behind, Duration::from_secs(5)).is_some());
    }

    /// A server whose `/hang` answers only when released (or after 10 s),
    /// and a loaded page from it with the panel attached.
    fn hanging_page() -> (ContentThread, u16, Arc<AtomicBool>) {
        let (port, release, _seen) = serve(|path, release| {
            if path.starts_with("/hang") {
                let until = Instant::now() + Duration::from_secs(10);
                while !release.load(std::sync::atomic::Ordering::SeqCst) && Instant::now() < until {
                    std::thread::sleep(Duration::from_millis(10));
                }
                return (200, "text/plain", b"late".to_vec());
            }
            (200, "text/html", b"<p id=\"t\">ready</p>".to_vec())
        });
        let page = page();
        let _nav = page.navigate(Url::parse(&format!("http://127.0.0.1:{port}/")).unwrap());
        let loaded = poll_until(&page, Duration::from_secs(8), |events| {
            events.iter().any(|event| matches!(event, PageEvent::LoadState { state: LoadState::Complete, .. }))
        });
        assert!(loaded.iter().any(|event| matches!(event, PageEvent::LoadState { state: LoadState::Complete, .. })));
        let mut attached = page.devtools_ask(crate::DevtoolsQuery::Attach(true));
        assert_eq!(wait_recv(&mut attached, Duration::from_secs(5)), Some(crate::DevtoolsAnswer::Done));
        let mut started = page.eval("fetch('/hang'); 'started'");
        assert!(wait_recv(&mut started, Duration::from_secs(5)).unwrap().unwrap().contains("started"));
        let deadline = Instant::now() + Duration::from_secs(2);
        while !read_network(&page, Duration::from_secs(1))
            .is_some_and(|n| n.requests.iter().any(|r| r.url.ends_with("/hang")))
        {
            assert!(Instant::now() < deadline, "the fetch never started");
            std::thread::sleep(Duration::from_millis(5));
        }
        (page, port, release)
    }

    #[test]
    fn a_navigation_mid_flight_records_one_cancel_in_the_old_document() {
        // T7 test 2: the task is aborted; its guard records the cancel in
        // the document the request started in, never a response, and the
        // new document's list does not show it.
        let (page, port, release) = hanging_page();
        let old_doc = read_network(&page, Duration::from_secs(1)).unwrap().doc;
        let _nav = page.navigate(Url::parse(&format!("http://127.0.0.1:{port}/next.html")).unwrap());
        let loaded = poll_until(&page, Duration::from_secs(8), |events| {
            events.iter().any(|event| matches!(event, PageEvent::LoadState { state: LoadState::Complete, .. }))
        });
        assert!(loaded.iter().any(|event| matches!(event, PageEvent::LoadState { state: LoadState::Complete, .. })));
        let inspector = page.devtools_inspector().unwrap();
        let hang_id = |records: &[crate::InspectorRecord]| {
            records.iter().find_map(|r| match &r.event {
                crate::InspectorEvent::Request { id, url, .. } if url.ends_with("/hang") => Some(*id),
                _ => None,
            })
        };
        let deadline = Instant::now() + Duration::from_secs(3);
        let terminals = loop {
            let records = inspector.snapshot_since(0).records;
            let id = hang_id(&records).expect("the request record is gone");
            let terminals: Vec<_> = records
                .into_iter()
                .filter(|r| {
                    matches!(&r.event, crate::InspectorEvent::Response { id: i, .. } | crate::InspectorEvent::Failure { id: i, .. } if *i == id)
                })
                .collect();
            if !terminals.is_empty() || Instant::now() >= deadline {
                break terminals;
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        // Let the server answer now: an aborted task writes nothing more.
        release.store(true, std::sync::atomic::Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(terminals.len(), 1, "not exactly one terminal record: {terminals:?}");
        assert!(
            matches!(&terminals[0].event, crate::InspectorEvent::Failure { stage: crate::inspector::FailureStage::Cancelled, .. }),
            "{terminals:?}"
        );
        assert_eq!(terminals[0].doc, old_doc, "the cancel is not in the document the request started in");
        let records = inspector.snapshot_since(0).records;
        let id = hang_id(&records).unwrap();
        let after = records
            .iter()
            .filter(|r| matches!(&r.event, crate::InspectorEvent::Response { id: i, .. } | crate::InspectorEvent::Failure { id: i, .. } if *i == id))
            .count();
        assert_eq!(after, 1, "a record was written after the cancel");
        let network = read_network(&page, Duration::from_secs(1)).unwrap();
        assert_ne!(network.doc, old_doc);
        assert!(!network.requests.iter().any(|r| r.url.ends_with("/hang")), "the old page's request is listed: {network:?}");
    }

    #[test]
    fn a_navigation_mid_flight_closes_the_stream_the_server_sees() {
        // T7 test 2, the server's side (Prometheus nit, carried to T8): the
        // aborted task drops its connection, so the server reads EOF within
        // the T7a C4 bound (about 1 s) of the navigation.
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let hang_seen = Arc::new(AtomicBool::new(false));
        let closed: Arc<Mutex<Option<Instant>>> = Arc::new(Mutex::new(None));
        let (seen_for_server, closed_for_server) = (Arc::clone(&hang_seen), Arc::clone(&closed));
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let (hang_seen, closed) = (Arc::clone(&seen_for_server), Arc::clone(&closed_for_server));
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
                    if path.starts_with("/hang") {
                        hang_seen.store(true, std::sync::atomic::Ordering::SeqCst);
                        let _ = stream.set_read_timeout(Some(Duration::from_millis(10)));
                        let until = Instant::now() + Duration::from_secs(10);
                        while Instant::now() < until {
                            match stream.read(&mut buf) {
                                Ok(0) => break,
                                Ok(_) => {}
                                Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => continue,
                                Err(_) => break,
                            }
                        }
                        if Instant::now() < until {
                            *closed.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
                        }
                        return;
                    }
                    let body = b"<p id=\"t\">ready</p>";
                    let _ = write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(body);
                });
            }
        });
        let page = page();
        let complete = |events: &[PageEvent]| {
            events.iter().any(|event| matches!(event, PageEvent::LoadState { state: LoadState::Complete, .. }))
        };
        let _nav = page.navigate(Url::parse(&format!("http://127.0.0.1:{port}/")).unwrap());
        assert!(complete(&poll_until(&page, Duration::from_secs(8), complete)));
        let mut started = page.eval("fetch('/hang'); 'started'");
        assert!(wait_recv(&mut started, Duration::from_secs(5)).unwrap().unwrap().contains("started"));
        let deadline = Instant::now() + Duration::from_secs(3);
        while !hang_seen.load(std::sync::atomic::Ordering::SeqCst) {
            assert!(Instant::now() < deadline, "the fetch never reached the server");
            std::thread::sleep(Duration::from_millis(5));
        }
        let navigated = Instant::now();
        let _nav = page.navigate(Url::parse(&format!("http://127.0.0.1:{port}/next.html")).unwrap());
        let deadline = navigated + Duration::from_secs(3);
        let closed_at = loop {
            if let Some(at) = *closed.lock().unwrap_or_else(|e| e.into_inner()) {
                break Some(at);
            }
            if Instant::now() >= deadline {
                break None;
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        let closed_at = closed_at.expect("the server never saw the stream close after the navigation");
        let after = closed_at.saturating_duration_since(navigated);
        assert!(after < Duration::from_secs(1), "the stream closed {after:?} after the navigation");
    }

    #[test]
    fn a_closed_view_frees_its_inspector_with_a_request_out() {
        // T7 test 3 (S5): the task holds the view's Inspector only until its
        // abort is processed. C4: within about a second of the close.
        let (page, _port, release) = hanging_page();
        let held = close_to_dead_inspector(page);
        release.store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(held < Duration::from_secs(1), "the closed view's Inspector was still held {held:?} after the close");
    }

    /// Drop `page` and time until its Inspector's last strong ref is gone
    /// (gives up at 5 s).
    fn close_to_dead_inspector(page: ContentThread) -> Duration {
        let weak = Arc::downgrade(&page.devtools_inspector().unwrap());
        let closed = Instant::now();
        drop(page);
        while weak.upgrade().is_some() && closed.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(1));
        }
        closed.elapsed()
    }

    #[test]
    #[ignore = "receipt: C4 close latency over 10 runs, idle"]
    fn close_latency_receipt() {
        // T7a R1 delta (Atlas 6): the "beside a busy rustkit-net" figure is
        // dropped; nothing showed a body still in flight at the close.
        let mut idle = Vec::new();
        for _ in 0..10 {
            let (page, _port, release) = hanging_page();
            idle.push(close_to_dead_inspector(page));
            release.store(true, std::sync::atomic::Ordering::SeqCst);
        }
        idle.sort();
        eprintln!(
            "C4 close->dead Weak idle: p50 {:.1} ms, max {:.1} ms, n {}",
            idle[idle.len() / 2].as_secs_f64() * 1e3,
            idle[idle.len() - 1].as_secs_f64() * 1e3,
            idle.len()
        );
    }

    #[test]
    #[ignore = "receipt: two heavy live pages loading at once in one process; run with HIWAVE_NET_RUNTIME on and =0"]
    fn two_heavy_tabs_receipt() {
        // T7a D1: both views' script requests share the one rustkit-net
        // thread. Wall clock from navigate to load complete, each view, and
        // the script request records each made.
        let sites = ["https://www.cnn.com/", "https://www.yahoo.com/"];
        let started = Instant::now();
        let pages: Vec<_> = sites
            .iter()
            .map(|url| {
                let page = page();
                let mut attached = page.devtools_ask(crate::DevtoolsQuery::Attach(true));
                let _ = wait_recv(&mut attached, Duration::from_secs(5));
                let _nav = page.navigate(Url::parse(url).unwrap());
                page
            })
            .collect();
        let mut done: Vec<Option<Duration>> = vec![None; pages.len()];
        while done.iter().any(Option::is_none) && started.elapsed() < Duration::from_secs(90) {
            for (i, page) in pages.iter().enumerate() {
                if done[i].is_none()
                    && page
                        .poll()
                        .iter()
                        .any(|event| matches!(event, PageEvent::LoadState { state: LoadState::Complete, .. }))
                {
                    done[i] = Some(started.elapsed());
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let arm = std::env::var("HIWAVE_NET_RUNTIME").unwrap_or_else(|_| "on".into());
        for ((url, page), at) in sites.iter().zip(&pages).zip(&done) {
            let requests = read_network(page, Duration::from_secs(2)).map_or(0, |n| n.requests.len());
            eprintln!(
                "two tabs HIWAVE_NET_RUNTIME={arm}: {url} load complete {} ({requests} network records listed)",
                at.map_or("not in 90 s".to_string(), |d| format!("{:.1} s", d.as_secs_f64()))
            );
        }
    }

    /// A plain `fetch` queued while a response is delivered, in the same
    /// turn as `location.href`, is handed to the network before the
    /// document can be replaced. That is h16's start beacon: not
    /// `sendBeacon` and not `keepalive`.
    #[test]
    fn a_fetch_queued_with_a_navigation_is_started_before_the_document_is_replaced() {
        let (port, _release, seen) = serve(|path, _release| {
            if path.starts_with("/tick") {
                return (200, "text/plain", b"go".to_vec());
            }
            if path.starts_with("/start") {
                return (200, "text/plain", b"started".to_vec());
            }
            (200, "text/html", b"<p id=\"t\">ready</p>".to_vec())
        });
        let page = page();
        let _nav = page.navigate(Url::parse(&format!("http://127.0.0.1:{port}/")).unwrap());
        let loaded = poll_until(&page, Duration::from_secs(8), |events| {
            events.iter().any(|event| {
                matches!(event, PageEvent::LoadState { state: LoadState::Complete, .. })
            })
        });
        assert!(
            loaded.iter().any(|event| {
                matches!(event, PageEvent::LoadState { state: LoadState::Complete, .. })
            }),
            "the page never finished loading"
        );
        let _ = page.poll();
        let mut armed = page.eval(
            "fetch('/tick').then(function (r) { return r.text(); }).then(function (reply) { \
                if (reply !== 'go') return; \
                fetch('/start'); \
                location.href = '/next.html'; \
             }); 'armed'",
        );
        assert!(wait_recv(&mut armed, Duration::from_secs(5)).unwrap().unwrap().contains("armed"));
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut asked = false;
        while Instant::now() < deadline && !asked {
            for event in page.poll() {
                if let PageEvent::NavigateRequested { url, from_user, .. } = event {
                    if url.contains("next.html") {
                        assert!(!from_user, "a script navigation was the user's");
                        // Replace the document now, as the window does. A
                        // fetch that is still only queued dies with it.
                        let _ = page.navigate(
                            Url::parse(&format!("http://127.0.0.1:{port}/left.html")).unwrap(),
                        );
                        asked = true;
                        break;
                    }
                }
            }
            if !asked {
                std::thread::yield_now();
            }
        }
        assert!(asked, "the page never asked to navigate");
        std::thread::sleep(Duration::from_millis(100));
        let paths = seen.lock().unwrap_or_else(|e| e.into_inner()).clone();
        assert!(
            paths.iter().any(|path| path.starts_with("/start")),
            "the start fetch was not on the wire before the document was replaced: {paths:?}"
        );
    }

    #[test]
    fn a_user_click_keeps_its_origin() {
        let page = page();
        let mut loaded = page.load_html(
            r#"<a id="l" href="https://example.com/clicked" style="display:block;position:absolute;left:0;top:0;width:800px;height:600px">go</a>"#,
        );
        assert!(wait_recv(&mut loaded, Duration::from_secs(10)).unwrap().is_ok());
        let _ = poll_until(&page, Duration::from_secs(3), |events| {
            events.iter().any(|event| matches!(event, PageEvent::Commit(_)))
        });
        let (reply, _rx) = tokio::sync::oneshot::channel();
        page.input(PageInput::Click {
            x: 20.0,
            y: 20.0,
            reply,
        });
        let events = poll_until(&page, Duration::from_secs(5), |events| {
            events
                .iter()
                .any(|event| matches!(event, PageEvent::NavigateRequested { url, .. } if url.contains("clicked")))
        });
        let found = events.iter().find_map(|event| match event {
            PageEvent::NavigateRequested { url, from_user, .. } if url.contains("clicked") => {
                Some(*from_user)
            }
            _ => None,
        });
        assert_eq!(found, Some(true), "the click was not the user's: {events:?}");
    }

    #[test]
    fn a_resize_during_a_timer_task_is_applied_at_the_next_yield() {
        let page = page();
        let mut loaded = page.load_html(r#"<div id="t" style="width:40px;height:40px;background:#cccccc"></div>"#);
        assert!(wait_recv(&mut loaded, Duration::from_secs(10)).unwrap().is_ok());
        let _ = poll_until(&page, Duration::from_secs(3), |events| {
            events.iter().any(|event| matches!(event, PageEvent::Commit(_)))
        });
        let mut arm = page.eval(
            r#"setTimeout(function () {
                var end = Date.now() + 40;
                while (Date.now() < end) {}
            }, 0);
            'armed'"#,
        );
        assert!(wait_recv(&mut arm, Duration::from_secs(5)).unwrap().unwrap().contains("armed"));
        std::thread::sleep(Duration::from_millis(10));
        page.resize(
            Bounds {
                x: 0,
                y: 0,
                width: 640,
                height: 480,
            },
            1.0,
        );
        let events = poll_until(&page, Duration::from_secs(3), |events| {
            events.iter().any(|event| {
                matches!(event, PageEvent::Commit(commit) if commit.laid_out_for.0 == 640)
            })
        });
        assert!(
            events.iter().any(|event| {
                matches!(event, PageEvent::Commit(commit) if commit.laid_out_for.0 == 640)
            }),
            "the resize was not laid out after the timer task yielded: {events:?}"
        );
    }

    #[test]
    fn native_stack_overflow_aborts_the_child_and_leaves_this_process() {
        if std::env::var_os("HIWAVE_OVERFLOW_CHILD").is_some() {
            let joined = std::thread::Builder::new()
                .name("overflow".into())
                .stack_size(128 * 1024)
                .spawn(|| smash(0))
                .expect("child thread")
                .join();
            // A caught panic is not the overflow we are looking for.
            let _ = joined;
            std::process::exit(0);
        }
        let status = std::process::Command::new(std::env::current_exe().expect("test exe"))
            .arg("content_thread::tests::native_stack_overflow_aborts_the_child_and_leaves_this_process")
            .arg("--exact")
            .env("HIWAVE_OVERFLOW_CHILD", "1")
            .env("RUST_BACKTRACE", "0")
            .status()
            .expect("child");
        assert!(!status.success(), "the overflowing child exited cleanly");
    }

    fn smash(depth: usize) -> usize {
        let frame = [0u8; 4096];
        let next = smash(depth + 1);
        next + frame[depth % frame.len()] as usize
    }
}
