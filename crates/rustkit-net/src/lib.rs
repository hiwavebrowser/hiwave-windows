//! # RustKit Net
//!
//! HTTP networking, request interception, and download management for the RustKit browser engine.
//!
//! ## Design Goals
//!
//! 1. **Async HTTP**: Non-blocking network requests
//! 2. **Request interception**: Filter/modify/block requests
//! 3. **Download management**: Progress, pause, resume, cancel
//! 4. **fetch() API**: JavaScript-compatible fetch interface

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode};
use mime::Mime;
use rustkit_http::Client as HttpClient;
use thiserror::Error;
use tokio::sync::{mpsc, RwLock};
use tracing::{debug, info, trace, warn};
use url::Url;

pub mod cache;
pub mod download;
pub mod intercept;
pub mod policy;
pub mod security;

pub use cache::{cache_eligibility, CacheConfig, CacheKey, CacheStats, CachedResponse, Ineligible, MemoryCache, parse_cache_control};
pub use download::{Download, DownloadEvent, DownloadId, DownloadManager, DownloadState};
pub use intercept::{InterceptAction, InterceptHandler, RequestInterceptor};
pub use security::{
    check_mixed_content, ContentSecurityPolicy, CookieAttributes, CorsChecker, CorsResult,
    CspDirective, CspSource, HashAlgorithm, MixedContentResult, MixedContentType, Origin,
    ReferrerPolicy, SameSite, SandboxFlags, SecurityContext, SecurityError,
};

/// Errors that can occur in networking.
#[derive(Error, Debug)]
pub enum NetError {
    #[error("Request failed: {0}")]
    RequestFailed(String),

    #[error("Invalid URL: {0}")]
    InvalidUrl(String),

    #[error("Timeout after {0:?}")]
    Timeout(Duration),

    #[error("Request cancelled")]
    Cancelled,

    #[error("Request blocked")]
    Blocked,

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("HTTP error: {0}")]
    HttpError(#[from] rustkit_http::HttpError),
}

/// Unique identifier for a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RequestId(u64);

impl RequestId {
    pub fn new() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(1);
        Self(COUNTER.fetch_add(1, Ordering::Relaxed))
    }

    pub fn raw(&self) -> u64 {
        self.0
    }
}

impl Default for RequestId {
    fn default() -> Self {
        Self::new()
    }
}

/// HTTP request.
/// What the fetched bytes are FOR — the fetch-spec "destination", carried on
/// the request so the shield can classify it (adblock filter lists key rules
/// on resource type: a script blocked on example.com may be fine as a
/// document). Privacy pin 2026-09-29: interception happens BEFORE bytes and
/// the census reports per destination.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RequestDestination {
    /// Top-level navigation HTML.
    Document,
    /// External stylesheet.
    Style,
    /// External script.
    Script,
    /// Raster or SVG image.
    Image,
    /// Web font.
    Font,
    /// Anything else (unknown).
    Other,
    /// A script's `fetch()` call (governed by [`policy::FetchPolicy`]).
    Fetch,
    /// A script's `XMLHttpRequest` (governed by [`policy::FetchPolicy`]).
    Xhr,
}

#[derive(Debug, Clone)]
pub struct Request {
    pub id: RequestId,
    pub url: Url,
    pub method: Method,
    pub headers: HeaderMap,
    pub body: Option<Bytes>,
    pub timeout: Option<Duration>,
    pub credentials: CredentialsMode,
    /// Fetch destination for shield classification; `Other` when the caller
    /// has not said. Conservative default: unknown types still hit the
    /// shield, just without type-specific rules.
    pub destination: RequestDestination,
    /// The URL of the document that made the request. The `Referer` header
    /// is derived from it through `referrer_policy`; this URL itself is
    /// never sent as-is.
    pub referrer: Option<Url>,
    pub referrer_policy: ReferrerPolicy,
    /// Whether this request has already been rewritten to a replay proxy.
    pub is_replay_proxied: bool,
}

impl Request {
    /// Create a GET request.
    pub fn get(url: Url) -> Self {
        Self {
            id: RequestId::new(),
            url,
            method: Method::GET,
            headers: HeaderMap::new(),
            body: None,
            timeout: Some(Duration::from_secs(30)),
            credentials: CredentialsMode::SameOrigin,
            referrer: None,
            referrer_policy: ReferrerPolicy::default(),
            destination: RequestDestination::Other,
            is_replay_proxied: false,
        }
    }

