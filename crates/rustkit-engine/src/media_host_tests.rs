//! MEDIA-AVF Milestone 1 step 3 (`docs/design/MEDIA_AVF.md` §3.2, §3.3, §5.1): the
//! engine's side of the main-thread `MediaHost`, against a scripted host.
//!
//! The players live on the main thread, so the engine only sends commands and drains
//! snapshots. Here a [`FakeHost`] stands in for the host (§5: "Event-order and
//! state-machine tests run on `FakeBackend`"): it records the commands, answers them
//! with the snapshots a test scripts, and its frames are pushed by the test. The real
//! host's own tests are `crates/rustkit-media/tests/media_host.rs`. The metadata is
//! still AVFoundation's (the step 2 probe, on this thread).
//!
//! Pinned (Chrome for Testing 148.0.7778.216, `tools/parity_oracle/media_frame_log.mjs`,
//! receipt `docs/diagnostics/2026-11-19/media_frame_cft148.json`): what a paused video
//! that never played paints once its first frame is in, at fixed points of the box,
//! under `object-fit: contain / cover / fill` and `object-position`. Chrome's decoder
//! gives frame 0 (encoded (255, 0, 0)) as about (233, 0, 2), and its compositor paints
//! about (227, 32, 0); the fake host's frame is the decoder's colour, and the points are
//! compared by the nearest fixture colour (white is the page), as step 1 planned.
//!
//! Also pinned, RustKit only (P-cost): a new frame is presented with no build and no
//! layout.

use super::*;
use crate::media_fetch_tests::serve_media;
use rustkit_media::host::{
    Command, ItemStatus, MediaBackend, PlayerId, RgbaFrame, Snapshot, VideoFrames,
};
use std::sync::Mutex;

const RECEIPT: &str = include_str!("../../../docs/diagnostics/2026-11-19/media_frame_cft148.json");
const COLOR_STEPS: &[u8] = include_bytes!("../../../fixtures/media/color-steps.mp4");

/// How the fake host answers `Load`.
#[derive(Clone, Copy, PartialEq)]
enum OnLoad {
    /// Ready, at the fixture's size and duration.
    Ready,
    /// The item fails.
    Fail,
    /// No answer (a command the host has not run yet).
    Silent,
}

#[derive(Default)]
struct FakeState {
    commands: Vec<(&'static str, PlayerId)>,
    outbox: HashMap<PlayerId, Vec<Snapshot>>,
    frames: HashMap<PlayerId, Arc<VideoFrames>>,
    answered: HashMap<PlayerId, u64>,
}

struct FakeHost {
    on_load: OnLoad,
    state: Mutex<FakeState>,
}

impl FakeHost {
    fn new(on_load: OnLoad) -> Arc<FakeHost> {
        Arc::new(FakeHost {
            on_load,
            state: Mutex::new(FakeState::default()),
        })
    }

    fn commands(&self) -> Vec<(&'static str, PlayerId)> {
        self.state.lock().unwrap().commands.clone()
    }

    /// The players created so far.
    fn players(&self) -> Vec<PlayerId> {
        self.commands()
            .into_iter()
            .filter(|(kind, _)| *kind == "create")
            .map(|(_, p)| p)
            .collect()
    }

    fn push_frame(&self, player: PlayerId, rgb: [u8; 3]) {
        let frames = self.state.lock().unwrap().frames[&player].clone();
        frames.push(RgbaFrame {
            width: 64,
            height: 48,
            rgba: (0..64 * 48)
                .flat_map(|_| [rgb[0], rgb[1], rgb[2], 255])
                .collect(),
        });
    }

