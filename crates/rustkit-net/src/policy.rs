//! The one allow/deny layer for script-initiated requests.
//!
//! `XMLHttpRequest` and `fetch()` (bindings, built elsewhere) hand a
//! [`ScriptRequest`] to a page's [`FetchPolicy`]; nothing a script asks for
//! reaches the network any other way. The policy owns scheme, private-network
//! (on the RESOLVED address, at every redirect hop), mixed-content, CSP
//! `connect-src`, same-origin/CORS (with preflight), redirects, and the
//! resource caps. A denied request returns [`Denial`]; script only ever sees
//! an error, never the denied response.
//!
//! Deliberate limits (stated, not half-built): script requests carry no
//! cookies (there is no cookie jar to partition; `credentials` degrades to
//! `omit` on the wire, though CORS is still checked as if the page had asked
//! for credentials), responses arrive whole (no streaming bodies), and
//! `WebSocket`/`EventSource`/`sendBeacon` are out of scope.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bytes::Bytes;
use http::{HeaderMap, HeaderValue, Method};
use rustkit_http::AddressPolicy;
use tokio::sync::Semaphore;
use url::Url;

use crate::security::{
    check_mixed_content, ContentSecurityPolicy, CorsChecker, CorsResult, CspDirective, CspSource,
    MixedContentResult, MixedContentType, Origin,
};
use crate::{
    CredentialsMode, NetError, RedirectMode, Request, RequestDestination, RequestId,
    ResourceLoader,
};

/// fetch()'s `mode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestMode {
    Cors,
    NoCors,
    SameOrigin,
}

/// A request a page script made, before any policy decision.
#[derive(Debug, Clone)]
pub struct ScriptRequest {
    pub id: RequestId,
    pub method: Method,
    pub url: Url,
    pub headers: HeaderMap,
    pub body: Option<Bytes>,
    pub mode: RequestMode,
    /// Only consulted for the CORS credentials rules: nothing is ever sent.
    pub credentials: CredentialsMode,
    pub redirect: RedirectMode,
    /// `Fetch`, `Xhr` or `Script` (module loads); anything else is treated as `Fetch`.
    pub destination: RequestDestination,
}

impl ScriptRequest {
    pub fn get(url: Url) -> Self {
        Self {
            id: RequestId::new(),
            method: Method::GET,
            url,
            headers: HeaderMap::new(),
            body: None,
            mode: RequestMode::Cors,
            credentials: CredentialsMode::SameOrigin,
            redirect: RedirectMode::Follow,
            destination: RequestDestination::Fetch,
        }
    }
}

/// What kind of response the script is allowed to see.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponseKind {
    /// Same-origin.
    Basic,
    /// Cross-origin, passed CORS: safelisted + exposed headers only.
    Cors,
    /// `no-cors` cross-origin: status 0, no headers, no body.
    Opaque,
    /// `redirect: manual` hit a redirect: status 0.
    OpaqueRedirect,
}

/// The script-visible result.
#[derive(Debug, Clone)]
pub struct ScriptResponse {
    pub id: RequestId,
    /// The final URL (after redirects).
    pub url: Url,
    pub status: u16,
    pub status_text: String,
    pub headers: HeaderMap,
    pub body: Bytes,
    pub kind: ResponseKind,
    pub redirected: bool,
}

/// A fetched module script: the URL it ended at (its identity, and the base
/// for its own imports) and its UTF-8 source.
#[derive(Debug, Clone)]
pub struct ModuleSource {
    pub final_url: Url,
    pub source: String,
}

/// Why a request was refused. Script sees an error event / `TypeError`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Denial {
    Scheme(String),
    /// Forbidden method, a body on GET/HEAD, header caps, or `no-cors` misuse.
    BadRequest(String),
    PrivateNetwork(String),
    MixedContent,
    Csp,
    SameOriginOnly,
    Cors(String),
    Preflight(String),
    Redirect(String),
    TooManyRedirects,
    RequestTooLarge,
    ResponseTooLarge,
    Timeout,
    BudgetExhausted,
    /// The shield (EasyList) blocked it, exactly as it blocks a subresource.
    Shield,
    /// A module script whose response is not a JavaScript MIME type.
    BadMime(String),
    /// A module script whose response is not 2xx.
    BadStatus(u16),
    Network(String),
    /// The page navigated away or the script aborted the request.
    Cancelled,
}

impl std::fmt::Display for Denial {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

/// Per-page caps (Pete's signed values).
#[derive(Debug, Clone)]
pub struct FetchLimits {
    pub max_redirects: usize,
    pub max_response_bytes: usize,
    pub max_request_body_bytes: usize,
    pub timeout: Duration,
    pub max_header_count: usize,
    pub max_header_bytes: usize,
    pub max_in_flight_per_origin: usize,
    pub max_in_flight_per_page: usize,
    pub max_total_requests: usize,
}

impl Default for FetchLimits {
    fn default() -> Self {
        Self {
            max_redirects: 20,
            max_response_bytes: 10 * 1024 * 1024,
            max_request_body_bytes: 10 * 1024 * 1024,
            timeout: Duration::from_secs(30),
            max_header_count: 64,
            max_header_bytes: 16 * 1024,
            max_in_flight_per_origin: 6,
            max_in_flight_per_page: 24,
            max_total_requests: 500,
        }
    }
}

/// Cancels one request: XHR `abort()`, a fetch `AbortSignal`. Clones share
/// the signal; cancelling is idempotent and works from any thread.
#[derive(Clone)]
pub struct CancelToken {
    tx: Arc<tokio::sync::watch::Sender<bool>>,
}

impl Default for CancelToken {
    fn default() -> Self {
        Self::new()
    }
}

impl CancelToken {
    pub fn new() -> Self {
        Self { tx: Arc::new(tokio::sync::watch::channel(false).0) }
    }

    pub fn cancel(&self) {
        self.tx.send_replace(true);
    }

    pub fn is_cancelled(&self) -> bool {
        *self.tx.borrow()
    }
}

/// The policy for one page. Share it behind an `Arc`; it holds the page's
/// request counters and preflight cache.
pub struct FetchPolicy {
    page_url: Url,
    page_origin: Origin,
    csp: Option<ContentSecurityPolicy>,
    limits: FetchLimits,
    total: AtomicUsize,
    cancel: tokio::sync::watch::Sender<bool>,
    page_slots: Arc<Semaphore>,
    origin_slots: Mutex<HashMap<String, Arc<Semaphore>>>,
    preflight: Mutex<HashMap<String, PreflightEntry>>,
    #[cfg(test)]
    address_override: Option<AddressPolicy>,
}

struct PreflightEntry {
    methods: HashSet<String>,
    headers: HashSet<String>,
    any_method: bool,
    any_header: bool,
    expires: Instant,
}

const SAFELISTED_RESPONSE_HEADERS: [&str; 7] = [
    "cache-control",
    "content-language",
    "content-length",
    "content-type",
    "expires",
    "last-modified",
    "pragma",
];

/// Request headers a script may not set; the engine owns them.
/// The JavaScript MIME types (WHATWG MIME Sniffing §4.6): the essence only,
/// parameters ignored, ASCII case-insensitive.
fn is_javascript_mime(content_type: &str) -> bool {
    let essence = content_type.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    matches!(
        essence.as_str(),
        "application/ecmascript"
            | "application/javascript"
            | "application/x-ecmascript"
            | "application/x-javascript"
            | "text/ecmascript"
            | "text/javascript"
            | "text/javascript1.0"
            | "text/javascript1.1"
            | "text/javascript1.2"
            | "text/javascript1.3"
            | "text/javascript1.4"
            | "text/javascript1.5"
            | "text/jscript"
            | "text/livescript"
            | "text/x-ecmascript"
            | "text/x-javascript"
    )
}

fn is_forbidden_request_header(name: &str) -> bool {
    matches!(
        name,
        "cookie"
            | "cookie2"
            | "host"
            | "origin"
            | "referer"
            | "content-length"
            | "connection"
            | "transfer-encoding"
            | "upgrade"
            | "keep-alive"
            | "te"
            | "trailer"
            | "expect"
            | "date"
            | "dnt"
            | "via"
            | "accept-charset"
            | "accept-encoding"
            | "access-control-request-headers"
            | "access-control-request-method"
    ) || name.starts_with("proxy-")
        || name.starts_with("sec-")
}

fn is_redirect_status(status: u16) -> bool {
    matches!(status, 301 | 302 | 303 | 307 | 308)
}

fn is_safelisted_request_header(name: &str, value: &str) -> bool {
    match name {
        "accept" | "accept-language" | "content-language" => true,
        "content-type" => {
            let v = value.trim().to_ascii_lowercase();
            v.starts_with("application/x-www-form-urlencoded")
                || v.starts_with("multipart/form-data")
                || v.starts_with("text/plain")
        }
        _ => false,
    }
}

fn non_safelisted_headers(headers: &HeaderMap) -> Vec<String> {
    let mut names: Vec<String> = headers
        .iter()
        .filter(|(n, v)| !is_safelisted_request_header(n.as_str(), v.to_str().unwrap_or("\u{0}")))
        .map(|(n, _)| n.as_str().to_ascii_lowercase())
        .collect();
    names.sort();
    names.dedup();
    names
}

fn is_safelisted_method(m: &Method) -> bool {
    matches!(*m, Method::GET | Method::HEAD | Method::POST)
}

fn header_str<'a>(h: &'a HeaderMap, name: &str) -> Option<&'a str> {
    h.get(name).and_then(|v| v.to_str().ok())
}

fn split_list(v: Option<&str>) -> Vec<String> {
    v.map(|s| {
        s.split(',')
            .map(|p| p.trim().to_string())
            .filter(|p| !p.is_empty())
            .collect()
    })
    .unwrap_or_default()
}

fn map_net_error(e: NetError) -> Denial {
    match e {
        NetError::Blocked => Denial::Shield,
        NetError::Timeout(_) | NetError::HttpError(rustkit_http::HttpError::Timeout) => Denial::Timeout,
        NetError::HttpError(rustkit_http::HttpError::AddressDenied(a)) => Denial::PrivateNetwork(a),
        NetError::HttpError(rustkit_http::HttpError::BodyTooLarge(_)) => Denial::ResponseTooLarge,
        other => Denial::Network(other.to_string()),
    }
}

/// Which addresses a request made on behalf of `page` for `target` may reach.
///
/// Public pages reach public addresses only. A page served from a private
/// origin (loopback / private literal / `localhost`) may additionally reach
/// its OWN origin: same port, and the page's own host (the literal itself, or
/// loopback for a `localhost` name). Anything else private stays denied, on
/// every redirect hop. A named intranet host counts as public (deny by
/// default); a page with no http(s) origin (`file:`, `data:`) gets
/// public-only.
pub(crate) fn page_address_policy(page: &Url, target: &Url) -> AddressPolicy {
    let same_origin = Origin::from_url(page).same_origin(&Origin::from_url(target));
    let page_ip: Option<std::net::IpAddr> = match page.host() {
        Some(url::Host::Ipv4(ip)) if !rustkit_http::is_public_ip(ip.into()) => Some(ip.into()),
        Some(url::Host::Ipv6(ip)) if !rustkit_http::is_public_ip(ip.into()) => Some(ip.into()),
        _ => None,
    };
    let page_local_name = matches!(page.host(), Some(url::Host::Domain(d)) if rustkit_http::is_local_name(d));
    if !same_origin || !matches!(page.scheme(), "http" | "https") || (page_ip.is_none() && !page_local_name) {
        return AddressPolicy::PublicOnly;
    }
    let port = target.port_or_known_default().unwrap_or(0);
    AddressPolicy::Custom(Arc::new(move |a| {
        rustkit_http::is_public_ip(a.ip())
            || (a.port() == port
                && match page_ip {
                    Some(ip) => a.ip() == ip,
                    None => a.ip().is_loopback(),
                })
    }))
}

