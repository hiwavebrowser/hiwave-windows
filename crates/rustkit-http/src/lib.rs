//! # RustKit HTTP
//!
//! Minimal HTTP/1.1 client for the RustKit browser engine.
//!
//! This crate provides a simple async HTTP client, eliminating the need for
//! reqwest and its transitive dependencies. TLS is rustls with a
//! browser-typical client profile (ALPN h2+http/1.1 advertised, negotiated
//! http/1.1 fallback) — the network-lane change measured in exchange #276.
//! The previous native-tls stack remains available for one release behind
//! the `native-tls` feature as a rollback path (Atlas #535).

use std::io::{self, Write};
use std::time::Duration;

use bytes::Bytes;
use http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, Version};
use std::sync::Arc;
use tokio_rustls::rustls::pki_types::ServerName;
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::time::timeout;
use tokio_rustls::TlsConnector;
use tracing::{debug, trace, warn};
use url::Url;

mod addr;
pub use addr::{is_local_name, is_public_ip, AddressPolicy, Resolve, ResolveFuture, SystemResolver};

/// HTTP client errors.
#[derive(Error, Debug)]
pub enum HttpError {
    #[error("Invalid URL: {0}")]
    InvalidUrl(String),

    #[error("Connection failed: {0}")]
    ConnectionFailed(String),

    #[error("TLS error: {0}")]
    TlsError(String),

    #[error("Request timeout")]
    Timeout,

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("Invalid response: {0}")]
    InvalidResponse(String),

    #[error("Too many redirects")]
    TooManyRedirects,

    #[error("Unsupported scheme: {0}")]
    UnsupportedScheme(String),

    #[error("Address not permitted: {0}")]
    AddressDenied(String),

    #[error("Response body exceeds {0} bytes")]
    BodyTooLarge(usize),
}

/// HTTP response.
#[derive(Debug)]
pub struct Response {
    /// HTTP status code.
    pub status: StatusCode,
    /// HTTP version.
    pub version: Version,
    /// Response headers.
    pub headers: HeaderMap,
    /// Response body.
    pub body: Bytes,
    /// Final URL (after redirects).
    pub url: Url,
}

impl Response {
    /// Get a header value as a string.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|v| v.to_str().ok())
    }

    /// Get content-length from headers.
    pub fn content_length(&self) -> Option<u64> {
        self.header("content-length").and_then(|s| s.parse().ok())
    }

    /// Get content-type from headers.
    pub fn content_type(&self) -> Option<&str> {
        self.header("content-type")
    }

    /// Check if response is success (2xx).
    pub fn is_success(&self) -> bool {
        self.status.is_success()
    }

    /// Get body as text.
    pub fn text(&self) -> Result<String, std::string::FromUtf8Error> {
        String::from_utf8(self.body.to_vec())
    }
}

/// The honest HiWave user agent: real product, real engine, real platform.
///
/// NEVER Chrome's UA — the network lane's constitution. The Mozilla/5.0
/// prefix is the universal compatibility token every shipping browser keeps;
/// everything after it says exactly what we are. amazon's WAF measurably
/// scores UA/TLS coherence (diagnosis, exchange #276), so this string ships
/// in the same change as the rustls profile, never separately.
pub fn default_user_agent() -> String {
    #[cfg(target_os = "macos")]
    let platform = "Macintosh; Intel Mac OS X 10_15_7";
    #[cfg(target_os = "windows")]
    let platform = "Windows NT 10.0; Win64; x64";
    #[cfg(all(unix, not(target_os = "macos")))]
    let platform = "X11; Linux x86_64";
    format!("Mozilla/5.0 ({platform}) HiWave/1.0 RustKit/1.0")
}


/// The platform root store, loaded ONCE per process.
///
/// `rustls_native_certs::load_native_certs` walks the macOS keychain's trust
/// settings and costs SECONDS there. Loaded per `Client` (as #346 shipped
/// it) it added ~5 s to every engine start — measured by the trench as the
/// real-site board falling 16/30 -> 7/30 when develop picked #346 up, every
/// lost site a 30 s-budget timeout, not a block. Linux reads a bundle file
/// in milliseconds, which is why the author's probes never saw it: the
/// platform-verification gap the network lane declared on day one, now with
/// its first scar. Approach and measurements from Atlas's
/// rs-tls-roots-once (307fc8e), rebuilt here against post-#355 develop —
/// #355 already removed the second (per-connection) load site.
#[cfg(not(feature = "native-tls"))]
fn platform_roots() -> Result<Arc<tokio_rustls::rustls::RootCertStore>, HttpError> {
    static ROOTS: RootsCache = std::sync::Mutex::new(None);
    roots_cached(&ROOTS, || {
        let mut roots = tokio_rustls::rustls::RootCertStore::empty();
        let loaded = rustls_native_certs::load_native_certs();
        let mut rejected = 0usize;
        for cert in loaded.certs {
            // A single unparseable platform cert must not kill the store.
            if roots.add(cert).is_err() {
                rejected += 1;
            }
        }
        let mut diagnostics = String::new();
        if !loaded.errors.is_empty() || rejected > 0 {
            diagnostics = format!(
                "{} load errors, {} rejected certs, accepted {}",
                loaded.errors.len(),
                rejected,
                roots.len()
            );
            if let Some(first) = loaded.errors.first() {
                diagnostics.push_str(&format!(", first error: {first}"));
            }
            warn!(target: "rustkit_http::roots", "platform root load: {diagnostics}");
        }
        (roots, diagnostics)
    })
}

type RootsCache = std::sync::Mutex<Option<Arc<tokio_rustls::rustls::RootCertStore>>>;

fn roots_cached(
    cache: &RootsCache,
    load: impl FnOnce() -> (tokio_rustls::rustls::RootCertStore, String),
) -> Result<Arc<tokio_rustls::rustls::RootCertStore>, HttpError> {
    let mut slot = cache.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(roots) = slot.as_ref() {
        return Ok(roots.clone());
    }
    // Only a usable store is cached: a transient platform failure (keychain
    // busy under load) must not turn every later handshake into an error.
    let (roots, diagnostics) = load();
    let roots = Arc::new(roots);
    if roots.is_empty() {
        return Err(HttpError::TlsError(if diagnostics.is_empty() {
            "no usable platform root certificates".into()
        } else {
            format!("no usable platform root certificates ({diagnostics})")
        }));
    }
    *slot = Some(roots.clone());
    Ok(roots)
}

/// ALPN outcome of a TLS handshake.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NegotiatedProtocol {
    H2,
    Http1,
}

/// HTTP client configuration.
#[derive(Clone)]
pub struct ClientConfig2 {
    /// User agent string.
    pub user_agent: String,
    /// Default request timeout.
    pub timeout: Duration,
    /// Maximum number of redirects to follow.
    pub max_redirects: usize,
    /// Whether to follow redirects.
    pub follow_redirects: bool,
}

impl Default for ClientConfig2 {
    fn default() -> Self {
        Self {
            user_agent: default_user_agent(),
            timeout: Duration::from_secs(30),
            max_redirects: 10,
            follow_redirects: true,
        }
    }
}