    /// Create a POST request.
    pub fn post(url: Url, body: Bytes) -> Self {
        Self {
            id: RequestId::new(),
            url,
            method: Method::POST,
            headers: HeaderMap::new(),
            body: Some(body),
            timeout: Some(Duration::from_secs(30)),
            credentials: CredentialsMode::SameOrigin,
            referrer: None,
            referrer_policy: ReferrerPolicy::default(),
            destination: RequestDestination::Other,
            is_replay_proxied: false,
        }
    }

    /// Add a header.
    pub fn header(mut self, name: HeaderName, value: HeaderValue) -> Self {
        self.headers.insert(name, value);
        self
    }

    /// Set timeout.
    pub fn timeout(mut self, duration: Duration) -> Self {
        self.timeout = Some(duration);
        self
    }

    /// Set referrer.
    pub fn referrer(mut self, referrer: Url) -> Self {
        self.referrer = Some(referrer);
        self
    }

    /// Set the referrer policy (default strict-origin-when-cross-origin).
    /// Tag what the fetched bytes are for (shield classification).
    pub fn destination(mut self, destination: RequestDestination) -> Self {
        self.destination = destination;
        self
    }

    pub fn referrer_policy(mut self, policy: ReferrerPolicy) -> Self {
        self.referrer_policy = policy;
        self
    }
}

/// Credentials mode for requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CredentialsMode {
    /// Never send cookies.
    Omit,
    /// Send cookies only for same-origin requests.
    #[default]
    SameOrigin,
    /// Always send cookies.
    Include,
}

/// Redirect handling mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RedirectMode {
    /// Follow redirects automatically (default).
    #[default]
    Follow,
    /// Don't follow redirects, return redirect response.
    Manual,
    /// Error on redirect.
    Error,
}

/// HTTP redirect status codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RedirectType {
    /// 301 Moved Permanently - permanent redirect, may change method to GET.
    MovedPermanently,
    /// 302 Found - temporary redirect, may change method to GET.
    Found,
    /// 303 See Other - redirect to GET.
    SeeOther,
    /// 307 Temporary Redirect - preserve method.
    TemporaryRedirect,
    /// 308 Permanent Redirect - preserve method.
    PermanentRedirect,
}

impl RedirectType {
    /// Parse from HTTP status code.
    pub fn from_status(status: StatusCode) -> Option<Self> {
        match status.as_u16() {
            301 => Some(RedirectType::MovedPermanently),
            302 => Some(RedirectType::Found),
            303 => Some(RedirectType::SeeOther),
            307 => Some(RedirectType::TemporaryRedirect),
            308 => Some(RedirectType::PermanentRedirect),
            _ => None,
        }
    }

    /// Check if this redirect preserves the HTTP method.
    pub fn preserves_method(self) -> bool {
        matches!(
            self,
            RedirectType::TemporaryRedirect | RedirectType::PermanentRedirect
        )
    }

    /// Check if this is a permanent redirect.
    pub fn is_permanent(self) -> bool {
        matches!(
            self,
            RedirectType::MovedPermanently | RedirectType::PermanentRedirect
        )
    }

    /// Get the HTTP status code.
    pub fn status_code(self) -> u16 {
        match self {
            RedirectType::MovedPermanently => 301,
            RedirectType::Found => 302,
            RedirectType::SeeOther => 303,
            RedirectType::TemporaryRedirect => 307,
            RedirectType::PermanentRedirect => 308,
        }
    }
}

/// Information about a redirect.
#[derive(Debug, Clone)]
pub struct RedirectInfo {
    /// The original request URL.
    pub from_url: Url,
    /// The redirect target URL.
    pub to_url: Url,
    /// The redirect type.
    pub redirect_type: RedirectType,
    /// Whether the method was changed (e.g., POST -> GET).
    pub method_changed: bool,
}

/// Redirect chain for tracking multiple redirects.
#[derive(Debug, Clone, Default)]
pub struct RedirectChain {
    /// List of redirects in order.
    pub redirects: Vec<RedirectInfo>,
    /// Maximum allowed redirects.
    pub max_redirects: usize,
}

impl RedirectChain {
    /// Create a new redirect chain with default max (20).
    pub fn new() -> Self {
        Self {
            redirects: Vec::new(),
            max_redirects: 20,
        }
    }

    /// Create with custom max redirects.
    pub fn with_max(max: usize) -> Self {
        Self {
            redirects: Vec::new(),
            max_redirects: max,
        }
    }

