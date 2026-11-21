//! Web Security implementation for RustKit.
//!
//! Implements:
//! - Origin model and same-origin policy
//! - Content Security Policy (CSP)
//! - Cross-Origin Resource Sharing (CORS)
//! - Referrer Policy
//! - Secure contexts
//!
//! Security is critical - all checks must be conservative (fail-safe).

use std::collections::{HashMap, HashSet};
use std::str::FromStr;
use thiserror::Error;
use url::Url;

// ==================== Origin ====================

/// A web origin (scheme + host + port).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Origin {
    /// A tuple origin (scheme, host, port).
    Tuple {
        scheme: String,
        host: String,
        port: Option<u16>,
    },
    /// An opaque origin (unique, cannot match anything).
    Opaque(String),
}

impl Origin {
    /// Create an origin from a URL.
    pub fn from_url(url: &Url) -> Self {
        // data: and file: URLs have opaque origins
        if url.scheme() == "data" || url.scheme() == "file" || url.scheme() == "javascript" {
            return Origin::Opaque(url.to_string());
        }

        // blob: URLs inherit origin from their creator
        if url.scheme() == "blob" {
            if let Some(inner) = url.path().strip_prefix('/') {
                if let Ok(inner_url) = Url::parse(inner) {
                    return Origin::from_url(&inner_url);
                }
            }
            return Origin::Opaque(url.to_string());
        }

        Origin::Tuple {
            scheme: url.scheme().to_string(),
            host: url.host_str().unwrap_or("").to_string(),
            port: url.port_or_known_default(),
        }
    }

    /// Check if two origins are the same.
    pub fn same_origin(&self, other: &Origin) -> bool {
        match (self, other) {
            (
                Origin::Tuple { scheme: s1, host: h1, port: p1 },
                Origin::Tuple { scheme: s2, host: h2, port: p2 },
            ) => s1 == s2 && h1.eq_ignore_ascii_case(h2) && p1 == p2,
            // Opaque origins are never same-origin (even with themselves)
            _ => false,
        }
    }

    /// Check if this is an opaque origin.
    pub fn is_opaque(&self) -> bool {
        matches!(self, Origin::Opaque(_))
    }

    /// Check if this is a secure origin (HTTPS, localhost, etc.).
    pub fn is_secure(&self) -> bool {
        match self {
            Origin::Tuple { scheme, host, .. } => {
                scheme == "https" || scheme == "wss" ||
                host == "localhost" || host == "127.0.0.1" || host == "::1" ||
                host.ends_with(".localhost")
            }
            Origin::Opaque(_) => false,
        }
    }

    /// Serialize to string (for Origin header).
    pub fn serialize(&self) -> String {
        match self {
            Origin::Tuple { scheme, host, port } => {
                let default_port = match scheme.as_str() {
                    "http" | "ws" => Some(80),
                    "https" | "wss" => Some(443),
                    _ => None,
                };
                
                if *port == default_port || port.is_none() {
                    format!("{}://{}", scheme, host)
                } else if let Some(p) = port {
                    format!("{}://{}:{}", scheme, host, p)
                } else {
                    format!("{}://{}", scheme, host)
                }
            }
            Origin::Opaque(_) => "null".to_string(),
        }
    }
}

impl std::fmt::Display for Origin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.serialize())
    }
}

// ==================== Content Security Policy ====================

/// A Content Security Policy.
#[derive(Debug, Clone, Default)]
pub struct ContentSecurityPolicy {
    /// Directive -> Sources mapping.
    pub directives: HashMap<CspDirective, Vec<CspSource>>,
    /// Whether this is report-only mode.
    pub report_only: bool,
    /// Report URI for violations.
    pub report_uri: Option<String>,
}

impl ContentSecurityPolicy {
    /// Create a new empty CSP.
    pub fn new() -> Self {
        Self::default()
    }

    /// Parse CSP from header value.
    pub fn parse(header: &str) -> Result<Self, SecurityError> {
        let mut csp = ContentSecurityPolicy::new();
        
        for directive_str in header.split(';') {
            let directive_str = directive_str.trim();
            if directive_str.is_empty() {
                continue;
            }

            let mut parts = directive_str.split_whitespace();
            let directive_name = parts.next()
                .ok_or_else(|| SecurityError::InvalidCsp("Empty directive".into()))?;

            let directive = CspDirective::from_str(directive_name)
                .map_err(|_| SecurityError::InvalidCsp(format!("Unknown directive: {}", directive_name)))?;

            let sources: Vec<CspSource> = parts
                .filter_map(|s| CspSource::parse(s).ok())
                .collect();

            csp.directives.insert(directive, sources);
        }

        Ok(csp)
    }