/// HTTP client.
#[derive(Clone)]
pub struct Client {
    config: ClientConfig2,
    address_policy: AddressPolicy,
    resolver: Arc<dyn Resolve>,
    max_body: Option<usize>,
    #[cfg(not(feature = "native-tls"))]
    tls_connector: TlsConnector,
    #[cfg(feature = "native-tls")]
    tls_connector: tokio_native_tls::TlsConnector,
}

impl Client {
    /// Create a new HTTP client with default configuration.
    pub fn new() -> Result<Self, HttpError> {
        Self::with_config(ClientConfig2::default())
    }

    /// Create a new HTTP client with custom configuration.
    /// Rollback constructor (Atlas #535): the pre-network-lane native-tls
    /// handshake, byte-for-byte the old behaviour. One release only.
    #[cfg(feature = "native-tls")]
    pub fn with_config(config: ClientConfig2) -> Result<Self, HttpError> {
        let native_connector = native_tls::TlsConnector::new()
            .map_err(|e| HttpError::TlsError(e.to_string()))?;
        let tls_connector = tokio_native_tls::TlsConnector::from(native_connector);
        Ok(Self {
            config,
            address_policy: AddressPolicy::default(),
            resolver: Arc::new(SystemResolver),
            max_body: None,
            tls_connector,
        })
    }

    #[cfg(feature = "native-tls")]
    async fn connect_tls(
        &self,
        host: &str,
        _addr: &str,
        stream: tokio::net::TcpStream,
    ) -> Result<(tokio_native_tls::TlsStream<tokio::net::TcpStream>, NegotiatedProtocol), HttpError>
    {
        // Rollback stack: no ALPN configured, identical to pre-lane behavior.
        let tls = self
            .tls_connector
            .connect(host, stream)
            .await
            .map_err(|e| HttpError::TlsError(e.to_string()))?;
        Ok((tls, NegotiatedProtocol::Http1))
    }

    #[cfg(not(feature = "native-tls"))]
    pub fn with_config(config: ClientConfig2) -> Result<Self, HttpError> {
        // rustls with a browser-typical client profile (network lane).
        //
        // MEASURED (2026-09-28 diagnosis, exchange #276): Cloudflare, Akamai
        // and DataDome default-deny known-library TLS ClientHellos at request
        // one; header shape and even real HTTP/2 do not flip the verdict, and
        // amazon actively punishes browser-claiming headers that ride a
        // library fingerprint. So the TLS layer is where coherence starts.
        // The old native-tls connector was built with `::new()` and sent NO
        // ALPN extension at all — an immediate tell.
        //
        // This is a WELL-FORMED MODERN CLIENT, not a Chrome imitation: rustls
        // already produces a contemporary extension set (X25519/P-256 key
        // shares, TLS 1.3 + 1.2, session tickets); we advertise ALPN
        // h2+http/1.1 like every current browser. If the peer selects h2 we
        // currently keep speaking HTTP/1.1 only when the peer permits it —
        // see `connect_tls`, which records the negotiated protocol so the
        // caller can refuse mismatches loudly instead of desyncing.
        let roots = platform_roots()?;

        // When multiple crypto providers (e.g. aws-lc-rs from rustkit-http and ring from
        // reqwest in the workspace) are active, rustls requires an explicit default provider
        // installed to avoid panicking on ClientConfig::builder().
        let _ = tokio_rustls::rustls::crypto::aws_lc_rs::default_provider().install_default();

        let mut tls_config = tokio_rustls::rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        // Browser-typical ALPN advertisement. http/1.1 first would be a lie
        // about preference; browsers prefer h2. Until this client SPEAKS h2,
        // connect_tls() falls back to a second, http/1.1-only handshake when
        // the peer selects h2 — an honest downgrade the peer agrees to, not
        // a silent protocol desync.
        tls_config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];

        let tls_connector = TlsConnector::from(Arc::new(tls_config));