    /// Add a redirect to the chain.
    pub fn add(&mut self, info: RedirectInfo) -> Result<(), NetError> {
        if self.redirects.len() >= self.max_redirects {
            return Err(NetError::RequestFailed(format!(
                "Too many redirects (max {})",
                self.max_redirects
            )));
        }

        // Check for redirect loop
        if self.redirects.iter().any(|r| r.to_url == info.to_url) {
            return Err(NetError::RequestFailed("Redirect loop detected".into()));
        }

        self.redirects.push(info);
        Ok(())
    }

    /// Get the number of redirects.
    pub fn count(&self) -> usize {
        self.redirects.len()
    }

    /// Check if there were any redirects.
    pub fn was_redirected(&self) -> bool {
        !self.redirects.is_empty()
    }

    /// Get the original URL (before any redirects).
    pub fn original_url(&self) -> Option<&Url> {
        self.redirects.first().map(|r| &r.from_url)
    }

    /// Get the final URL (after all redirects).
    pub fn final_url(&self) -> Option<&Url> {
        self.redirects.last().map(|r| &r.to_url)
    }
}

/// HTTP response.
#[derive(Debug)]
pub struct Response {
    pub request_id: RequestId,
    pub url: Url,
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub content_type: Option<Mime>,
    pub content_length: Option<u64>,
    body: ResponseBody,
}

/// Response body variants.
#[derive(Debug)]
#[allow(dead_code)]
enum ResponseBody {
    /// Full body already loaded.
    Full(Bytes),
    /// Streaming body.
    Stream(mpsc::Receiver<Result<Bytes, NetError>>),
    /// Empty.
    Empty,
}

impl Response {
    /// Check if request was successful (2xx).
    pub fn ok(&self) -> bool {
        self.status.is_success()
    }

    /// Get the body as bytes.
    pub async fn bytes(self) -> Result<Bytes, NetError> {
        match self.body {
            ResponseBody::Full(b) => Ok(b),
            ResponseBody::Stream(mut rx) => {
                let mut chunks = Vec::new();
                while let Some(chunk) = rx.recv().await {
                    chunks.push(chunk?);
                }
                Ok(chunks.into_iter().flatten().collect())
            }
            ResponseBody::Empty => Ok(Bytes::new()),
        }
    }

    /// Get the body as text.
    pub async fn text(self) -> Result<String, NetError> {
        let bytes = self.bytes().await?;
        String::from_utf8(bytes.to_vec()).map_err(|e| NetError::RequestFailed(e.to_string()))
    }

    /// Get the body as JSON.
    pub async fn json<T: serde::de::DeserializeOwned>(self) -> Result<T, NetError> {
        let bytes = self.bytes().await?;
        serde_json::from_slice(&bytes).map_err(|e| NetError::RequestFailed(e.to_string()))
    }

    /// Get a suggested filename from Content-Disposition or URL.
    pub fn suggested_filename(&self) -> Option<String> {
        // Try Content-Disposition header
        if let Some(cd) = self.headers.get("content-disposition") {
            if let Ok(cd_str) = cd.to_str() {
                if let Some(start) = cd_str.find("filename=") {
                    let start = start + 9;
                    let filename = &cd_str[start..];
                    let filename = filename.trim_matches('"').trim_matches('\'');
                    if let Some(end) = filename.find(';') {
                        return Some(filename[..end].to_string());
                    }
                    return Some(filename.to_string());
                }
            }
        }

        // Fall back to URL path
        self.url
            .path_segments()
            .and_then(|mut segments| segments.next_back())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
    }
}

/// Resource loader configuration.
#[derive(Debug, Clone)]
pub struct LoaderConfig {
    /// User agent string.
    pub user_agent: String,
    /// Accept-Language header.
    pub accept_language: String,
    /// Default timeout.
    pub default_timeout: Duration,
    /// Maximum redirects.
    pub max_redirects: usize,
    /// Enable cookies.
    pub cookies_enabled: bool,
    /// Optional replay proxy URL (test-only, for deterministic HAR replay).
    pub replay_proxy: Option<Url>,
}

impl Default for LoaderConfig {
    fn default() -> Self {
        Self {
            user_agent: rustkit_http::default_user_agent(),
            accept_language: "en-US,en;q=0.9".to_string(),
            default_timeout: Duration::from_secs(30),
            max_redirects: 10,
            cookies_enabled: true,
            replay_proxy: None,
        }
    }
}

