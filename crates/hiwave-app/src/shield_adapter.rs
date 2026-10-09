//! Adapter to connect hiwave-shield to rustkit-net's request interception.
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
use rustkit_net::{InterceptAction, InterceptHandler, Request, RequestDestination};
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use tracing::{debug, trace};

/// Blocked domains for ad/tracker blocking.
/// These are the most common ad and tracking domains.
const BLOCKED_DOMAINS: &[&str] = &[
    "doubleclick.net",
    "googlesyndication.com",
    "googleadservices.com",
    "adtrafficquality.google",
    "ads.twitter.com",
    "facebook.com/tr",
    "connect.facebook.net",
    "tr.snapchat.com",
    "amazon-adsystem.com",
    "criteo.com",
    "adnxs.com",
    "adsrvr.org",
    "adroll.com",
    "taboola.com",
    "outbrain.com",
    "rubiconproject.com",
    "openx.net",
    "pubmatic.com",
    "scorecardresearch.com",
    "chartbeat.com",
    "segment.io",
    "segment.com",
    "mixpanel.com",
    "hotjar.com",
    "fullstory.com",
    "googletagmanager.com",
];

/// Thread-safe adapter that implements rustkit-net's InterceptHandler.
pub struct ShieldInterceptHandler {
    /// Whether blocking is enabled.
    enabled: Arc<AtomicBool>,
    /// Counter for blocked requests.
    blocked_count: Arc<AtomicU64>,
    /// Set of blocked domain patterns (fallback + tests).
    blocked_domains: HashSet<String>,
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
        let blocked_domains: HashSet<String> = BLOCKED_DOMAINS
            .iter()
            .map(|s| s.to_string())
            .collect();

        Self {
            enabled: Arc::new(AtomicBool::new(true)),
            blocked_count: Arc::new(AtomicU64::new(0)),
            blocked_domains,
            census: Arc::new(ShieldCensus::default()),
            on_blocked: None,
        }
    }

    /// Create with a shared counter for tracking blocked requests.
    pub fn with_counter(blocked_count: Arc<AtomicU64>) -> Self {
        // Production constructor (webview_rustkit path): start closing the
        // pending window here too, not only in new().
        let _ = shield_worker();
        let blocked_domains: HashSet<String> = BLOCKED_DOMAINS
            .iter()
            .map(|s| s.to_string())
            .collect();

        Self {
            enabled: Arc::new(AtomicBool::new(true)),
            blocked_count,
            blocked_domains,
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

    /// Set a callback to be called when a request is blocked.
    pub fn with_on_blocked<F>(mut self, callback: F) -> Self
    where
        F: Fn(&str) + Send + Sync + 'static,
    {
        self.on_blocked = Some(Box::new(callback));
        self
    }

    /// Check if a host should be blocked.
    fn should_block_host(&self, host: &str) -> bool {
        let host_lower = host.to_lowercase();
        for domain in &self.blocked_domains {
            if host_lower == *domain || host_lower.ends_with(&format!(".{}", domain)) {
                return true;
            }
        }
        false
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
        trace!(url = %request.url, "Shield checking request");

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

        // First party = the document that made the request. The loader
        // carries it as the (policy-governed) referrer; a subresource with
        // no referrer context is judged against its own origin, which
        // disables third-party-only rules rather than inventing a party.
        let source = request.referrer.as_ref().unwrap_or(&request.url);

        let blocked = match shared_block(&request.url, source, dest_to_shield(dest)) {
            Some(verdict) => verdict,
            None => {
                // PENDING FLOOR (Prometheus R1 on this PR): while EasyList is
                // still compiling/downloading, the interim domain list keeps
                // blocking — the pre-PR tip protected from the FIRST
                // subresource and this PR must never lower that, not even
                // for a measured window (and a failed init-thread spawn must
                // not mean permanent allow). The census still counts the
                // window so the upgrade's coverage delta stays a number.
                self.census.engine_pending.fetch_add(1, Ordering::Relaxed);
                request
                    .url
                    .host_str()
                    .map(|host| self.should_block_host(host))
                    .unwrap_or(false)
            }
        };

        if blocked {
            self.census.blocked(dest);
            self.blocked_count.fetch_add(1, Ordering::Relaxed);
            debug!(url = %request.url, ?dest, "Shield blocked request");
            if let Some(ref callback) = self.on_blocked {
                callback(request.url.as_str());
            }
            InterceptAction::Block
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
    reply: std::sync::mpsc::Sender<bool>,
}

fn shield_worker() -> Option<std::sync::mpsc::Sender<ShieldQuery>> {
    use std::sync::mpsc;
    static WORKER: std::sync::OnceLock<Option<std::sync::Mutex<mpsc::Sender<ShieldQuery>>>> =
        std::sync::OnceLock::new();
    static READY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

    let slot = WORKER.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<ShieldQuery>();
        let spawned = std::thread::Builder::new()
            .name("shield-filter-worker".into())
            .spawn(move || {
                let engine = hiwave_shield::AdBlocker::with_filter_lists();
                READY.store(true, std::sync::atomic::Ordering::Release);
                tracing::info!("shield filter engine ready");
                while let Ok(q) = rx.recv() {
                    let verdict = match (Url::parse(&q.url), Url::parse(&q.source)) {
                        (Ok(url), Ok(source)) => {
                            engine.should_block(&url, &source, q.resource_type)
                        }
                        _ => false,
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

/// Ask the worker; None means "floor decides" (building, dead, or hop failed).
fn shared_block(url: &Url, source: &Url, rt: hiwave_shield::ResourceType) -> Option<bool> {
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
        // uses BLOCKED_DOMAINS, and the compiled engine includes the same
        // hosts. Either path must Block before bytes.
        let handler = ShieldInterceptHandler::new();
        let action = handler.intercept(&test_request_with(
            "https://doubleclick.net/pagead.js",
            RequestDestination::Script,
            Some("https://news.example/"),
        ));
        assert!(matches!(action, InterceptAction::Block));
        assert_eq!(handler.blocked_count(), 1);
        let census = handler.census_snapshot();
        assert_eq!(census[2].0, 1, "script attempted");
        assert_eq!(census[2].1, 1, "script blocked");
    }

    #[test]
    fn subdomain_of_a_blocked_host_is_blocked_by_the_floor() {
        let handler = ShieldInterceptHandler::new();
        assert!(handler.should_block_host("secure.adnxs.com"));
        assert!(handler.should_block_host("ADNXS.COM"));
        assert!(!handler.should_block_host("not-adnxs.com"));
        assert!(!handler.should_block_host("example.com"));
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
}