    /// Send `player` a snapshot, as the host's timer would.
    fn report(&self, snapshot: Snapshot) {
        let mut state = self.state.lock().unwrap();
        let answered = state.answered.get(&snapshot.player).copied().unwrap_or(0);
        state
            .outbox
            .entry(snapshot.player)
            .or_default()
            .push(Snapshot {
                answered,
                ..snapshot
            });
    }
}

fn ready(player: PlayerId, size: (f64, f64)) -> Snapshot {
    Snapshot {
        player,
        status: ItemStatus::Ready,
        duration: Some(2.0),
        paused: true,
        natural_size: Some(size),
        ..Snapshot::default()
    }
}

impl MediaBackend for FakeHost {
    fn send(&self, command: Command) {
        let player = command.player();
        let mut state = self.state.lock().unwrap();
        let snapshot = match command {
            Command::Create { frames, .. } => {
                state.commands.push(("create", player));
                state.frames.insert(player, frames);
                Some(Snapshot {
                    player,
                    paused: true,
                    ..Snapshot::default()
                })
            }
            Command::Load { .. } => {
                state.commands.push(("load", player));
                match self.on_load {
                    OnLoad::Ready => Some(ready(player, (64.0, 48.0))),
                    OnLoad::Fail => Some(Snapshot {
                        player,
                        status: ItemStatus::Failed,
                        ..Snapshot::default()
                    }),
                    OnLoad::Silent => None,
                }
            }
            Command::Destroy { .. } => {
                state.commands.push(("destroy", player));
                state.outbox.remove(&player);
                None
            }
        };
        if let Some(snapshot) = snapshot {
            let answered = state.answered.entry(player).or_default();
            *answered += 1;
            let answered = *answered;
            state.outbox.entry(player).or_default().push(Snapshot {
                answered,
                ..snapshot
            });
        }
    }

    fn drain(&self, player: PlayerId) -> Vec<Snapshot> {
        self.state
            .lock()
            .unwrap()
            .outbox
            .remove(&player)
            .unwrap_or_default()
    }
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
        .replace("\\\"", "\"")
}

struct Page {
    engine: Engine,
    view: EngineViewId,
    rt: tokio::runtime::Runtime,
    _server: crate::media_fetch_tests::MediaServer,
}

impl Page {
    /// `body` on a white page, with the fixture served, and `host` as the engine's.
    fn open(host: Option<Arc<FakeHost>>, body: &str) -> Page {
        let html = format!(
            "<!doctype html><style>html,body{{margin:0;background:#fff}}video{{display:block}}</style>\
             <script>window.__ev=[];['resize','error','loadedmetadata'].forEach(function(t){{\
             document.addEventListener(t,function(e){{if(e.target.id==='m')__ev.push(t)}},true)}});</script>{body}"
        );
        let server = serve_media(vec![
            ("/", "text/html", html.into_bytes()),
            ("/color-steps.mp4", "video/mp4", COLOR_STEPS.to_vec()),
            ("/again.mp4", "video/mp4", COLOR_STEPS.to_vec()),
        ]);
        let mut engine = Engine::new(EngineConfig::default()).expect("engine");
        engine.set_media_backend(host.map(|h| h as Arc<dyn MediaBackend>));
        let view = engine
            .create_headless_view(Bounds {
                x: 0,
                y: 0,
                width: 300,
                height: 200,
            })
            .expect("view");
        let url = Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap();
        let rt = rt();
        rt.block_on(engine.load_url(view, url)).expect("load_url");
        Page {
            engine,
            view,
            rt,
            _server: server,
        }
    }

    fn pump(&mut self) {
        self.rt.block_on(self.engine.pump_live(self.view, 16));
    }

