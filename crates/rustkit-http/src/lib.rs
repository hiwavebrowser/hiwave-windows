//! # RustKit HTTP
//!
//! Minimal HTTP/1.1 client for the RustKit browser engine.
//!
//! This crate provides a simple async HTTP client using native-tls for TLS,
//! eliminating the need for reqwest and its transitive dependencies.

use std::io::{self, Write};
use std::time::Duration;

use bytes::Bytes;
use http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, Version};
use native_tls::TlsConnector as NativeTlsConnector;
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::time::timeout;
use tokio_native_tls::TlsConnector;
use tracing::{debug, trace, warn};
use url::Url;

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
            user_agent: "RustKit/1.0".to_string(),
            timeout: Duration::from_secs(30),
            max_redirects: 10,
            follow_redirects: true,
        }
    }
}

/// HTTP client.
pub struct Client {
    config: ClientConfig2,
    tls_connector: TlsConnector,
}

impl Client {
    /// Create a new HTTP client with default configuration.
    pub fn new() -> Result<Self, HttpError> {
        Self::with_config(ClientConfig2::default())
    }

    /// Create a new HTTP client with custom configuration.
    pub fn with_config(config: ClientConfig2) -> Result<Self, HttpError> {
        // Build native-tls connector
        let native_connector = NativeTlsConnector::new()
            .map_err(|e| HttpError::TlsError(e.to_string()))?;

        let tls_connector = TlsConnector::from(native_connector);

        Ok(Self {
            config,
            tls_connector,
        })
    }

    /// Create a client builder.
    pub fn builder() -> ClientBuilder {
        ClientBuilder::new()
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
        let stream = TcpStream::connect(&addr)
            .await
            .map_err(|e| HttpError::ConnectionFailed(e.to_string()))?;

        let tls_stream = self
            .tls_connector
            .connect(host, stream)
            .await
            .map_err(|e| HttpError::TlsError(e.to_string()))?;

        self.send_request(tls_stream, host, method, url, headers, body)
            .await
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
        let addr = format!("{}:{}", host, port);
        let stream = TcpStream::connect(&addr)
            .await
            .map_err(|e| HttpError::ConnectionFailed(e.to_string()))?;

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

        let mut request = Vec::new();
        writeln!(request, "{} {} HTTP/1.1\r", method, path)?;
        writeln!(request, "Host: {}\r", host)?;
        writeln!(request, "User-Agent: {}\r", self.config.user_agent)?;
        writeln!(request, "Accept: */*\r")?;
        // Every browser sends this, and some sites treat a client that
        // doesn't as a bot (microsoft.com serves an "automated process"
        // page). Only encodings decode_content_encoding can undo.
        if !headers.contains_key("accept-encoding") {
            writeln!(request, "Accept-Encoding: {}\r", ACCEPT_ENCODING)?;
        }
        writeln!(request, "Connection: close\r")?;

        // Add custom headers
        for (name, value) in headers.iter() {
            if let Ok(v) = value.to_str() {
                writeln!(request, "{}: {}\r", name, v)?;
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
        let body = read_body(&mut reader, &response_headers).await?;
        let body = decode_content_encoding(body, &mut response_headers)?;

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

/// Undo the response's `Content-Encoding`, so callers always see the
/// resource's bytes. A decoded body drops `Content-Encoding` and
/// `Content-Length` (which described the encoded bytes).
///
/// A body cut off mid-stream keeps what decoded, as a truncated chunked
/// body does. An encoding we never advertised is passed through untouched.
fn decode_content_encoding(body: Bytes, headers: &mut HeaderMap) -> Result<Bytes, HttpError> {
    use std::io::Read;

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
        "gzip" | "x-gzip" => flate2::read::MultiGzDecoder::new(&body[..]).read_to_end(&mut out),
        // "deflate" is zlib-wrapped per RFC 9110, but some servers send raw
        // deflate; browsers accept both.
        "deflate" => match flate2::read::ZlibDecoder::new(&body[..]).read_to_end(&mut out) {
            Ok(n) => Ok(n),
            Err(_) if out.is_empty() => {
                flate2::read::DeflateDecoder::new(&body[..]).read_to_end(&mut out)
            }
            Err(e) => Err(e),
        },
        other => {
            warn!(encoding = other, "Unsupported Content-Encoding; body left encoded");
            return Ok(body);
        }
    };
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
) -> Result<Bytes, HttpError> {
    // Check for Content-Length
    if let Some(len) = headers
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<usize>().ok())
    {
        let mut buf = vec![0u8; len];
        reader.read_exact(&mut buf).await?;
        return Ok(Bytes::from(buf));
    }

    // Check for chunked transfer encoding
    if let Some(te) = headers.get("transfer-encoding").and_then(|v| v.to_str().ok()) {
        if te.to_lowercase().contains("chunked") {
            return read_chunked_body(reader).await;
        }
    }

    // Read until EOF
    let mut buf = Vec::new();
    reader.read_to_end(&mut buf).await?;
    Ok(Bytes::from(buf))
}

/// Read chunked transfer encoding body.
async fn read_chunked_body<R: tokio::io::AsyncBufRead + Unpin>(
    reader: &mut R,
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
        let stream = TcpStream::connect(&addr)
            .await
            .map_err(|e| HttpError::ConnectionFailed(e.to_string()))?;

        let tls_stream = self
            .tls_connector
            .connect(host, stream)
            .await
            .map_err(|e| HttpError::TlsError(e.to_string()))?;

        self.send_streaming_request(tls_stream, host, url).await
    }

    async fn streaming_http(
        &self,
        host: &str,
        port: u16,
        url: &Url,
    ) -> Result<StreamingResponse, HttpError> {
        let addr = format!("{}:{}", host, port);
        let stream = TcpStream::connect(&addr)
            .await
            .map_err(|e| HttpError::ConnectionFailed(e.to_string()))?;

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
                .map_err(|e| HttpError::IoError(io::Error::new(io::ErrorKind::Other, e)))?;

            let inner = super::Client::with_config(self.config)?;

            Ok(Client { runtime, inner })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        rt.block_on(read_chunked_body(&mut reader))
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
        assert_eq!(config.user_agent, "RustKit/1.0");
        assert_eq!(config.timeout, Duration::from_secs(30));
        assert_eq!(config.max_redirects, 10);
        assert!(config.follow_redirects);
    }
}