/// Resource loader for fetching URLs.
pub struct ResourceLoader {
    client: HttpClient,
    config: LoaderConfig,
    interceptor: Option<Arc<RwLock<RequestInterceptor>>>,
    download_manager: Arc<DownloadManager>,
    cache: Arc<MemoryCache>,
}

impl ResourceLoader {
    /// Create a new resource loader.
    pub fn new(config: LoaderConfig) -> Result<Self, NetError> {
        Self::with_interceptor(config, None)
    }

    /// Create a new resource loader with an optional request interceptor.
    pub fn with_interceptor(
        config: LoaderConfig,
        interceptor: Option<RequestInterceptor>,
    ) -> Result<Self, NetError> {
        let client = HttpClient::builder()
            .user_agent(&config.user_agent)
            .timeout(config.default_timeout)
            .redirect(true, config.max_redirects)
            .cookie_store(config.cookies_enabled)
            .build()
            .map_err(|e| NetError::RequestFailed(e.to_string()))?;

        if interceptor.is_some() {
            info!("ResourceLoader initialized with request interceptor and cache");
        } else {
            info!("ResourceLoader initialized with cache");
        }

        Ok(Self {
            client,
            config,
            interceptor: interceptor.map(|i| Arc::new(RwLock::new(i))),
            download_manager: Arc::new(DownloadManager::new()),
            cache: Arc::new(MemoryCache::new()),
        })
    }
    
    /// Get a reference to the memory cache.
    pub fn cache(&self) -> &Arc<MemoryCache> {
        &self.cache
    }
    
    /// Get cache statistics.
    pub fn cache_stats(&self) -> CacheStats {
        self.cache.stats()
    }

    /// Set the request interceptor.
    pub fn set_interceptor(&mut self, interceptor: RequestInterceptor) {
        self.interceptor = Some(Arc::new(RwLock::new(interceptor)));
    }

    /// Test seam: resolve names through `resolver` instead of the system.
    #[cfg(test)]
    pub(crate) fn with_resolver(mut self, resolver: Arc<dyn rustkit_http::Resolve>) -> Self {
        self.client = self.client.with_resolver(resolver);
        self
    }

    /// Get the download manager.
    pub fn download_manager(&self) -> Arc<DownloadManager> {
        Arc::clone(&self.download_manager)
    }

    /// Get a reference to the HTTP client.
    pub fn client(&self) -> &HttpClient {
        &self.client
    }

    /// Returns the restricted loopback address policy when a replay proxy is configured.
    pub fn replay_proxy_address_policy(&self) -> Option<rustkit_http::AddressPolicy> {
        self.config.replay_proxy.as_ref().map(|proxy| {
            let port = proxy.port_or_known_default().unwrap_or(80);
            rustkit_http::AddressPolicy::Custom(Arc::new(move |a| {
                a.ip().is_loopback() && a.port() == port
            }))
        })
    }

    /// Fetch a URL.
    pub async fn fetch(&self, request: Request) -> Result<Response, NetError> {
        // A subresource (anything with a document referrer that is not itself
        // the navigation) may not reach private addresses on behalf of a
        // public page, on any redirect hop. Navigations and referrer-less
        // loads are unchanged.
        let policy = match self.replay_proxy_address_policy() {
            Some(proxy_policy) => proxy_policy,
            None => match (&request.referrer, request.destination) {
                (Some(page), dest) if dest != RequestDestination::Document => {
                    policy::page_address_policy(page, &request.url)
                }
                _ => rustkit_http::AddressPolicy::Any,
            },
        };
        if matches!(policy, rustkit_http::AddressPolicy::Any) {
            return self.fetch_with(request, &self.client, true).await;
        }
        // The shared cache is keyed by URL alone and would answer a private
        // URL without connecting.
        let use_cache = !policy::url_host_is_private(&request.url);
        let client = self.client.clone().with_address_policy(policy);
        self.fetch_with(request, &client, use_cache).await
    }

    /// One hop of a governed (script-initiated) request: the same pipeline as
    /// [`fetch`](Self::fetch) (interceptor/shield, `data:`, cache, headers),
    /// over a client that refuses addresses outside `address_policy`, does
    /// not follow redirects (the policy vets and follows each hop itself) and
    /// refuses bodies over `max_body`. `use_cache` is false for requests whose
    /// response depends on the `Origin` header.
    pub(crate) async fn fetch_governed_hop(
        &self,
        request: Request,
        address_policy: rustkit_http::AddressPolicy,
        max_body: usize,
        use_cache: bool,
    ) -> Result<Response, NetError> {
        let policy = self.replay_proxy_address_policy().unwrap_or(address_policy);
        let client = self
            .client
            .clone()
            .with_address_policy(policy)
            .with_follow_redirects(false)
            .with_max_body(max_body);
        self.fetch_with(request, &client, use_cache).await
    }