/// True when `url`'s host is an IP literal outside the public range or a
/// local name — something a restricted client will refuse to connect to, and
/// therefore must not be answered from the shared cache either.
pub(crate) fn url_host_is_private(url: &Url) -> bool {
    match url.host() {
        Some(url::Host::Ipv4(ip)) => !rustkit_http::is_public_ip(ip.into()),
        Some(url::Host::Ipv6(ip)) => !rustkit_http::is_public_ip(ip.into()),
        Some(url::Host::Domain(d)) => rustkit_http::is_local_name(d),
        None => false,
    }
}

/// The per-hop state of one script request.
struct Hop {
    url: Url,
    method: Method,
    body: Option<Bytes>,
    headers: HeaderMap,
    tainted: bool,
}

impl FetchPolicy {
    pub fn for_page(page_url: Url, csp: Option<ContentSecurityPolicy>) -> Self {
        Self::with_limits(page_url, csp, FetchLimits::default())
    }

    pub fn with_limits(
        page_url: Url,
        csp: Option<ContentSecurityPolicy>,
        limits: FetchLimits,
    ) -> Self {
        Self {
            page_origin: Origin::from_url(&page_url),
            csp,
            total: AtomicUsize::new(0),
            cancel: tokio::sync::watch::channel(false).0,
            page_slots: Arc::new(Semaphore::new(limits.max_in_flight_per_page)),
            origin_slots: Mutex::new(HashMap::new()),
            preflight: Mutex::new(HashMap::new()),
            limits,
            page_url,
            #[cfg(test)]
            address_override: None,
        }
    }

    /// Cancel every request this policy has in flight or queued, and refuse
    /// any later one with `Denial::Cancelled`. Idempotent, any thread.
    pub fn cancel(&self) {
        self.cancel.send_replace(true);
    }

    /// True once [`cancel`](Self::cancel) has been called.
    pub fn is_cancelled(&self) -> bool {
        *self.cancel.borrow()
    }

    /// Run one request to completion under the policy.
    ///
    /// Racing the whole request against [`cancel`](Self::cancel) is what
    /// closes the socket: dropping the in-flight future drops its connection
    /// and releases its queue slots.
    pub async fn execute(
        &self,
        loader: &ResourceLoader,
        req: ScriptRequest,
    ) -> Result<ScriptResponse, Denial> {
        self.execute_raced(loader, req, None).await
    }

    /// [`execute`](Self::execute) that `token` can also end. An aborted
    /// request closes only its own connection and frees its queue slots; the
    /// rest of the page's requests are untouched.
    pub async fn execute_cancellable(
        &self,
        loader: &ResourceLoader,
        req: ScriptRequest,
        token: &CancelToken,
    ) -> Result<ScriptResponse, Denial> {
        self.execute_raced(loader, req, Some(token)).await
    }

    async fn execute_raced(
        &self,
        loader: &ResourceLoader,
        req: ScriptRequest,
        token: Option<&CancelToken>,
    ) -> Result<ScriptResponse, Denial> {
        let mut page = self.cancel.subscribe();
        let mut own = token.map(|t| t.tx.subscribe());
        tokio::select! {
            biased;
            _ = async { let _ = page.wait_for(|c| *c).await; } => Err(Denial::Cancelled),
            _ = async {
                match own.as_mut() {
                    Some(rx) => { let _ = rx.wait_for(|c| *c).await; }
                    None => std::future::pending::<()>().await,
                }
            } => Err(Denial::Cancelled),
            r = self.execute_governed(loader, req) => r,
        }
    }

    async fn execute_governed(
        &self,
        loader: &ResourceLoader,
        req: ScriptRequest,
    ) -> Result<ScriptResponse, Denial> {
        if self.total.fetch_add(1, Ordering::SeqCst) >= self.limits.max_total_requests {
            self.total.fetch_sub(1, Ordering::SeqCst);
            return Err(Denial::BudgetExhausted);
        }
        self.validate_shape(&req)?;

        // Origin slot first, then page slot: every task takes them in the same
        // order (no deadlock), and a queued request holds only its own
        // origin's slot, so one busy origin cannot starve the rest of the page.
        let origin_key = Origin::from_url(&req.url).serialize();
        let origin_sem = self
            .origin_slots
            .lock()
            .unwrap()
            .entry(origin_key)
            .or_insert_with(|| Arc::new(Semaphore::new(self.limits.max_in_flight_per_origin)))
            .clone();
        let _origin_slot = origin_sem.acquire_owned().await.map_err(|_| Denial::BudgetExhausted)?;
        let _page_slot = self
            .page_slots
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| Denial::BudgetExhausted)?;

