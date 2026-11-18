//! Adapter to connect hiwave-shield to rustkit-net's request interception.
//! Its own crate so a headless run (`parity-capture --shield`) installs the
//! same interceptor as the app (dev-tools T2); it was `hiwave-app`'s
//! `shield_adapter` module.
//!
//! This module bridges the gap between the ad blocking engine (hiwave-shield)
//! and RustKit's network layer (rustkit-net), allowing sub-resource requests
//! to be filtered by the shield.
//!
//! Note: The main hiwave-shield uses Brave's adblock engine which is not Send+Sync.
//! For RustKit's async network layer, we use a simple domain-based filter that
//! mirrors the most common blocking rules. Full adblock filtering still happens
//! at the navigation level.


use hiwave_shield::ResourceType as ShieldResourceType;
use url::Url;
use rustkit_net::{BlockReason, InterceptAction, InterceptHandler, Request, RequestDestination};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use tracing::{debug, trace};

/// Thread-safe adapter that implements rustkit-net's InterceptHandler.
pub struct ShieldInterceptHandler {
    /// Whether blocking is enabled.
    enabled: Arc<AtomicBool>,
    /// Counter for blocked requests.
    blocked_count: Arc<AtomicU64>,
    /// Per-destination attempted/blocked census.
    census: Arc<ShieldCensus>,
    /// Callback to notify when a request is blocked (for UI updates).
    on_blocked: Option<Box<dyn Fn(&str) + Send + Sync>>,
}

impl ShieldInterceptHandler {
    /// Create a new shield intercept handler with default blocked domains.
    pub fn new() -> Self {
        // Kick the background engine build immediately so the
        // allow-until-ready window starts closing at construction, not at
        // the first request.
        let _ = shield_worker();
        Self {
            enabled: Arc::new(AtomicBool::new(true)),
            blocked_count: Arc::new(AtomicU64::new(0)),
            census: Arc::new(ShieldCensus::default()),
            on_blocked: None,
        }
    }

    /// Create with a shared counter for tracking blocked requests.
    pub fn with_counter(blocked_count: Arc<AtomicU64>) -> Self {
        // Production constructor (webview_rustkit path): start closing the
        // pending window here too, not only in new().
        let _ = shield_worker();
        Self {
            enabled: Arc::new(AtomicBool::new(true)),
            blocked_count,
            census: Arc::new(ShieldCensus::default()),
            on_blocked: None,
        }
    }