    async fn fetch_with(
        &self,
        request: Request,
        client: &HttpClient,
        use_cache: bool,
    ) -> Result<Response, NetError> {
        debug!(url = %request.url, method = %request.method, "Fetching resource");

        // If replay_proxy is configured, rewrite outbound socket request to proxy on first entry
        if let Some(ref proxy) = self.config.replay_proxy {
            if !request.is_replay_proxied {
                let mut req = request.clone();
                req.is_replay_proxied = true;
                let orig_url_str = req.url.to_string();
                if let Ok(val) = HeaderValue::try_from(orig_url_str.as_str()) {
                    req.headers.insert(HeaderName::from_static("x-original-url"), val);
                }
                if let Some(host) = req.url.host_str() {
                    if let Ok(val) = HeaderValue::try_from(host) {
                        req.headers.insert(HeaderName::from_static("host"), val);
                    }
                }
                let mut proxy_target = proxy.clone();
                proxy_target.set_path(req.url.path());
                proxy_target.set_query(req.url.query());
                req.url = proxy_target;
                return Box::pin(self.fetch_with(req, client, false)).await;
            }
        }

        // Apply interception
        if let Some(interceptor) = &self.interceptor {
            let action = interceptor.read().await.intercept(&request).await;
            match action {
                InterceptAction::Allow => {}
                InterceptAction::Block => {
                    warn!(url = %request.url, "Request blocked by interceptor");
                    return Err(NetError::Blocked);
                }
                InterceptAction::Redirect(new_url) => {
                    debug!(url = %request.url, new_url = %new_url, "Request redirected");
                    let mut new_request = request.clone();
                    new_request.url = new_url;
                    return Box::pin(self.fetch_with(new_request, client, use_cache)).await;
                }
                InterceptAction::Modify(modified) => {
                    return Box::pin(self.fetch_with(*modified, client, use_cache)).await;
                }
            }
        }
        
        // data: URLs (RFC 2397) carry their own body — answer them here instead
        // of sending them to the HTTP client, which rejects them for having no
        // host. Sites inline small stylesheets, fonts, images and scripts this
        // way (facebook ships a base64 `data:text/css` sheet).
        if request.url.scheme() == "data" {
            let (content_type, body) = decode_data_url(request.url.as_str())?;
            let mut headers = HeaderMap::new();
            if let Ok(v) = HeaderValue::try_from(content_type.as_str()) {
                headers.insert(HeaderName::from_static("content-type"), v);
            }
            return Ok(Response {
                request_id: request.id,
                url: request.url.clone(),
                status: StatusCode::OK,
                headers,
                content_type: content_type.parse::<Mime>().ok(),
                content_length: Some(body.len() as u64),
                body: ResponseBody::Full(Bytes::from(body)),
            });
        }

        // Check cache for GET requests
        let cache_key = if use_cache && request.method == Method::GET && self.cache.enabled() {
            let key = CacheKey::new(&request.url);
            if let Some(cached) = self.cache.get(&key) {
                debug!(url = %request.url, "Serving from cache");
                
                // Parse content type
                let content_type = cached.headers
                    .get("content-type")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|s| s.parse::<Mime>().ok());
                
                return Ok(Response {
                    request_id: request.id,
                    url: request.url.clone(),
                    status: cached.status,
                    headers: cached.headers,
                    content_type,
                    content_length: Some(cached.body.len() as u64),
                    body: ResponseBody::Full(cached.body),
                });
            }
            Some(key)
        } else {
            None
        };

        // Build headers for rustkit-http request
        let mut headers = request.headers.clone();

        // Add Accept-Language
        if let Ok(val) = HeaderValue::try_from(&self.config.accept_language) {
            headers.insert(HeaderName::from_static("accept-language"), val);
        }

        // Referer, as the request's policy allows (never the raw referrer
        // URL, and never a caller-set header that could say more). Redirects
        // are safe: rustkit-http follows them with fresh headers, so this
        // value never reaches a redirect target.
        headers.remove(HeaderName::from_static("referer"));
        if let Some(value) = request
            .referrer
            .as_ref()
            .and_then(|referrer| request.referrer_policy.compute_referrer(referrer, &request.url))
        {
            if let Ok(val) = HeaderValue::try_from(value) {
                headers.insert(HeaderName::from_static("referer"), val);
            }
        }

