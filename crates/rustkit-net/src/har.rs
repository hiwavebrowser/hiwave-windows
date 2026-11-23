//! QB-ARCHIVE-INPUT: a recording of what the loader fetched, as HAR 1.2, so a real-site load can
//! be replayed offline (the loopback HAR replay server, `--replay-proxy`) and a lost board point
//! bisected without loading the live site again.
//!
//! Off unless [`start_recording`] was called (parity-capture `--dump-har`); the loader then hands
//! every response it answers to [`record`]. Bodies are kept as the page received them (decoded:
//! `Content-Encoding`, `Content-Length` and `Transfer-Encoding` are left out of the recorded
//! headers). Image, video and audio bodies are not kept (the entry stays, with its size and
//! `_skipped: "media"`), so recording does not hold every image a page loads: SVG is kept, and
//! images are kept when the recording was started with `keep_images` (the board's
//! `REALSITE_HAR_KEEP_IMAGES=1`). `Cookie`, `Set-Cookie`, `Authorization` and
//! `Proxy-Authorization` values are never recorded. A memory-cache
//! hit and a request already sent to a replay proxy are not recorded. Each redirect hop is an
//! entry of its own with its status and a `Location` naming the next URL, so a replay of the first
//! URL follows the same chain.

use std::io::{self, Write};
use std::path::Path;
use std::sync::Mutex;
use std::time::{Instant, SystemTime};

use base64::Engine as _;
use http::HeaderMap;
use serde_json::{json, Value};

use crate::{NetError, Request, Response};

struct Recording {
    entries: Vec<Value>,
    keep_images: bool,
}

static RECORDING: Mutex<Option<Recording>> = Mutex::new(None);

/// Response header names never written to a recording (replay serves the rest as recorded).
const SECRET_HEADERS: &[&str] =
    &["cookie", "cookie2", "set-cookie", "set-cookie2", "authorization", "proxy-authorization"];
/// The only request header names written to a recording: what replay or a diagnosis needs. Every
/// other request header (`x-csrf-token`, `x-api-key`, ...) is left out whatever it is called;
/// replay matches on method and URL. Mirrors `REQUEST_HEADERS` in scripts/realsite_har.py.
const REQUEST_HEADERS: &[&str] = &[
    ":method",
    ":path",
    ":authority",
    ":scheme",
    "host",
    "accept",
    "accept-language",
    "accept-encoding",
    "content-type",
    "range",
    "referer",
    "user-agent",
    "origin",
];

fn request_header_kept(name: &str) -> bool {
    REQUEST_HEADERS.contains(&name) || name.starts_with("sec-fetch-")
}
/// Headers that describe the transfer, not the decoded body the recording keeps.
const TRANSFER_HEADERS: &[&str] = &["content-encoding", "content-length", "transfer-encoding"];

/// Start recording this process's fetches (a recording already running is restarted). Image
/// bodies are kept only with `keep_images`; video and audio bodies never are.
pub fn start_recording(keep_images: bool) {
    *RECORDING.lock().unwrap_or_else(|e| e.into_inner()) =
        Some(Recording { entries: Vec::new(), keep_images });
}

/// Whether a recording is running.
pub fn recording() -> bool {
    RECORDING.lock().map(|r| r.is_some()).unwrap_or(false)
}

#[derive(serde::Serialize)]
struct HarDoc<'a> {
    log: HarLog<'a>,
}

#[derive(serde::Serialize)]
struct HarLog<'a> {
    version: &'static str,
    creator: Value,
    entries: &'a [Value],
}