        Ok(Self {
            config,
            address_policy: AddressPolicy::default(),
            resolver: Arc::new(SystemResolver),
            max_body: None,
            tls_connector,
        })
    }

    /// Open a TLS connection and report the ALPN-negotiated protocol.
    ///
    /// PR 1 of the network lane advertised h2 but re-handshook http/1.1-only
    /// when the peer selected it — disclosed as an odd, costly pattern.
    /// PR 2 removes it: the caller now SPEAKS whichever protocol was
    /// negotiated, h2 included, over this single handshake.
    #[cfg(not(feature = "native-tls"))]
    async fn connect_tls(
        &self,
        host: &str,
        _addr: &str,
        stream: tokio::net::TcpStream,
    ) -> Result<(tokio_rustls::client::TlsStream<tokio::net::TcpStream>, NegotiatedProtocol), HttpError>
    {
        let server_name = ServerName::try_from(host.to_string())
            .map_err(|e| HttpError::TlsError(format!("invalid server name: {e}")))?;

        let tls_stream = self
            .tls_connector
            .connect(server_name, stream)
            .await
            .map_err(|e| HttpError::TlsError(e.to_string()))?;

        let negotiated = match tls_stream.get_ref().1.alpn_protocol() {
            Some(b"h2") => NegotiatedProtocol::H2,
            _ => NegotiatedProtocol::Http1,
        };
        Ok((tls_stream, negotiated))
    }

    /// Create a client builder.
    pub fn builder() -> ClientBuilder {
        ClientBuilder::new()
    }

    /// The same client restricted to the given resolved-address policy.
    pub fn with_address_policy(mut self, policy: AddressPolicy) -> Self {
        self.address_policy = policy;
        self
    }

    /// The same client with a different name resolver (tests, DoH later).
    pub fn with_resolver(mut self, resolver: Arc<dyn Resolve>) -> Self {
        self.resolver = resolver;
        self
    }

    /// The same client, following redirects or not. With `false` a 3xx comes
    /// back as the response (status, `Location` and all) for the caller to
    /// vet and follow itself, hop by hop.
    pub fn with_follow_redirects(mut self, follow: bool) -> Self {
        self.config.follow_redirects = follow;
        self
    }

    /// The same client, refusing any response body (after decoding) larger
    /// than `max` bytes with [`HttpError::BodyTooLarge`].
    pub fn with_max_body(mut self, max: usize) -> Self {
        self.max_body = Some(max);
        self
    }

    /// Resolve `host`, vet every address, connect to a vetted one.
    async fn connect(&self, host: &str, port: u16) -> Result<TcpStream, HttpError> {
        addr::connect_vetted(&*self.resolver, &self.address_policy, host, port).await
    }

    /// Perform a GET request.
    pub async fn get(&self, url: &str) -> Result<Response, HttpError> {
        self.request(Method::GET, url, HeaderMap::new(), None).await
    }

    /// Perform a POST request.
    pub async fn post(&self, url: &str, body: Bytes) -> Result<Response, HttpError> {
        self.request(Method::POST, url, HeaderMap::new(), Some(body))
            .await
    }

    /// Perform an HTTP request.
    pub async fn request(
        &self,
        method: Method,
        url: &str,
        headers: HeaderMap,
        body: Option<Bytes>,
    ) -> Result<Response, HttpError> {
        let parsed_url = Url::parse(url).map_err(|e| HttpError::InvalidUrl(e.to_string()))?;
        self.request_url(method, parsed_url, headers, body, 0).await
    }

    /// Internal request implementation with redirect counting.
    async fn request_url(
        &self,
        method: Method,
        url: Url,
        headers: HeaderMap,
        body: Option<Bytes>,
        redirect_count: usize,
    ) -> Result<Response, HttpError> {
        if redirect_count > self.config.max_redirects {
            return Err(HttpError::TooManyRedirects);
        }

        let scheme = url.scheme();
        let host = url
            .host_str()
            .ok_or_else(|| HttpError::InvalidUrl("Missing host".to_string()))?;
        let port = url.port_or_known_default().unwrap_or(if scheme == "https" {
            443
        } else {
            80
        });

        debug!(method = %method, url = %url, "HTTP request");

        // Connect with timeout
        let response = timeout(self.config.timeout, async {
            match scheme {
                "https" => self.request_https(host, port, &method, &url, &headers, &body).await,
                "http" => self.request_http(host, port, &method, &url, &headers, &body).await,
                _ => Err(HttpError::UnsupportedScheme(scheme.to_string())),
            }
        })
        .await
        .map_err(|_| HttpError::Timeout)??;

        // Handle redirects
        if self.config.follow_redirects && response.status.is_redirection() {
            if let Some(location) = response.header("location") {
                let redirect_url = url
                    .join(location)
                    .map_err(|e| HttpError::InvalidUrl(e.to_string()))?;
                debug!(from = %url, to = %redirect_url, "Following redirect");
                return Box::pin(self.request_url(Method::GET, redirect_url, HeaderMap::new(), None, redirect_count + 1))
                    .await;
            }
        }

        Ok(Response {
            status: response.status,
            version: response.version,
            headers: response.headers,
            body: response.body,
            url,
        })
    }

    /// HTTPS request.
    async fn request_https(
        &self,
        host: &str,
        port: u16,
        method: &Method,
        url: &Url,
        headers: &HeaderMap,
        body: &Option<Bytes>,
    ) -> Result<RawResponse, HttpError> {
        let addr = format!("{}:{}", host, port);
        let stream = self.connect(host, port).await?;

        let (tls_stream, negotiated) = self.connect_tls(host, &addr, stream).await?;

        match negotiated {
            NegotiatedProtocol::H2 => {
                self.send_request_h2(tls_stream, method, url, headers, body)
                    .await
            }
            NegotiatedProtocol::Http1 => {
                self.send_request(tls_stream, host, method, url, headers, body)
                    .await
            }
        }
    }

    /// Send one request over a freshly negotiated HTTP/2 connection.
    ///
    /// Real h2 (the `h2` crate over the rustls stream), not a facade: HPACK,
    /// flow control (capacity released as body chunks arrive), server
    /// half-close honored. One request per connection for now — matching the
    /// existing h1 path, which also reconnects per request; connection reuse
    /// is a lane follow-up for BOTH protocols, not an h2 regression.
    ///
    /// Connection-specific headers (Connection family) are h2-ILLEGAL and are
    /// not sent; the browser-shaped known set from the h1 emission carries
    /// over minus those, callers' headers after, all lowercase per RFC 9113.
    async fn send_request_h2<S>(
        &self,
        stream: S,
        method: &Method,
        url: &Url,
        headers: &HeaderMap,
        body: &Option<Bytes>,
    ) -> Result<RawResponse, HttpError>
    where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
    {
        let (mut send_request, connection) = h2::client::handshake(stream)
            .await
            .map_err(|e| HttpError::ConnectionFailed(format!("h2 handshake: {e}")))?;

        // Drive the connection; ends when the request completes and both
        // sides close. JoinHandle dropped deliberately: the task owns nothing
        // beyond the connection it is draining.
        tokio::spawn(async move {
            let _ = connection.await;
        });

        let mut request = http::Request::builder()
            .method(method.clone())
            .uri(url.as_str())
            .version(http::Version::HTTP_2);

        // Browser-shaped known set, minus h2-illegal connection headers.
        for (name, value) in h2_request_headers(&self.config.user_agent, headers) {
            request = request.header(name, value);
        }

        let request = request
            .body(())
            .map_err(|e| HttpError::InvalidResponse(format!("h2 request build: {e}")))?;

        let has_body = body.is_some();
        let (response_fut, mut send_stream) = send_request
            .send_request(request, !has_body)
            .map_err(|e| HttpError::ConnectionFailed(format!("h2 send: {e}")))?;
        if let Some(b) = body {
            send_stream
                .send_data(b.clone(), true)
                .map_err(|e| HttpError::ConnectionFailed(format!("h2 body: {e}")))?;
        }

        let response = response_fut
            .await
            .map_err(|e| HttpError::InvalidResponse(format!("h2 response: {e}")))?;
        let status = response.status();
        let mut response_headers = HeaderMap::new();
        for (name, value) in response.headers() {
            response_headers.insert(name.clone(), value.clone());
        }

        let mut recv = response.into_body();
        let mut collected: Vec<u8> = Vec::new();
        while let Some(chunk) = recv.data().await {
            let chunk = chunk.map_err(|e| HttpError::InvalidResponse(format!("h2 body read: {e}")))?;
            collected.extend_from_slice(&chunk);
            if self.max_body.is_some_and(|max| collected.len() > max) {
                return Err(HttpError::BodyTooLarge(self.max_body.unwrap_or(0)));
            }
            // Flow control: hand the window back or the peer stalls at 64KB.
            let _ = recv.flow_control().release_capacity(chunk.len());
        }

        let body = decode_content_encoding_capped(Bytes::from(collected), &mut response_headers, self.max_body)?;

        Ok(RawResponse {
            status,
            version: Version::HTTP_2,
            headers: response_headers,
            body,
        })
    }

    /// HTTP request.
    async fn request_http(
        &self,
        host: &str,
        port: u16,
        method: &Method,
        url: &Url,
        headers: &HeaderMap,
        body: &Option<Bytes>,
    ) -> Result<RawResponse, HttpError> {
        let stream = self.connect(host, port).await?;

        self.send_request(stream, host, method, url, headers, body)
            .await
    }

    /// Send HTTP request and read response.
    async fn send_request<S>(
        &self,
        stream: S,
        host: &str,
        method: &Method,
        url: &Url,
        headers: &HeaderMap,
        body: &Option<Bytes>,
    ) -> Result<RawResponse, HttpError>
    where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    {
        let (reader, mut writer) = tokio::io::split(stream);
        let mut reader = BufReader::new(reader);

        // Build request
        let path = if let Some(query) = url.query() {
            format!("{}?{}", url.path(), query)
        } else {
            url.path().to_string()
        };
        let path = if path.is_empty() { "/" } else { &path };

        // BROWSER-SHAPED EMISSION (network lane). The old block wrote five
        // Title-Case headers, `Accept: */*`, `Connection: close`, then every
        // caller header in lowercase after them — three tells in one block
        // (mixed casing, close-on-navigate, wildcard Accept). MEASURED
        // (#276): header shape alone does not unblock any WAF vendor, but
        // amazon scores header/TLS COHERENCE, so the shape ships together
        // with the rustls profile as one coherent client identity.
        //
        // Order and casing follow shipping browsers' HTTP/1.1 form. Caller
        // headers override any default; the ordered known set is emitted
        // first, remaining caller headers after, all in canonical casing.
        let mut request = Vec::new();
        writeln!(request, "{} {} HTTP/1.1\r", method, path)?;
        let host_header = headers.get("host").and_then(|v| v.to_str().ok()).unwrap_or(host);
        writeln!(request, "Host: {}\r", host_header)?;
        writeln!(request, "Connection: keep-alive\r")?;
        writeln!(request, "User-Agent: {}\r", self.config.user_agent)?;

        let canonical = |name: &str| -> String {
            // HTTP/1.1 browser casing: Title-Case per hyphenated segment,
            // with the Sec-* and *-CH-* families' internal caps preserved
            // by the segment rule itself.
            name.split('-')
                .map(|seg| {
                    let mut c = seg.chars();
                    match c.next() {
                        Some(f) => f.to_ascii_uppercase().to_string() + c.as_str(),
                        None => String::new(),
                    }
                })
                .collect::<Vec<_>>()
                .join("-")
        };

        // Known headers in browser order, caller value winning over default.
        const ORDERED: &[(&str, Option<&str>)] = &[
            (
                "accept",
                Some(
                    "text/html,application/xhtml+xml,application/xml;q=0.9,\
image/avif,image/webp,*/*;q=0.8",
                ),
            ),
            ("accept-language", None),
            ("accept-encoding", Some(ACCEPT_ENCODING)),
            ("upgrade-insecure-requests", Some("1")),
            ("sec-fetch-dest", Some("document")),
            ("sec-fetch-mode", Some("navigate")),
            ("sec-fetch-site", Some("none")),
            ("sec-fetch-user", Some("?1")),
            ("referer", None),
            ("cookie", None),
        ];
        let mut written: Vec<&str> = vec![];
        for (name, default) in ORDERED {
            let value = headers
                .get(*name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
                .or_else(|| default.map(str::to_string));
            if let Some(v) = value {
                writeln!(request, "{}: {}\r", canonical(name), v)?;
                written.push(name);
            }
        }

        // Remaining caller headers, canonical casing, after the known set.
        for (name, value) in headers.iter() {
            if name.as_str() == "host" || written.contains(&name.as_str()) {
                continue;
            }
            if let Ok(v) = value.to_str() {
                writeln!(request, "{}: {}\r", canonical(name.as_str()), v)?;
            }
        }

        // Content-Length for body
        if let Some(b) = body {
            writeln!(request, "Content-Length: {}\r", b.len())?;
        }

        writeln!(request, "\r")?;

        // Send headers
        writer.write_all(&request).await?;

        // Send body
        if let Some(b) = body {
            writer.write_all(b).await?;
        }

        writer.flush().await?;

        // Read response status line
        let mut status_line = String::new();
        reader.read_line(&mut status_line).await?;

        let (version, status) = parse_status_line(&status_line)?;

        // Read headers
        let mut response_headers = HeaderMap::new();
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).await?;
            let line = line.trim();
            if line.is_empty() {
                break;
            }

            if let Some((name, value)) = line.split_once(':') {
                if let (Ok(n), Ok(v)) = (
                    HeaderName::try_from(name.trim()),
                    HeaderValue::try_from(value.trim()),
                ) {
                    response_headers.insert(n, v);
                }
            }
        }

        // Read body
        let body = read_body(&mut reader, &response_headers, self.max_body).await?;
        let body = decode_content_encoding_capped(body, &mut response_headers, self.max_body)?;

        trace!(status = %status, body_len = body.len(), "Response received");

        Ok(RawResponse {
            status,
            version,
            headers: response_headers,
            body,
        })
    }
}

