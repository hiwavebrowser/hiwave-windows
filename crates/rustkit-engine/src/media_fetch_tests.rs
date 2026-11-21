//! MEDIA-AVF step 2, #1344 R1: a media element fetches what AVFoundation reads for its
//! metadata, by `Range`, a chunk at a time; never the whole resource for its own sake,
//! never more than a cap, and nothing once the element is gone or has another resource.
//!
//! [`serve_media`] is a test server that answers `Range` with 206 (as media servers
//! do), logs each request's range and the bytes it sent, holds `/slow…` paths 1 s, and
//! ignores `Range` on `/norange…` paths (a 200 with the whole body).

use super::*;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

const COLOR_STEPS: &[u8] = include_bytes!("../../../fixtures/media/color-steps.mp4");

/// One request the media server answered: its path, its `Range` header, the bytes sent.
#[derive(Clone, Debug)]
pub(crate) struct Served {
    pub(crate) path: String,
    pub(crate) range: Option<String>,
    pub(crate) sent: usize,
}

pub(crate) struct MediaServer {
    pub(crate) port: u16,
    pub(crate) log: Arc<Mutex<Vec<Served>>>,
}

impl MediaServer {
    pub(crate) fn served(&self, path: &str) -> Vec<Served> {
        self.log
            .lock()
            .unwrap()
            .iter()
            .filter(|s| s.path == path)
            .cloned()
            .collect()
    }
}

/// Serve `routes` (path, content type, body), answering `Range: bytes=a-b` with 206.
pub(crate) fn serve_media(routes: Vec<(&'static str, &'static str, Vec<u8>)>) -> MediaServer {
    let routes = Arc::new(routes);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let log = Arc::new(Mutex::new(Vec::new()));
    let logged = log.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let (routes, logged) = (routes.clone(), logged.clone());
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
                let range = head.lines().find_map(|l| {
                    let (name, value) = l.split_once(':')?;
                    name.trim()
                        .eq_ignore_ascii_case("range")
                        .then(|| value.trim().to_string())
                });
                if path.starts_with("/slow") {
                    std::thread::sleep(std::time::Duration::from_millis(1000));
                }
                let Some((_, content_type, body)) = routes.iter().find(|(p, _, _)| *p == path)
                else {
                    let _ = write!(
                        stream,
                        "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    );
                    return;
                };
                let ranged = range
                    .as_deref()
                    .filter(|_| !path.starts_with("/norange"))
                    .and_then(|r| {
                        let (a, b) = r.strip_prefix("bytes=")?.split_once('-')?;
                        let a: usize = a.parse().ok()?;
                        let b: usize = if b.is_empty() {
                            body.len() - 1
                        } else {
                            b.parse().ok()?
                        };
                        (a < body.len()).then(|| (a, b.min(body.len() - 1)))
                    });
                let (status, extra, part) = match ranged {
                    Some((a, b)) => (
                        "206 Partial Content",
                        format!("Content-Range: bytes {a}-{b}/{}\r\n", body.len()),
                        &body[a..=b],
                    ),
                    None => ("200 OK", String::new(), &body[..]),
                };
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nAccept-Ranges: bytes\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n",
                    part.len()
                );
                let _ = stream.write_all(part);
                logged.lock().unwrap().push(Served {
                    path,
                    range,
                    sent: part.len(),
                });
            });
        }
    });
    MediaServer { port, log }
}

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

fn read(engine: &mut Engine, view: EngineViewId, expr: &str) -> String {
    let value = engine
        .execute_script(view, &format!("String({expr})"))
        .unwrap();
    value
        .strip_prefix("String(\"")
        .and_then(|v| v.strip_suffix("\")"))
        .unwrap_or(&value)
        .to_string()
}

fn open(server: &MediaServer) -> (Engine, EngineViewId, tokio::runtime::Runtime) {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds {
            x: 0,
            y: 0,
            width: 800,
            height: 600,
        })
        .expect("view");
    let url = Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap();
    let rt = rt();
    rt.block_on(engine.load_url(view, url)).expect("load_url");
    (engine, view, rt)
}