    /// Pump until `expr` reads "true", for up to 5 s.
    fn until(&mut self, expr: &str) {
        let started = std::time::Instant::now();
        while started.elapsed() < std::time::Duration::from_secs(5) {
            self.pump();
            if read(&mut self.engine, self.view, expr) == "true" {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("never: {expr}");
    }

    fn read(&mut self, expr: &str) -> String {
        read(&mut self.engine, self.view, expr)
    }

    fn metadata(&mut self) {
        self.until(
            "document.getElementById('m').readyState>=1||!!document.getElementById('m').error",
        );
    }

    fn stats(&self) -> RelayoutStats {
        self.engine.views[&self.view].relayout_stats.clone()
    }

    /// The image keys the view's display list names.
    fn image_keys(&self) -> Vec<String> {
        self.engine.views[&self.view]
            .display_list
            .as_ref()
            .map(|list| {
                list.commands
                    .iter()
                    .filter_map(|c| match c {
                        rustkit_layout::DisplayCommand::Image { url, .. } => Some(url.clone()),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The pixel at (x, y) of the presented frame (no render of its own: what the last
    /// present drew, captured).
    fn pixels(&mut self) -> (usize, Vec<u8>) {
        let path = std::env::temp_dir().join(format!(
            "rustkit-media-host-{}-{:?}-{:?}.ppm",
            std::process::id(),
            std::thread::current().id(),
            self.view
        ));
        self.engine
            .capture_frame(self.view, path.to_str().unwrap())
            .expect("capture");
        let ppm = std::fs::read(&path).expect("frame");
        let _ = std::fs::remove_file(&path);
        let mut fields = Vec::new();
        let mut pos = 0;
        while fields.len() < 4 {
            let start = pos;
            while !ppm[pos].is_ascii_whitespace() {
                pos += 1;
            }
            fields.push(std::str::from_utf8(&ppm[start..pos]).unwrap().to_string());
            pos += 1;
        }
        let width: usize = fields[1].parse().unwrap();
        (width, ppm[pos..].to_vec())
    }

    fn pixel(&mut self, x: usize, y: usize) -> [u8; 3] {
        let (width, rgb) = self.pixels();
        let at = (y * width + x) * 3;
        [rgb[at], rgb[at + 1], rgb[at + 2]]
    }
}

/// The nearest fixture colour (tools/media_fixtures), white included, to `rgb`.
fn nearest(rgb: [u8; 3]) -> &'static str {
    const TABLE: [(&str, [u8; 3]); 10] = [
        ("red", [255, 0, 0]),
        ("green", [0, 255, 0]),
        ("blue", [0, 0, 255]),
        ("yellow", [255, 255, 0]),
        ("cyan", [0, 255, 255]),
        ("magenta", [255, 0, 255]),
        ("white", [255, 255, 255]),
        ("black", [0, 0, 0]),
        ("orange", [255, 128, 0]),
        ("violet", [128, 0, 255]),
    ];
    let d = |c: [u8; 3]| -> i32 { (0..3).map(|i| (rgb[i] as i32 - c[i] as i32).pow(2)).sum() };
    TABLE.iter().min_by_key(|(_, c)| d(*c)).unwrap().0
}

fn receipt() -> serde_json::Value {
    let receipt: serde_json::Value = serde_json::from_str(RECEIPT).unwrap();
    assert_eq!(receipt["chrome"], "148.0.7778.216");
    receipt
}

fn rgb(value: &serde_json::Value) -> [u8; 3] {
    [0, 1, 2].map(|i| value[i].as_u64().unwrap() as u8)
}

/// §3.2: once a video's metadata is in, the engine makes it a player on the host
/// (create, then load) and its box paints under the player's key; a new resource is
/// a new player, and the old one is destroyed; so is the player of an element that
/// leaves the document.
#[test]
#[cfg_attr(windows, ignore = "Windows media backend row: the engine reads media metadata through AVFoundation (MEDIA-AVF step 2 probe, macOS only); Windows has no backend yet")]
fn a_video_gets_a_player_on_the_host_and_loses_it_with_its_resource() {
    let host = FakeHost::new(OnLoad::Ready);
    let mut page = Page::open(
        Some(host.clone()),
        "<video id=m preload=auto src='/color-steps.mp4'></video>",
    );
    page.metadata();
    page.pump();
    let players = host.players();
    assert_eq!(players.len(), 1, "{:?}", host.commands());
    let first = players[0];
    assert_eq!(host.commands(), vec![("create", first), ("load", first)]);
    let keys = page.image_keys();
    assert!(
        keys.iter()
            .any(|k| k.starts_with("rustkit-video:") && k.ends_with(&format!("@{first}"))),
        "the box paints under the player's key: {keys:?}"
    );
    // A new resource: the old player goes, a new one comes.
    page.engine
        .execute_script(page.view, "document.getElementById('m').src='/again.mp4'")
        .unwrap();
    page.until("document.getElementById('m').currentSrc.indexOf('again')>0&&document.getElementById('m').readyState>=1");
    page.pump();
    let commands = host.commands();
    let second = *host.players().last().unwrap();
    assert_ne!(second, first);
    assert!(commands.contains(&("destroy", first)), "{commands:?}");
    assert!(
        commands.ends_with(&[("create", second), ("load", second)]),
        "{commands:?}"
    );
    // The element leaves: its player goes.
    page.engine
        .execute_script(page.view, "document.getElementById('m').remove()")
        .unwrap();
    for _ in 0..4 {
        page.pump();
    }
    assert_eq!(host.commands().last(), Some(&("destroy", second)));
}

/// An `<audio>` gets a player too (its sound is step 4), with no frames and no key.
#[test]
#[cfg_attr(windows, ignore = "Windows media backend row: the engine reads media metadata through AVFoundation (MEDIA-AVF step 2 probe, macOS only); Windows has no backend yet")]
fn an_audio_element_gets_a_player_with_no_frames() {
    let host = FakeHost::new(OnLoad::Ready);
    let mut page = Page::open(
        Some(host.clone()),
        "<audio id=m controls preload=auto src='/color-steps.mp4'></audio>",
    );
    page.metadata();
    page.pump();
    assert_eq!(host.players().len(), 1, "{:?}", host.commands());
    assert!(!page
        .image_keys()
        .iter()
        .any(|k| k.starts_with("rustkit-video:")));
}

/// With no host (the default until step 6), nothing changes from step 2: no player.
#[test]
#[cfg_attr(windows, ignore = "Windows media backend row: the engine reads media metadata through AVFoundation (MEDIA-AVF step 2 probe, macOS only); Windows has no backend yet")]
fn with_no_host_a_video_loads_its_metadata_and_nothing_more() {
    let mut page = Page::open(
        None,
        "<video id=m preload=auto src='/color-steps.mp4'></video>",
    );
    page.metadata();
    for _ in 0..4 {
        page.pump();
    }
    assert_eq!(page.read("document.getElementById('m').videoWidth"), "64");
    assert_eq!(page.stats().video_frames_presented, 0);
}

/// P-fit and the first frame (Chrome 148): a paused video that never played paints
/// frame 0 in its box, letterboxed by `object-fit: contain`, filling it under `cover` and
/// `fill`, placed by `object-position`; what is outside the frame is the page's white.
#[test]
#[cfg_attr(windows, ignore = "Windows media backend row: the engine reads media metadata through AVFoundation (MEDIA-AVF step 2 probe, macOS only); Windows has no backend yet")]
fn the_first_frame_paints_as_chrome_paints_it() {
    let receipt = receipt();
    let frame0 = rgb(&receipt["cases"][0]["frame0_centre"]);
    let mut failures = Vec::new();
    for case in receipt["cases"].as_array().unwrap() {
        let label = case["label"].as_str().unwrap();
        let style = case["style"].as_str().unwrap();
        let host = FakeHost::new(OnLoad::Ready);
        let mut page = Page::open(
            Some(host.clone()),
            &format!("<video id=m preload=auto src='/color-steps.mp4' style='{style}'></video>"),
        );
        page.metadata();
        page.pump();
        let player = *host.players().first().expect("a player");
        host.push_frame(player, frame0);
        page.pump();
        page.pump();
        let box_chrome = case["box"].as_array().unwrap();
        let box_ours = page.read(
            "(function(){var r=document.getElementById('m').getBoundingClientRect();return r.width+'x'+r.height})()",
        );
        let want_box = format!("{}x{}", box_chrome[2], box_chrome[3]);
        if box_ours != want_box {
            failures.push(format!("{label}: box {box_ours}, Chrome {want_box}"));
        }
        for point in case["points"].as_array().unwrap() {
            let (x, y) = (
                point["x"].as_u64().unwrap() as usize,
                point["y"].as_u64().unwrap() as usize,
            );
            let chrome = rgb(&point["rgb"]);
            let ours = page.pixel(x, y);
            if nearest(ours) != nearest(chrome) {
                failures.push(format!(
                    "{label} ({x},{y}): ours {ours:?} ({}), Chrome {chrome:?} ({})",
                    nearest(ours),
                    nearest(chrome)
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

/// P-cost (RustKit only): a new frame changes no box, so it is presented with no build
/// and no layout; the painted pixel follows the frame.
#[test]
#[cfg_attr(windows, ignore = "Windows media backend row: the engine reads media metadata through AVFoundation (MEDIA-AVF step 2 probe, macOS only); Windows has no backend yet")]
fn a_new_frame_is_presented_with_no_build_and_no_layout() {
    let host = FakeHost::new(OnLoad::Ready);
    let mut page = Page::open(
        Some(host.clone()),
        "<video id=m preload=auto src='/color-steps.mp4'></video>",
    );
    page.metadata();
    page.pump();
    let player = host.players()[0];
    host.push_frame(player, [233, 0, 2]);
    page.pump();
    page.pump();
    assert_eq!(nearest(page.pixel(32, 24)), "red");
    let before = page.stats();
    let list = page.engine.views[&page.view].display_list.clone().unwrap();
    for (n, colour) in [[0, 255, 0], [0, 0, 255], [255, 255, 0]]
        .into_iter()
        .enumerate()
    {
        host.push_frame(player, colour);
        page.pump();
        assert_eq!(page.pixel(32, 24), colour, "frame {n}");
    }
    let after = page.stats();
    assert_eq!(after.relayouts, before.relayouts, "no relayout for a frame");
    assert_eq!(
        after.video_frames_presented,
        before.video_frames_presented + 3
    );
    assert!(
        Arc::ptr_eq(
            &list,
            page.engine.views[&page.view].display_list.as_ref().unwrap()
        ),
        "no build: the same display list"
    );
    // Nothing new: no present.
    page.pump();
    assert_eq!(
        page.stats().video_frames_presented,
        after.video_frames_presented
    );
}

/// A snapshot whose item failed is `MEDIA_ERR_DECODE`, with an `error` event, and the
/// player goes.
#[test]
#[cfg_attr(windows, ignore = "Windows media backend row: the engine reads media metadata through AVFoundation (MEDIA-AVF step 2 probe, macOS only); Windows has no backend yet")]
fn a_failed_item_is_a_decode_error() {
    let host = FakeHost::new(OnLoad::Fail);
    let mut page = Page::open(
        Some(host.clone()),
        "<video id=m preload=auto src='/color-steps.mp4'></video>",
    );
    page.until("!!document.getElementById('m').error");
    assert_eq!(page.read("document.getElementById('m').error.code"), "3");
    assert_eq!(page.read("__ev.join()"), "resize,loadedmetadata,error");
    let player = host.players()[0];
    assert_eq!(host.commands().last(), Some(&("destroy", player)));
}

/// A presentation size the host reports later (a resolution switch) is a `resize` and
/// a relayout of the box.
#[test]
#[cfg_attr(windows, ignore = "Windows media backend row: the engine reads media metadata through AVFoundation (MEDIA-AVF step 2 probe, macOS only); Windows has no backend yet")]
fn a_new_presentation_size_is_a_resize() {
    let host = FakeHost::new(OnLoad::Ready);
    let mut page = Page::open(
        Some(host.clone()),
        "<video id=m preload=auto src='/color-steps.mp4'></video>",
    );
    page.metadata();
    page.pump();
    assert_eq!(
        page.read("document.getElementById('m').getBoundingClientRect().width"),
        "64"
    );
    let player = host.players()[0];
    host.report(ready(player, (32.0, 24.0)));
    page.until("document.getElementById('m').videoWidth===32");
    page.pump();
    assert_eq!(page.read("document.getElementById('m').videoHeight"), "24");
    assert_eq!(
        page.read("document.getElementById('m').getBoundingClientRect().width"),
        "32"
    );
    assert_eq!(page.read("__ev.join()"), "resize,loadedmetadata,resize");
}

/// §3.2: while a command is unanswered the view's media is busy, so the live loop keeps
/// turning; once answered and framed it is not.
#[test]
#[cfg_attr(windows, ignore = "Windows media backend row: the engine reads media metadata through AVFoundation (MEDIA-AVF step 2 probe, macOS only); Windows has no backend yet")]
fn an_unanswered_command_keeps_the_media_busy() {
    let host = FakeHost::new(OnLoad::Silent);
    let mut page = Page::open(
        Some(host.clone()),
        "<video id=m preload=auto src='/color-steps.mp4'></video>",
    );
    page.metadata();
    for _ in 0..4 {
        page.pump();
    }
    assert!(
        page.engine.views[&page.view].media.busy(),
        "load unanswered"
    );
    let player = host.players()[0];
    // The host answers the load; then the first frame comes.
    {
        let mut state = host.state.lock().unwrap();
        *state.answered.entry(player).or_default() += 1;
    }
    host.report(ready(player, (64.0, 48.0)));
    page.pump();
    assert!(
        page.engine.views[&page.view].media.busy(),
        "waiting for the first frame"
    );
    host.push_frame(player, [233, 0, 2]);
    page.pump();
    page.pump();
    assert!(!page.engine.views[&page.view].media.busy());
}