impl Default for Client {
    fn default() -> Self {
        Self::new().expect("Failed to create default HTTP client")
    }
}

/// Raw response (before redirect handling).
struct RawResponse {
    status: StatusCode,
    version: Version,
    headers: HeaderMap,
    body: Bytes,
}

impl RawResponse {
    /// Get a header value as a string.
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|v| v.to_str().ok())
    }
}

/// Client builder for configuring HTTP client.
pub struct ClientBuilder {
    config: ClientConfig2,
}

impl ClientBuilder {
    /// Create a new builder.
    pub fn new() -> Self {
        Self {
            config: ClientConfig2::default(),
        }
    }

    /// Set user agent.
    pub fn user_agent(mut self, user_agent: &str) -> Self {
        self.config.user_agent = user_agent.to_string();
        self
    }

    /// Set timeout.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.config.timeout = timeout;
        self
    }

    /// Set redirect policy.
    pub fn redirect(mut self, follow: bool, max: usize) -> Self {
        self.config.follow_redirects = follow;
        self.config.max_redirects = max;
        self
    }

    /// Placeholder for cookie_store (not implemented in minimal client).
    pub fn cookie_store(self, _enabled: bool) -> Self {
        // Cookie support would require additional implementation
        self
    }

    /// Build the client.
    pub fn build(self) -> Result<Client, HttpError> {
        Client::with_config(self.config)
    }
}

impl Default for ClientBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Parse HTTP status line.
fn parse_status_line(line: &str) -> Result<(Version, StatusCode), HttpError> {
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() < 2 {
        return Err(HttpError::InvalidResponse("Invalid status line".to_string()));
    }

    let version = match parts[0] {
        "HTTP/1.0" => Version::HTTP_10,
        "HTTP/1.1" => Version::HTTP_11,
        "HTTP/2" | "HTTP/2.0" => Version::HTTP_2,
        _ => Version::HTTP_11,
    };

    let status_code: u16 = parts[1]
        .parse()
        .map_err(|_| HttpError::InvalidResponse("Invalid status code".to_string()))?;

    let status = StatusCode::from_u16(status_code)
        .map_err(|_| HttpError::InvalidResponse("Invalid status code".to_string()))?;

    Ok((version, status))
}

/// The `Accept-Encoding` sent on requests: what `decode_content_encoding`
/// can undo.
const ACCEPT_ENCODING: &str = "gzip, deflate";

/// Connection-specific header fields (RFC 9113 §8.1.2) plus `host`
/// (carried by `:authority`). Must not appear on an HTTP/2 request.
fn is_h2_illegal_request_header(name: &str) -> bool {
    matches!(
        name,
        "connection"
            | "keep-alive"
            | "proxy-connection"
            | "transfer-encoding"
            | "upgrade"
            | "host"
    )
}