        // Execute request using rustkit-http
        let http_response = client
            .request(
                request.method.clone(),
                request.url.as_str(),
                headers,
                request.body.clone(),
            )
            .await?;

        let url = http_response.url.clone();

        // Parse content type
        let content_type = http_response
            .content_type()
            .and_then(|s| s.parse::<Mime>().ok());

        // Get content length
        let content_length = http_response.content_length();

        trace!(
            url = %url,
            status = %http_response.status,
            content_type = ?content_type,
            content_length = ?content_length,
            body_len = http_response.body.len(),
            "Response received"
        );
        
        // Cache successful GET responses
        if let Some(key) = cache_key {
            if http_response.status.is_success() {
                use std::time::Instant;
                
                // Determine TTL from Cache-Control, falling back to the
                // CACHE's default TTL.
                //
                // This used to fall back to `self.config.default_timeout` —
                // the loader's NETWORK REQUEST TIMEOUT (30s). Two separate
                // bugs in one expression: header-less responses were cached
                // for the wrong duration, and `CacheConfig::default_ttl`
                // (300s) became dead config that the cache still announces in
                // its startup log. A number printed at boot and applied
                // nowhere is worse than no number.
                // Eligibility BEFORE freshness. A response can carry a
                // perfectly good max-age and still be ineligible — credentialed
                // requests, Cache-Control: private, and anything carrying Vary
                // (which this cache does not key on, so serving it would return
                // the wrong body for a differing request).
                // Eligibility BEFORE freshness: a response can carry a
                // perfectly good max-age and still be ineligible.
                //
                // Deliberately NOT an early return. The first version of this
                // returned a second Response here, which duplicated the exit at
                // the bottom of the function and got one field wrong: it sent
                // `request.url` (pre-redirect) where the real exit sends `url`
                // (post-redirect, from http_response). Every relative CSS,
                // image and script path then resolved against the wrong base on
                // any site that redirects — which is nearly all of them — and
                // because Vary makes most real responses ineligible, that rare
                // path became the common one. One exit means the mismatch cannot
                // recur.
                match cache_eligibility(
                    self.cache.enabled(),
                    &request.headers,
                    &http_response.headers,
                ) {
                    Some(reason) => {
                        debug!(url = %url, ?reason, "Response not cacheable");
                    }
                    None => {
                        let ttl = if self.cache.respects_cache_control() {
                            parse_cache_control(&http_response.headers)
                                .unwrap_or_else(|| self.cache.default_ttl())
                        } else {
                            self.cache.default_ttl()
                        };

                        if ttl > Duration::ZERO {
                            let cached = CachedResponse {
                                status: http_response.status,
                                headers: http_response.headers.clone(),
                                body: http_response.body.clone(),
                                cached_at: Instant::now(),
                                expires_at: Instant::now() + ttl,
                                size: http_response.body.len(),
                            };
                            self.cache.put(key, cached);
                        }
                    }
                }
            }
        }

        Ok(Response {
            request_id: request.id,
            url,
            status: http_response.status,
            headers: http_response.headers,
            content_type,
            content_length,
            body: ResponseBody::Full(http_response.body),
        })
    }

    /// Start a download.
    pub async fn start_download(
        &self,
        url: Url,
        destination: PathBuf,
    ) -> Result<DownloadId, NetError> {
        let request = Request::get(url);
        self.download_manager
            .start(request, destination, &self.client)
            .await
    }
}

/// Fetch API for JavaScript compatibility.
pub struct FetchApi {
    loader: Arc<ResourceLoader>,
}

impl FetchApi {
    /// Create a new fetch API.
    pub fn new(loader: Arc<ResourceLoader>) -> Self {
        Self { loader }
    }

    /// Fetch with options similar to JavaScript fetch().
    pub async fn fetch(&self, url: &str, options: FetchOptions) -> Result<Response, NetError> {
        let url = Url::parse(url).map_err(|e| NetError::InvalidUrl(e.to_string()))?;

        let mut request = match options.method.as_deref() {
            Some("POST") => Request::post(url, options.body.unwrap_or_default()),
            Some("PUT") => {
                let mut req = Request::get(url);
                req.method = Method::PUT;
                req.body = options.body;
                req
            }
            Some("DELETE") => {
                let mut req = Request::get(url);
                req.method = Method::DELETE;
                req
            }
            _ => Request::get(url),
        };

        // Add headers
        for (name, value) in options.headers {
            if let (Ok(n), Ok(v)) = (
                HeaderName::try_from(name.as_str()),
                HeaderValue::try_from(value.as_str()),
            ) {
                request.headers.insert(n, v);
            }
        }

        // Set credentials
        request.credentials = match options.credentials.as_deref() {
            Some("omit") => CredentialsMode::Omit,
            Some("include") => CredentialsMode::Include,
            _ => CredentialsMode::SameOrigin,
        };

        self.loader.fetch(request).await
    }
}