    /// Set whether blocking is enabled.
    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Relaxed);
    }

    /// Get the blocked request count.
    pub fn blocked_count(&self) -> u64 {
        self.blocked_count.load(Ordering::Relaxed)
    }

    /// Get the counter Arc for sharing.
    pub fn counter(&self) -> Arc<AtomicU64> {
        Arc::clone(&self.blocked_count)
    }

    /// Test-only census snapshot: (attempted, blocked) per destination.
    #[cfg(test)]
    fn census_snapshot(&self) -> [(u64, u64); 8] {
        self.census.snapshot()
    }

    /// Set a callback to be called when a request is blocked. It is handed the FULL URL, for
    /// in-process use (a counter, a UI badge); a callback must not log it, as the shield's own log
    /// lines carry the host only (the standing rule: logs carry shapes, never bodies).
    pub fn with_on_blocked<F>(mut self, callback: F) -> Self
    where
        F: Fn(&str) + Send + Sync + 'static,
    {
        self.on_blocked = Some(Box::new(callback));
        self
    }

    /// What the interim floor says about `request` while the filter lists are still building: the
    /// built-in defaults (SH2), the same rules the compiled engine carries as its own, so a request
    /// is judged the same before and after the lists are ready. It was a separate hand-kept domain
    /// list that blocked more than the engine (all of criteo.com, connect.facebook.net and
    /// googletagmanager.com) and judged a tracker's own site by the same rule. The engine is
    /// neither `Send` nor `Sync`, so each thread that asks builds its own, once (about 40 rules).
    fn pending_floor(&self, request: &Request) -> Option<BlockReason> {
        thread_local! {
            static FLOOR: hiwave_shield::AdBlocker = hiwave_shield::AdBlocker::new();
        }
        let source = first_party(request);
        FLOOR
            .with(|engine| engine.check(&request.url, &source, dest_to_shield(request.destination)))
            .map(|found| BlockReason { list: "domain-floor".into(), rule: found.rule })
    }

    /// Convert HTTP method and URL to a shield ResourceType.
    #[allow(dead_code)]
    fn guess_resource_type(request: &Request) -> ShieldResourceType {
        let url_str = request.url.as_str().to_lowercase();
        let path = request.url.path().to_lowercase();

        // Check file extension
        if path.ends_with(".js") || path.ends_with(".mjs") {
            return ShieldResourceType::Script;
        }
        if path.ends_with(".css") {
            return ShieldResourceType::Stylesheet;
        }
        if path.ends_with(".png")
            || path.ends_with(".jpg")
            || path.ends_with(".jpeg")
            || path.ends_with(".gif")
            || path.ends_with(".webp")
            || path.ends_with(".svg")
            || path.ends_with(".ico")
        {
            return ShieldResourceType::Image;
        }
        if path.ends_with(".woff")
            || path.ends_with(".woff2")
            || path.ends_with(".ttf")
            || path.ends_with(".otf")
            || path.ends_with(".eot")
        {
            return ShieldResourceType::Font;
        }
        if path.ends_with(".mp4")
            || path.ends_with(".webm")
            || path.ends_with(".mp3")
            || path.ends_with(".ogg")
        {
            return ShieldResourceType::Media;
        }

        // Check common ad/tracker patterns in URL
        if url_str.contains("/pixel")
            || url_str.contains("/beacon")
            || url_str.contains("/track")
            || url_str.contains("/analytics")
            || url_str.contains("/collect")
        {
            return ShieldResourceType::Xhr;
        }

        // Check Accept header for hints
        if let Some(accept) = request.headers.get("accept") {
            if let Ok(accept_str) = accept.to_str() {
                if accept_str.contains("application/json")
                    || accept_str.contains("application/xml")
                {
                    return ShieldResourceType::Xhr;
                }
                if accept_str.contains("image/") {
                    return ShieldResourceType::Image;
                }
                if accept_str.contains("text/css") {
                    return ShieldResourceType::Stylesheet;
                }
            }
        }

        ShieldResourceType::Other
    }
}

impl InterceptHandler for ShieldInterceptHandler {
    fn intercept(&self, request: &Request) -> InterceptAction {
        trace!(host = %rustkit_net::origin_for_log(&request.url), "Shield checking request");

        // Privacy pin 2026-09-29: interception happens BEFORE bytes; the
        // census counts every attempt per destination so "blocked" is a
        // measured quantity, not an inference from failures.
        let dest = request.destination;
        self.census.attempted(dest);

        // Global kill-switch (debugging): HIWAVE_SHIELD_OFF=1 wins over
        // everything, read once per process.
        static KILL: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        let killed = *KILL.get_or_init(|| std::env::var("HIWAVE_SHIELD_OFF").is_ok());
        if killed || !self.enabled.load(Ordering::Relaxed) {
            return InterceptAction::Allow;
        }

        // TOP-LEVEL DOCUMENTS ARE NEVER BLOCKED. EasyList carries rules that
        // match ad-tech hosts a user may still navigate to on purpose; the
        // shield's job is protecting a page's subresource graph, not
        // refusing navigations. (Per-site deny of navigations is product
        // policy above this layer, not a filter-list outcome.)
        if dest == RequestDestination::Document {
            return InterceptAction::Allow;
        }

        // First party = the document that made the request (see `first_party`).
        let source = first_party(request);

        // Which list and rule blocked it (dev-tools T2: the event log's
        // `failure.blocked_by`); the floor names itself and its domain.
        let blocked = match shared_block(&request.url, &source, dest_to_shield(dest)) {
            Some(verdict) => verdict.map(|found| BlockReason { list: found.list, rule: found.rule }),
            None => {
                // PENDING FLOOR (Prometheus R1 on this PR): while EasyList is
                // still compiling/downloading, the interim domain list keeps
                // blocking — the pre-PR tip protected from the FIRST
                // subresource and this PR must never lower that, not even
                // for a measured window (and a failed init-thread spawn must
                // not mean permanent allow). The census still counts the
                // window so the upgrade's coverage delta stays a number.
                self.census.engine_pending.fetch_add(1, Ordering::Relaxed);
                self.pending_floor(request)
            }
        };

        if let Some(reason) = blocked {
            self.census.blocked(dest);
            self.blocked_count.fetch_add(1, Ordering::Relaxed);
            debug!(host = %rustkit_net::origin_for_log(&request.url), ?dest, list = %reason.list, "Shield blocked request");
            if let Some(ref callback) = self.on_blocked {
                callback(request.url.as_str());
            }
            InterceptAction::BlockedBy(reason)
        } else {
            InterceptAction::Allow
        }
    }
}