/// Header pairs `send_request_h2` writes onto the request, in emission order.
/// Caller headers that are connection-specific, already emitted, or the
/// user-agent (set from config) are dropped.
fn h2_request_headers(user_agent: &str, headers: &HeaderMap) -> Vec<(HeaderName, HeaderValue)> {
    const ORDERED_H2: &[(&str, Option<&str>)] = &[
        (
            "accept",
            Some(
                "text/html,application/xhtml+xml,application/xml;q=0.9,\
image/avif,image/webp,*/*;q=0.8",
            ),
        ),
        ("accept-language", None),
        ("accept-encoding", Some(ACCEPT_ENCODING)),
        ("upgrade-insecure-requests", Some("1")),
        ("sec-fetch-dest", Some("document")),
        ("sec-fetch-mode", Some("navigate")),
        ("sec-fetch-site", Some("none")),
        ("sec-fetch-user", Some("?1")),
        ("referer", None),
        ("cookie", None),
    ];

    let mut out = Vec::new();
    out.push((
        HeaderName::from_static("user-agent"),
        HeaderValue::from_str(user_agent).unwrap_or_else(|_| HeaderValue::from_static("")),
    ));
    let mut written: Vec<&str> = vec![];
    for (name, default) in ORDERED_H2 {
        // Same as before the extract: a non-UTF-8 caller value falls through
        // to the default (or is omitted when there is none).
        let value = headers
            .get(*name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
            .or_else(|| default.map(str::to_string));
        if let Some(v) = value {
            out.push((
                HeaderName::from_static(*name),
                HeaderValue::from_str(&v).unwrap_or_else(|_| HeaderValue::from_static("")),
            ));
            written.push(name);
        }
    }
    for (name, value) in headers.iter() {
        let n = name.as_str();
        if written.contains(&n) || is_h2_illegal_request_header(n) || n == "user-agent" {
            continue;
        }
        out.push((name.clone(), value.clone()));
    }
    out
}

/// Undo the response's `Content-Encoding`, so callers always see the
/// resource's bytes. A decoded body drops `Content-Encoding` and
/// `Content-Length` (which described the encoded bytes).
///
/// A body cut off mid-stream keeps what decoded, as a truncated chunked
/// body does. An encoding we never advertised is passed through untouched.
#[cfg(test)]
fn decode_content_encoding(body: Bytes, headers: &mut HeaderMap) -> Result<Bytes, HttpError> {
    decode_content_encoding_capped(body, headers, None)
}

/// As [`decode_content_encoding`], refusing to inflate past `max` bytes (a
/// small compressed body must not become an unbounded allocation).
fn decode_content_encoding_capped(
    body: Bytes,
    headers: &mut HeaderMap,
    max: Option<usize>,
) -> Result<Bytes, HttpError> {
    use std::io::Read;
    let limit = max.map_or(u64::MAX, |m| m as u64 + 1);

    let Some(encoding) = headers
        .get("content-encoding")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim().to_ascii_lowercase())
    else {
        return Ok(body);
    };
    if body.is_empty() || encoding.is_empty() || encoding == "identity" {
        return Ok(body);
    }

    let mut out = Vec::new();
    let result = match encoding.as_str() {
        "gzip" | "x-gzip" => flate2::read::MultiGzDecoder::new(&body[..]).take(limit).read_to_end(&mut out),
        // "deflate" is zlib-wrapped per RFC 9110, but some servers send raw
        // deflate; browsers accept both.
        "deflate" => match flate2::read::ZlibDecoder::new(&body[..]).take(limit).read_to_end(&mut out) {
            Ok(n) => Ok(n),
            Err(_) if out.is_empty() => {
                flate2::read::DeflateDecoder::new(&body[..]).take(limit).read_to_end(&mut out)
            }
            Err(e) => Err(e),
        },
        other => {
            warn!(encoding = other, "Unsupported Content-Encoding; body left encoded");
            return Ok(body);
        }
    };
    if let Some(max) = max {
        if out.len() > max {
            return Err(HttpError::BodyTooLarge(max));
        }
    }
    if let Err(e) = result {
        if out.is_empty() {
            return Err(HttpError::InvalidResponse(format!(
                "Content-Encoding {encoding}: {e}"
            )));
        }
        warn!(%encoding, error = %e, decoded = out.len(), "Encoded body cut off; keeping what decoded");
    }
    headers.remove("content-encoding");
    headers.remove("content-length");
    Ok(Bytes::from(out))
}

/// Read response body based on headers.
async fn read_body<R: tokio::io::AsyncBufRead + Unpin>(
    reader: &mut R,
    headers: &HeaderMap,
    max: Option<usize>,
) -> Result<Bytes, HttpError> {
    // Check for Content-Length
    if let Some(len) = headers
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<usize>().ok())
    {
        if let Some(max) = max {
            if len > max {
                return Err(HttpError::BodyTooLarge(max));
            }
        }
        let mut buf = vec![0u8; len];
        reader.read_exact(&mut buf).await?;
        return Ok(Bytes::from(buf));
    }

    // Check for chunked transfer encoding
    if let Some(te) = headers.get("transfer-encoding").and_then(|v| v.to_str().ok()) {
        if te.to_lowercase().contains("chunked") {
            return read_chunked_body(reader, max).await;
        }
    }

    // Read until EOF
    let mut buf = Vec::new();
    match max {
        Some(max) => {
            (&mut *reader).take(max as u64 + 1).read_to_end(&mut buf).await?;
            if buf.len() > max {
                return Err(HttpError::BodyTooLarge(max));
            }
        }
        None => {
            reader.read_to_end(&mut buf).await?;
        }
    }
    Ok(Bytes::from(buf))
}

/// Read chunked transfer encoding body.
async fn read_chunked_body<R: tokio::io::AsyncBufRead + Unpin>(
    reader: &mut R,
    max: Option<usize>,
) -> Result<Bytes, HttpError> {
    let mut body = Vec::new();

    loop {
        let mut size_line = String::new();
        if reader.read_line(&mut size_line).await? == 0 {
            // The peer closed before the terminating 0-size chunk. Keep what
            // arrived, as Chrome does for a document: netflix.com's edge
            // intermittently closes at exactly 512 KiB of a ~660 KiB page,
            // and failing the whole navigation left a blank tab.
            warn!(received = body.len(), "chunked body truncated by EOF; using partial body");
            break;
        }

        // RFC 9112 §7.1.1: chunk-size may be followed by chunk extensions.
        let size_field = size_line.split(';').next().unwrap_or("").trim();
        let size = usize::from_str_radix(size_field, 16)
            .map_err(|_| HttpError::InvalidResponse("Invalid chunk size".to_string()))?;

        if size == 0 {
            // Read trailing CRLF
            let mut _trailer = String::new();
            let _ = reader.read_line(&mut _trailer).await;
            break;
        }

        if max.is_some_and(|max| body.len().saturating_add(size) > max) {
            return Err(HttpError::BodyTooLarge(max.unwrap_or(0)));
        }
        let mut chunk = Vec::with_capacity(size);
        (&mut *reader).take(size as u64).read_to_end(&mut chunk).await?;
        let complete = chunk.len() == size;
        body.extend_from_slice(&chunk);
        if !complete {
            warn!(received = body.len(), "chunked body truncated by EOF mid-chunk; using partial body");
            break;
        }

        // Read trailing CRLF after chunk
        let mut _crlf = [0u8; 2];
        let _ = reader.read_exact(&mut _crlf).await;
    }

    Ok(Bytes::from(body))
}