        match tokio::time::timeout(self.limits.timeout, self.run(loader, req)).await {
            Ok(r) => r,
            Err(_) => Err(Denial::Timeout),
        }
    }

    /// Fetch one module script (static import, `<script type=module src>`).
    ///
    /// Module fetches are always CORS-mode with same-origin credentials (and
    /// cookies are never sent), go through the same budgets, per-hop vet,
    /// shield and CSP (`script-src`) as every other governed request, and
    /// must answer 2xx with a JavaScript MIME type. `referrer` is the
    /// importing module's URL; it is not sent (Referer is page-scoped).
    pub async fn fetch_module(
        &self,
        loader: &ResourceLoader,
        specifier_url: &Url,
        _referrer: &Url,
    ) -> Result<ModuleSource, Denial> {
        let mut req = ScriptRequest::get(specifier_url.clone());
        req.destination = RequestDestination::Script;
        let r = self.execute(loader, req).await?;
        if !(200..300).contains(&r.status) {
            return Err(Denial::BadStatus(r.status));
        }
        let ty = header_str(&r.headers, "content-type").unwrap_or("");
        if !is_javascript_mime(ty) {
            return Err(Denial::BadMime(ty.to_string()));
        }
        // Modules are always UTF-8, whatever the charset label says.
        let text = String::from_utf8_lossy(&r.body);
        let source = text.strip_prefix('\u{feff}').unwrap_or(&text).to_string();
        Ok(ModuleSource { final_url: r.url, source })
    }

    fn validate_shape(&self, req: &ScriptRequest) -> Result<(), Denial> {
        if matches!(req.method.as_str(), "CONNECT" | "TRACE" | "TRACK") {
            return Err(Denial::BadRequest(format!("forbidden method {}", req.method)));
        }
        if req.body.is_some() && matches!(req.method, Method::GET | Method::HEAD) {
            return Err(Denial::BadRequest("GET/HEAD cannot carry a body".into()));
        }
        if let Some(b) = &req.body {
            if b.len() > self.limits.max_request_body_bytes {
                return Err(Denial::RequestTooLarge);
            }
        }
        let bytes: usize = req.headers.iter().map(|(n, v)| n.as_str().len() + v.len()).sum();
        if req.headers.len() > self.limits.max_header_count || bytes > self.limits.max_header_bytes {
            return Err(Denial::BadRequest("too many or too large request headers".into()));
        }
        if req.mode == RequestMode::NoCors && !is_safelisted_method(&req.method) {
            return Err(Denial::BadRequest("no-cors requires GET, HEAD or POST".into()));
        }
        Ok(())
    }

    fn is_cross_origin(&self, url: &Url) -> bool {
        !self.page_origin.same_origin(&Origin::from_url(url))
    }

    fn check_url(&self, url: &Url, first_hop: bool, dest: RequestDestination) -> Result<(), Denial> {
        match url.scheme() {
            "http" | "https" => {}
            "data" if first_hop => return Ok(()),
            s => return Err(Denial::Scheme(s.to_string())),
        }
        let is_script = dest == RequestDestination::Script;
        let mixed = if is_script { MixedContentType::Script } else { MixedContentType::Fetch };
        if check_mixed_content(&self.page_url, url, mixed) != MixedContentResult::Allowed {
            return Err(Denial::MixedContent);
        }
        if let Some(csp) = &self.csp {
            let directive = if is_script { CspDirective::ScriptSrc } else { CspDirective::ConnectSrc };
            if !self.csp_allows(csp, directive, url) {
                return Err(Denial::Csp);
            }
        }
        Ok(())
    }

    /// `connect-src` (fetch/XHR) or `script-src` (module loads), including the
    /// sources `security.rs` cannot evaluate without a document origin:
    /// `'self'` and `*`.
    fn csp_allows(&self, csp: &ContentSecurityPolicy, directive: CspDirective, url: &Url) -> bool {
        let Some(sources) = csp.get_sources(directive) else {
            return true;
        };
        let same = !self.is_cross_origin(url);
        sources.iter().any(|s| match s {
            CspSource::Self_ => same,
            CspSource::Host(h) if h == "*" => matches!(url.scheme(), "http" | "https"),
            _ if directive == CspDirective::ScriptSrc => csp.allows_script(Some(url), false, None, None),
            _ => csp.allows_connect(url),
        })
    }

    fn address_policy(&self, url: &Url) -> AddressPolicy {
        #[cfg(test)]
        if let Some(p) = &self.address_override {
            return p.clone();
        }
        page_address_policy(&self.page_url, url)
    }

    async fn run(&self, loader: &ResourceLoader, req: ScriptRequest) -> Result<ScriptResponse, Denial> {
        let with_credentials = req.credentials == CredentialsMode::Include;
        let mut headers = HeaderMap::new();
        for (n, v) in req.headers.iter() {
            let no_cors_blocked = req.mode == RequestMode::NoCors
                && !is_safelisted_request_header(n.as_str(), v.to_str().unwrap_or(""));
            if !is_forbidden_request_header(n.as_str()) && !no_cors_blocked {
                headers.append(n.clone(), v.clone());
            }
        }
        let mut hop = Hop {
            url: req.url.clone(),
            method: req.method.clone(),
            body: req.body.clone(),
            headers,
            tainted: false,
        };
        let mut redirects = 0usize;
        let mut redirected = false;

        loop {
            self.check_url(&hop.url, redirects == 0, req.destination)?;
            let is_data = hop.url.scheme() == "data";
            let cross = !is_data && self.is_cross_origin(&hop.url);
            if cross && req.mode == RequestMode::SameOrigin {
                return Err(Denial::SameOriginOnly);
            }
            if is_data && req.mode == RequestMode::SameOrigin {
                return Err(Denial::SameOriginOnly);
            }

            let origin_header = if hop.tainted {
                Some("null".to_string())
            } else if cross || !matches!(hop.method, Method::GET | Method::HEAD) {
                Some(self.page_origin.serialize())
            } else {
                None
            };

            if cross && req.mode == RequestMode::Cors {
                self.preflight_if_needed(loader, &hop, origin_header.as_deref().unwrap_or("null"), with_credentials)
                    .await?;
            }

            let resp = self
                .send(loader, &req, &hop, origin_header.as_deref(), cross)
                .await?;
            let status = resp.status.as_u16();

            if is_redirect_status(status) && resp.headers.contains_key("location") {
                if cross && req.mode == RequestMode::Cors {
                    self.cors_check(origin_header.as_deref().unwrap_or("null"), &resp.headers, with_credentials)?;
                }
                match req.redirect {
                    RedirectMode::Error => {
                        return Err(Denial::Redirect("redirect refused (redirect: error)".into()))
                    }
                    RedirectMode::Manual => {
                        return Ok(ScriptResponse {
                            id: req.id,
                            url: hop.url,
                            status: 0,
                            status_text: String::new(),
                            headers: HeaderMap::new(),
                            body: Bytes::new(),
                            kind: ResponseKind::OpaqueRedirect,
                            redirected,
                        })
                    }
                    RedirectMode::Follow => {}
                }
                redirects += 1;
                if redirects > self.limits.max_redirects {
                    return Err(Denial::TooManyRedirects);
                }
                let loc = header_str(&resp.headers, "location").unwrap_or_default().to_string();
                let next = hop
                    .url
                    .join(&loc)
                    .map_err(|_| Denial::Redirect(format!("unparseable Location {loc:?}")))?;
                if !matches!(next.scheme(), "http" | "https") {
                    return Err(Denial::Redirect(format!("redirect to {}: scheme", next.scheme())));
                }
                let cur_origin = Origin::from_url(&hop.url);
                let next_origin = Origin::from_url(&next);
                let leaves_origin = !cur_origin.same_origin(&next_origin);
                if leaves_origin && (!next.username().is_empty() || next.password().is_some()) {
                    return Err(Denial::Redirect("cross-origin redirect with credentials in URL".into()));
                }
                if !self.page_origin.same_origin(&next_origin) && !self.page_origin.same_origin(&cur_origin) {
                    hop.tainted = true;
                }
                hop.headers.remove("proxy-authorization");
                if leaves_origin {
                    hop.headers.remove("authorization");
                }
                let to_get = ((status == 301 || status == 302) && hop.method == Method::POST)
                    || (status == 303 && !matches!(hop.method, Method::GET | Method::HEAD));
                if to_get {
                    hop.method = Method::GET;
                    hop.body = None;
                    for h in ["content-type", "content-encoding", "content-language", "content-location"] {
                        hop.headers.remove(h);
                    }
                }
                hop.url = next;
                redirected = true;
                continue;
            }

            return self.finish(req, resp, cross, origin_header.as_deref(), with_credentials, redirected).await;
        }
    }

    fn cors_check(&self, origin: &str, headers: &HeaderMap, with_credentials: bool) -> Result<(), Denial> {
        match CorsChecker::new().check_response(
            origin,
            header_str(headers, "access-control-allow-origin"),
            header_str(headers, "access-control-allow-credentials"),
            with_credentials,
        ) {
            CorsResult::Allowed => Ok(()),
            CorsResult::Denied(why) => Err(Denial::Cors(why)),
            CorsResult::PreflightRequired => Err(Denial::Cors("preflight required".into())),
        }
    }

    async fn send(
        &self,
        loader: &ResourceLoader,
        req: &ScriptRequest,
        hop: &Hop,
        origin: Option<&str>,
        cross: bool,
    ) -> Result<crate::Response, Denial> {
        let mut headers = hop.headers.clone();
        if let Some(o) = origin {
            if let Ok(v) = HeaderValue::from_str(o) {
                headers.insert("origin", v);
            }
        }
        let mut request = Request::get(hop.url.clone());
        request.method = hop.method.clone();
        request.headers = headers;
        request.body = hop.body.clone();
        request.destination = match req.destination {
            RequestDestination::Xhr => RequestDestination::Xhr,
            RequestDestination::Script => RequestDestination::Script,
            _ => RequestDestination::Fetch,
        };
        request.credentials = CredentialsMode::Omit;
        request.timeout = Some(self.limits.timeout);
        request.referrer = Some(self.page_url.clone());
        // The shared cache is keyed by URL alone, so only a same-origin GET
        // may be served from it: a CORS response varies by `Origin`, and a
        // cross-origin hit would skip the address vet.
        let use_cache = !cross && !hop.tainted && hop.method == Method::GET;
        let resp = loader
            .fetch_governed_hop(
                request,
                self.address_policy(&hop.url),
                self.limits.max_response_bytes,
                use_cache,
            )
            .await
            .map_err(map_net_error)?;
        Ok(resp)
    }

    async fn preflight_if_needed(
        &self,
        loader: &ResourceLoader,
        hop: &Hop,
        origin: &str,
        with_credentials: bool,
    ) -> Result<(), Denial> {
        let extra = non_safelisted_headers(&hop.headers);
        if is_safelisted_method(&hop.method) && extra.is_empty() {
            return Ok(());
        }
        let key = format!("{origin}|{with_credentials}|{}", {
            let mut u = hop.url.clone();
            u.set_fragment(None);
            u
        });
        if let Some(e) = self.preflight.lock().unwrap().get(&key) {
            let method_ok = is_safelisted_method(&hop.method)
                || e.any_method
                || e.methods.contains(hop.method.as_str());
            let headers_ok = extra
                .iter()
                .all(|h| e.headers.contains(h) || (e.any_header && h != "authorization"));
            if e.expires > Instant::now() && method_ok && headers_ok {
                return Ok(());
            }
        }

        let mut h = HeaderMap::new();
        h.insert("access-control-request-method", HeaderValue::from_str(hop.method.as_str()).map_err(|_| Denial::Preflight("bad method".into()))?);
        if !extra.is_empty() {
            h.insert(
                "access-control-request-headers",
                HeaderValue::from_str(&extra.join(",")).map_err(|_| Denial::Preflight("bad header name".into()))?,
            );
        }
        if let Ok(v) = HeaderValue::from_str(origin) {
            h.insert("origin", v);
        }
        let mut request = Request::get(hop.url.clone());
        request.method = Method::OPTIONS;
        request.headers = h;
        request.destination = RequestDestination::Fetch;
        request.credentials = CredentialsMode::Omit;
        request.timeout = Some(self.limits.timeout);
        request.referrer = Some(self.page_url.clone());
        let resp = loader
            .fetch_governed_hop(
                request,
                self.address_policy(&hop.url),
                self.limits.max_response_bytes,
                false,
            )
            .await
            .map_err(|e| match map_net_error(e) {
                d @ (Denial::PrivateNetwork(_) | Denial::Shield | Denial::Timeout) => d,
                other => Denial::Preflight(other.to_string()),
            })?;
        if !resp.status.is_success() {
            return Err(Denial::Preflight(format!("preflight status {}", resp.status.as_u16())));
        }
        self.cors_check(origin, &resp.headers, with_credentials)
            .map_err(|d| Denial::Preflight(d.to_string()))?;

        let methods = split_list(header_str(&resp.headers, "access-control-allow-methods"));
        let allowed_headers = split_list(header_str(&resp.headers, "access-control-allow-headers"));
        let any_method = !with_credentials && methods.iter().any(|m| m == "*");
        let any_header = !with_credentials && allowed_headers.iter().any(|m| m == "*");
        let methods: HashSet<String> = methods.into_iter().map(|m| m.to_ascii_uppercase()).collect();
        let allowed_headers: HashSet<String> =
            allowed_headers.into_iter().map(|m| m.to_ascii_lowercase()).collect();

        if !is_safelisted_method(&hop.method) && !any_method && !methods.contains(hop.method.as_str()) {
            return Err(Denial::Preflight(format!("method {} not allowed", hop.method)));
        }
        if let Some(bad) = extra
            .iter()
            .find(|h| !allowed_headers.contains(*h) && !(any_header && *h != "authorization"))
        {
            return Err(Denial::Preflight(format!("header {bad} not allowed")));
        }

        let max_age = header_str(&resp.headers, "access-control-max-age")
            .and_then(|v| v.trim().parse::<u64>().ok())
            .unwrap_or(5)
            .min(600);
        self.preflight.lock().unwrap().insert(
            key,
            PreflightEntry {
                methods,
                headers: allowed_headers,
                any_method,
                any_header,
                expires: Instant::now() + Duration::from_secs(max_age),
            },
        );
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn finish(
        &self,
        req: ScriptRequest,
        resp: crate::Response,
        cross: bool,
        origin: Option<&str>,
        with_credentials: bool,
        redirected: bool,
    ) -> Result<ScriptResponse, Denial> {
        let status = resp.status.as_u16();
        let status_text = resp.status.canonical_reason().unwrap_or("").to_string();
        let url = resp.url.clone();
        let mut headers = resp.headers.clone();

        let kind = if !cross {
            headers.remove("set-cookie");
            headers.remove("set-cookie2");
            ResponseKind::Basic
        } else if req.mode == RequestMode::NoCors {
            return Ok(ScriptResponse {
                id: req.id,
                url,
                status: 0,
                status_text: String::new(),
                headers: HeaderMap::new(),
                body: Bytes::new(),
                kind: ResponseKind::Opaque,
                redirected,
            });
        } else {
            let origin = origin.unwrap_or("null");
            self.cors_check(origin, &headers, with_credentials)?;
            let mut keep: HashSet<String> =
                SAFELISTED_RESPONSE_HEADERS.iter().map(|s| s.to_string()).collect();
            let expose = split_list(header_str(&headers, "access-control-expose-headers"));
            let star = !with_credentials && expose.iter().any(|e| e == "*");
            keep.extend(expose.into_iter().map(|e| e.to_ascii_lowercase()));
            let mut filtered = HeaderMap::new();
            for (n, v) in headers.iter() {
                let name = n.as_str();
                if keep.contains(name) || (star && !matches!(name, "set-cookie" | "set-cookie2")) {
                    filtered.append(n.clone(), v.clone());
                }
            }
            headers = filtered;
            ResponseKind::Cors
        };

        let body = resp.bytes().await.map_err(map_net_error)?;
        if body.len() > self.limits.max_response_bytes {
            return Err(Denial::ResponseTooLarge);
        }
        Ok(ScriptResponse { id: req.id, url, status, status_text, headers, body, kind, redirected })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{InterceptAction, InterceptHandler, LoaderConfig, RequestInterceptor};
    use std::sync::atomic::AtomicUsize;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    // ---- a small scriptable HTTP/1.1 server ---------------------------------

    #[derive(Clone, Debug)]
    struct Seen {
        method: String,
        path: String,
        headers: HashMap<String, String>,
        body: Vec<u8>,
    }

    struct Server {
        port: u16,
        hits: Arc<AtomicUsize>,
        seen: Arc<Mutex<Vec<Seen>>>,
        peak: Arc<AtomicUsize>,
        closed: Arc<AtomicUsize>,
    }

    impl Server {
        fn hits(&self) -> usize {
            self.hits.load(Ordering::SeqCst)
        }
        /// Connections the client hung up on while the server was still
        /// holding its response.
        fn closed(&self) -> usize {
            self.closed.load(Ordering::SeqCst)
        }
        fn seen(&self) -> Vec<Seen> {
            self.seen.lock().unwrap().clone()
        }
        fn url(&self, path: &str) -> Url {
            Url::parse(&format!("http://127.0.0.1:{}{}", self.port, path)).unwrap()
        }
    }

    fn resp(status: u16, headers: &[(&str, &str)], body: &[u8]) -> Vec<u8> {
        let mut out = format!("HTTP/1.1 {status} X\r\nContent-Length: {}\r\n", body.len());
        for (k, v) in headers {
            out.push_str(&format!("{k}: {v}\r\n"));
        }
        out.push_str("Connection: close\r\n\r\n");
        let mut v = out.into_bytes();
        v.extend_from_slice(body);
        v
    }

    /// `handler` maps a request to a raw response; `delay` holds each response.
    async fn serve_with(
        delay: Duration,
        handler: impl Fn(&Seen) -> Vec<u8> + Send + Sync + 'static,
    ) -> Server {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        let hits = Arc::new(AtomicUsize::new(0));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let peak = Arc::new(AtomicUsize::new(0));
        let live = Arc::new(AtomicUsize::new(0));
        let closed = Arc::new(AtomicUsize::new(0));
        let handler = Arc::new(handler);
        let (h, sn, pk, cl) = (hits.clone(), seen.clone(), peak.clone(), closed.clone());
        tokio::spawn(async move {
            while let Ok((mut s, _)) = l.accept().await {
                h.fetch_add(1, Ordering::SeqCst);
                let (sn, pk, live, handler, cl) = (sn.clone(), pk.clone(), live.clone(), handler.clone(), cl.clone());
                tokio::spawn(async move {
                    let mut buf = Vec::new();
                    let mut chunk = [0u8; 4096];
                    let head_end = loop {
                        let n = s.read(&mut chunk).await.unwrap_or(0);
                        if n == 0 {
                            return;
                        }
                        buf.extend_from_slice(&chunk[..n]);
                        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                            break i + 4;
                        }
                    };
                    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
                    let mut lines = head.lines();
                    let first = lines.next().unwrap_or("").to_string();
                    let mut parts = first.split_whitespace();
                    let method = parts.next().unwrap_or("").to_string();
                    let path = parts.next().unwrap_or("").to_string();
                    let mut headers = HashMap::new();
                    for l in lines {
                        if let Some((k, v)) = l.split_once(':') {
                            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
                        }
                    }
                    let want: usize = headers.get("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
                    while buf.len() < head_end + want {
                        let n = s.read(&mut chunk).await.unwrap_or(0);
                        if n == 0 {
                            break;
                        }
                        buf.extend_from_slice(&chunk[..n]);
                    }
                    let seen_req = Seen { method, path, headers, body: buf[head_end..].to_vec() };
                    sn.lock().unwrap().push(seen_req.clone());
                    let now = live.fetch_add(1, Ordering::SeqCst) + 1;
                    pk.fetch_max(now, Ordering::SeqCst);
                    if !delay.is_zero() {
                        tokio::select! {
                            _ = tokio::time::sleep(delay) => {}
                            n = s.read(&mut chunk) => {
                                if matches!(n, Ok(0) | Err(_)) {
                                    live.fetch_sub(1, Ordering::SeqCst);
                                    cl.fetch_add(1, Ordering::SeqCst);
                                    return;
                                }
                            }
                        }
                    }
                    let reply = handler(&seen_req);
                    live.fetch_sub(1, Ordering::SeqCst);
                    let _ = s.write_all(&reply).await;
                    let _ = s.shutdown().await;
                });
            }
        });
        Server { port, hits, seen, peak, closed }
    }

    async fn serve(handler: impl Fn(&Seen) -> Vec<u8> + Send + Sync + 'static) -> Server {
        serve_with(Duration::ZERO, handler).await
    }

    fn loader() -> ResourceLoader {
        ResourceLoader::new(LoaderConfig::default()).unwrap()
    }

    fn page(url: &str) -> FetchPolicy {
        FetchPolicy::for_page(Url::parse(url).unwrap(), None)
    }

    /// Treat the given loopback ports as if they were public servers. Every
    /// other loopback port stays private under the real rule.
    fn allow_ports(mut p: FetchPolicy, ports: &[u16]) -> FetchPolicy {
        let ports: Vec<u16> = ports.to_vec();
        p.address_override = Some(AddressPolicy::Custom(Arc::new(move |a| {
            a.ip().is_loopback() && ports.contains(&a.port())
        })));
        p
    }

    fn get(url: &str) -> ScriptRequest {
        ScriptRequest::get(Url::parse(url).unwrap())
    }

    // ---- §7.1 / 7.2: private network ---------------------------------------

    #[tokio::test]
    async fn a_public_page_cannot_reach_private_addresses() {
        let private = serve(|_| resp(200, &[("Access-Control-Allow-Origin", "*")], b"secret")).await;
        let p = page("http://example.com/");
        let l = loader();
        let port = private.port;
        for target in [
            format!("http://127.0.0.1:{port}/"),
            format!("http://localhost:{port}/"),
            format!("http://[::1]:{port}/"),
            format!("http://0.0.0.0:{port}/"),
            "http://169.254.169.254/latest/meta-data/".to_string(),
            "http://10.0.0.1/".to_string(),
            "http://192.168.1.1/".to_string(),
            "http://172.16.0.1/".to_string(),
            "http://100.64.0.1/".to_string(),
            format!("https://127.0.0.1:{port}/"),
        ] {
            let r = p.execute(&l, get(&target)).await;
            assert!(matches!(r, Err(Denial::PrivateNetwork(_))), "{target}: {r:?}");
        }
        assert_eq!(private.hits(), 0, "the private server saw a connection");
    }

    struct Rebind;
    impl rustkit_http::Resolve for Rebind {
        fn resolve<'a>(&'a self, host: &'a str, port: u16) -> rustkit_http::ResolveFuture<'a> {
            let ip: std::net::IpAddr = match host {
                "rebind.test" => "127.0.0.1".parse().unwrap(),
                "mixed.test" => "127.0.0.1".parse().unwrap(),
                _ => "8.8.8.8".parse().unwrap(),
            };
            let mut out = vec![std::net::SocketAddr::new(ip, port)];
            if host == "mixed.test" {
                out.insert(0, std::net::SocketAddr::new("93.184.216.34".parse().unwrap(), port));
            }
            Box::pin(async move { Ok(out) })
        }
    }

    #[tokio::test]
    async fn a_name_that_resolves_to_loopback_is_denied_before_any_socket_opens() {
        let private = serve(|_| resp(200, &[("Access-Control-Allow-Origin", "*")], b"secret")).await;
        let l = loader().with_resolver(Arc::new(Rebind));
        let p = page("http://example.com/");
        for host in ["rebind.test", "mixed.test"] {
            let r = p.execute(&l, get(&format!("http://{host}:{}/", private.port))).await;
            assert!(matches!(r, Err(Denial::PrivateNetwork(_))), "{host}: {r:?}");
        }
        assert_eq!(private.hits(), 0, "the loopback server saw a connection");
    }

    #[tokio::test]
    async fn a_private_page_reaches_only_its_own_origin() {
        let page_server = serve(|r| resp(200, &[], format!("own {}", r.path).as_bytes())).await;
        let other = serve(|_| resp(200, &[("Access-Control-Allow-Origin", "*")], b"other")).await;
        let p = FetchPolicy::for_page(page_server.url("/index.html"), None);
        let l = loader();
        let own = p.execute(&l, get(page_server.url("/data").as_str())).await.expect("same-origin");
        assert_eq!(own.kind, ResponseKind::Basic);
        assert_eq!(&own.body[..], b"own /data");
        let r = p.execute(&l, get(other.url("/").as_str())).await;
        assert!(matches!(r, Err(Denial::PrivateNetwork(_))), "{r:?}");
        assert_eq!(other.hits(), 0);
    }

    // ---- §7.3: redirects ----------------------------------------------------

    #[tokio::test]
    async fn a_redirect_to_a_private_address_is_denied_at_the_hop() {
        let private = serve(|_| resp(200, &[("Access-Control-Allow-Origin", "*")], b"secret")).await;
        let target = format!("http://127.0.0.1:{}/", private.port);
        let public = serve(move |_| resp(302, &[("Location", &target), ("Access-Control-Allow-Origin", "*")], b"")).await;
        let p = allow_ports(page("http://page.test/"), &[public.port]);
        let r = p.execute(&loader(), ScriptRequest::get(public.url("/"))).await;
        assert!(matches!(r, Err(Denial::PrivateNetwork(_))), "{r:?}");
        assert_eq!(public.hits(), 1);
        assert_eq!(private.hits(), 0, "the private server saw a connection");
    }

    #[tokio::test]
    async fn a_cross_origin_redirect_drops_credentials_and_taints_the_origin() {
        let end = serve(|_| resp(200, &[("Access-Control-Allow-Origin", "*")], b"end")).await;
        let to = format!("http://127.0.0.1:{}/end", end.port);
        let start = serve(move |r| {
            if r.method == "OPTIONS" {
                return resp(204, &[("Access-Control-Allow-Origin", "*"), ("Access-Control-Allow-Headers", "authorization")], b"");
            }
            resp(302, &[("Location", &to), ("Access-Control-Allow-Origin", "*")], b"")
        })
        .await;
        let p = allow_ports(page("http://page.test/"), &[start.port, end.port]);
        let mut req = ScriptRequest::get(start.url("/"));
        req.headers.insert("authorization", HeaderValue::from_static("Bearer s3cret"));
        req.headers.insert("cookie", HeaderValue::from_static("sid=1"));
        let r = p.execute(&loader(), req).await.expect("followed");
        assert!(r.redirected);
        assert_eq!(&r.body[..], b"end");
        let first = start.seen().into_iter().find(|r| r.method == "GET").expect("the GET reached start");
        assert!(!first.headers.contains_key("cookie"), "script-set Cookie must never be sent");
        let last = &end.seen()[0];
        assert!(!last.headers.contains_key("authorization"), "credentials survived a cross-origin redirect");
        assert!(!last.headers.contains_key("cookie"));
        assert_eq!(last.headers.get("origin").map(String::as_str), Some("null"), "origin is tainted after a cross-origin redirect");
    }

    #[tokio::test]
    async fn the_hop_cap_trips() {
        let me = Arc::new(Mutex::new(0u16));
        let m = me.clone();
        let s = serve(move |_| {
            let port = *m.lock().unwrap();
            resp(302, &[("Location", &format!("http://127.0.0.1:{port}/again")), ("Access-Control-Allow-Origin", "*")], b"")
        })
        .await;
        *me.lock().unwrap() = s.port;
        let limits = FetchLimits { max_redirects: 3, ..FetchLimits::default() };
        let p = allow_ports(
            FetchPolicy::with_limits(Url::parse("http://page.test/").unwrap(), None, limits),
            &[s.port],
        );
        let r = p.execute(&loader(), ScriptRequest::get(s.url("/"))).await;
        assert_eq!(r.unwrap_err(), Denial::TooManyRedirects);
        assert_eq!(s.hits(), 4, "the original request plus three followed hops");
    }

    #[tokio::test]
    async fn redirect_status_decides_method_and_body_and_modes_decide_following() {
        let sink = serve(|_| resp(200, &[("Access-Control-Allow-Origin", "*")], b"ok")).await;
        let to = format!("http://127.0.0.1:{}/sink", sink.port);
        let (to2, to3) = (to.clone(), to.clone());
        let hop = serve(move |r| match r.path.as_str() {
            "/302" => resp(302, &[("Location", &to2), ("Access-Control-Allow-Origin", "*")], b""),
            _ => resp(307, &[("Location", &to3), ("Access-Control-Allow-Origin", "*")], b""),
        })
        .await;
        let p = allow_ports(page("http://page.test/"), &[hop.port, sink.port]);
        let l = loader();
        let mut post = ScriptRequest::get(hop.url("/302"));
        post.method = Method::POST;
        post.body = Some(Bytes::from_static(b"payload"));
        post.headers.insert("content-type", HeaderValue::from_static("text/plain"));
        p.execute(&l, post.clone()).await.expect("302");
        let s1 = sink.seen().pop().unwrap();
        assert_eq!(s1.method, "GET", "302 turns POST into GET");
        assert!(s1.body.is_empty());

        post.url = hop.url("/307");
        p.execute(&l, post.clone()).await.expect("307");
        let s2 = sink.seen().pop().unwrap();
        assert_eq!((s2.method.as_str(), &s2.body[..]), ("POST", &b"payload"[..]), "307 preserves method and body");

        let mut manual = ScriptRequest::get(hop.url("/302"));
        manual.redirect = RedirectMode::Manual;
        let r = p.execute(&l, manual).await.expect("manual");
        assert_eq!((r.kind, r.status), (ResponseKind::OpaqueRedirect, 0));
        let mut err = ScriptRequest::get(hop.url("/302"));
        err.redirect = RedirectMode::Error;
        assert!(matches!(p.execute(&l, err).await, Err(Denial::Redirect(_))));
    }

    // ---- §7.4: mixed content, schemes, CSP -----------------------------------

    #[tokio::test]
    async fn an_https_page_cannot_fetch_http() {
        let s = serve(|_| resp(200, &[("Access-Control-Allow-Origin", "*")], b"x")).await;
        let p = allow_ports(page("https://example.com/"), &[s.port]);
        let r = p.execute(&loader(), ScriptRequest::get(s.url("/"))).await;
        assert_eq!(r.unwrap_err(), Denial::MixedContent);
        assert_eq!(s.hits(), 0);
    }

    #[tokio::test]
    async fn only_http_https_and_data_schemes_are_requestable() {
        let p = page("http://example.com/");
        let l = loader();
        for u in ["file:///etc/passwd", "ftp://example.com/x", "javascript:alert(1)", "ws://example.com/", "blob:http://example.com/x"] {
            let r = p.execute(&l, get(u)).await;
            assert!(matches!(r, Err(Denial::Scheme(_))), "{u}: {r:?}");
        }
        let d = p.execute(&l, get("data:text/plain,hello")).await.expect("data:");
        assert_eq!(&d.body[..], b"hello");
        assert_eq!(d.kind, ResponseKind::Basic);
    }

    #[tokio::test]
    async fn connect_src_self_means_the_page_origin() {
        let s = serve(|_| resp(200, &[], b"mine")).await;
        let csp = ContentSecurityPolicy::parse("connect-src 'self'").unwrap();
        let p = FetchPolicy::for_page(s.url("/page"), Some(csp));
        let r = p.execute(&loader(), ScriptRequest::get(s.url("/x"))).await.expect("'self' allows the page origin");
        assert_eq!(&r.body[..], b"mine");
    }

    #[tokio::test]
    async fn connect_src_gates_the_request() {
        let s = serve(|_| resp(200, &[("Access-Control-Allow-Origin", "*")], b"x")).await;
        let csp = ContentSecurityPolicy::parse("connect-src 'self'").unwrap();
        let p = allow_ports(
            FetchPolicy::for_page(Url::parse("http://page.test/").unwrap(), Some(csp)),
            &[s.port],
        );
        let r = p.execute(&loader(), ScriptRequest::get(s.url("/"))).await;
        assert_eq!(r.unwrap_err(), Denial::Csp);
        assert_eq!(s.hits(), 0);
    }

    // ---- §7.5 / 7.6: CORS ---------------------------------------------------

    #[tokio::test]
    async fn cross_origin_needs_the_right_allow_origin() {
        let s = serve(|r| match r.path.as_str() {
            "/none" => resp(200, &[], b"hidden"),
            "/ok" => resp(200, &[("Access-Control-Allow-Origin", "http://page.test"), ("Content-Type", "text/plain"), ("X-Secret", "1")], b"shown"),
            "/wrong" => resp(200, &[("Access-Control-Allow-Origin", "http://evil.test")], b"hidden"),
            "/star" => resp(200, &[("Access-Control-Allow-Origin", "*")], b"star"),
            "/expose" => resp(200, &[("Access-Control-Allow-Origin", "*"), ("X-Secret", "1"), ("Access-Control-Expose-Headers", "x-secret")], b"e"),
            _ => resp(404, &[], b""),
        })
        .await;
        let p = allow_ports(page("http://page.test/"), &[s.port]);
        let l = loader();
        for path in ["/none", "/wrong"] {
            let r = p.execute(&l, ScriptRequest::get(s.url(path))).await;
            assert!(matches!(r, Err(Denial::Cors(_))), "{path}: {r:?}");
        }
        let ok = p.execute(&l, ScriptRequest::get(s.url("/ok"))).await.expect("allowed");
        assert_eq!((ok.kind, &ok.body[..]), (ResponseKind::Cors, &b"shown"[..]));
        assert!(ok.headers.contains_key("content-type"));
        assert!(!ok.headers.contains_key("x-secret"), "non-safelisted header leaked to script");
        let ex = p.execute(&l, ScriptRequest::get(s.url("/expose"))).await.expect("expose");
        assert!(ex.headers.contains_key("x-secret"));
        // `*` is fine without credentials and refused with them.
        p.execute(&l, ScriptRequest::get(s.url("/star"))).await.expect("star");
        let mut with_creds = ScriptRequest::get(s.url("/star"));
        with_creds.credentials = CredentialsMode::Include;
        let r = p.execute(&l, with_creds).await;
        assert!(matches!(r, Err(Denial::Cors(_))), "{r:?}");
        let sent = s.seen();
        assert!(sent.iter().all(|r| r.headers.get("origin").map(String::as_str) == Some("http://page.test")));
        assert!(sent.iter().all(|r| !r.headers.contains_key("cookie")));
    }

    #[tokio::test]
    async fn a_non_simple_request_preflights_and_is_denied_without_permission() {
        let s = serve(|r| {
            if r.method == "OPTIONS" {
                return match r.path.as_str() {
                    "/no-method" => resp(204, &[("Access-Control-Allow-Origin", "http://page.test"), ("Access-Control-Allow-Headers", "x-token")], b""),
                    "/no-header" => resp(204, &[("Access-Control-Allow-Origin", "http://page.test"), ("Access-Control-Allow-Methods", "PUT")], b""),
                    "/fail" => resp(403, &[("Access-Control-Allow-Origin", "http://page.test"), ("Access-Control-Allow-Methods", "PUT"), ("Access-Control-Allow-Headers", "x-token")], b""),
                    _ => resp(204, &[("Access-Control-Allow-Origin", "http://page.test"), ("Access-Control-Allow-Methods", "PUT"), ("Access-Control-Allow-Headers", "x-token"), ("Access-Control-Max-Age", "60")], b""),
                };
            }
            resp(200, &[("Access-Control-Allow-Origin", "http://page.test")], b"done")
        })
        .await;
        let p = allow_ports(page("http://page.test/"), &[s.port]);
        let l = loader();
        let put = |path: &str| {
            let mut r = ScriptRequest::get(s.url(path));
            r.method = Method::PUT;
            r.body = Some(Bytes::from_static(b"x"));
            r.headers.insert("x-token", HeaderValue::from_static("1"));
            r
        };
        for path in ["/no-method", "/no-header", "/fail"] {
            let r = p.execute(&l, put(path)).await;
            assert!(matches!(r, Err(Denial::Preflight(_))), "{path}: {r:?}");
        }
        assert!(s.seen().iter().all(|r| r.method == "OPTIONS"), "the real request went out after a failed preflight");
        let ok = p.execute(&l, put("/yes")).await.expect("preflight permits");
        assert_eq!(&ok.body[..], b"done");
        let log: Vec<String> = s.seen().iter().rev().take(2).map(|r| r.method.clone()).collect();
        assert_eq!(log, ["PUT", "OPTIONS"], "preflight first, then the request");
        let pre = s.seen().into_iter().rev().nth(1).unwrap();
        assert_eq!(pre.headers.get("access-control-request-method").map(String::as_str), Some("PUT"));
        assert_eq!(pre.headers.get("access-control-request-headers").map(String::as_str), Some("x-token"));
        // Cached for its max-age: a second PUT does not preflight again.
        let before = s.hits();
        p.execute(&l, put("/yes")).await.expect("cached preflight");
        assert_eq!(s.hits(), before + 1, "preflight was not cached");
    }

    #[tokio::test]
    async fn no_cors_cross_origin_is_opaque() {
        let s = serve(|_| resp(200, &[("Content-Type", "text/plain")], b"unreadable")).await;
        let p = allow_ports(page("http://page.test/"), &[s.port]);
        let mut req = ScriptRequest::get(s.url("/"));
        req.mode = RequestMode::NoCors;
        let r = p.execute(&loader(), req).await.expect("opaque");
        assert_eq!((r.kind, r.status), (ResponseKind::Opaque, 0));
        assert!(r.body.is_empty() && r.headers.is_empty());
        let mut put = ScriptRequest::get(s.url("/"));
        put.mode = RequestMode::NoCors;
        put.method = Method::PUT;
        assert!(p.execute(&loader(), put).await.is_err(), "no-cors needs a simple method");
        let mut same = ScriptRequest::get(s.url("/"));
        same.mode = RequestMode::SameOrigin;
        assert_eq!(p.execute(&loader(), same).await.unwrap_err(), Denial::SameOriginOnly);
    }

    #[tokio::test]
    async fn a_no_cors_request_never_carries_a_non_safelisted_header() {
        // The Request guard in script is the first line; this is the boundary
        // that must hold even if script lets one through (no preflight runs
        // for no-cors, so nothing else would stop Authorization at the wire).
        let s = serve(|_| resp(200, &[("Content-Type", "text/plain")], b"ok")).await;
        let p = allow_ports(page("http://page.test/"), &[s.port]);
        let l = loader();
        let mk = |ct: &str| {
            let mut req = ScriptRequest::get(s.url("/"));
            req.mode = RequestMode::NoCors;
            req.method = Method::POST;
            req.body = Some(Bytes::from_static(b"a=1"));
            for (k, v) in [
                ("authorization", "Bearer s3cret"),
                ("x-custom", "1"),
                ("range", "bytes=0-9"),
                ("accept", "text/html"),
                ("accept-language", "en"),
                ("content-type", ct),
            ] {
                req.headers.insert(k, HeaderValue::from_str(v).unwrap());
            }
            req
        };
        p.execute(&l, mk("application/json")).await.expect("opaque");
        p.execute(&l, mk("text/plain;charset=UTF-8")).await.expect("opaque");
        let seen = s.seen();
        assert_eq!(seen.len(), 2);
        for r in &seen {
            for h in ["authorization", "x-custom", "range"] {
                assert!(!r.headers.contains_key(h), "{h} reached the wire on a no-cors request");
            }
            assert_eq!(r.headers.get("accept").map(String::as_str), Some("text/html"));
        }
        assert!(
            !seen[0].headers.contains_key("content-type"),
            "a non-safelisted Content-Type reached the wire on a no-cors request"
        );
        assert_eq!(
            seen[1].headers.get("content-type").map(String::as_str),
            Some("text/plain;charset=UTF-8")
        );
    }

    #[tokio::test]
    async fn script_never_sees_set_cookie() {
        let s = serve(|_| resp(200, &[("Set-Cookie", "a=b"), ("Content-Type", "text/plain")], b"hi")).await;
        let p = FetchPolicy::for_page(s.url("/page"), None);
        let r = p.execute(&loader(), ScriptRequest::get(s.url("/x"))).await.unwrap();
        assert!(!r.headers.contains_key("set-cookie"));
        assert!(r.headers.contains_key("content-type"));
    }

    // ---- §7.7: caps ---------------------------------------------------------

    #[tokio::test]
    async fn caps_abort_oversize_responses_and_requests_and_budget_and_time() {
        let big = serve(|_| resp(200, &[("Access-Control-Allow-Origin", "*")], &[b'x'; 500])).await;
        let slow = serve_with(Duration::from_secs(3), |_| resp(200, &[("Access-Control-Allow-Origin", "*")], b"late")).await;
        let limits = FetchLimits {
            max_response_bytes: 100,
            max_request_body_bytes: 10,
            timeout: Duration::from_millis(300),
            max_total_requests: 4,
            ..FetchLimits::default()
        };
        let p = allow_ports(
            FetchPolicy::with_limits(Url::parse("http://page.test/").unwrap(), None, limits),
            &[big.port, slow.port],
        );
        let l = loader();
        assert_eq!(p.execute(&l, ScriptRequest::get(big.url("/"))).await.unwrap_err(), Denial::ResponseTooLarge);
        let mut post = ScriptRequest::get(big.url("/"));
        post.method = Method::POST;
        post.body = Some(Bytes::from(vec![b'y'; 11]));
        let hits = big.hits();
        assert_eq!(p.execute(&l, post).await.unwrap_err(), Denial::RequestTooLarge);
        assert_eq!(big.hits(), hits, "an oversize request body reached the network");
        assert_eq!(p.execute(&l, ScriptRequest::get(slow.url("/"))).await.unwrap_err(), Denial::Timeout);
        // That was request 3 of 4; one more fits, then the budget is spent.
        let _ = p.execute(&l, ScriptRequest::get(big.url("/"))).await;
        assert_eq!(p.execute(&l, ScriptRequest::get(big.url("/"))).await.unwrap_err(), Denial::BudgetExhausted);
    }

    #[tokio::test]
    async fn too_many_in_flight_requests_queue() {
        let s = serve_with(Duration::from_millis(250), |_| resp(200, &[("Access-Control-Allow-Origin", "*")], b"ok")).await;
        let limits = FetchLimits { max_in_flight_per_origin: 2, ..FetchLimits::default() };
        let p = Arc::new(allow_ports(
            FetchPolicy::with_limits(Url::parse("http://page.test/").unwrap(), None, limits),
            &[s.port],
        ));
        let l = Arc::new(loader());
        let mut tasks = Vec::new();
        for i in 0..5 {
            let (p, l, url) = (p.clone(), l.clone(), s.url(&format!("/{i}")));
            tasks.push(tokio::spawn(async move { p.execute(&l, ScriptRequest::get(url)).await }));
        }
        for t in tasks {
            t.await.unwrap().expect("queued request completes");
        }
        assert_eq!(s.hits(), 5);
        assert!(s.peak.load(Ordering::SeqCst) <= 2, "more than 2 in flight to one origin: {}", s.peak.load(Ordering::SeqCst));
    }

    // ---- cancellation (Z2-C5): navigating away must close the sockets ---------

    async fn wait_until(mut f: impl FnMut() -> bool) {
        for _ in 0..300 {
            if f() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    #[tokio::test]
    async fn cancel_ends_an_in_flight_request_and_closes_its_socket() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<FetchPolicy>();
        let s = serve_with(Duration::from_secs(30), |_| resp(200, &[("Access-Control-Allow-Origin", "*")], b"late")).await;
        let p = Arc::new(allow_ports(page("http://page.test/"), &[s.port]));
        let l = Arc::new(loader());
        let task = {
            let (p, l, url) = (p.clone(), l.clone(), s.url("/slow"));
            tokio::spawn(async move { p.execute(&l, ScriptRequest::get(url)).await })
        };
        wait_until(|| s.seen().len() == 1).await;
        assert_eq!(s.seen().len(), 1, "the request never reached the server");
        let started = Instant::now();
        let canceller = p.clone();
        std::thread::spawn(move || canceller.cancel()).join().unwrap();
        assert_eq!(task.await.unwrap().unwrap_err(), Denial::Cancelled);
        assert!(started.elapsed() < Duration::from_secs(2), "cancel did not return promptly");
        wait_until(|| s.closed() == 1).await;
        assert_eq!(s.closed(), 1, "the connection stayed open after cancel");
        assert!(p.is_cancelled());
    }

    #[tokio::test]
    async fn cancel_also_ends_requests_queued_behind_the_in_flight_cap() {
        let s = serve_with(Duration::from_secs(30), |_| resp(200, &[("Access-Control-Allow-Origin", "*")], b"late")).await;
        let limits = FetchLimits { max_in_flight_per_origin: 1, ..FetchLimits::default() };
        let p = Arc::new(allow_ports(
            FetchPolicy::with_limits(Url::parse("http://page.test/").unwrap(), None, limits),
            &[s.port],
        ));
        let l = Arc::new(loader());
        let mut tasks = Vec::new();
        for i in 0..3 {
            let (p, l, url) = (p.clone(), l.clone(), s.url(&format!("/{i}")));
            tasks.push(tokio::spawn(async move { p.execute(&l, ScriptRequest::get(url)).await }));
        }
        wait_until(|| s.seen().len() == 1).await;
        p.cancel();
        for t in tasks {
            assert_eq!(t.await.unwrap().unwrap_err(), Denial::Cancelled);
        }
        wait_until(|| s.closed() == 1).await;
        assert_eq!(s.hits(), 1, "a queued request connected after cancel");
        assert_eq!(s.closed(), 1);
    }

    #[tokio::test]
    async fn after_cancel_every_call_is_refused_at_once_without_spending_the_budget() {
        let s = serve(|_| resp(200, &[("Access-Control-Allow-Origin", "*")], b"ok")).await;
        let limits = FetchLimits { max_total_requests: 2, ..FetchLimits::default() };
        let p = allow_ports(
            FetchPolicy::with_limits(Url::parse("http://page.test/").unwrap(), None, limits),
            &[s.port],
        );
        let l = loader();
        p.execute(&l, ScriptRequest::get(s.url("/a"))).await.expect("before cancel");
        p.cancel();
        p.cancel();
        for i in 0..5 {
            let r = p.execute(&l, ScriptRequest::get(s.url(&format!("/{i}")))).await;
            assert_eq!(r.unwrap_err(), Denial::Cancelled, "call {i}");
        }
        let m = p.fetch_module(&l, &s.url("/m.js"), &s.url("/")).await;
        assert_eq!(m.unwrap_err(), Denial::Cancelled);
        assert_eq!(s.hits(), 1, "a request connected after cancel");
    }

    #[tokio::test]
    async fn cancel_ends_an_in_flight_module_fetch() {
        let s = serve_with(Duration::from_secs(30), |_| resp(200, &[("Access-Control-Allow-Origin", "*"), ("Content-Type", "text/javascript")], b"export {}")).await;
        let p = Arc::new(allow_ports(page("http://page.test/"), &[s.port]));
        let l = Arc::new(loader());
        let task = {
            let (p, l, url) = (p.clone(), l.clone(), s.url("/m.js"));
            tokio::spawn(async move { p.fetch_module(&l, &url, &url).await })
        };
        wait_until(|| s.seen().len() == 1).await;
        p.cancel();
        assert_eq!(task.await.unwrap().unwrap_err(), Denial::Cancelled);
        wait_until(|| s.closed() == 1).await;
        assert_eq!(s.closed(), 1);
    }

    #[tokio::test]
    async fn aborting_one_request_closes_only_its_socket_and_frees_its_slot() {
        // Request 0 is slow and holds the only origin slot; request 1 queues
        // behind it. Aborting 0 must close 0's socket and let 1 through.
        let s = serve_with(Duration::from_millis(400), |r| {
            resp(200, &[("Access-Control-Allow-Origin", "*")], r.path.as_bytes())
        })
        .await;
        let limits = FetchLimits { max_in_flight_per_origin: 1, ..FetchLimits::default() };
        let p = Arc::new(allow_ports(
            FetchPolicy::with_limits(Url::parse("http://page.test/").unwrap(), None, limits),
            &[s.port],
        ));
        let l = Arc::new(loader());
        let (t0, t1) = (CancelToken::new(), CancelToken::new());
        let first = {
            let (p, l, t, url) = (p.clone(), l.clone(), t0.clone(), s.url("/0"));
            tokio::spawn(async move { p.execute_cancellable(&l, ScriptRequest::get(url), &t).await })
        };
        wait_until(|| s.seen().len() == 1).await;
        let second = {
            let (p, l, t, url) = (p.clone(), l.clone(), t1.clone(), s.url("/1"));
            tokio::spawn(async move { p.execute_cancellable(&l, ScriptRequest::get(url), &t).await })
        };
        tokio::time::sleep(Duration::from_millis(50)).await;
        let aborter = t0.clone();
        std::thread::spawn(move || aborter.cancel()).join().unwrap();
        assert_eq!(first.await.unwrap().unwrap_err(), Denial::Cancelled);
        let r = second.await.unwrap().expect("the queued request got the freed slot");
        assert_eq!(&r.body[..], b"/1");
        assert_eq!(s.closed(), 1, "the aborted request's socket stayed open");
        assert!(t0.is_cancelled() && !t1.is_cancelled());
        assert!(!p.is_cancelled(), "aborting a request must not cancel the page");
    }

    #[tokio::test]
    async fn a_queued_request_aborted_before_it_connects_never_connects() {
        let s = serve_with(Duration::from_secs(30), |_| resp(200, &[("Access-Control-Allow-Origin", "*")], b"late")).await;
        let limits = FetchLimits { max_in_flight_per_origin: 1, ..FetchLimits::default() };
        let p = Arc::new(allow_ports(
            FetchPolicy::with_limits(Url::parse("http://page.test/").unwrap(), None, limits),
            &[s.port],
        ));
        let l = Arc::new(loader());
        let holder = {
            let (p, l, url) = (p.clone(), l.clone(), s.url("/hold"));
            tokio::spawn(async move { p.execute(&l, ScriptRequest::get(url)).await })
        };
        wait_until(|| s.seen().len() == 1).await;
        let token = CancelToken::new();
        let queued = {
            let (p, l, t, url) = (p.clone(), l.clone(), token.clone(), s.url("/queued"));
            tokio::spawn(async move { p.execute_cancellable(&l, ScriptRequest::get(url), &t).await })
        };
        tokio::time::sleep(Duration::from_millis(50)).await;
        token.cancel();
        assert_eq!(queued.await.unwrap().unwrap_err(), Denial::Cancelled);
        assert_eq!(s.hits(), 1, "the aborted queued request connected");
        p.cancel();
        assert_eq!(holder.await.unwrap().unwrap_err(), Denial::Cancelled);
    }

    #[tokio::test]
    async fn an_already_aborted_token_is_refused_at_once_without_spending_the_budget() {
        let s = serve(|_| resp(200, &[("Access-Control-Allow-Origin", "*")], b"ok")).await;
        let limits = FetchLimits { max_total_requests: 1, ..FetchLimits::default() };
        let p = allow_ports(
            FetchPolicy::with_limits(Url::parse("http://page.test/").unwrap(), None, limits),
            &[s.port],
        );
        let l = loader();
        let token = CancelToken::new();
        token.cancel();
        token.cancel();
        for _ in 0..3 {
            let r = p.execute_cancellable(&l, ScriptRequest::get(s.url("/x")), &token).await;
            assert_eq!(r.unwrap_err(), Denial::Cancelled);
        }
        assert_eq!(s.hits(), 0);
        let live = CancelToken::new();
        let ok = p.execute_cancellable(&l, ScriptRequest::get(s.url("/y")), &live).await;
        assert!(ok.is_ok(), "an aborted token spent the page's request budget: {ok:?}");
    }

    #[tokio::test]
    async fn a_page_cancel_also_ends_requests_that_carry_a_token() {
        let s = serve_with(Duration::from_secs(30), |_| resp(200, &[("Access-Control-Allow-Origin", "*")], b"late")).await;
        let p = Arc::new(allow_ports(page("http://page.test/"), &[s.port]));
        let l = Arc::new(loader());
        let token = CancelToken::new();
        let task = {
            let (p, l, t, url) = (p.clone(), l.clone(), token.clone(), s.url("/"));
            tokio::spawn(async move { p.execute_cancellable(&l, ScriptRequest::get(url), &t).await })
        };
        wait_until(|| s.seen().len() == 1).await;
        p.cancel();
        assert_eq!(task.await.unwrap().unwrap_err(), Denial::Cancelled);
        wait_until(|| s.closed() == 1).await;
        assert_eq!(s.closed(), 1);
        assert!(!token.is_cancelled());
    }

    // ---- §7.8: the shield ---------------------------------------------------

    struct BlockAds;
    impl InterceptHandler for BlockAds {
        fn intercept(&self, request: &Request) -> InterceptAction {
            if request.url.path().contains("/ads/") {
                InterceptAction::Block
            } else {
                InterceptAction::Allow
            }
        }
    }

    #[tokio::test]
    async fn the_shield_blocks_script_requests_exactly_as_it_blocks_subresources() {
        let s = serve(|_| resp(200, &[("Access-Control-Allow-Origin", "*")], b"ad")).await;
        let mut ic = RequestInterceptor::new();
        ic.add_handler(Arc::new(BlockAds));
        let l = ResourceLoader::with_interceptor(LoaderConfig::default(), Some(ic)).unwrap();
        let sub = l.fetch(Request::get(s.url("/ads/x.js"))).await;
        assert!(matches!(sub, Err(NetError::Blocked)));
        let p = allow_ports(page("http://page.test/"), &[s.port]);
        let r = p.execute(&l, ScriptRequest::get(s.url("/ads/x.js"))).await;
        assert_eq!(r.unwrap_err(), Denial::Shield);
        assert_eq!(s.hits(), 0);
        // And a redirect hop into a blocked URL is blocked too.
        let to = format!("http://127.0.0.1:{}/ads/y.js", s.port);
        let r2 = serve(move |_| resp(302, &[("Location", &to), ("Access-Control-Allow-Origin", "*")], b"")).await;
        let p = allow_ports(page("http://page.test/"), &[s.port, r2.port]);
        assert_eq!(p.execute(&l, ScriptRequest::get(r2.url("/"))).await.unwrap_err(), Denial::Shield);
        assert_eq!(s.hits(), 0);
    }

    // ---- subresources: the same rule, driven by the document referrer --------

    fn sub(url: &Url, page: &str, dest: RequestDestination) -> Request {
        Request::get(url.clone())
            .destination(dest)
            .referrer(Url::parse(page).unwrap())
    }

    fn denied(r: Result<crate::Response, NetError>) -> bool {
        matches!(r, Err(NetError::HttpError(rustkit_http::HttpError::AddressDenied(_))))
    }

    #[tokio::test]
    async fn a_public_page_cannot_load_private_subresources() {
        let private = serve(|_| resp(200, &[], b"secret")).await;
        let l = loader();
        let port = private.port;
        for dest in [
            RequestDestination::Image,
            RequestDestination::Style,
            RequestDestination::Script,
            RequestDestination::Font,
            RequestDestination::Other,
        ] {
            for target in [
                format!("http://127.0.0.1:{port}/a"),
                format!("http://localhost:{port}/a"),
                format!("http://[::1]:{port}/a"),
                "http://169.254.169.254/latest/".to_string(),
                "http://10.0.0.1/a".to_string(),
                "http://192.168.1.1/a".to_string(),
            ] {
                let r = l.fetch(sub(&Url::parse(&target).unwrap(), "http://example.com/", dest)).await;
                assert!(denied(r), "{dest:?} {target}");
            }
        }
        assert_eq!(private.hits(), 0, "the private server saw a connection");
    }

    #[tokio::test]
    async fn a_name_resolving_private_is_denied_for_subresources_too() {
        let private = serve(|_| resp(200, &[], b"secret")).await;
        let l = loader().with_resolver(Arc::new(Rebind));
        let target = Url::parse(&format!("http://rebind.test:{}/x.png", private.port)).unwrap();
        assert!(denied(l.fetch(sub(&target, "http://example.com/", RequestDestination::Image)).await));
        assert_eq!(private.hits(), 0);
    }

    #[tokio::test]
    async fn navigations_and_referrerless_loads_are_unchanged() {
        let s = serve(|_| resp(200, &[], b"page")).await;
        let l = loader();
        let nav = Request::get(s.url("/")).destination(RequestDestination::Document);
        assert_eq!(l.fetch(nav).await.expect("navigation to loopback").bytes().await.unwrap(), "page");
        l.fetch(Request::get(s.url("/b"))).await.expect("referrer-less load");
    }

    #[tokio::test]
    async fn a_private_page_loads_its_own_origin_and_nothing_else_private() {
        let own = serve(|r| match r.path.as_str() {
            "/hop" => resp(302, &[("Location", "/ok.css")], b""),
            _ => resp(200, &[], b"own"),
        })
        .await;
        let other = serve(|_| resp(200, &[], b"other")).await;
        let to_other = format!("http://127.0.0.1:{}/x", other.port);
        let bounce = serve(move |_| resp(302, &[("Location", &to_other)], b"")).await;
        let l = loader();
        let page = own.url("/index.html").to_string();
        for p in ["/ok.css", "/hop"] {
            let r = l.fetch(sub(&own.url(p), &page, RequestDestination::Style)).await.expect(p);
            assert_eq!(r.bytes().await.unwrap(), "own");
        }
        assert!(denied(l.fetch(sub(&other.url("/x"), &page, RequestDestination::Image)).await));
        // A same-origin URL that redirects to ANOTHER private server is denied at the hop.
        let page2 = bounce.url("/p").to_string();
        assert!(denied(l.fetch(sub(&bounce.url("/img"), &page2, RequestDestination::Image)).await));
        assert_eq!(other.hits(), 0, "the redirect target saw a connection");
    }

    #[tokio::test]
    async fn a_localhost_page_reaches_its_own_loopback_origin() {
        let own = serve(|_| resp(200, &[], b"own")).await;
        let l = loader();
        let page = format!("http://localhost:{}/", own.port);
        let url = Url::parse(&format!("http://localhost:{}/a.css", own.port)).unwrap();
        l.fetch(sub(&url, &page, RequestDestination::Style)).await.expect("own origin by name");
        let other = serve(|_| resp(200, &[], b"o")).await;
        let url = Url::parse(&format!("http://localhost:{}/a.css", other.port)).unwrap();
        assert!(denied(l.fetch(sub(&url, &page, RequestDestination::Style)).await));
        assert_eq!(other.hits(), 0);
    }

    #[tokio::test]
    async fn the_cache_cannot_answer_a_private_url_for_a_public_page() {
        let s = serve(|_| resp(200, &[("Cache-Control", "max-age=600")], b"private body")).await;
        let l = loader();
        let url = s.url("/cached.css");
        let nav = Request::get(url.clone()).destination(RequestDestination::Document);
        l.fetch(nav).await.expect("navigation caches it");
        assert!(denied(l.fetch(sub(&url, "http://example.com/", RequestDestination::Style)).await));
        assert_eq!(s.hits(), 1, "second request must not connect, and must not be served from cache");
    }

    #[test]
    fn page_policy_matrix() {
        let allows = |page: &str, target: &str, addr: &str| {
            page_address_policy(&Url::parse(page).unwrap(), &Url::parse(target).unwrap())
                .permits(&addr.parse().unwrap())
        };
        // Public page: public only.
        assert!(allows("https://a.com/", "https://b.com/", "93.184.216.34:443"));
        assert!(!allows("https://a.com/", "https://a.com/", "127.0.0.1:443"));
        // file: and data: documents get no private reach.
        assert!(!allows("file:///tmp/x.html", "http://127.0.0.1:80/", "127.0.0.1:80"));
        assert!(!allows("data:text/html,x", "http://127.0.0.1:80/", "127.0.0.1:80"));
        // Private page: its own host and port only.
        assert!(allows("http://127.0.0.1:8000/", "http://127.0.0.1:8000/x", "127.0.0.1:8000"));
        assert!(!allows("http://127.0.0.1:8000/", "http://127.0.0.1:8000/x", "127.0.0.1:8001"));
        assert!(!allows("http://127.0.0.1:8000/", "http://127.0.0.1:8000/x", "127.0.0.2:8000"));
        assert!(!allows("http://127.0.0.1:8000/", "http://10.0.0.5:8000/x", "10.0.0.5:8000"));
        assert!(allows("http://127.0.0.1:8000/", "http://127.0.0.1:8000/x", "93.184.216.34:8000"));
        // A named intranet page is a public page.
        assert!(!allows("http://intranet.corp/", "http://intranet.corp/", "10.1.2.3:80"));
    }

    // ---- C2: module scripts ----------------------------------------------------

    const JS: &str = "text/javascript";

    async fn module(p: &FetchPolicy, l: &ResourceLoader, url: &Url) -> Result<ModuleSource, Denial> {
        p.fetch_module(l, url, &Url::parse("http://page.test/app.js").unwrap()).await
    }

    #[tokio::test]
    async fn a_same_origin_module_with_a_javascript_type_loads() {
        let s = serve(|_| resp(200, &[("Content-Type", "text/javascript; charset=utf-8")], b"export const a = 1;")).await;
        let p = FetchPolicy::for_page(s.url("/index.html"), None);
        let m = module(&p, &loader(), &s.url("/m.js")).await.expect("module");
        assert_eq!(m.source, "export const a = 1;");
        assert_eq!(m.final_url, s.url("/m.js"));
        assert_eq!(s.seen()[0].headers.get("origin"), None, "same-origin GET carries no Origin");
    }

    #[tokio::test]
    async fn every_javascript_mime_type_is_accepted_and_nothing_else() {
        let l = loader();
        for ty in [
            "text/javascript", "application/javascript", "application/x-javascript", "text/ecmascript",
            "application/ecmascript", "text/jscript", "text/x-javascript", "TEXT/JavaScript; charset=UTF-8",
            "application/javascript ; foo=bar",
        ] {
            let s = serve(move |_| resp(200, &[("Content-Type", ty)], b"1")).await;
            let p = FetchPolicy::for_page(s.url("/"), None);
            assert!(module(&p, &l, &s.url("/m.js")).await.is_ok(), "{ty} must be accepted");
        }
        for ty in [
            "text/html", "text/plain", "application/json", "text/css", "image/png", "application/octet-stream",
            "text/javascriptx", "application/javascript+json", "text/", "javascript", "",
        ] {
            let s = serve(move |_| resp(200, &[("Content-Type", ty)], b"export {}")).await;
            let p = FetchPolicy::for_page(s.url("/"), None);
            let r = module(&p, &l, &s.url("/m.js")).await;
            assert!(matches!(r, Err(Denial::BadMime(_))), "{ty:?} must be refused: {r:?}");
        }
    }

    #[tokio::test]
    async fn a_module_with_no_content_type_is_refused() {
        let s = serve(|_| resp(200, &[], b"export {}")).await;
        let p = FetchPolicy::for_page(s.url("/"), None);
        let r = module(&p, &loader(), &s.url("/m.js")).await;
        assert!(matches!(r, Err(Denial::BadMime(_))), "{r:?}");
    }

    #[tokio::test]
    async fn a_module_that_is_not_2xx_is_refused_even_with_a_javascript_body() {
        let l = loader();
        for status in [404u16, 500, 403, 401] {
            let s = serve(move |_| resp(status, &[("Content-Type", JS)], b"export {}")).await;
            let p = FetchPolicy::for_page(s.url("/"), None);
            let r = module(&p, &l, &s.url("/m.js")).await;
            assert_eq!(r.unwrap_err(), Denial::BadStatus(status), "status {status}");
        }
    }

    #[tokio::test]
    async fn a_module_strips_a_bom_and_decodes_utf8_regardless_of_charset_label() {
        let mut body = vec![0xEF, 0xBB, 0xBF];
        body.extend_from_slice("export const s = '\u{e9}';".as_bytes());
        let s = serve(move |_| resp(200, &[("Content-Type", "text/javascript; charset=iso-8859-1")], &body)).await;
        let p = FetchPolicy::for_page(s.url("/"), None);
        let m = module(&p, &loader(), &s.url("/m.js")).await.unwrap();
        assert_eq!(m.source, "export const s = '\u{e9}';");
    }

    #[tokio::test]
    async fn a_cross_origin_module_needs_cors_and_the_body_is_not_exposed_without_it() {
        let l = loader();
        let none = serve(|_| resp(200, &[("Content-Type", JS)], b"export const stolen = 1;")).await;
        let p = allow_ports(page("http://page.test/"), &[none.port]);
        let r = module(&p, &l, &none.url("/m.js")).await;
        assert!(matches!(r, Err(Denial::Cors(_))), "{r:?}");

        let wrong = serve(|_| resp(200, &[("Content-Type", JS), ("Access-Control-Allow-Origin", "http://evil.test")], b"1")).await;
        let p = allow_ports(page("http://page.test/"), &[wrong.port]);
        assert!(matches!(module(&p, &l, &wrong.url("/m.js")).await, Err(Denial::Cors(_))));

        let ok = serve(|_| resp(200, &[("Content-Type", JS), ("Access-Control-Allow-Origin", "http://page.test")], b"export {}")).await;
        let p = allow_ports(page("http://page.test/"), &[ok.port]);
        let m = module(&p, &l, &ok.url("/m.js")).await.expect("matching ACAO");
        assert_eq!(m.source, "export {}");
        assert_eq!(ok.seen()[0].headers.get("origin").map(String::as_str), Some("http://page.test"));

        let star = serve(|_| resp(200, &[("Content-Type", JS), ("Access-Control-Allow-Origin", "*")], b"export {}")).await;
        let p = allow_ports(page("http://page.test/"), &[star.port]);
        assert!(module(&p, &l, &star.url("/m.js")).await.is_ok(), "* without credentials");
    }

    #[tokio::test]
    async fn a_module_is_never_a_no_cors_opaque_result_and_never_carries_cookies() {
        let s = serve(|_| resp(200, &[("Content-Type", JS), ("Access-Control-Allow-Origin", "*")], b"export {}")).await;
        let p = allow_ports(page("http://page.test/"), &[s.port]);
        let m = module(&p, &loader(), &s.url("/m.js")).await.expect("cors");
        assert_eq!(m.source, "export {}", "an opaque response would have an empty body");
        let seen = &s.seen()[0];
        assert!(!seen.headers.contains_key("cookie") && !seen.headers.contains_key("authorization"));
    }

    #[tokio::test]
    async fn a_module_cannot_reach_a_private_address_and_no_connection_is_made() {
        let private = serve(|_| resp(200, &[("Content-Type", JS), ("Access-Control-Allow-Origin", "*")], b"export {}")).await;
        let p = page("http://example.com/");
        let l = loader();
        for target in [
            format!("http://127.0.0.1:{}/m.js", private.port),
            format!("http://localhost:{}/m.js", private.port),
            "http://169.254.169.254/m.js".to_string(),
            "http://10.0.0.1/m.js".to_string(),
        ] {
            let r = module(&p, &l, &Url::parse(&target).unwrap()).await;
            assert!(matches!(r, Err(Denial::PrivateNetwork(_))), "{target}: {r:?}");
        }
        let l = loader().with_resolver(Arc::new(Rebind));
        let r = module(&p, &l, &Url::parse(&format!("http://rebind.test:{}/m.js", private.port)).unwrap()).await;
        assert!(matches!(r, Err(Denial::PrivateNetwork(_))), "{r:?}");
        assert_eq!(private.hits(), 0, "the private server saw a connection");
    }

    #[tokio::test]
    async fn a_module_redirect_into_a_private_address_or_a_wrong_type_is_denied_at_the_hop() {
        let private = serve(|_| resp(200, &[("Content-Type", JS), ("Access-Control-Allow-Origin", "*")], b"export {}")).await;
        let to = format!("http://127.0.0.1:{}/m.js", private.port);
        let hop = serve(move |_| resp(302, &[("Location", &to), ("Access-Control-Allow-Origin", "*")], b"")).await;
        let p = allow_ports(page("http://page.test/"), &[hop.port]);
        let r = module(&p, &loader(), &hop.url("/m.js")).await;
        assert!(matches!(r, Err(Denial::PrivateNetwork(_))), "{r:?}");
        assert_eq!(private.hits(), 0);

        // A redirect that ends on HTML is judged on the FINAL response.
        let html = serve(|_| resp(200, &[("Content-Type", "text/html"), ("Access-Control-Allow-Origin", "*")], b"<html>")).await;
        let to = format!("http://127.0.0.1:{}/m.js", html.port);
        let hop = serve(move |_| resp(302, &[("Location", &to), ("Access-Control-Allow-Origin", "*")], b"")).await;
        let p = allow_ports(page("http://page.test/"), &[hop.port, html.port]);
        assert!(matches!(module(&p, &loader(), &hop.url("/m.js")).await, Err(Denial::BadMime(_))));
    }

    #[tokio::test]
    async fn a_redirected_module_reports_its_final_url_as_its_identity() {
        let end = serve(|_| resp(200, &[("Content-Type", JS), ("Access-Control-Allow-Origin", "*")], b"export {}")).await;
        let to = format!("http://127.0.0.1:{}/final.js", end.port);
        let hop = serve(move |_| resp(302, &[("Location", &to), ("Access-Control-Allow-Origin", "*")], b"")).await;
        let p = allow_ports(page("http://page.test/"), &[hop.port, end.port]);
        let m = module(&p, &loader(), &hop.url("/start.js")).await.unwrap();
        assert_eq!(m.final_url, end.url("/final.js"));
    }

    #[tokio::test]
    async fn module_fetches_obey_mixed_content_the_shield_and_script_src() {
        // https page -> http module: blocked as mixed content.
        let s = serve(|_| resp(200, &[("Content-Type", JS), ("Access-Control-Allow-Origin", "*")], b"export {}")).await;
        let p = allow_ports(page("https://page.test/"), &[s.port]);
        assert_eq!(module(&p, &loader(), &s.url("/m.js")).await.unwrap_err(), Denial::MixedContent);

        // The shield sees a module exactly as it sees a script.
        let mut ic = RequestInterceptor::new();
        ic.add_handler(Arc::new(BlockAds));
        let l = ResourceLoader::with_interceptor(LoaderConfig::default(), Some(ic)).unwrap();
        let p = allow_ports(page("http://page.test/"), &[s.port]);
        assert_eq!(module(&p, &l, &s.url("/ads/m.js")).await.unwrap_err(), Denial::Shield);
        assert_eq!(s.hits(), 0);

        // script-src, not connect-src, governs modules; 'self' and * work.
        let csp = |d: &str| ContentSecurityPolicy::parse(d).unwrap();
        let own = serve(|_| resp(200, &[("Content-Type", JS)], b"export {}")).await;
        let url = own.url("/m.js");
        let l = loader();
        let with = |c: &str| FetchPolicy::for_page(own.url("/index.html"), Some(csp(c)));
        assert!(module(&with("script-src 'self'"), &l, &url).await.is_ok(), "'self'");
        assert!(module(&with("script-src *"), &l, &url).await.is_ok(), "*");
        assert!(module(&with("default-src 'self'"), &l, &url).await.is_ok(), "default-src 'self'");
        assert_eq!(module(&with("script-src https://cdn.example"), &l, &url).await.unwrap_err(), Denial::Csp);
        assert_eq!(module(&with("script-src 'none'"), &l, &url).await.unwrap_err(), Denial::Csp);
        // connect-src must NOT decide a module.
        assert!(module(&with("connect-src 'none'"), &l, &url).await.is_ok(), "connect-src is irrelevant to modules");
        // ...and script-src must not decide fetch().
        let f = with("script-src 'none'").execute(&l, ScriptRequest::get(url.clone())).await;
        assert!(f.is_ok(), "script-src is irrelevant to fetch(): {f:?}");
    }

    #[tokio::test]
    async fn module_fetches_share_the_per_page_budgets() {
        let s = serve(|_| resp(200, &[("Content-Type", JS)], b"export {}")).await;
        let limits = FetchLimits { max_total_requests: 2, ..FetchLimits::default() };
        let p = FetchPolicy::with_limits(s.url("/"), None, limits);
        let l = loader();
        assert!(module(&p, &l, &s.url("/a.js")).await.is_ok());
        assert!(p.execute(&l, ScriptRequest::get(s.url("/b"))).await.is_ok());
        assert_eq!(module(&p, &l, &s.url("/c.js")).await.unwrap_err(), Denial::BudgetExhausted);
    }

    #[tokio::test]
    async fn an_oversized_module_is_refused() {
        let s = serve(|_| resp(200, &[("Content-Type", JS)], &vec![b'a'; 4096])).await;
        let limits = FetchLimits { max_response_bytes: 1024, ..FetchLimits::default() };
        let p = FetchPolicy::with_limits(s.url("/"), None, limits);
        assert_eq!(module(&p, &loader(), &s.url("/m.js")).await.unwrap_err(), Denial::ResponseTooLarge);
    }

    #[tokio::test]
    async fn data_and_non_http_module_urls() {
        let p = page("http://page.test/");
        let l = loader();
        let d = Url::parse("data:text/javascript,export%20const%20x%3D1%3B").unwrap();
        let m = module(&p, &l, &d).await.expect("data: module");
        assert_eq!(m.source, "export const x=1;");
        let bad = Url::parse("data:text/html,%3Cb%3E").unwrap();
        assert!(matches!(module(&p, &l, &bad).await, Err(Denial::BadMime(_))));
        for u in ["file:///etc/passwd", "ftp://x.test/m.js", "javascript:alert(1)"] {
            let r = module(&p, &l, &Url::parse(u).unwrap()).await;
            assert!(matches!(r, Err(Denial::Scheme(_))), "{u}: {r:?}");
        }
    }
}