    /// Get effective sources for a directive (falls back to default-src).
    pub fn get_sources(&self, directive: CspDirective) -> Option<&Vec<CspSource>> {
        self.directives.get(&directive)
            .or_else(|| self.directives.get(&CspDirective::DefaultSrc))
    }

    /// Check if a script source is allowed.
    pub fn allows_script(&self, url: Option<&Url>, is_inline: bool, nonce: Option<&str>, hash: Option<&str>) -> bool {
        let sources = match self.get_sources(CspDirective::ScriptSrc) {
            Some(s) => s,
            None => return true, // No restriction
        };

        if is_inline {
            // Check for 'unsafe-inline' or nonce/hash
            for source in sources {
                match source {
                    CspSource::UnsafeInline => return true,
                    CspSource::Nonce(n) if Some(n.as_str()) == nonce => return true,
                    CspSource::Hash(_, h) if Some(h.as_str()) == hash => return true,
                    _ => {}
                }
            }
            return false;
        }

        // External script - check URL
        if let Some(url) = url {
            self.url_matches_sources(url, sources)
        } else {
            false
        }
    }

    /// Check if a style source is allowed.
    pub fn allows_style(&self, url: Option<&Url>, is_inline: bool, nonce: Option<&str>, hash: Option<&str>) -> bool {
        let sources = match self.get_sources(CspDirective::StyleSrc) {
            Some(s) => s,
            None => return true,
        };

        if is_inline {
            for source in sources {
                match source {
                    CspSource::UnsafeInline => return true,
                    CspSource::Nonce(n) if Some(n.as_str()) == nonce => return true,
                    CspSource::Hash(_, h) if Some(h.as_str()) == hash => return true,
                    _ => {}
                }
            }
            return false;
        }

        if let Some(url) = url {
            self.url_matches_sources(url, sources)
        } else {
            false
        }
    }

    /// Check if an image source is allowed.
    pub fn allows_image(&self, url: &Url) -> bool {
        let sources = match self.get_sources(CspDirective::ImgSrc) {
            Some(s) => s,
            None => return true,
        };
        self.url_matches_sources(url, sources)
    }

    /// Check if a connect source (XHR, fetch, WebSocket) is allowed.
    pub fn allows_connect(&self, url: &Url) -> bool {
        let sources = match self.get_sources(CspDirective::ConnectSrc) {
            Some(s) => s,
            None => return true,
        };
        self.url_matches_sources(url, sources)
    }

    /// Check if a font source is allowed.
    pub fn allows_font(&self, url: &Url) -> bool {
        let sources = match self.get_sources(CspDirective::FontSrc) {
            Some(s) => s,
            None => return true,
        };
        self.url_matches_sources(url, sources)
    }

    /// Check if a frame source is allowed.
    pub fn allows_frame(&self, url: &Url) -> bool {
        let sources = match self.get_sources(CspDirective::FrameSrc)
            .or_else(|| self.get_sources(CspDirective::ChildSrc))
        {
            Some(s) => s,
            None => return true,
        };
        self.url_matches_sources(url, sources)
    }

    /// Check if eval() is allowed.
    pub fn allows_eval(&self) -> bool {
        let sources = match self.get_sources(CspDirective::ScriptSrc) {
            Some(s) => s,
            None => return true,
        };
        sources.iter().any(|s| matches!(s, CspSource::UnsafeEval))
    }

    /// Check if a URL matches the source list.
    fn url_matches_sources(&self, url: &Url, sources: &[CspSource]) -> bool {
        for source in sources {
            if source.matches_url(url) {
                return true;
            }
        }
        false
    }
}

/// CSP Directive types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CspDirective {
    DefaultSrc,
    ScriptSrc,
    StyleSrc,
    ImgSrc,
    FontSrc,
    ConnectSrc,
    MediaSrc,
    ObjectSrc,
    FrameSrc,
    ChildSrc,
    WorkerSrc,
    BaseUri,
    FormAction,
    FrameAncestors,
    ReportUri,
    ReportTo,
}

impl FromStr for CspDirective {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "default-src" => Ok(CspDirective::DefaultSrc),
            "script-src" => Ok(CspDirective::ScriptSrc),
            "style-src" => Ok(CspDirective::StyleSrc),
            "img-src" => Ok(CspDirective::ImgSrc),
            "font-src" => Ok(CspDirective::FontSrc),
            "connect-src" => Ok(CspDirective::ConnectSrc),
            "media-src" => Ok(CspDirective::MediaSrc),
            "object-src" => Ok(CspDirective::ObjectSrc),
            "frame-src" => Ok(CspDirective::FrameSrc),
            "child-src" => Ok(CspDirective::ChildSrc),
            "worker-src" => Ok(CspDirective::WorkerSrc),
            "base-uri" => Ok(CspDirective::BaseUri),
            "form-action" => Ok(CspDirective::FormAction),
            "frame-ancestors" => Ok(CspDirective::FrameAncestors),
            "report-uri" => Ok(CspDirective::ReportUri),
            "report-to" => Ok(CspDirective::ReportTo),
            _ => Err(()),
        }
    }
}