/// Streaming response for downloads.
pub struct StreamingResponse {
    /// HTTP status code.
    pub status: StatusCode,
    /// Response headers.
    pub headers: HeaderMap,
    /// Content length if known.
    pub content_length: Option<u64>,
    /// The underlying stream reader.
    reader: Box<dyn tokio::io::AsyncRead + Send + Unpin>,
}

impl StreamingResponse {
    /// Read a chunk of data.
    pub async fn chunk(&mut self, buf: &mut [u8]) -> Result<usize, HttpError> {
        use tokio::io::AsyncReadExt;
        let n = self.reader.read(buf).await?;
        Ok(n)
    }
}

/// Client extension for streaming downloads.
impl Client {
    /// Start a streaming GET request (for downloads).
    pub async fn get_streaming(&self, url: &str) -> Result<StreamingResponse, HttpError> {
        let parsed_url = Url::parse(url).map_err(|e| HttpError::InvalidUrl(e.to_string()))?;

        let scheme = parsed_url.scheme();
        let host = parsed_url
            .host_str()
            .ok_or_else(|| HttpError::InvalidUrl("Missing host".to_string()))?;
        let port = parsed_url.port_or_known_default().unwrap_or(if scheme == "https" {
            443
        } else {
            80
        });

        match scheme {
            "https" => self.streaming_https(host, port, &parsed_url).await,
            "http" => self.streaming_http(host, port, &parsed_url).await,
            _ => Err(HttpError::UnsupportedScheme(scheme.to_string())),
        }
    }

    async fn streaming_https(
        &self,
        host: &str,
        port: u16,
        url: &Url,
    ) -> Result<StreamingResponse, HttpError> {
        let addr = format!("{}:{}", host, port);
        let stream = self.connect(host, port).await?;

        // Streaming stays HTTP/1.1 in this PR: the streaming reader is a
        // BufRead line/chunk parser. When h2 is negotiated we buffer via the
        // h2 path and stream from memory — correct, just not incremental;
        // incremental h2 streaming is the follow-up.
        let (tls_stream, negotiated) = self.connect_tls(host, &addr, stream).await?;
        if negotiated == NegotiatedProtocol::H2 {
            let raw = self
                .send_request_h2(tls_stream, &Method::GET, url, &HeaderMap::new(), &None)
                .await?;
            let len = raw.body.len() as u64;
            return Ok(StreamingResponse {
                status: raw.status,
                headers: raw.headers,
                content_length: Some(len),
                reader: Box::new(std::io::Cursor::new(raw.body)),
            });
        }

        self.send_streaming_request(tls_stream, host, url).await
    }

    async fn streaming_http(
        &self,
        host: &str,
        port: u16,
        url: &Url,
    ) -> Result<StreamingResponse, HttpError> {
        let stream = self.connect(host, port).await?;

        self.send_streaming_request(stream, host, url).await
    }

    async fn send_streaming_request<S>(
        &self,
        mut stream: S,
        host: &str,
        url: &Url,
    ) -> Result<StreamingResponse, HttpError>
    where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
    {
        // Build and send request
        let path = if let Some(query) = url.query() {
            format!("{}?{}", url.path(), query)
        } else {
            url.path().to_string()
        };
        let path = if path.is_empty() { "/" } else { &path };

        let request = format!(
            "GET {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: {}\r\nAccept: */*\r\nConnection: close\r\n\r\n",
            path, host, self.config.user_agent
        );

        stream.write_all(request.as_bytes()).await?;
        stream.flush().await?;

        // Read status and headers
        let mut reader = BufReader::new(stream);

        let mut status_line = String::new();
        reader.read_line(&mut status_line).await?;
        let (_, status) = parse_status_line(&status_line)?;

        let mut headers = HeaderMap::new();
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).await?;
            let line = line.trim();
            if line.is_empty() {
                break;
            }

            if let Some((name, value)) = line.split_once(':') {
                if let (Ok(n), Ok(v)) = (
                    HeaderName::try_from(name.trim()),
                    HeaderValue::try_from(value.trim()),
                ) {
                    headers.insert(n, v);
                }
            }
        }

        let content_length = headers
            .get("content-length")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse().ok());

        Ok(StreamingResponse {
            status,
            headers,
            content_length,
            reader: Box::new(reader),
        })
    }
}

/// Blocking client for synchronous code (e.g., filter list downloads).
pub mod blocking {
    use super::*;

    /// Blocking HTTP client.
    pub struct Client {
        runtime: tokio::runtime::Runtime,
        inner: super::Client,
    }

    impl Client {
        /// Create a new blocking client with default config.
        pub fn new() -> Result<Self, HttpError> {
            Self::builder().build()
        }

        /// Create a client builder.
        pub fn builder() -> ClientBuilder {
            ClientBuilder {
                config: ClientConfig2::default(),
            }
        }

        /// Perform a blocking GET request.
        pub fn get(&self, url: &str) -> Result<Response, HttpError> {
            self.runtime.block_on(self.inner.get(url))
        }
    }

    /// Blocking client builder.
    pub struct ClientBuilder {
        config: ClientConfig2,
    }

    impl ClientBuilder {
        /// Set timeout.
        pub fn timeout(mut self, timeout: Duration) -> Self {
            self.config.timeout = timeout;
            self
        }

        /// Build the blocking client.
        pub fn build(self) -> Result<Client, HttpError> {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| HttpError::IoError(io::Error::other(e)))?;

            let inner = super::Client::with_config(self.config)?;

            Ok(Client { runtime, inner })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn h2_request_headers_strip_connection_specific_fields() {
        // RFC 9113 §8.1.2: these are illegal on h2. Before the extract they
        // lived only inside `send_request_h2`, so a regression could only be
        // caught by a live h2 negotiation.
        let mut headers = HeaderMap::new();
        headers.insert("connection", HeaderValue::from_static("keep-alive"));
        headers.insert("keep-alive", HeaderValue::from_static("timeout=5"));
        headers.insert("proxy-connection", HeaderValue::from_static("close"));
        headers.insert("transfer-encoding", HeaderValue::from_static("chunked"));
        headers.insert("upgrade", HeaderValue::from_static("h2c"));
        headers.insert("host", HeaderValue::from_static("evil.example"));
        headers.insert("x-custom", HeaderValue::from_static("ok"));
        headers.insert("referer", HeaderValue::from_static("https://doc.example/p"));

        let pairs = h2_request_headers("HiWave/test", &headers);
        let names: Vec<&str> = pairs.iter().map(|(n, _)| n.as_str()).collect();

        for illegal in [
            "connection",
            "keep-alive",
            "proxy-connection",
            "transfer-encoding",
            "upgrade",
            "host",
        ] {
            assert!(
                !names.contains(&illegal),
                "{illegal} must not appear on an h2 request: {names:?}"
            );
            assert!(is_h2_illegal_request_header(illegal));
        }

        assert_eq!(names[0], "user-agent");
        assert_eq!(pairs[0].1.to_str().unwrap(), "HiWave/test");
        assert!(names.contains(&"referer"));
        assert!(names.contains(&"x-custom"));
        assert!(names.contains(&"accept-encoding"));
        assert!(!is_h2_illegal_request_header("referer"));
        assert!(!is_h2_illegal_request_header("x-custom"));
    }