/// Options for fetch API.
#[derive(Debug, Default)]
pub struct FetchOptions {
    pub method: Option<String>,
    pub headers: HashMap<String, String>,
    pub body: Option<Bytes>,
    pub credentials: Option<String>,
    pub mode: Option<String>,
    pub cache: Option<String>,
    pub redirect: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_request_builder() {
        let url = Url::parse("https://example.com").unwrap();
        let request = Request::get(url.clone())
            .header(
                HeaderName::from_static("accept"),
                HeaderValue::from_static("application/json"),
            )
            .timeout(Duration::from_secs(10));

        assert_eq!(request.url, url);
        assert_eq!(request.method, Method::GET);
        assert!(request.headers.contains_key("accept"));
        assert_eq!(request.timeout, Some(Duration::from_secs(10)));
        assert_eq!(
            request.destination,
            RequestDestination::Other,
            "untagged fetches still hit the shield without a type-specific option"
        );
    }

    #[test]
    fn request_destination_tags_carry_through_the_builder() {
        // Privacy pin #364: every engine call site tags destination so
        // EasyList `$script`/`$image`/… options classify correctly.
        let url = Url::parse("https://cdn.example/a.js").unwrap();
        let referrer = Url::parse("https://news.example/").unwrap();
        let request = Request::get(url)
            .destination(RequestDestination::Script)
            .referrer(referrer.clone());
        assert_eq!(request.destination, RequestDestination::Script);
        assert_eq!(request.referrer, Some(referrer));
    }

    #[test]
    fn test_request_id_uniqueness() {
        let id1 = RequestId::new();
        let id2 = RequestId::new();
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_credentials_mode_default() {
        assert_eq!(CredentialsMode::default(), CredentialsMode::SameOrigin);
    }

    #[test]
    fn test_loader_config_default() {
        let config = LoaderConfig::default();
        // The default is the honest per-platform HiWave UA (network lane);
        // pin its invariants rather than one platform's exact string.
        assert!(config.user_agent.starts_with("Mozilla/5.0 ("));
        assert!(config.user_agent.contains("HiWave/1.0"));
        assert!(config.user_agent.contains("RustKit/1.0"));
        assert!(!config.user_agent.contains("Chrome"), "never Chrome's UA");
        assert!(config.cookies_enabled);
    }
}

/// Largest `data:` payload the loader will decode (bytes, after decoding).
pub const MAX_DATA_URL_BYTES: usize = 32 * 1024 * 1024;

/// Decode an RFC 2397 `data:[<mediatype>][;base64],<data>` URL into its media
/// type (default `text/plain;charset=US-ASCII`) and body bytes.
pub fn decode_data_url(url: &str) -> Result<(String, Vec<u8>), NetError> {
    let rest = url
        .get(..5)
        .filter(|s| s.eq_ignore_ascii_case("data:"))
        .map(|_| &url[5..])
        .ok_or_else(|| NetError::InvalidUrl("not a data: URL".into()))?;
    let (meta, payload) = rest
        .split_once(',')
        .ok_or_else(|| NetError::InvalidUrl("data: URL has no ','".into()))?;
    // Base64 encodes 3 bytes in 4 chars, and percent-encoding never grows the
    // payload, so bounding the input bounds the output.
    if payload.len() / 4 * 3 > MAX_DATA_URL_BYTES && payload.len() > MAX_DATA_URL_BYTES {
        return Err(NetError::RequestFailed("data: URL payload too large".into()));
    }
    let mut params: Vec<&str> = meta.split(';').map(str::trim).collect();
    let is_base64 = params.last().is_some_and(|p| p.eq_ignore_ascii_case("base64"));
    if is_base64 {
        params.pop();
    }
    let media = if params.first().is_none_or(|m| m.is_empty()) {
        let mut p = vec!["text/plain"];
        p.extend(params.iter().skip(1).copied());
        if p.len() == 1 {
            p.push("charset=US-ASCII");
        }
        p.join(";")
    } else {
        params.join(";")
    };
    let raw = percent_decode_bytes(payload);
    let body = if is_base64 {
        use base64::Engine as _;
        let compact: Vec<u8> = raw.into_iter().filter(|b| !b.is_ascii_whitespace()).collect();
        base64::engine::general_purpose::STANDARD
            .decode(&compact)
            .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(&compact))
            .map_err(|e| NetError::RequestFailed(format!("bad base64 in data: URL: {e}")))?
    } else {
        raw
    };
    if body.len() > MAX_DATA_URL_BYTES {
        return Err(NetError::RequestFailed("data: URL payload too large".into()));
    }
    Ok((media, body))
}