/// The ONE filter engine per process, owned by a dedicated worker thread.
///
/// Two constraints meet here. #346's scar: with_filter_lists() can DOWNLOAD
/// EasyList on a stale cache and compiles ~60k rules — seconds, never on a
/// request path, never per handler. And adblock-rust's inner resource
/// backend is !Sync + !Send (caught by f1-test-compile on the Mac CI; this
/// Linux seat cannot compile the app crate — the declared platform split),
/// so no shared static can hold the engine at all. The engine therefore
/// lives on ONE worker thread that answers queries over channels: a check
/// is two channel hops (~µs) against a network fetch it may save. While the
/// worker is still building — or if it ever dies — callers get None and use
/// the pending FLOOR, so protection never drops below the pre-EasyList tip.
struct ShieldQuery {
    url: String,
    source: String,
    resource_type: hiwave_shield::ResourceType,
    /// The list and rule that block it, `None` when allowed.
    reply: std::sync::mpsc::Sender<Option<hiwave_shield::BlockMatch>>,
}

/// The worker's engine is built (see [`shield_worker`]).
static READY: AtomicBool = AtomicBool::new(false);

/// The worker builds its engine naming matched rules (see [`name_matched_rules`]).
static NAME_RULES: AtomicBool = AtomicBool::new(false);

/// Build the filter engine so a block names its rule and list (the event
/// log's `failure.blocked_by`). The rule text costs about 9 MB, so only a
/// tool that reports it asks (parity-capture `--shield`); call it before
/// the first request or [`wait_until_ready`]. Without it a block from the
/// filter lists names no rule.
pub fn name_matched_rules() {
    NAME_RULES.store(true, Ordering::Release);
}

/// Wait up to `timeout` for the filter engine to be built, starting its
/// build if nothing has yet. True when it is ready: until then every
/// request is judged by the pending floor (`list = "domain-floor"`), so a
/// run that must measure EasyList waits here first.
pub fn wait_until_ready(timeout: std::time::Duration) -> bool {
    let _ = shield_worker();
    let until = std::time::Instant::now() + timeout;
    while !READY.load(Ordering::Acquire) {
        if std::time::Instant::now() >= until {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    true
}

fn shield_worker() -> Option<std::sync::mpsc::Sender<ShieldQuery>> {
    use std::sync::mpsc;
    static WORKER: std::sync::OnceLock<Option<std::sync::Mutex<mpsc::Sender<ShieldQuery>>>> =
        std::sync::OnceLock::new();

    let slot = WORKER.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<ShieldQuery>();
        let spawned = std::thread::Builder::new()
            .name("shield-filter-worker".into())
            .spawn(move || {
                let engine = if NAME_RULES.load(Ordering::Acquire) {
                    hiwave_shield::AdBlocker::with_filter_lists_naming_rules()
                } else {
                    hiwave_shield::AdBlocker::with_filter_lists()
                };
                READY.store(true, std::sync::atomic::Ordering::Release);
                tracing::info!("shield filter engine ready");
                while let Ok(q) = rx.recv() {
                    let verdict = match (Url::parse(&q.url), Url::parse(&q.source)) {
                        (Ok(url), Ok(source)) => engine.check(&url, &source, q.resource_type),
                        _ => None,
                    };
                    let _ = q.reply.send(verdict);
                }
            })
            .is_ok();
        if spawned {
            Some(std::sync::Mutex::new(tx))
        } else {
            None
        }
    });
    // Not ready yet (still downloading/compiling) => None => pending floor.
    if !READY.load(std::sync::atomic::Ordering::Acquire) {
        return None;
    }
    slot.as_ref()
        .and_then(|m| m.lock().ok().map(|tx| tx.clone()))
}