    #[test]
    fn test_parse_status_line() {
        let (version, status) = parse_status_line("HTTP/1.1 200 OK\r\n").unwrap();
        assert_eq!(version, Version::HTTP_11);
        assert_eq!(status, StatusCode::OK);

        let (version, status) = parse_status_line("HTTP/1.0 404 Not Found").unwrap();
        assert_eq!(version, Version::HTTP_10);
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[test]
    fn an_empty_platform_root_load_is_not_cached_for_the_life_of_the_process() {
        use tokio_rustls::rustls::pki_types::{Der, TrustAnchor};
        use tokio_rustls::rustls::RootCertStore;
        let one_anchor = || RootCertStore {
            roots: vec![TrustAnchor {
                subject: Der::from_slice(b"subject"),
                subject_public_key_info: Der::from_slice(b"spki"),
                name_constraints: None,
            }],
        };
        let cache: RootsCache = std::sync::Mutex::new(None);
        let mut loads = 0;
        assert!(
            roots_cached(&cache, || {
                loads += 1;
                (RootCertStore::empty(), String::new())
            })
            .is_err(),
            "an empty load is an error"
        );
        let got = roots_cached(&cache, || {
            loads += 1;
            (one_anchor(), String::new())
        });
        assert!(got.is_ok(), "a later successful load must be used, not the earlier empty one");
        assert_eq!(loads, 2);
        assert!(
            roots_cached(&cache, || {
                loads += 1;
                (RootCertStore::empty(), String::new())
            })
            .is_ok()
        );
        assert_eq!(loads, 2, "a good store is cached and not reloaded");
    }

    #[test]
    fn an_empty_platform_root_error_says_why() {
        use tokio_rustls::rustls::RootCertStore;
        let cache: RootsCache = std::sync::Mutex::new(None);
        let err = roots_cached(&cache, || {
            (RootCertStore::empty(), "2 load errors, first: keychain busy".to_string())
        })
        .unwrap_err()
        .to_string();
        assert!(err.contains("keychain busy"), "the platform's reason must reach the caller: {err}");
        assert!(err.contains("no usable platform root certificates"), "{err}");
    }

    #[test]
    fn test_client_builder() {
        let client = Client::builder()
            .user_agent("TestAgent/1.0")
            .timeout(Duration::from_secs(10))
            .redirect(true, 5)
            .build()
            .unwrap();

        assert_eq!(client.config.user_agent, "TestAgent/1.0");
        assert_eq!(client.config.timeout, Duration::from_secs(10));
        assert_eq!(client.config.max_redirects, 5);
    }

    #[test]
    fn test_response_helpers() {
        let mut headers = HeaderMap::new();
        headers.insert("content-type", HeaderValue::from_static("text/html"));
        headers.insert("content-length", HeaderValue::from_static("1234"));

        let response = Response {
            status: StatusCode::OK,
            version: Version::HTTP_11,
            headers,
            body: Bytes::from("Hello"),
            url: Url::parse("https://example.com").unwrap(),
        };

        assert!(response.is_success());
        assert_eq!(response.content_type(), Some("text/html"));
        assert_eq!(response.content_length(), Some(1234));
        assert_eq!(response.text().unwrap(), "Hello");
    }

    fn decode_chunked(raw: &[u8]) -> Result<Bytes, HttpError> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let mut reader = BufReader::new(raw);
        rt.block_on(read_chunked_body(&mut reader, None))
    }

    #[test]
    fn chunked_body_decodes_extensions_and_terminator() {
        let body = decode_chunked(b"5;name=val\r\nhello\r\n6\r\n world\r\n0\r\n\r\n").unwrap();
        assert_eq!(&body[..], b"hello world");
    }

    #[test]
    fn chunked_body_truncated_by_eof_keeps_what_arrived() {
        // EOF where the next size line should be (netflix.com's edge closes
        // at 512 KiB), and EOF inside a chunk: both used to fail the whole
        // navigation with "Invalid chunk size" / UnexpectedEof.
        let at_boundary = decode_chunked(b"5\r\nhello\r\n").unwrap();
        assert_eq!(&at_boundary[..], b"hello");
        let mid_chunk = decode_chunked(b"5\r\nhello\r\na\r\n wor").unwrap();
        assert_eq!(&mid_chunk[..], b"hello wor");
    }

    #[test]
    fn chunked_body_rejects_a_garbage_size_line() {
        assert!(matches!(
            decode_chunked(b"zz\r\nhello\r\n0\r\n\r\n"),
            Err(HttpError::InvalidResponse(_))
        ));
    }

    fn gzip(data: &[u8]) -> Vec<u8> {
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut enc, data).unwrap();
        enc.finish().unwrap()
    }