fn percent_decode_bytes(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Some(v) = std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

#[cfg(test)]
mod data_url_tests {
    use super::*;

    #[test]
    fn decodes_base64_and_percent_encoded_payloads_with_their_media_type() {
        let (ct, body) = decode_data_url("data:text/css; charset=utf-8;base64,Lm93N1g1NzQub3c3WDU3NHtkaXNwbGF5Om5vbmV9Cg==").unwrap();
        assert_eq!(ct, "text/css;charset=utf-8");
        assert_eq!(body, b".ow7X574.ow7X574{display:none}\n");
        let (ct, body) = decode_data_url("data:,a%20b%2Cc").unwrap();
        assert_eq!(ct, "text/plain;charset=US-ASCII");
        assert_eq!(body, b"a b,c");
        let (ct, body) = decode_data_url("data:image/svg+xml,%3Csvg%3E%3C/svg%3E").unwrap();
        assert_eq!(ct, "image/svg+xml");
        assert_eq!(body, b"<svg></svg>");
        // Unpadded base64 and whitespace inside the payload are accepted.
        assert_eq!(decode_data_url("data:;base64,aGk").unwrap().1, b"hi");
        assert_eq!(decode_data_url("data:;base64,aG k=").unwrap().1, b"hi");
    }

    #[test]
    fn rejects_malformed_and_oversized_data_urls() {
        assert!(decode_data_url("data:text/plain").is_err(), "no comma");
        assert!(decode_data_url("data:;base64,@@@@").is_err(), "bad base64");
        let huge = format!("data:,{}", "a".repeat(MAX_DATA_URL_BYTES + 1));
        assert!(decode_data_url(&huge).is_err(), "over the cap");
    }

    #[test]
    fn the_loader_answers_a_data_url_without_the_network() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let loader = ResourceLoader::new(LoaderConfig::default()).unwrap();
        let url = Url::parse("data:text/css;base64,Ym9keXtjb2xvcjpyZWR9").unwrap();
        let resp = rt.block_on(loader.fetch(Request::get(url))).expect("data: fetch");
        assert!(resp.ok());
        assert_eq!(resp.content_type.as_ref().map(|m| m.essence_str().to_string()), Some("text/css".into()));
        assert_eq!(rt.block_on(resp.text()).unwrap(), "body{color:red}");
    }
}

#[cfg(test)]
mod replay_proxy_tests {
    use super::*;
    use std::net::SocketAddr;

    #[test]
    fn test_shipped_default_replay_proxy_is_none_and_leaves_address_policy_untouched() {
        let config = LoaderConfig::default();
        assert!(config.replay_proxy.is_none(), "shipped LoaderConfig default must have replay_proxy == None");

        let loader = ResourceLoader::new(config).unwrap();
        assert!(loader.replay_proxy_address_policy().is_none(), "default loader must yield None for replay_proxy_address_policy");

        // When replay_proxy is configured, it must restrict to loopback on the proxy port
        let mut custom_config = LoaderConfig::default();
        custom_config.replay_proxy = Some(Url::parse("http://127.0.0.1:8765").unwrap());
        let proxy_loader = ResourceLoader::new(custom_config).unwrap();
        let policy = proxy_loader.replay_proxy_address_policy().expect("proxy policy must be present");

        let loopback_match: SocketAddr = "127.0.0.1:8765".parse().unwrap();
        let loopback_wrong_port: SocketAddr = "127.0.0.1:8080".parse().unwrap();
        let non_loopback: SocketAddr = "93.184.216.34:8765".parse().unwrap();

        assert!(policy.permits(&loopback_match), "loopback with proxy port must be allowed");
        assert!(!policy.permits(&loopback_wrong_port), "loopback with different port must be denied");
        assert!(!policy.permits(&non_loopback), "non-loopback address must be denied");
    }
}