/// CSP Source values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CspSource {
    /// 'self' - same origin
    Self_,
    /// 'none' - nothing allowed
    None,
    /// 'unsafe-inline' - inline scripts/styles allowed
    UnsafeInline,
    /// 'unsafe-eval' - eval() allowed
    UnsafeEval,
    /// 'strict-dynamic' - trust scripts loaded by trusted scripts
    StrictDynamic,
    /// 'nonce-xxx' - specific nonce value
    Nonce(String),
    /// 'sha256-xxx' or similar hash
    Hash(HashAlgorithm, String),
    /// Host source (e.g., "example.com", "*.example.com")
    Host(String),
    /// Scheme source (e.g., "https:", "data:")
    Scheme(String),
}

impl CspSource {
    /// Parse a CSP source value.
    pub fn parse(s: &str) -> Result<Self, ()> {
        let s = s.trim();
        
        if s == "'self'" {
            return Ok(CspSource::Self_);
        }
        if s == "'none'" {
            return Ok(CspSource::None);
        }
        if s == "'unsafe-inline'" {
            return Ok(CspSource::UnsafeInline);
        }
        if s == "'unsafe-eval'" {
            return Ok(CspSource::UnsafeEval);
        }
        if s == "'strict-dynamic'" {
            return Ok(CspSource::StrictDynamic);
        }
        if let Some(nonce) = s.strip_prefix("'nonce-").and_then(|s| s.strip_suffix('\'')) {
            return Ok(CspSource::Nonce(nonce.to_string()));
        }
        if let Some(hash) = s.strip_prefix("'sha256-").and_then(|s| s.strip_suffix('\'')) {
            return Ok(CspSource::Hash(HashAlgorithm::Sha256, hash.to_string()));
        }
        if let Some(hash) = s.strip_prefix("'sha384-").and_then(|s| s.strip_suffix('\'')) {
            return Ok(CspSource::Hash(HashAlgorithm::Sha384, hash.to_string()));
        }
        if let Some(hash) = s.strip_prefix("'sha512-").and_then(|s| s.strip_suffix('\'')) {
            return Ok(CspSource::Hash(HashAlgorithm::Sha512, hash.to_string()));
        }
        if s.ends_with(':') {
            return Ok(CspSource::Scheme(s.to_string()));
        }
        
        // Assume it's a host source
        Ok(CspSource::Host(s.to_string()))
    }

    /// Check if this source matches a URL.
    fn matches_url(&self, url: &Url) -> bool {
        match self {
            CspSource::Self_ => false, // Requires document origin to compare
            CspSource::None => false,
            CspSource::UnsafeInline | CspSource::UnsafeEval | CspSource::StrictDynamic => false,
            CspSource::Nonce(_) | CspSource::Hash(_, _) => false,
            CspSource::Scheme(scheme) => {
                let url_scheme = format!("{}:", url.scheme());
                &url_scheme == scheme
            }
            CspSource::Host(pattern) => {
                let host = url.host_str().unwrap_or("");
                if let Some(domain) = pattern.strip_prefix("*.") {
                    // Wildcard match
                    host == domain || host.ends_with(&format!(".{}", domain))
                } else if pattern.contains('/') {
                    // Path pattern
                    let full = format!("{}{}", host, url.path());
                    full.starts_with(pattern)
                } else {
                    host == pattern
                }
            }
        }
    }
}

/// Hash algorithm for CSP.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HashAlgorithm {
    Sha256,
    Sha384,
    Sha512,
}

// ==================== CORS ====================

/// CORS check result.
#[derive(Debug, Clone)]
pub enum CorsResult {
    /// CORS check passed.
    Allowed,
    /// CORS check failed.
    Denied(String),
    /// Preflight required.
    PreflightRequired,
}

/// CORS checker.
#[derive(Debug, Clone)]
pub struct CorsChecker {
    /// Allowed methods from preflight cache.
    pub allowed_methods: HashSet<String>,
    /// Allowed headers from preflight cache.
    pub allowed_headers: HashSet<String>,
    /// Cache max-age.
    pub max_age: Option<u64>,
}

impl CorsChecker {
    /// Create a new CORS checker.
    pub fn new() -> Self {
        Self {
            allowed_methods: HashSet::new(),
            allowed_headers: HashSet::new(),
            max_age: None,
        }
    }