fn with_doc<T>(f: impl FnOnce(&HarDoc<'_>) -> T) -> Option<T> {
    let guard = RECORDING.lock().unwrap_or_else(|e| e.into_inner());
    let recording = guard.as_ref()?;
    let doc = HarDoc {
        log: HarLog {
            version: "1.2",
            creator: json!({"name": "rustkit-net", "version": env!("CARGO_PKG_VERSION")}),
            entries: &recording.entries,
        },
    };
    Some(f(&doc))
}

/// The recording so far as a HAR 1.2 document, or `None` when none was started. Recording goes on.
pub fn snapshot() -> Option<Value> {
    with_doc(|doc| serde_json::to_value(doc).unwrap_or(Value::Null))
}

/// Write the recording so far to `path` (no copy of the entries; through a temporary file renamed
/// into place while the recording is locked, so the timeout thread and the main thread cannot
/// interleave and a reader never sees a torn file). `Ok(false)` when none was started.
pub fn write(path: &Path) -> io::Result<bool> {
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    let written = with_doc(|doc| -> io::Result<()> {
        let result = (|| {
            let mut out = io::BufWriter::new(std::fs::File::create(&tmp)?);
            serde_json::to_writer(&mut out, doc)?;
            out.flush()?;
            drop(out);
            std::fs::rename(&tmp, path)
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        result
    });
    written.map_or(Ok(false), |r| r.map(|()| true))
}

/// Recorded request headers: the allow-list only.
fn request_headers_json(headers: &HeaderMap) -> Vec<Value> {
    headers
        .iter()
        .filter(|(name, _)| request_header_kept(name.as_str()))
        .map(|(name, value)| {
            json!({"name": name.as_str(), "value": String::from_utf8_lossy(value.as_bytes())})
        })
        .collect()
}

/// Recorded response headers: all but the secret and transfer ones.
fn headers_json(headers: &HeaderMap, drop_transfer: bool) -> Vec<Value> {
    headers
        .iter()
        .filter(|(name, _)| {
            let name = name.as_str();
            !(SECRET_HEADERS.contains(&name) || drop_transfer && TRANSFER_HEADERS.contains(&name))
        })
        .map(|(name, value)| {
            json!({"name": name.as_str(), "value": String::from_utf8_lossy(value.as_bytes())})
        })
        .collect()
}

/// Now as an ISO 8601 UTC date with milliseconds (HAR `startedDateTime`).
fn started_iso() -> String {
    let ms = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let (days, rem) = (ms.div_euclid(86_400_000), ms.rem_euclid(86_400_000));
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    let (h, m, s, milli) = (rem / 3_600_000, rem / 60_000 % 60, rem / 1000 % 60, rem % 1000);
    format!("{year:04}-{month:02}-{day:02}T{h:02}:{m:02}:{s:02}.{milli:03}Z")
}

fn entry(method: &str, url: &str, request_headers: &[Value], status: u16, headers: Vec<Value>, content: Value, ms: f64) -> Value {
    json!({
        "startedDateTime": started_iso(),
        "time": ms,
        "request": {
            "method": method,
            "url": url,
            "httpVersion": "HTTP/1.1",
            "headers": request_headers,
            "queryString": [],
            "cookies": [],
            "headersSize": -1,
            "bodySize": -1,
        },
        "response": {
            "status": status,
            "statusText": "",
            "httpVersion": "HTTP/1.1",
            "headers": headers,
            "cookies": [],
            "content": content,
            "redirectURL": "",
            "headersSize": -1,
            "bodySize": -1,
        },
        "cache": {},
        "timings": {"send": 0, "wait": ms, "receive": 0},
    })
}

/// Whether a body of this MIME type is left out of the recording: image, video and audio, except
/// SVG (small text that drives layout) and, with `keep_images`, images.
fn is_skipped_media(mime: &str, keep_images: bool) -> bool {
    let essence = mime.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    if essence == "image/svg+xml" {
        return false;
    }
    if essence.starts_with("image/") {
        return !keep_images;
    }
    essence.starts_with("video/") || essence.starts_with("audio/")
}

/// Whether bytes of this MIME type are kept as text in the recording.
fn is_text(mime: &str, body: &[u8]) -> bool {
    let mime = mime.to_ascii_lowercase();
    let textual = mime.starts_with("text/")
        || mime.contains("json")
        || mime.contains("javascript")
        || mime.contains("xml")
        || mime.contains("css")
        || mime.contains("svg");
    textual && std::str::from_utf8(body).is_ok()
}

/// Record one answered fetch (called by the loader; does nothing while no recording runs).
pub(crate) fn record(request: &Request, result: &Result<Response, NetError>, began: Instant) {
    let Ok(response) = result else { return };
    if response.from_cache || request.is_replay_proxied {
        return;
    }
    let mut guard = RECORDING.lock().unwrap_or_else(|e| e.into_inner());
    let Some(recording) = guard.as_mut() else { return };
    let ms = began.elapsed().as_secs_f64() * 1000.0;
    let method = request.method.as_str().to_string();
    let request_headers = request_headers_json(&request.headers);
    // Each redirect hop: its URL, its status, and where it sent the request.
    let chain = &response.redirect_chain;
    for (i, (from, status)) in chain.iter().enumerate() {
        let next = chain.get(i + 1).map_or_else(|| response.url.to_string(), |(u, _)| u.to_string());
        let headers = vec![json!({"name": "location", "value": next})];
        let content = json!({"size": 0, "mimeType": ""});
        recording
            .entries
            .push(entry(&method, from.as_str(), &request_headers, status.as_u16(), headers, content, 0.0));
    }
    let mime = response
        .content_type
        .as_ref()
        .map(|m| m.to_string())
        .unwrap_or_default();
    let body: &[u8] = response.full_body().map_or(&[], |b| b.as_ref());
    let mut content = json!({"size": body.len(), "mimeType": mime});
    if body.is_empty() {
    } else if is_skipped_media(&mime, recording.keep_images) {
        // The entry stays (a replay answers it), the body is not copied.
        content["_skipped"] = Value::String("media".into());
    } else if is_text(&mime, body) {
        content["text"] = Value::String(String::from_utf8_lossy(body).into_owned());
    } else {
        content["text"] = Value::String(base64::engine::general_purpose::STANDARD.encode(body));
        content["encoding"] = Value::String("base64".into());
    }
    let url = if chain.is_empty() { request.url.as_str() } else { response.url.as_str() };
    recording.entries.push(entry(
        &method,
        url,
        &request_headers,
        response.status.as_u16(),
        headers_json(&response.headers, true),
        content,
        ms,
    ));
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::{HeaderValue, StatusCode};

    fn response(url: &str, status: u16, mime: &str, body: &[u8], chain: Vec<(&str, u16)>) -> Response {
        let mut headers = HeaderMap::new();
        headers.insert("content-type", HeaderValue::from_str(mime).unwrap());
        headers.insert("set-cookie", HeaderValue::from_static("sid=secret"));
        headers.insert("set-cookie2", HeaderValue::from_static("sid2=secret"));
        headers.insert("content-encoding", HeaderValue::from_static("gzip"));
        headers.insert("x-kept", HeaderValue::from_static("yes"));
        Response::for_test(
            url::Url::parse(url).unwrap(),
            StatusCode::from_u16(status).unwrap(),
            headers,
            mime.parse().ok(),
            bytes::Bytes::copy_from_slice(body),
            chain
                .into_iter()
                .map(|(u, s)| (url::Url::parse(u).unwrap(), StatusCode::from_u16(s).unwrap()))
                .collect(),
        )
    }

    fn png_req() -> Request {
        Request::get(url::Url::parse("http://a.test/i.png").unwrap())
    }

    /// The recording is process-wide, so a loader test fetching on another thread while it runs
    /// lands in it too (NET-LAZY-TLS R1: the policy test's local-server fetch). The test below
    /// reads only its own `http://a.test/` entries.
    fn own_entries(har: &Value) -> Vec<Value> {
        har["log"]["entries"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| e["request"]["url"].as_str().is_some_and(|u| u.starts_with("http://a.test/")))
            .cloned()
            .collect()
    }

    /// One test: the recording is process-wide.
    #[test]
    fn a_recording_keeps_bodies_and_redirect_hops_and_no_secrets() {
        let mut request = Request::get(url::Url::parse("http://a.test/start").unwrap());
        request.headers.insert("cookie", HeaderValue::from_static("sid=secret"));
        request.headers.insert("accept", HeaderValue::from_static("text/html"));
        request.headers.insert("x-csrf-token", HeaderValue::from_static("secret-csrf"));
        request.headers.insert("x-api-key", HeaderValue::from_static("secret-key"));
        request.headers.insert("sec-fetch-mode", HeaderValue::from_static("navigate"));
        // Not recording: nothing kept.
        record(&request, &Ok(response("http://a.test/start", 200, "text/html", b"x", vec![])), Instant::now());
        assert!(snapshot().is_none());

        start_recording(false);
        let page = response(
            "http://a.test/final",
            200,
            "text/html; charset=utf-8",
            b"<p>hi</p>",
            vec![("http://a.test/start", 301), ("http://a.test/mid", 302)],
        );
        record(&request, &Ok(page), Instant::now());
        let png = Request::get(url::Url::parse("http://a.test/i.png").unwrap());
        record(&png, &Ok(response("http://a.test/i.png", 200, "image/png", &[0, 159, 146, 150], vec![])), Instant::now());
        record(&png, &Err(NetError::Cancelled), Instant::now());
        let svg = Request::get(url::Url::parse("http://a.test/i.svg").unwrap());
        let svg_body: &[u8] = b"<svg viewBox='0 0 8 8'/>";
        record(&svg, &Ok(response("http://a.test/i.svg", 200, "image/svg+xml", svg_body, vec![])), Instant::now());
        let mp4 = Request::get(url::Url::parse("http://a.test/v.mp4").unwrap());
        record(&mp4, &Ok(response("http://a.test/v.mp4", 200, "video/mp4", &[1, 2, 3], vec![])), Instant::now());

        let har = snapshot().unwrap();
        let entries = own_entries(&har);
        let got: Vec<(String, u64)> = entries
            .iter()
            .map(|e| (e["request"]["url"].as_str().unwrap().to_string(), e["response"]["status"].as_u64().unwrap()))
            .collect();
        assert_eq!(
            got,
            [
                ("http://a.test/start".to_string(), 301),
                ("http://a.test/mid".to_string(), 302),
                ("http://a.test/final".to_string(), 200),
                ("http://a.test/i.png".to_string(), 200),
                ("http://a.test/i.svg".to_string(), 200),
                ("http://a.test/v.mp4".to_string(), 200),
            ]
        );
        assert_eq!(entries[0]["response"]["headers"][0]["value"], "http://a.test/mid");
        assert_eq!(entries[1]["response"]["headers"][0]["value"], "http://a.test/final");
        assert_eq!(entries[2]["response"]["content"]["text"], "<p>hi</p>");
        // Request headers: the allow-list only.
        let names: Vec<&str> =
            entries[2]["request"]["headers"].as_array().unwrap().iter().map(|h| h["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["accept", "sec-fetch-mode"]);
        // Media bodies are not kept: the entry stays with its size.
        let png = &entries[3]["response"]["content"];
        assert_eq!(png["_skipped"], "media");
        assert_eq!(png["size"], 4);
        assert!(png.get("text").is_none() && png.get("encoding").is_none());
        assert_eq!(entries[4]["response"]["content"]["text"], "<svg viewBox='0 0 8 8'/>", "SVG is kept");
        assert_eq!(entries[5]["response"]["content"]["_skipped"], "media");
        let text = Value::Array(entries.clone()).to_string();
        assert!(!text.contains("secret"), "no cookie value is recorded");
        assert!(!text.to_ascii_lowercase().contains("content-encoding"));
        assert!(text.contains("x-kept"));
        let date = entries[0]["startedDateTime"].as_str().unwrap();
        assert!(date.len() == 24 && date.ends_with('Z') && date.as_bytes()[10] == b'T', "{date}");

        // `write` puts the same document on disk, with no temporary file left.
        let dir = std::env::temp_dir().join(format!("har-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("out.har");
        assert!(write(&path).unwrap());
        let back: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        // Compared through the same text round trip: a parsed f64 `time` can differ by an ulp.
        let expected: Value = serde_json::from_str(&har.to_string()).unwrap();
        assert_eq!(own_entries(&back), own_entries(&expected));
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        std::fs::remove_dir_all(&dir).unwrap();

        // Started keeping images: the PNG body is kept, video still is not.
        start_recording(true);
        record(&png_req(), &Ok(response("http://a.test/i.png", 200, "image/png", &[0, 159, 146, 150], vec![])), Instant::now());
        record(&mp4, &Ok(response("http://a.test/v.mp4", 200, "video/mp4", &[1, 2, 3], vec![])), Instant::now());
        let har = own_entries(&snapshot().unwrap());
        assert_eq!(har[0]["response"]["content"]["text"], "AJ+Slg==");
        assert_eq!(har[1]["response"]["content"]["_skipped"], "media");
        *RECORDING.lock().unwrap() = None;
        assert!(!write(&path).unwrap(), "nothing written when no recording runs");
    }
}