/// Pump until `done` reads true (or 5 s of wall time).
fn settle(
    engine: &mut Engine,
    view: EngineViewId,
    rt: &tokio::runtime::Runtime,
    done: &str,
) -> bool {
    let started = std::time::Instant::now();
    while started.elapsed() < std::time::Duration::from_secs(5) {
        rt.block_on(engine.pump_live(view, 16));
        if read(engine, view, done) == "true" {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    false
}

const SETTLED: &str =
    "(function(){var m=document.getElementById('m');return m.readyState>=1||!!m.error;})()";

/// The fixture with 8 MB the metadata does not need (a `free` box) between its media
/// data and its `moov` atom, which sits at the end.
fn padded_fixture() -> Vec<u8> {
    let mut at = 0usize;
    let mut moov = None;
    while at + 8 <= COLOR_STEPS.len() {
        let mut size = u32::from_be_bytes(COLOR_STEPS[at..at + 4].try_into().unwrap()) as usize;
        if size == 1 {
            size = u64::from_be_bytes(COLOR_STEPS[at + 8..at + 16].try_into().unwrap()) as usize;
        }
        if &COLOR_STEPS[at + 4..at + 8] == b"moov" {
            moov = Some(at);
        }
        if size < 8 {
            break;
        }
        at += size;
    }
    let moov = moov.expect("moov");
    let pad: u32 = 8 << 20;
    let mut out = COLOR_STEPS[..moov].to_vec();
    out.extend_from_slice(&pad.to_be_bytes());
    out.extend_from_slice(b"free");
    out.resize(out.len() + pad as usize - 8, 0);
    out.extend_from_slice(&COLOR_STEPS[moov..]);
    out
}

/// Every media request carries `Range`.
#[test]
#[cfg_attr(windows, ignore = "Windows media backend row: MEDIA-AVF plays media through AVFoundation (macOS only); Windows has no backend yet")]
fn media_is_fetched_by_range() {
    let server = serve_media(vec![
        (
            "/",
            "text/html",
            b"<!doctype html><video id=m src=/color-steps.mp4 preload=auto></video>".to_vec(),
        ),
        ("/color-steps.mp4", "video/mp4", COLOR_STEPS.to_vec()),
    ]);
    let (mut engine, view, rt) = open(&server);
    assert!(settle(&mut engine, view, &rt, SETTLED));
    let served = server.served("/color-steps.mp4");
    assert!(!served.is_empty());
    assert!(served.iter().all(|s| s.range.is_some()), "{served:?}");
    assert_eq!(
        read(&mut engine, view, "document.getElementById('m').duration"),
        "2"
    );
}

/// The metadata of a resource with 8 MB it does not need is read from a few ranges: the
/// header, and the `moov` atom at the end by a seek. Not the resource.
#[test]
#[cfg_attr(windows, ignore = "Windows media backend row: MEDIA-AVF plays media through AVFoundation (macOS only); Windows has no backend yet")]
fn the_metadata_fetches_only_what_it_reads() {
    let big = padded_fixture();
    let total = big.len();
    let server = serve_media(vec![
        (
            "/",
            "text/html",
            b"<!doctype html><video id=m src=/big.mp4></video>".to_vec(),
        ),
        ("/big.mp4", "video/mp4", big),
    ]);
    let (mut engine, view, rt) = open(&server);
    assert!(settle(&mut engine, view, &rt, SETTLED));
    assert_eq!(
        read(&mut engine, view, "document.getElementById('m').duration"),
        "2"
    );
    assert_eq!(
        read(&mut engine, view, "document.getElementById('m').videoWidth"),
        "64"
    );
    let sent: usize = server.served("/big.mp4").iter().map(|s| s.sent).sum();
    assert!(
        sent < (3 << 20),
        "sent {sent} of {total} bytes: {:?}",
        server.served("/big.mp4")
    );
    // Not every byte: the metadata, not the data (HAVE_METADATA).
    assert_eq!(
        read(&mut engine, view, "document.getElementById('m').readyState"),
        "1"
    );
}

/// A server that ignores `Range` answers with the whole body: past the cap, the fetch is
/// refused and the element errors with MEDIA_ERR_NETWORK.
#[test]
#[cfg_attr(windows, ignore = "Windows media backend row: MEDIA-AVF plays media through AVFoundation (macOS only); Windows has no backend yet")]
fn a_whole_body_past_the_cap_is_a_network_error() {
    media::CAP_FOR_TESTS.with(|c| c.set(Some(4096)));
    let server = serve_media(vec![
        (
            "/",
            "text/html",
            b"<!doctype html><video id=m src=/norange.mp4 preload=auto></video>".to_vec(),
        ),
        ("/norange.mp4", "video/mp4", COLOR_STEPS.to_vec()),
    ]);
    let (mut engine, view, rt) = open(&server);
    let ok = settle(&mut engine, view, &rt, SETTLED);
    media::CAP_FOR_TESTS.with(|c| c.set(None));
    assert!(ok);
    assert_eq!(
        read(&mut engine, view, "document.getElementById('m').error.code"),
        "2"
    );
    assert_eq!(
        read(
            &mut engine,
            view,
            "document.getElementById('m').networkState"
        ),
        "3"
    );
}

/// Within the cap, a server that ignores `Range` still works: its 200 is the resource.
#[test]
#[cfg_attr(windows, ignore = "Windows media backend row: MEDIA-AVF plays media through AVFoundation (macOS only); Windows has no backend yet")]
fn a_server_that_ignores_range_still_loads() {
    let server = serve_media(vec![
        (
            "/",
            "text/html",
            b"<!doctype html><video id=m src=/norange.mp4 preload=auto></video>".to_vec(),
        ),
        ("/norange.mp4", "video/mp4", COLOR_STEPS.to_vec()),
    ]);
    let (mut engine, view, rt) = open(&server);
    assert!(settle(&mut engine, view, &rt, SETTLED));
    assert_eq!(
        read(&mut engine, view, "document.getElementById('m').duration"),
        "2"
    );
}

/// The element's fetches in flight, as (run serial) per request.
fn in_flight(engine: &Engine, view: EngineViewId) -> Vec<u64> {
    engine.views[&view]
        .live_media
        .iter()
        .map(|(_, serial, _)| *serial)
        .collect()
}

/// Pump until the element's first fetch is out: AVFoundation asks for its first range
/// from its own queue, in wall time (5 s at most).
fn wait_for_fetch(engine: &mut Engine, view: EngineViewId, rt: &tokio::runtime::Runtime) {
    let started = std::time::Instant::now();
    while in_flight(engine, view).is_empty()
        && started.elapsed() < std::time::Duration::from_secs(5)
    {
        rt.block_on(engine.pump_live(view, 16));
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

/// Removing a `<video>` mid-fetch drops its request and its bytes.
#[test]
#[cfg_attr(windows, ignore = "Windows media backend row: MEDIA-AVF plays media through AVFoundation (macOS only); Windows has no backend yet")]
fn removing_a_video_cancels_its_fetch_and_frees_its_bytes() {
    let server = serve_media(vec![
        (
            "/",
            "text/html",
            b"<!doctype html><video id=m src=/slow.mp4 preload=auto></video>".to_vec(),
        ),
        ("/slow.mp4", "video/mp4", COLOR_STEPS.to_vec()),
    ]);
    let (mut engine, view, rt) = open(&server);
    wait_for_fetch(&mut engine, view, &rt);
    assert!(
        !in_flight(&engine, view).is_empty(),
        "the slow fetch is out"
    );
    read(
        &mut engine,
        view,
        "(document.getElementById('m').remove(), 1)",
    );
    for _ in 0..5 {
        rt.block_on(engine.pump_live(view, 16));
    }
    assert!(
        in_flight(&engine, view).is_empty(),
        "the removed video's fetch is still out"
    );
    assert_eq!(engine.views[&view].media.held(), 0);
    assert!(!engine.views[&view].media.busy(), "nothing is loading");
}

/// A new `src` drops the old resource's request; the new one loads.
#[test]
#[cfg_attr(windows, ignore = "Windows media backend row: MEDIA-AVF plays media through AVFoundation (macOS only); Windows has no backend yet")]
fn a_new_src_cancels_the_old_fetch() {
    let server = serve_media(vec![
        (
            "/",
            "text/html",
            b"<!doctype html><video id=m src=/slow.mp4 preload=auto></video>".to_vec(),
        ),
        ("/slow.mp4", "video/mp4", COLOR_STEPS.to_vec()),
        ("/color-steps.mp4", "video/mp4", COLOR_STEPS.to_vec()),
    ]);
    let (mut engine, view, rt) = open(&server);
    wait_for_fetch(&mut engine, view, &rt);
    let old = in_flight(&engine, view);
    assert!(!old.is_empty(), "the slow fetch is out");
    read(
        &mut engine,
        view,
        "(document.getElementById('m').src = '/color-steps.mp4', 1)",
    );
    rt.block_on(engine.pump_live(view, 16));
    let now = in_flight(&engine, view);
    assert!(
        now.iter().all(|s| !old.contains(s)),
        "the old run's fetch is still out: {old:?} -> {now:?}"
    );
    assert!(settle(&mut engine, view, &rt, SETTLED));
    assert_eq!(
        read(
            &mut engine,
            view,
            "document.getElementById('m').currentSrc.replace(location.origin, '')"
        ),
        "/color-steps.mp4"
    );
    assert_eq!(
        read(&mut engine, view, "document.getElementById('m').duration"),
        "2"
    );
}