    /// Check if a request is a "simple" CORS request (no preflight needed).
    pub fn is_simple_request(method: &str, headers: &[(&str, &str)]) -> bool {
        // Simple methods
        let simple_methods = ["GET", "HEAD", "POST"];
        if !simple_methods.contains(&method.to_uppercase().as_str()) {
            return false;
        }

        // Simple headers (CORS-safelisted)
        let simple_headers = [
            "accept", "accept-language", "content-language", "content-type",
        ];
        
        for (name, value) in headers {
            let name_lower = name.to_lowercase();
            if !simple_headers.contains(&name_lower.as_str()) {
                return false;
            }
            
            // Content-Type must be simple
            if name_lower == "content-type" {
                let simple_types = [
                    "application/x-www-form-urlencoded",
                    "multipart/form-data",
                    "text/plain",
                ];
                let value_lower = value.to_lowercase();
                if !simple_types.iter().any(|t| value_lower.starts_with(t)) {
                    return false;
                }
            }
        }

        true
    }

    /// Check CORS response headers.
    pub fn check_response(
        &self,
        request_origin: &str,
        response_allow_origin: Option<&str>,
        response_allow_credentials: Option<&str>,
        with_credentials: bool,
    ) -> CorsResult {
        match response_allow_origin {
            None => {
                CorsResult::Denied("No Access-Control-Allow-Origin header".into())
            }
            Some("*") if with_credentials => {
                CorsResult::Denied("Wildcard not allowed with credentials".into())
            }
            Some("*") => CorsResult::Allowed,
            Some(allowed) if allowed == request_origin => {
                // Check credentials
                if with_credentials {
                    match response_allow_credentials {
                        Some("true") => CorsResult::Allowed,
                        _ => CorsResult::Denied("Credentials not allowed".into()),
                    }
                } else {
                    CorsResult::Allowed
                }
            }
            Some(allowed) => {
                CorsResult::Denied(format!(
                    "Origin '{}' not allowed (allowed: '{}')",
                    request_origin, allowed
                ))
            }
        }
    }

    /// Parse preflight response and update cache.
    pub fn parse_preflight_response(
        &mut self,
        allow_methods: Option<&str>,
        allow_headers: Option<&str>,
        max_age: Option<&str>,
    ) {
        if let Some(methods) = allow_methods {
            self.allowed_methods = methods
                .split(',')
                .map(|s| s.trim().to_uppercase())
                .collect();
        }

        if let Some(headers) = allow_headers {
            self.allowed_headers = headers
                .split(',')
                .map(|s| s.trim().to_lowercase())
                .collect();
        }

        if let Some(age) = max_age {
            self.max_age = age.parse().ok();
        }
    }

    /// Check if a method is allowed (from preflight cache).
    pub fn is_method_allowed(&self, method: &str) -> bool {
        self.allowed_methods.is_empty() || self.allowed_methods.contains(&method.to_uppercase())
    }

    /// Check if a header is allowed (from preflight cache).
    pub fn is_header_allowed(&self, header: &str) -> bool {
        self.allowed_headers.is_empty() || self.allowed_headers.contains(&header.to_lowercase())
    }
}

impl Default for CorsChecker {
    fn default() -> Self {
        Self::new()
    }
}

// ==================== Referrer Policy ====================

/// Referrer policy values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReferrerPolicy {
    /// No referrer sent.
    NoReferrer,
    /// No referrer for cross-origin, URL for same-origin.
    NoReferrerWhenDowngrade,
    /// Origin only (no path).
    Origin,
    /// Origin only for cross-origin, full URL for same-origin.
    OriginWhenCrossOrigin,
    /// Full URL for same-origin, none for cross-origin.
    SameOrigin,
    /// Origin when downgrading, none otherwise.
    StrictOrigin,
    /// Default - origin when downgrading cross-origin, full URL for same-origin.
    #[default]
    StrictOriginWhenCrossOrigin,
    /// Always send full referrer URL.
    UnsafeUrl,
}

impl ReferrerPolicy {
    /// Compute the `Referer` header value for a request (Fetch, "determine
    /// request's referrer"). `None` means send no `Referer` at all.
    ///
    /// The referrer is stripped before any policy applies: no fragment and no
    /// userinfo, ever, and only http(s) documents have a referrer (a file:,
    /// data: or about: page must not leak its URL). The origin-only form is
    /// the origin with an empty path, `https://example.com/`, as Chrome sends.
    pub fn compute_referrer(
        &self,
        referrer_url: &Url,
        target_url: &Url,
    ) -> Option<String> {
        if !matches!(referrer_url.scheme(), "http" | "https") {
            return None;
        }
        let mut full = referrer_url.clone();
        full.set_fragment(None);
        let _ = full.set_username("");
        let _ = full.set_password(None);
        let mut origin = full.clone();
        origin.set_path("/");
        origin.set_query(None);
        let full = full.to_string();
        let origin = origin.to_string();

        let same_origin = Origin::from_url(referrer_url).same_origin(&Origin::from_url(target_url));
        let is_downgrade = referrer_url.scheme() == "https" && target_url.scheme() == "http";

        match self {
            ReferrerPolicy::NoReferrer => None,
            ReferrerPolicy::NoReferrerWhenDowngrade => (!is_downgrade).then_some(full),
            ReferrerPolicy::Origin => Some(origin),
            ReferrerPolicy::OriginWhenCrossOrigin => Some(if same_origin { full } else { origin }),
            ReferrerPolicy::SameOrigin => same_origin.then_some(full),
            ReferrerPolicy::StrictOrigin => (!is_downgrade).then_some(origin),
            ReferrerPolicy::StrictOriginWhenCrossOrigin => {
                if is_downgrade {
                    None
                } else if same_origin {
                    Some(full)
                } else {
                    Some(origin)
                }
            }
            ReferrerPolicy::UnsafeUrl => Some(full),
        }
    }

