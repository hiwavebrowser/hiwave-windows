//! A canvas in a frame's document draws images, and decides whether it may
//! be read, as a canvas in a top-level document does (IMG-F1).
//!
//! Chrome for Testing 148.0.7778.216 on the probe pages
//! (`docs/diagnostics/2026-11-16/frame_canvas{.html,_inner.html,_shape.*}`,
//! record `frame_canvas_cft148.json`). In Chrome's run the page is at
//! `127.0.0.1` and the other origin is the same server as `localhost`.
//!
//! Here the same two origins are served, with real redirects, but the
//! engine's network rule lets a page at a private address reach its own
//! origin only (rustkit-net `page_address_policy`): a request to the other
//! origin, or a redirect hop to it, is refused, so those images fail to
//! load and there is nothing to read. That outcome is asserted as it is.
//! What a canvas decides about another origin's image is tested with the
//! image put in the image store as a response, and, for redirect chains,
//! in rustkit-bindings (`canvas_origin_checks_follow_the_response_urls`),
//! in a frame's realm too.

#![cfg(all(target_os = "macos", feature = "headless"))]

use super::*;
use crate::canvas_context_tests::serve_with_redirects;
use std::time::Duration;

const PARENT: &str = include_str!("../../../docs/diagnostics/2026-11-16/frame_canvas.html");
const INNER: &str = include_str!("../../../docs/diagnostics/2026-11-16/frame_canvas_inner.html");
const SVG: &[u8] = include_bytes!("../../../docs/diagnostics/2026-11-16/frame_canvas_shape.svg");
const PNG: &[u8] = include_bytes!("../../../docs/diagnostics/2026-11-16/frame_canvas_shape.png");
const PIN: &str = include_str!("../../../docs/diagnostics/2026-11-16/frame_canvas_cft148.json");

/// An origin no request is made to: its image is put in the image store.
const STORED: &str = "http://other.test";

/// The probe's record for `page` with `fragment` (`{port}` in it is the
/// server's), or `None` when the page never reported. `stored`: another
/// origin's image already in the image store, as a response.
fn record(nested_contexts: bool, page: &'static str, fragment: &str, stored: bool) -> Option<serde_json::Value> {
    let port = serve_with_redirects(|port| {
        vec![
            ("/frame_canvas.html".into(), 200, String::new(), "text/html", PARENT.as_bytes().to_vec()),
            ("/frame_canvas_inner.html".into(), 200, String::new(), "text/html", INNER.as_bytes().to_vec()),
            ("/frame_canvas_shape.svg".into(), 200, String::new(), "image/svg+xml", SVG.to_vec()),
            ("/frame_canvas_shape.png".into(), 200, String::new(), "image/png", PNG.to_vec()),
            ("/frame_canvas_to_same".into(), 302, "Location: /frame_canvas_shape.png\r\n".into(), "text/plain", Vec::new()),
            ("/frame_canvas_to_other".into(), 302, format!("Location: http://localhost:{port}/frame_canvas_shape.png\r\n"), "text/plain", Vec::new()),
            ("/frame_canvas_back".into(), 302, format!("Location: http://127.0.0.1:{port}/frame_canvas_shape.png\r\n"), "text/plain", Vec::new()),
        ]
    });
    let config = EngineConfig { nested_contexts, post_load_requests_live: true, ..EngineConfig::default() };
    let mut engine = Engine::new(config).expect("engine");
    if stored {
        let far = Url::parse(&format!("{STORED}/frame_canvas_shape.png")).unwrap();
        engine.lctx.image_manager.insert_fetched(&far, Some("image/png"), PNG).expect("the PNG decodes");
    }
    let view = engine.create_headless_view(Bounds { x: 0, y: 0, width: 800, height: 600 }).expect("view");
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let fragment = fragment.replace("{port}", &port.to_string());
    rt.block_on(engine.load_url(view, Url::parse(&format!("http://127.0.0.1:{port}/{page}{fragment}")).unwrap())).expect("load_url");
    for _ in 0..300 {
        std::thread::sleep(Duration::from_millis(10));
        rt.block_on(engine.pump_live(view, 20));
        let out = engine.execute_script(view, "window.__out || ''").unwrap();
        if out.len() > "String(\"\")".len() {
            let text: String = serde_json::from_str(out.trim_start_matches("String(").trim_end_matches(')')).unwrap();
            return Some(serde_json::from_str(&text).unwrap());
        }
    }
    None
}