/// Ask the worker; None means "floor decides" (building, dead, or hop
/// failed), `Some(None)` allowed, `Some(Some(_))` blocked by that rule.
fn shared_block(
    url: &Url,
    source: &Url,
    rt: hiwave_shield::ResourceType,
) -> Option<Option<hiwave_shield::BlockMatch>> {
    let tx = shield_worker()?;
    let (reply_tx, reply_rx) = std::sync::mpsc::channel();
    tx.send(ShieldQuery {
        url: url.as_str().to_string(),
        source: source.as_str().to_string(),
        resource_type: rt,
        reply: reply_tx,
    })
    .ok()?;
    reply_rx
        .recv_timeout(std::time::Duration::from_millis(250))
        .ok()
}

/// Map the loader's fetch destination onto adblock-rust's resource type.
fn dest_to_shield(dest: RequestDestination) -> hiwave_shield::ResourceType {
    use hiwave_shield::ResourceType as R;
    match dest {
        RequestDestination::Document => R::Document,
        RequestDestination::Style => R::Stylesheet,
        RequestDestination::Script => R::Script,
        RequestDestination::Image => R::Image,
        RequestDestination::Font => R::Font,
        RequestDestination::Other => R::Other,
        // Filter lists key script requests on `$xmlhttprequest`; fetch() is
        // the same class of request.
        RequestDestination::Fetch | RequestDestination::Xhr => R::Xhr,
    }
}

/// Per-destination attempted/blocked counters — the census the privacy pin
/// requires so the board can separate "tracker blocked" (working as
/// intended) from "site failed".
#[derive(Default)]
pub struct ShieldCensus {
    attempted: [AtomicU64; 8],
    blocked: [AtomicU64; 8],
    /// Requests that passed unchecked while the filter engine was still
    /// building at startup (the allow-until-ready window).
    pub engine_pending: AtomicU64,
}

impl ShieldCensus {
    fn idx(dest: RequestDestination) -> usize {
        match dest {
            RequestDestination::Document => 0,
            RequestDestination::Style => 1,
            RequestDestination::Script => 2,
            RequestDestination::Image => 3,
            RequestDestination::Font => 4,
            RequestDestination::Other => 5,
            RequestDestination::Fetch => 6,
            RequestDestination::Xhr => 7,
        }
    }
    fn attempted(&self, d: RequestDestination) {
        self.attempted[Self::idx(d)].fetch_add(1, Ordering::Relaxed);
    }
    fn blocked(&self, d: RequestDestination) {
        self.blocked[Self::idx(d)].fetch_add(1, Ordering::Relaxed);
    }
    /// (attempted, blocked) per destination, in enum order.
    pub fn snapshot(&self) -> [(u64, u64); 8] {
        std::array::from_fn(|i| {
            (
                self.attempted[i].load(Ordering::Relaxed),
                self.blocked[i].load(Ordering::Relaxed),
            )
        })
    }
}

impl Default for ShieldInterceptHandler {
    fn default() -> Self {
        Self::new()
    }
}

/// Create a request interceptor with the default shield handler.
pub fn create_shield_interceptor() -> rustkit_net::RequestInterceptor {
    let handler = ShieldInterceptHandler::new();
    let mut interceptor = rustkit_net::RequestInterceptor::new();
    interceptor.add_handler(Arc::new(handler));
    interceptor
}

/// Create a request interceptor with a shared counter.
pub fn create_shield_interceptor_with_counter(
    counter: Arc<AtomicU64>,
) -> rustkit_net::RequestInterceptor {
    let handler = ShieldInterceptHandler::with_counter(counter);
    let mut interceptor = rustkit_net::RequestInterceptor::new();
    interceptor.add_handler(Arc::new(handler));
    interceptor
}

/// Create a request interceptor with a blocked callback.
pub fn create_shield_interceptor_with_callback<F>(
    on_blocked: F,
) -> rustkit_net::RequestInterceptor
where
    F: Fn(&str) + Send + Sync + 'static,
{
    let handler = ShieldInterceptHandler::new().with_on_blocked(on_blocked);
    let mut interceptor = rustkit_net::RequestInterceptor::new();
    interceptor.add_handler(Arc::new(handler));
    interceptor
}