    /// Parse a `Referrer-Policy` header value: a comma-separated list where
    /// the last recognised token wins, so a new policy can be listed after
    /// a fallback for older browsers. `None` when no token is recognised.
    pub fn parse_header(value: &str) -> Option<Self> {
        value
            .split(',')
            .filter_map(|token| token.trim().parse().ok())
            .next_back()
    }
}

impl FromStr for ReferrerPolicy {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "no-referrer" => Ok(ReferrerPolicy::NoReferrer),
            "no-referrer-when-downgrade" => Ok(ReferrerPolicy::NoReferrerWhenDowngrade),
            "origin" => Ok(ReferrerPolicy::Origin),
            "origin-when-cross-origin" => Ok(ReferrerPolicy::OriginWhenCrossOrigin),
            "same-origin" => Ok(ReferrerPolicy::SameOrigin),
            "strict-origin" => Ok(ReferrerPolicy::StrictOrigin),
            "strict-origin-when-cross-origin" => Ok(ReferrerPolicy::StrictOriginWhenCrossOrigin),
            "unsafe-url" => Ok(ReferrerPolicy::UnsafeUrl),
            _ => Err(()),
        }
    }
}

// ==================== Cookie Security ====================

/// Cookie SameSite attribute.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SameSite {
    /// Strict - only same-site requests.
    Strict,
    /// Lax - same-site + top-level navigation.
    #[default]
    Lax,
    /// None - all requests (requires Secure).
    None,
}

impl FromStr for SameSite {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "strict" => Ok(SameSite::Strict),
            "lax" => Ok(SameSite::Lax),
            "none" => Ok(SameSite::None),
            _ => Err(()),
        }
    }
}

/// Secure cookie attributes.
#[derive(Debug, Clone, Default)]
pub struct CookieAttributes {
    /// Cookie name.
    pub name: String,
    /// Cookie value.
    pub value: String,
    /// Domain scope.
    pub domain: Option<String>,
    /// Path scope.
    pub path: Option<String>,
    /// Expiration time.
    pub expires: Option<std::time::SystemTime>,
    /// Max-Age in seconds.
    pub max_age: Option<i64>,
    /// Secure flag (HTTPS only).
    pub secure: bool,
    /// HttpOnly flag (no JS access).
    pub http_only: bool,
    /// SameSite attribute.
    pub same_site: SameSite,
}

impl CookieAttributes {
    /// Check if cookie should be sent for a request.
    pub fn should_send(
        &self,
        request_url: &Url,
        is_same_site: bool,
        is_top_level_navigation: bool,
    ) -> bool {
        // Check Secure flag
        if self.secure && request_url.scheme() != "https" {
            return false;
        }

        // Check SameSite
        match self.same_site {
            SameSite::Strict => {
                if !is_same_site {
                    return false;
                }
            }
            SameSite::Lax => {
                if !is_same_site && !is_top_level_navigation {
                    return false;
                }
            }
            SameSite::None => {
                // SameSite=None requires Secure
                if !self.secure {
                    return false;
                }
            }
        }

        // Check domain
        if let Some(ref domain) = self.domain {
            let host = request_url.host_str().unwrap_or("");
            if !host.eq_ignore_ascii_case(domain) && !host.ends_with(&format!(".{}", domain)) {
                return false;
            }
        }

        // Check path
        if let Some(ref path) = self.path {
            let request_path = request_url.path();
            if !request_path.starts_with(path) {
                return false;
            }
        }

        true
    }
}

// ==================== Security Context ====================

/// Full security context for a document.
#[derive(Debug, Clone)]
pub struct SecurityContext {
    /// Document origin.
    pub origin: Origin,
    /// Content Security Policy.
    pub csp: Option<ContentSecurityPolicy>,
    /// Referrer policy.
    pub referrer_policy: ReferrerPolicy,
    /// Whether this is a secure context.
    pub is_secure_context: bool,
    /// Whether sandboxed.
    pub sandboxed: bool,
    /// Sandbox flags (if sandboxed).
    pub sandbox_flags: SandboxFlags,
}

