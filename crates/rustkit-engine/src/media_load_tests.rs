//! MEDIA-AVF Milestone 1 step 2 (docs/design/MEDIA_AVF.md §3.8; pins P-events, P-meta,
//! P-error and the box after metadata): a media element loads its resource through
//! rustkit-net, AVFoundation reads the metadata, and the element's events, state and
//! box follow Chrome 148. Nothing plays.
//!
//! Oracle: Chrome for Testing 148.0.7778.216 through
//! `tools/parity_oracle/media_load_log.mjs`, receipt
//! `docs/diagnostics/2026-11-18/media_load_cft148.json`, which this file reads. The
//! pages are the oracle's; the fixtures are `fixtures/media/`. AVFoundation loads on
//! its own threads, so the live loop is pumped with short real-time pauses until the
//! element settles (canplaythrough, an error, or 5 s), as the oracle waited.
//!
//! Compared per case: the events with readyState / networkState at each, the final
//! readyState, networkState, error code, currentSrc, duration, video size, buffered,
//! seekable, preload and the border box. Not compared: `ended` (Chrome reads true for
//! the one-frame clip; playback state is step 4) and, for the default-preload case, the
//! states at the events (Chrome had the whole tiny file before the metadata events, a
//! timing of its own; the event names are compared).

use super::*;
use crate::media_fetch_tests::serve_media;

const RECEIPT: &str = include_str!("../../../docs/diagnostics/2026-11-18/media_load_cft148.json");
const COLOR_STEPS: &[u8] = include_bytes!("../../../fixtures/media/color-steps.mp4");
const SIZE_32X24: &[u8] = include_bytes!("../../../fixtures/media/size-32x24.mp4");

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
        .replace("\\\"", "\"")
}

/// The oracle's page for `markup`, with every media event at `#m` logged.
fn page(receipt: &serde_json::Value, markup: &str) -> String {
    let events = serde_json::to_string(&receipt["events_watched"]).unwrap();
    format!(
        "<!doctype html><style>body{{margin:0}}</style><script>window.__ev=[];\
         {events}.forEach(function(t){{document.addEventListener(t,function(e){{if(e.target.id==='m')\
         __ev.push(t+':rs'+e.target.readyState+':ns'+e.target.networkState)}},true)}});</script>{markup}"
    )
}

const STATE: &str = "JSON.stringify((function(){var m=document.getElementById('m');\
    function ranges(t){var o=[];for(var i=0;i<t.length;i++)o.push([t.start(i),t.end(i)]);return o;}\
    var r=m.getBoundingClientRect();\
    return {events:__ev.slice(),readyState:m.readyState,networkState:m.networkState,error:m.error?m.error.code:null,\
    currentSrc:m.currentSrc.replace(location.origin,''),duration:isNaN(m.duration)?'NaN':m.duration,\
    videoWidth:m.videoWidth===undefined?null:m.videoWidth,videoHeight:m.videoHeight===undefined?null:m.videoHeight,\
    paused:m.paused,seeking:m.seeking,currentTime:m.currentTime,buffered:ranges(m.buffered),seekable:ranges(m.seekable),\
    preload:m.preload,width:r.width,height:r.height};})())";