fn pin() -> serde_json::Value {
    let pin: serde_json::Value = serde_json::from_str(PIN).unwrap();
    assert_eq!(pin["browser"], "148.0.7778.216");
    pin
}

/// A same-origin frame's script draws a loaded PNG and a loaded SVG image
/// into a canvas and reads it back: the pixels are there, as at top level.
#[test]
fn a_frames_canvas_draws_a_loaded_image_as_in_chrome() {
    let pin = pin();
    let top = record(true, "frame_canvas_inner.html", "", false).expect("the document on its own reports");
    let framed = record(true, "frame_canvas.html", "", false).expect("the frame reports");
    for kind in ["svg", "png"] {
        assert_eq!(top["top"][kind], pin["top"][kind], "top {kind}");
        assert_eq!(framed["framed"][kind], pin["framed"][kind], "framed {kind}");
    }
}

/// With nested contexts off a frame has no document: nothing reports.
#[test]
fn with_nested_contexts_off_a_frames_canvas_probe_does_not_run() {
    assert_eq!(record(false, "frame_canvas.html", "", false), None);
    let pin = pin();
    let top = record(false, "frame_canvas_inner.html", "", false).expect("the document on its own reports");
    assert_eq!(top["top"]["png"], pin["top"]["png"]);
}

/// A canvas that drew another origin's image cannot be read
/// (`getImageData`: SecurityError), nor can a canvas that drew that
/// canvas; a canvas beside them can. In a frame the decision is made with
/// the frame's own origin, and comes out as at top level. The other
/// origin's image is in the image store (see the module's note).
#[test]
fn a_frames_canvas_that_drew_another_origins_image_cannot_be_read_as_in_chrome() {
    let pin = pin();
    assert_eq!(pin["framed"]["other_origin"]["getImageData"], "SecurityError");
    let fragment = format!("#other={}", STORED.replace(':', "%3A").replace('/', "%2F"));
    let mut top = record(true, "frame_canvas_inner.html", &fragment, true).expect("the document on its own reports");
    let mut framed = record(true, "frame_canvas.html", &fragment, true).expect("the frame reports");
    // Known difference, in a frame as at top level: `canvas.toDataURL` does
    // not exist yet (web_canvas.js lists it), so the call is a TypeError
    // where Chrome's is a SecurityError. Nothing is read either way. When
    // it lands this turns red and the two lines go.
    for (got, level) in [(&mut top, "top"), (&mut framed, "framed")] {
        assert_eq!(got[level]["other_origin"]["toDataURL"], "TypeError", "{level}");
        got[level]["other_origin"]["toDataURL"] = "SecurityError".into();
        assert_eq!(got[level]["other_origin"], pin[level]["other_origin"], "{level}");
    }
}

/// The redirect cases with real requests and real 302s, the other origin
/// being this server as `localhost`. A redirect within the origin is
/// followed and the canvas reads, as in Chrome, in a frame as at top
/// level. Every request that would leave the page's own private origin is
/// refused by the engine's network rule, so those images do not load: no
/// pixel of another origin reaches a canvas at all. (Chrome, which loads
/// them, answers SecurityError for all three; that decision is tested in
/// the test above and in rustkit-bindings.)
#[test]
fn a_frames_image_requests_follow_redirects_only_within_its_origin() {
    let pin = pin();
    let fragment = "#other=http%3A%2F%2Flocalhost%3A{port}&redirects";
    for (page, level) in [("frame_canvas_inner.html", "top"), ("frame_canvas.html", "framed")] {
        let got = record(true, page, fragment, false).unwrap_or_else(|| panic!("{level} reports"));
        assert_eq!(got[level]["redirect_same_to_same"], pin[level]["redirect_same_to_same"], "{level}");
        assert_eq!(got[level]["redirect_same_to_same"], "ok", "{level}");
        for refused in ["redirect_same_to_other", "redirect_other_to_same"] {
            assert_eq!(pin[level][refused], "SecurityError", "{level} {refused}: Chrome");
            assert_eq!(got[level][refused], "load-error", "{level} {refused}");
        }
        assert_eq!(got[level]["other_origin"], "load-error", "{level}: the other origin's image asked for directly");
    }
}