impl SecurityContext {
    /// Create a security context from a URL.
    pub fn from_url(url: &Url) -> Self {
        let origin = Origin::from_url(url);
        let is_secure = origin.is_secure();

        Self {
            origin,
            csp: None,
            referrer_policy: ReferrerPolicy::default(),
            is_secure_context: is_secure,
            sandboxed: false,
            sandbox_flags: SandboxFlags::default(),
        }
    }

    /// Check if same-origin with another URL.
    pub fn is_same_origin(&self, url: &Url) -> bool {
        self.origin.same_origin(&Origin::from_url(url))
    }

    /// Check if a script is allowed to run.
    pub fn allows_script(&self, url: Option<&Url>, is_inline: bool, nonce: Option<&str>) -> bool {
        // Check sandbox
        if self.sandboxed && !self.sandbox_flags.allow_scripts {
            return false;
        }

        // Check CSP
        if let Some(ref csp) = self.csp {
            return csp.allows_script(url, is_inline, nonce, None);
        }

        true
    }

    /// Check if eval() is allowed.
    pub fn allows_eval(&self) -> bool {
        if self.sandboxed && !self.sandbox_flags.allow_scripts {
            return false;
        }

        if let Some(ref csp) = self.csp {
            return csp.allows_eval();
        }

        true
    }
}

/// Sandbox flags for iframes.
#[derive(Debug, Clone, Copy, Default)]
pub struct SandboxFlags {
    /// Allow scripts to run.
    pub allow_scripts: bool,
    /// Allow same-origin access.
    pub allow_same_origin: bool,
    /// Allow form submission.
    pub allow_forms: bool,
    /// Allow popups.
    pub allow_popups: bool,
    /// Allow pointer lock.
    pub allow_pointer_lock: bool,
    /// Allow top navigation.
    pub allow_top_navigation: bool,
    /// Allow modals.
    pub allow_modals: bool,
}

impl SandboxFlags {
    /// Parse sandbox attribute value.
    pub fn parse(value: &str) -> Self {
        let mut flags = SandboxFlags::default();
        
        for token in value.split_whitespace() {
            match token.to_lowercase().as_str() {
                "allow-scripts" => flags.allow_scripts = true,
                "allow-same-origin" => flags.allow_same_origin = true,
                "allow-forms" => flags.allow_forms = true,
                "allow-popups" => flags.allow_popups = true,
                "allow-pointer-lock" => flags.allow_pointer_lock = true,
                "allow-top-navigation" => flags.allow_top_navigation = true,
                "allow-modals" => flags.allow_modals = true,
                _ => {}
            }
        }

        flags
    }
}

// ==================== Mixed Content ====================

/// Mixed content check result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MixedContentResult {
    /// Content is safe to load.
    Allowed,
    /// Content is optionally blockable (images, video, audio).
    OptionallyBlockable,
    /// Content is blockable (scripts, styles, iframes).
    Blockable,
}

/// Check mixed content for a resource.
pub fn check_mixed_content(page_url: &Url, resource_url: &Url, resource_type: MixedContentType) -> MixedContentResult {
    // Not a mixed content situation if page is not HTTPS
    if page_url.scheme() != "https" {
        return MixedContentResult::Allowed;
    }

    // Resource is HTTPS - allowed
    if resource_url.scheme() == "https" || resource_url.scheme() == "wss" {
        return MixedContentResult::Allowed;
    }

    // Data URLs and blob URLs are allowed
    if resource_url.scheme() == "data" || resource_url.scheme() == "blob" {
        return MixedContentResult::Allowed;
    }

    // Check resource type
    match resource_type {
        MixedContentType::Image | MixedContentType::Video | MixedContentType::Audio => {
            MixedContentResult::OptionallyBlockable
        }
        MixedContentType::Script | MixedContentType::Style | MixedContentType::Frame |
        MixedContentType::Font | MixedContentType::Fetch | MixedContentType::Other => {
            MixedContentResult::Blockable
        }
    }
}

/// Mixed content resource types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MixedContentType {
    Script,
    Style,
    Image,
    Video,
    Audio,
    Frame,
    Font,
    Fetch,
    Other,
}

// ==================== Errors ====================

/// Security-related errors.
#[derive(Debug, Clone, Error)]
pub enum SecurityError {
    #[error("Same-origin policy violation")]
    SameOriginViolation,

    #[error("CSP violation: {0}")]
    CspViolation(String),

    #[error("CORS error: {0}")]
    CorsError(String),

    #[error("Mixed content blocked")]
    MixedContentBlocked,