/// Load `label`'s page and let its element settle.
fn run(label: &str) -> (serde_json::Value, serde_json::Value) {
    let receipt: serde_json::Value = serde_json::from_str(RECEIPT).unwrap();
    assert_eq!(receipt["chrome"], "148.0.7778.216");
    let case = receipt["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["label"] == label)
        .unwrap_or_else(|| panic!("no case {label:?}"))
        .clone();
    let html = page(&receipt, case["markup"].as_str().unwrap());
    let server = serve_media(vec![
        ("/", "text/html", html.into_bytes()),
        ("/color-steps.mp4", "video/mp4", COLOR_STEPS.to_vec()),
        ("/size-32x24.mp4", "video/mp4", SIZE_32X24.to_vec()),
        (
            "/not-media.mp4",
            "video/mp4",
            b"this is not a media file ".repeat(40),
        ),
    ]);
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
    let started = std::time::Instant::now();
    let settled =
        "(function(){var m=document.getElementById('m');return m.readyState===4||!!m.error;})()";
    while started.elapsed() < std::time::Duration::from_secs(5) {
        rt.block_on(engine.pump_live(view, 16));
        if read(&mut engine, view, settled) == "true" {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    // The oracle waited 200 ms more for events of the same task.
    for _ in 0..12 {
        rt.block_on(engine.pump_live(view, 16));
    }
    let got: serde_json::Value =
        serde_json::from_str(&read(&mut engine, view, STATE)).expect("state");
    drop(server);
    (case, got)
}

fn close(a: &serde_json::Value, b: &serde_json::Value) -> bool {
    match (a.as_f64(), b.as_f64()) {
        (Some(x), Some(y)) => (x - y).abs() <= 0.01,
        _ => match (a.as_array(), b.as_array()) {
            (Some(x), Some(y)) => x.len() == y.len() && x.iter().zip(y).all(|(p, q)| close(p, q)),
            _ => a == b,
        },
    }
}

fn check(label: &str, event_states: bool) {
    check_except(label, event_states, &[]);
}

fn check_except(label: &str, event_states: bool, skip: &[&str]) {
    let (case, got) = run(label);
    let mut failures = Vec::new();
    let events = |v: &serde_json::Value, states: bool| -> Vec<String> {
        v.as_array()
            .unwrap()
            .iter()
            .map(|e| {
                let e = e.as_str().unwrap();
                if states {
                    e.to_string()
                } else {
                    e.split(':').next().unwrap().to_string()
                }
            })
            .collect()
    };
    if events(&got["events"], event_states) != events(&case["events"], event_states) {
        failures.push(format!(
            "events {:?}, Chrome {:?}",
            got["events"], case["events"]
        ));
    }
    for key in [
        "readyState",
        "networkState",
        "error",
        "currentSrc",
        "duration",
        "videoWidth",
        "videoHeight",
        "paused",
        "seeking",
        "currentTime",
        "buffered",
        "seekable",
        "preload",
        "width",
        "height",
    ] {
        if skip.contains(&key) {
            continue;
        }
        if !close(&got[key], &case[key]) {
            failures.push(format!("{key} {}, Chrome {}", got[key], case[key]));
        }
    }
    assert!(failures.is_empty(), "{label}:\n  {}", failures.join("\n  "));
}

#[test]
#[cfg_attr(windows, ignore = "Windows media backend row: MEDIA-AVF plays media through AVFoundation (macOS only); Windows has no backend yet")]
fn a_video_with_preload_auto_loads_its_metadata() {
    check("video src preload=auto", true);
}

#[test]
#[cfg_attr(windows, ignore = "Windows media backend row: MEDIA-AVF plays media through AVFoundation (macOS only); Windows has no backend yet")]
fn a_video_with_the_default_preload_loads_its_metadata() {
    check("video src default preload", false);
}

#[test]
fn a_video_with_preload_none_waits() {
    check("video src preload=none", true);
}

/// The box takes the video's ratio once the metadata is in: 100x75.
#[test]
#[cfg_attr(windows, ignore = "Windows media backend row: MEDIA-AVF plays media through AVFoundation (macOS only); Windows has no backend yet")]
fn a_videos_box_takes_its_ratio_after_metadata() {
    check("video width=100 after metadata", true);
}

/// The size is the pin here. The duration is not compared: this one-frame fixture's
/// movie and track say 0.1 s (what AVFoundation reports) while its one sample lasts
/// 1/15 s (what Chrome reports, 0.066667), an artifact of how AVAssetWriter writes a
/// single frame rather than a behaviour to match.
#[test]
#[cfg_attr(windows, ignore = "Windows media backend row: MEDIA-AVF plays media through AVFoundation (macOS only); Windows has no backend yet")]
fn a_small_videos_box_takes_its_size() {
    check_except(
        "video 32x24 after metadata",
        true,
        &["duration", "buffered", "seekable"],
    );
}

/// A `<source>` whose type canPlayType rejects is skipped.
#[test]
#[cfg_attr(windows, ignore = "Windows media backend row: MEDIA-AVF plays media through AVFoundation (macOS only); Windows has no backend yet")]
fn source_selection_skips_a_type_that_cannot_play() {
    check("video source selection", true);
}

#[test]
fn a_404_is_an_error() {
    check("video 404", true);
}

#[test]
#[cfg_attr(windows, ignore = "Windows media backend row: MEDIA-AVF plays media through AVFoundation (macOS only); Windows has no backend yet")]
fn bytes_that_are_not_media_are_an_error() {
    check("video not media", true);
}

#[test]
#[cfg_attr(windows, ignore = "Windows media backend row: MEDIA-AVF plays media through AVFoundation (macOS only); Windows has no backend yet")]
fn an_audio_element_loads_its_metadata() {
    check("audio src preload=auto", true);
}

/// Chrome 148's canPlayType answers (media_codecs_cft148.json) where AVFoundation
/// agrees; where it does not, RustKit's own answer and why.
#[test]
#[cfg_attr(windows, ignore = "Windows media backend row: MEDIA-AVF plays media through AVFoundation (macOS only); Windows has no backend yet")]
fn can_play_type_answers() {
    let receipt: serde_json::Value = serde_json::from_str(include_str!(
        "../../../docs/diagnostics/2026-11-18/media_codecs_cft148.json"
    ))
    .unwrap();
    // Differences: WebM and VP9 (AVFoundation does not play WebM), AV1 (not on every
    // Mac this runs on), HLS (kept out of Milestone 1 by Pete).
    let ours: &[(&str, &str)] = &[
        ("video/webm", ""),
        ("video/webm; codecs=\"vp9\"", ""),
        ("video/webm; codecs=\"vp9, opus\"", ""),
        ("video/mp4; codecs=\"av01.0.04M.08\"", "*"),
        ("application/vnd.apple.mpegurl", ""),
        ("audio/mpeg", "maybe"),
    ];
    let server = serve_media(vec![(
        "/",
        "text/html",
        b"<!doctype html><video id=m></video>".to_vec(),
    )]);
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds {
            x: 0,
            y: 0,
            width: 200,
            height: 200,
        })
        .expect("view");
    let url = Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap();
    rt().block_on(engine.load_url(view, url)).expect("load_url");
    let mut failures = Vec::new();
    for (mime, answers) in receipt["can"].as_object().unwrap() {
        let chrome = answers["canPlayType"].as_str().unwrap();
        let want = ours
            .iter()
            .find(|(m, _)| m == mime)
            .map_or(chrome, |(_, w)| *w);
        let got = read(
            &mut engine,
            view,
            &format!(
                "document.getElementById('m').canPlayType({})",
                serde_json::to_string(mime).unwrap()
            ),
        );
        if want != "*" && got != want {
            failures.push(format!(
                "{mime}: {got:?}, want {want:?} (Chrome {chrome:?})"
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