    #[test]
    fn requests_gzip_and_decodes_it() {
        // Serves gzip only to a client that asks for it, as real sites do;
        // anything else gets the page a bot gets.
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut buf = [0u8; 1024];
            while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                match stream.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => request.extend_from_slice(&buf[..n]),
                }
            }
            let request = String::from_utf8_lossy(&request).to_ascii_lowercase();
            if request.contains("\r\naccept-encoding: gzip") {
                let body = gzip(b"<p>the real page</p>");
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Encoding: gzip\r\nContent-Length: {}\r\n\r\n",
                    body.len()
                )
                .unwrap();
                stream.write_all(&body).unwrap();
            } else {
                stream
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 13\r\n\r\nautomated bot")
                    .unwrap();
            }
        });

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let client = Client::builder().build().unwrap();
        let response = rt
            .block_on(client.request(
                Method::GET,
                &format!("http://127.0.0.1:{port}/"),
                HeaderMap::new(),
                None,
            ))
            .unwrap();
        assert_eq!(response.text().unwrap(), "<p>the real page</p>");
        assert!(response.headers.get("content-encoding").is_none());
        assert!(response.headers.get("content-length").is_none());
    }

    #[test]
    fn requests_chunked_gzip_and_decodes_after_reassembling_chunks() {
        use std::io::{Read, Write};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut buf = [0u8; 1024];
            while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                match stream.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => request.extend_from_slice(&buf[..n]),
                }
            }

            let body = gzip(b"<p>chunked and compressed</p>");
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\
                      Content-Encoding: gzip\r\n\r\n",
                )
                .unwrap();
            for chunk in body.chunks(7) {
                write!(stream, "{:x};part=test\r\n", chunk.len()).unwrap();
                stream.write_all(chunk).unwrap();
                stream.write_all(b"\r\n").unwrap();
            }
            stream.write_all(b"0\r\n\r\n").unwrap();
        });

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let client = Client::builder().build().unwrap();
        let response = rt
            .block_on(client.request(
                Method::GET,
                &format!("http://127.0.0.1:{port}/"),
                HeaderMap::new(),
                None,
            ))
            .unwrap();

        server.join().unwrap();
        assert_eq!(response.text().unwrap(), "<p>chunked and compressed</p>");
        assert!(response.headers.get("content-encoding").is_none());
    }

    #[test]
    fn content_encoding_decodes_deflate_both_ways_and_keeps_a_truncated_prefix() {
        let decode = |encoding: &str, body: Vec<u8>| {
            let mut headers = HeaderMap::new();
            headers.insert("content-encoding", HeaderValue::from_str(encoding).unwrap());
            decode_content_encoding(Bytes::from(body), &mut headers)
        };
        let text = b"hello hello hello hello world".repeat(50);

        let mut zlib = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut zlib, &text).unwrap();
        assert_eq!(&decode("deflate", zlib.finish().unwrap()).unwrap()[..], &text[..]);

        let mut raw = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut raw, &text).unwrap();
        assert_eq!(&decode("deflate", raw.finish().unwrap()).unwrap()[..], &text[..]);

        let big: Vec<u8> = (0..200_000u32).flat_map(|i| i.to_le_bytes()).collect();
        let mut cut = gzip(&big);
        cut.truncate(cut.len() / 2);
        let partial = decode("gzip", cut).unwrap();
        assert!(!partial.is_empty() && big.starts_with(&partial));

        // Never advertised: left alone.
        assert_eq!(&decode("br", b"xyz".to_vec()).unwrap()[..], b"xyz");
        assert!(decode("gzip", b"not gzip".to_vec()).is_err());
    }

    #[test]
    fn test_default_config() {
        let config = ClientConfig2::default();
        assert!(config.user_agent.starts_with("Mozilla/5.0 ("));
        assert!(config.user_agent.contains("HiWave/1.0"));
        assert!(config.user_agent.contains("RustKit/1.0"));
        assert!(!config.user_agent.contains("Chrome"), "never Chrome\'s UA");
        assert_eq!(config.timeout, Duration::from_secs(30));
        assert_eq!(config.max_redirects, 10);
        assert!(config.follow_redirects);
    }
}


#[cfg(test)]
mod governed_tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// Serve `reply` to each connection until the listener is dropped; count accepts.
    async fn serve(reply: Vec<u8>) -> (u16, Arc<std::sync::atomic::AtomicUsize>) {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let h = hits.clone();
        tokio::spawn(async move {
            while let Ok((mut s, _)) = l.accept().await {
                h.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let reply = reply.clone();
                tokio::spawn(async move {
                    let mut buf = [0u8; 2048];
                    let _ = s.read(&mut buf).await;
                    let _ = s.write_all(&reply).await;
                    let _ = s.shutdown().await;
                });
            }
        });
        (port, hits)
    }

    fn client() -> Client {
        Client::new().unwrap()
    }

    #[tokio::test]
    async fn a_public_only_client_never_connects_to_loopback_on_any_site() {
        let (port, hits) = serve(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nhi".to_vec()).await;
        let c = client().with_address_policy(AddressPolicy::PublicOnly);
        for url in [
            format!("http://127.0.0.1:{port}/"),
            format!("http://localhost:{port}/"),
            format!("http://[::1]:{port}/"),
        ] {
            let r = c.get(&url).await;
            assert!(matches!(r, Err(HttpError::AddressDenied(_))), "{url}: {r:?}");
            let r = c.get_streaming(&url).await;
            assert!(matches!(r, Err(HttpError::AddressDenied(_))), "streaming {url}");
        }
        // https goes through the same connect: refused before the handshake.
        let r = c.get(&format!("https://127.0.0.1:{port}/")).await;
        assert!(matches!(r, Err(HttpError::AddressDenied(_))), "https: {r:?}");
        assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 0, "a socket was opened");
    }

    #[tokio::test]
    async fn the_default_client_still_reaches_loopback() {
        let (port, hits) = serve(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nhi".to_vec()).await;
        let r = client().get(&format!("http://127.0.0.1:{port}/")).await.unwrap();
        assert_eq!(r.text().unwrap(), "hi");
        assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn without_following_a_redirect_comes_back_as_the_response() {
        let (target_port, target_hits) = serve(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nhi".to_vec()).await;
        let reply = format!(
            "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:{target_port}/x\r\nContent-Length: 0\r\n\r\n"
        );
        let (port, _) = serve(reply.into_bytes()).await;
        let r = client()
            .with_follow_redirects(false)
            .get(&format!("http://127.0.0.1:{port}/"))
            .await
            .unwrap();
        assert_eq!(r.status.as_u16(), 302);
        assert_eq!(r.header("location"), Some(&format!("http://127.0.0.1:{target_port}/x")[..]));
        assert_eq!(target_hits.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn a_body_cap_refuses_content_length_chunked_eof_and_gzip_bombs() {
        let small = client().with_max_body(10);
        let cl = format!("HTTP/1.1 200 OK\r\nContent-Length: 20\r\n\r\n{}", "x".repeat(20));
        let (p, _) = serve(cl.into_bytes()).await;
        let r = small.get(&format!("http://127.0.0.1:{p}/")).await;
        assert!(matches!(r, Err(HttpError::BodyTooLarge(10))), "content-length: {r:?}");

        let chunked = format!(
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n{}\r\n0\r\n\r\n",
            20,
            "x".repeat(20)
        );
        let (p, _) = serve(chunked.into_bytes()).await;
        let r = small.get(&format!("http://127.0.0.1:{p}/")).await;
        assert!(matches!(r, Err(HttpError::BodyTooLarge(10))), "chunked: {r:?}");

        let eof = format!("HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n{}", "x".repeat(20));
        let (p, _) = serve(eof.into_bytes()).await;
        let r = small.get(&format!("http://127.0.0.1:{p}/")).await;
        assert!(matches!(r, Err(HttpError::BodyTooLarge(10))), "eof: {r:?}");

        // 1 MiB of zeros gzips to about a kilobyte: small on the wire, large decoded.
        use flate2::write::GzEncoder;
        let mut enc = GzEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(&vec![0u8; 1 << 20]).unwrap();
        let gz = enc.finish().unwrap();
        let mut reply = format!(
            "HTTP/1.1 200 OK\r\nContent-Encoding: gzip\r\nContent-Length: {}\r\n\r\n",
            gz.len()
        )
        .into_bytes();
        reply.extend_from_slice(&gz);
        let (p, _) = serve(reply).await;
        let r = small.get(&format!("http://127.0.0.1:{p}/")).await;
        assert!(matches!(r, Err(HttpError::BodyTooLarge(10))), "gzip bomb: {r:?}");

        // Under the cap is untouched.
        let ok = "HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello";
        let (p, _) = serve(ok.as_bytes().to_vec()).await;
        let r = small.get(&format!("http://127.0.0.1:{p}/")).await.unwrap();
        assert_eq!(r.text().unwrap(), "hello");
    }
}