    #[error("Invalid CSP: {0}")]
    InvalidCsp(String),

    #[error("Sandbox violation: {0}")]
    SandboxViolation(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_origin_from_url() {
        let url = Url::parse("https://example.com:8080/path").unwrap();
        let origin = Origin::from_url(&url);

        match origin {
            Origin::Tuple { scheme, host, port } => {
                assert_eq!(scheme, "https");
                assert_eq!(host, "example.com");
                assert_eq!(port, Some(8080));
            }
            _ => panic!("Expected tuple origin"),
        }
    }

    #[test]
    fn test_origin_same_origin() {
        let url1 = Url::parse("https://example.com/page1").unwrap();
        let url2 = Url::parse("https://example.com/page2").unwrap();
        let url3 = Url::parse("https://other.com/page").unwrap();
        let url4 = Url::parse("http://example.com/page").unwrap();

        let origin1 = Origin::from_url(&url1);
        let origin2 = Origin::from_url(&url2);
        let origin3 = Origin::from_url(&url3);
        let origin4 = Origin::from_url(&url4);

        assert!(origin1.same_origin(&origin2));
        assert!(!origin1.same_origin(&origin3));
        assert!(!origin1.same_origin(&origin4)); // Different scheme
    }

    #[test]
    fn test_origin_opaque() {
        let data_url = Url::parse("data:text/html,<h1>Hi</h1>").unwrap();
        let origin = Origin::from_url(&data_url);
        assert!(origin.is_opaque());

        let file_url = Url::parse("file:///path/to/file.html").unwrap();
        let origin = Origin::from_url(&file_url);
        assert!(origin.is_opaque());
    }

    #[test]
    fn test_origin_secure() {
        let https = Url::parse("https://example.com/").unwrap();
        assert!(Origin::from_url(&https).is_secure());

        let http = Url::parse("http://example.com/").unwrap();
        assert!(!Origin::from_url(&http).is_secure());

        let localhost = Url::parse("http://localhost:3000/").unwrap();
        assert!(Origin::from_url(&localhost).is_secure());
    }

    #[test]
    fn test_csp_parse() {
        let csp = ContentSecurityPolicy::parse(
            "default-src 'self'; script-src 'self' https://cdn.example.com; img-src *"
        ).unwrap();

        assert!(csp.directives.contains_key(&CspDirective::DefaultSrc));
        assert!(csp.directives.contains_key(&CspDirective::ScriptSrc));
        assert!(csp.directives.contains_key(&CspDirective::ImgSrc));
    }

    #[test]
    fn test_csp_source_parse() {
        assert_eq!(CspSource::parse("'self'").unwrap(), CspSource::Self_);
        assert_eq!(CspSource::parse("'none'").unwrap(), CspSource::None);
        assert_eq!(CspSource::parse("'unsafe-inline'").unwrap(), CspSource::UnsafeInline);
        assert_eq!(CspSource::parse("'unsafe-eval'").unwrap(), CspSource::UnsafeEval);
        
        if let CspSource::Nonce(n) = CspSource::parse("'nonce-abc123'").unwrap() {
            assert_eq!(n, "abc123");
        } else {
            panic!("Expected nonce");
        }
    }

    #[test]
    fn test_cors_simple_request() {
        assert!(CorsChecker::is_simple_request("GET", &[]));
        assert!(CorsChecker::is_simple_request("POST", &[("content-type", "text/plain")]));
        assert!(!CorsChecker::is_simple_request("PUT", &[]));
        assert!(!CorsChecker::is_simple_request("POST", &[("content-type", "application/json")]));
    }

    #[test]
    fn test_cors_check_response() {
        let checker = CorsChecker::new();

        // Wildcard allowed
        let result = checker.check_response("https://example.com", Some("*"), None, false);
        assert!(matches!(result, CorsResult::Allowed));

        // Wildcard with credentials - denied
        let result = checker.check_response("https://example.com", Some("*"), None, true);
        assert!(matches!(result, CorsResult::Denied(_)));

        // Exact match
        let result = checker.check_response(
            "https://example.com",
            Some("https://example.com"),
            None,
            false
        );
        assert!(matches!(result, CorsResult::Allowed));

        // No match
        let result = checker.check_response(
            "https://example.com",
            Some("https://other.com"),
            None,
            false
        );
        assert!(matches!(result, CorsResult::Denied(_)));
    }

    #[test]
    fn test_referrer_policy() {
        let referrer = Url::parse("https://example.com/page?secret=123").unwrap();
        let target = Url::parse("https://other.com/").unwrap();

        // Origin only
        let policy = ReferrerPolicy::Origin;
        assert_eq!(
            policy.compute_referrer(&referrer, &target),
            Some("https://example.com/".to_string())
        );

        // No referrer
        let policy = ReferrerPolicy::NoReferrer;
        assert_eq!(policy.compute_referrer(&referrer, &target), None);

        // Same origin (cross-origin request)
        let policy = ReferrerPolicy::SameOrigin;
        assert_eq!(policy.compute_referrer(&referrer, &target), None);
    }

    /// Chrome's default policy, strict-origin-when-cross-origin, case by case.
    #[test]
    fn strict_origin_when_cross_origin_sends_no_more_than_the_policy_allows() {
        let policy = ReferrerPolicy::default();
        assert_eq!(policy, ReferrerPolicy::StrictOriginWhenCrossOrigin);
        let doc = Url::parse("https://user:pw@www.apple.com/mac/?q=1#specs").unwrap();

        // Same-origin: the full URL, without fragment or userinfo.
        let same = Url::parse("https://www.apple.com/wss/fonts?family=SF").unwrap();
        assert_eq!(
            policy.compute_referrer(&doc, &same).as_deref(),
            Some("https://www.apple.com/mac/?q=1")
        );
        // Cross-origin: the origin only (a different host, scheme or port).
        for target in ["https://cdn.apple.com/x.css", "https://www.apple.com:8443/x"] {
            let target = Url::parse(target).unwrap();
            assert_eq!(
                policy.compute_referrer(&doc, &target).as_deref(),
                Some("https://www.apple.com/")
            );
        }
        // https -> http downgrade: nothing, same host or not.
        for target in ["http://www.apple.com/x.css", "http://cdn.example/x.js"] {
            let target = Url::parse(target).unwrap();
            assert_eq!(policy.compute_referrer(&doc, &target), None);
        }
        // A non-http(s) document has no referrer.
        let file = Url::parse("file:///Users/me/secret.html").unwrap();
        let target = Url::parse("https://example.com/a.css").unwrap();
        assert_eq!(policy.compute_referrer(&file, &target), None);
        // Userinfo and fragment are stripped under every policy.
        assert_eq!(
            ReferrerPolicy::UnsafeUrl.compute_referrer(&doc, &target).as_deref(),
            Some("https://www.apple.com/mac/?q=1")
        );
    }

    #[test]
    fn referrer_policy_header_takes_the_last_recognised_token() {
        assert_eq!(
            ReferrerPolicy::parse_header("no-referrer, strict-origin-when-cross-origin"),
            Some(ReferrerPolicy::StrictOriginWhenCrossOrigin)
        );
        assert_eq!(
            ReferrerPolicy::parse_header("same-origin, made-up-policy"),
            Some(ReferrerPolicy::SameOrigin)
        );
        assert_eq!(ReferrerPolicy::parse_header("  Origin "), Some(ReferrerPolicy::Origin));
        assert_eq!(ReferrerPolicy::parse_header("bogus"), None);
        assert_eq!(ReferrerPolicy::parse_header(""), None);
    }

    #[test]
    fn test_cookie_same_site() {
        let url = Url::parse("https://example.com/page").unwrap();

        let cookie = CookieAttributes {
            name: "test".into(),
            value: "value".into(),
            same_site: SameSite::Strict,
            secure: true,
            ..Default::default()
        };

        // Same-site - allowed
        assert!(cookie.should_send(&url, true, false));

        // Cross-site - denied
        assert!(!cookie.should_send(&url, false, false));
    }

    #[test]
    fn test_mixed_content() {
        let https_page = Url::parse("https://example.com/").unwrap();
        let http_resource = Url::parse("http://example.com/image.png").unwrap();
        let https_resource = Url::parse("https://example.com/script.js").unwrap();

        // HTTPS resource on HTTPS page - allowed
        assert_eq!(
            check_mixed_content(&https_page, &https_resource, MixedContentType::Script),
            MixedContentResult::Allowed
        );

        // HTTP image on HTTPS page - optionally blockable
        assert_eq!(
            check_mixed_content(&https_page, &http_resource, MixedContentType::Image),
            MixedContentResult::OptionallyBlockable
        );

        // HTTP script on HTTPS page - blockable
        assert_eq!(
            check_mixed_content(&https_page, &http_resource, MixedContentType::Script),
            MixedContentResult::Blockable
        );
    }

    #[test]
    fn test_sandbox_flags() {
        let flags = SandboxFlags::parse("allow-scripts allow-forms");
        assert!(flags.allow_scripts);
        assert!(flags.allow_forms);
        assert!(!flags.allow_popups);
    }

    #[test]
    fn test_security_context() {
        let url = Url::parse("https://example.com/").unwrap();
        let ctx = SecurityContext::from_url(&url);

        assert!(ctx.is_secure_context);
        assert!(ctx.is_same_origin(&url));
        assert!(!ctx.is_same_origin(&Url::parse("https://other.com/").unwrap()));
    }
}