/// The first party a request is judged against: the document that made it, which the loader
/// carries as the referrer (the document's URL, whatever its referrer policy sends as a header).
/// A request with none (a view loaded without a URL) gets an opaque first party that matches no
/// site, so every third-party rule applies to it, as for a document whose origin is opaque.
/// Judging it against its own URL made it first-party to itself and turned the `$third-party`
/// defaults off there (R1 on #1132).
fn first_party(request: &Request) -> Url {
    match &request.referrer {
        Some(referrer) => referrer.clone(),
        None => Url::parse("https://opaque.invalid/").expect("a constant URL"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::Method;
    use rustkit_net::RequestId;
    use url::Url;

    fn test_request(url_str: &str) -> Request {
        test_request_with(url_str, RequestDestination::Other, None)
    }

    fn test_request_with(
        url_str: &str,
        destination: RequestDestination,
        referrer: Option<&str>,
    ) -> Request {
        Request {
            id: RequestId::new(),
            url: Url::parse(url_str).unwrap(),
            method: Method::GET,
            headers: Default::default(),
            body: None,
            timeout: None,
            credentials: Default::default(),
            referrer: referrer.map(|r| Url::parse(r).unwrap()),
            referrer_policy: Default::default(),
            destination,
            is_replay_proxied: false,
        }
    }

    #[test]
    fn test_resource_type_detection() {
        let js_req = test_request("https://example.com/script.js");
        assert!(matches!(
            ShieldInterceptHandler::guess_resource_type(&js_req),
            ShieldResourceType::Script
        ));

        let css_req = test_request("https://example.com/style.css");
        assert!(matches!(
            ShieldInterceptHandler::guess_resource_type(&css_req),
            ShieldResourceType::Stylesheet
        ));

        let img_req = test_request("https://example.com/image.png");
        assert!(matches!(
            ShieldInterceptHandler::guess_resource_type(&img_req),
            ShieldResourceType::Image
        ));

        let track_req = test_request("https://example.com/pixel/track");
        assert!(matches!(
            ShieldInterceptHandler::guess_resource_type(&track_req),
            ShieldResourceType::Xhr
        ));
    }

    #[test]
    fn dest_to_shield_covers_every_fetch_destination() {
        // A missing arm would fail to compile; pin the adblock strings the
        // EasyList `$type` options expect — including Fetch/Xhr → xmlhttprequest.
        assert!(matches!(
            dest_to_shield(RequestDestination::Document),
            hiwave_shield::ResourceType::Document
        ));
        assert!(matches!(
            dest_to_shield(RequestDestination::Style),
            hiwave_shield::ResourceType::Stylesheet
        ));
        assert!(matches!(
            dest_to_shield(RequestDestination::Script),
            hiwave_shield::ResourceType::Script
        ));
        assert!(matches!(
            dest_to_shield(RequestDestination::Image),
            hiwave_shield::ResourceType::Image
        ));
        assert!(matches!(
            dest_to_shield(RequestDestination::Font),
            hiwave_shield::ResourceType::Font
        ));
        assert!(matches!(
            dest_to_shield(RequestDestination::Other),
            hiwave_shield::ResourceType::Other
        ));
        assert!(matches!(
            dest_to_shield(RequestDestination::Fetch),
            hiwave_shield::ResourceType::Xhr
        ));
        assert!(matches!(
            dest_to_shield(RequestDestination::Xhr),
            hiwave_shield::ResourceType::Xhr
        ));
    }

    #[test]
    fn top_level_documents_are_never_blocked() {
        // EasyList carries host rules that match ad-tech origins a user may
        // still navigate to. The interceptor must refuse to block Document
        // even when the host is on the domain floor / EasyList.
        let handler = ShieldInterceptHandler::new();
        let action = handler.intercept(&test_request_with(
            "https://doubleclick.net/",
            RequestDestination::Document,
            None,
        ));
        assert!(matches!(action, InterceptAction::Allow));
        assert_eq!(handler.blocked_count(), 0);
        let census = handler.census_snapshot();
        assert_eq!(census[0].0, 1, "document attempts are still counted");
        assert_eq!(census[0].1, 0, "documents must never enter blocked");
    }

    #[test]
    fn known_tracker_subresources_are_blocked() {
        // Deterministic regardless of EasyList readiness: the pending floor
        // is the compiled engine's own defaults (SH2), so both paths agree.
        // Either path must Block before bytes.
        let handler = ShieldInterceptHandler::new();
        let action = handler.intercept(&test_request_with(
            "https://doubleclick.net/pagead.js",
            RequestDestination::Script,
            Some("https://news.example/"),
        ));
        // Before the lists are ready: the floor and the default rule it shares with the engine. Once ready,
        // with no `name_matched_rules()` (these tests never call it), two
        // lists name neither list nor rule; with no lists on disk the engine
        // is the defaults, one list that names its rule.
        match action {
            InterceptAction::BlockedBy(reason) => assert!(
                reason.list == "domain-floor" && reason.rule.as_deref() == Some("||doubleclick.net^$third-party")
                    || reason.list == hiwave_shield::UNATTRIBUTED && reason.rule.is_none()
                    || reason.list == "HiWave defaults" && reason.rule.as_deref() == Some("||doubleclick.net^$third-party"),
                "{reason:?}"
            ),
            other => panic!("not blocked with a reason: {other:?}"),
        }
        assert_eq!(handler.blocked_count(), 1);
        let census = handler.census_snapshot();
        assert_eq!(census[2].0, 1, "script attempted");
        assert_eq!(census[2].1, 1, "script blocked");
    }

    #[derive(Clone, Default)]
    struct Buf(Arc<std::sync::Mutex<Vec<u8>>>);
    impl std::io::Write for Buf {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Buf {
        type Writer = Buf;
        fn make_writer(&'a self) -> Buf {
            self.clone()
        }
    }

    /// Logs carry shapes, never bodies (the standing rule): the handler logs a blocked request by
    /// its host. Its path, query and fragment, and the page it came from, are not logged.
    #[test]
    fn a_blocked_request_is_logged_by_host_only() {
        let buf = Buf::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(buf.clone())
            .with_max_level(tracing::Level::TRACE)
            .with_ansi(false)
            .finish();
        let handler = ShieldInterceptHandler::new();
        let action = tracing::subscriber::with_default(subscriber, || {
            handler.intercept(&test_request_with(
                "https://adsrvr.org/pixel/u123abc?session=s3cr3t#frag9",
                RequestDestination::Script,
                Some("https://inbox.example/mail/thread42?token=t0k3n"),
            ))
        });
        assert!(!matches!(action, InterceptAction::Allow), "adsrvr.org is blocked: {action:?}");
        let text = String::from_utf8(buf.0.lock().unwrap().clone()).unwrap();
        assert!(text.contains("adsrvr.org"), "the block is logged, by host: {text}");
        for secret in ["pixel", "u123abc", "session", "s3cr3t", "frag9", "inbox", "thread42", "t0k3n"] {
            assert!(!text.contains(secret), "{secret} reached a log line: {text}");
        }
    }

    /// SH1, end to end through the app's own worker and the lists on disk: once the engine is
    /// READY (the test waits for it, bounded, instead of racing it) the bare floor host is still
    /// blocked, which the lists alone did not do (EasyList names `ad.doubleclick.net`, not the
    /// bare host), and what a captcha or a login needs is still allowed.
    #[test]
    fn the_ready_engine_still_blocks_the_bare_floor_host_and_allows_captchas_and_logins() {
        let source = Url::parse("https://news.example/").unwrap();
        let ask = |url: &str, rt: hiwave_shield::ResourceType| {
            let url = Url::parse(url).unwrap();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
            loop {
                if let Some(verdict) = shared_block(&url, &source, rt) {
                    return verdict;
                }
                assert!(std::time::Instant::now() < deadline, "the filter engine never became ready: {url}");
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
        };
        use hiwave_shield::ResourceType as R;
        // A list really loaded (Prometheus #1058 nit 1): `READY` is also set when no list was on
        // disk and the engine fell back to the built-in rules alone, which would make the rest of
        // this test prove nothing. `track.adform.net` is blocked by EasyList and by no built-in
        // rule, so when the lists are cached it must be blocked here. (Nothing cached: say so.)
        let cached = dirs::data_local_dir()
            .map(|d| d.join("hiwave").join("filter-lists").join("easylist.txt"))
            .is_some_and(|p| std::fs::metadata(p).is_ok_and(|m| m.len() > 100_000));
        if cached {
            assert!(ask("https://track.adform.net/x.js", R::Script).is_some(), "EasyList is cached, so the engine must have loaded it");
        } else {
            eprintln!("NOTE: no cached EasyList: this run proves the defaults-only fallback, not the union");
        }
        assert!(ask("https://doubleclick.net/pagead.js", R::Script).is_some(), "the bare floor host is blocked");
        assert!(ask("https://www.google.com/recaptcha/api2/aframe", R::Other).is_none(), "a captcha frame is allowed");
        assert!(ask("https://www.google.com/recaptcha/api2/anchor?k=1", R::Other).is_none(), "the reCAPTCHA anchor is allowed");
        assert!(ask("https://hcaptcha.com/1/api.js", R::Script).is_none(), "hCaptcha is allowed");
        assert!(ask("https://connect.facebook.net/en_US/sdk.js", R::Script).is_none(), "the Facebook SDK is allowed");
        assert!(ask("https://news.example/app.js", R::Script).is_none(), "a first-party script is allowed");
    }

    #[test]
    fn subdomain_of_a_blocked_host_is_blocked_by_the_floor() {
        let handler = ShieldInterceptHandler::new();
        let blocked = |url: &str| {
            handler
                .pending_floor(&test_request_with(url, RequestDestination::Script, Some("https://news.example/")))
                .is_some()
        };
        assert!(blocked("https://secure.adnxs.com/a.js"));
        assert!(blocked("https://ADNXS.COM/a.js"));
        assert!(!blocked("https://not-adnxs.com/a.js"));
        assert!(!blocked("https://example.com/a.js"));
    }

    #[test]
    fn benign_subresources_are_allowed() {
        let handler = ShieldInterceptHandler::new();
        let action = handler.intercept(&test_request_with(
            "https://example.com/app.js",
            RequestDestination::Script,
            Some("https://example.com/"),
        ));
        assert!(matches!(action, InterceptAction::Allow));
        assert_eq!(handler.blocked_count(), 0);
    }

    #[test]
    fn disabled_handler_allows_even_known_trackers() {
        let handler = ShieldInterceptHandler::new();
        handler.set_enabled(false);
        let action = handler.intercept(&test_request_with(
            "https://doubleclick.net/pagead.js",
            RequestDestination::Script,
            Some("https://news.example/"),
        ));
        assert!(matches!(action, InterceptAction::Allow));
        assert_eq!(handler.blocked_count(), 0);
        let census = handler.census_snapshot();
        assert_eq!(census[2].0, 1, "attempts still counted while disabled");
        assert_eq!(census[2].1, 0);
    }

    /// SH2: the pending floor and the compiled defaults say the same thing about the same request,
    /// so a page does not behave differently before and after the filter lists are ready.
    #[test]
    fn the_pending_floor_agrees_with_the_defaults_engine() {
        let handler = ShieldInterceptHandler::new();
        let defaults = hiwave_shield::AdBlocker::new();
        let page = "https://news.example/";
        let cases = [
            ("https://mixpanel.com/lib.js", "https://mixpanel.com/"),
            ("https://mixpanel.com/lib.js", page),
            ("https://connect.facebook.net/en_US/sdk.js", page),
            ("https://www.criteo.com/ads.js", page),
            ("https://static.criteo.com/sync", page),
            ("https://www.googletagmanager.com/gtm.js?id=GTM-1", page),
            ("https://doubleclick.net/pagead.js", page),
            ("https://example.com/app.js", page),
        ];
        let mut differ = Vec::new();
        for (url, source) in cases {
            let request = test_request_with(url, RequestDestination::Script, Some(source));
            let floor = handler.pending_floor(&request).is_some();
            let engine = defaults
                .check(&Url::parse(url).unwrap(), &Url::parse(source).unwrap(), hiwave_shield::ResourceType::Script)
                .is_some();
            if floor != engine {
                differ.push(format!("{url} from {source}: floor {floor}, defaults engine {engine}"));
            }
        }
        assert!(differ.is_empty(), "{}", differ.join("
"));
    }

    /// A request with no first party (a view loaded without a URL passes no referrer) is judged
    /// against an opaque first party that matches no site, so every `$third-party` default applies
    /// (R1 LOW on #1132). It was judged against its own URL, which made it first-party to itself and
    /// turned every default off there.
    #[test]
    fn a_request_with_no_first_party_still_meets_the_third_party_defaults() {
        let handler = ShieldInterceptHandler::new();
        for url in ["https://doubleclick.net/pagead.js", "https://mixpanel.com/lib.js"] {
            let request = test_request_with(url, RequestDestination::Script, None);
            assert!(handler.pending_floor(&request).is_some(), "{url} with no first party was allowed");
        }
        let own = test_request_with("https://mixpanel.com/lib.js", RequestDestination::Script, Some("https://mixpanel.com/"));
        assert!(handler.pending_floor(&own).is_none(), "a site's own script was blocked");
    }
}
