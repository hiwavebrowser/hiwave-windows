//! The real main-thread `MediaHost` (MEDIA-AVF Milestone 1 step 3,
//! `docs/design/MEDIA_AVF.md` §3.2, §3.3, §5.1).
//!
//! `AVPlayer` and `AVPlayerItem` are main-thread only, and the host runs its commands on
//! the main queue. libtest runs tests on worker threads and parks its main thread, so
//! this binary has no harness: `main` runs the main run loop (which drains the main
//! queue, as AppKit's does in the app) and a worker thread plays the engine, sending
//! commands, feeding the bytes the players read and draining snapshots. The worker ends
//! the process with the result.
//!
//! Pinned against Chrome for Testing 148.0.7778.216
//! (`docs/diagnostics/2026-11-19/media_frame_cft148.json`, from
//! `tools/parity_oracle/media_frame_log.mjs`): a paused video that never played shows
//! frame 0, whose colour Chrome's decoder gives as (233, 0, 2) for an encoded
//! (255, 0, 0). Run with `cargo test -p rustkit-media --features media-avf --test media_host`.

#![cfg(target_os = "macos")]

use bytes::Bytes;
use rustkit_media::avf::{Feed, MediaSource, CHUNK};
use rustkit_media::avf_host::AvfHost;
use rustkit_media::host::{
    next_player_id, Command, ItemStatus, MediaBackend, PlayerId, RgbaFrame, Snapshot, VideoFrames,
};
use std::sync::Arc;
use std::time::{Duration, Instant};

const RECEIPT: &str = include_str!("../../../docs/diagnostics/2026-11-19/media_frame_cft148.json");

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(format!(
        "{}/../../fixtures/media/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .expect("fixture")
}

/// One player, driven as the engine drives it.
struct Rig {
    host: Arc<AvfHost>,
    player: PlayerId,
    frames: Arc<VideoFrames>,
    source: Arc<MediaSource>,
    feed: Arc<Feed>,
    resource: Vec<u8>,
    /// Every span the player read, as (start, end).
    fetched: Vec<(u64, u64)>,
    snapshots: Vec<Snapshot>,
}

impl Rig {
    fn new(host: &Arc<AvfHost>, resource: Vec<u8>) -> Rig {
        let source = MediaSource::new();
        Rig {
            host: host.clone(),
            player: next_player_id(),
            frames: VideoFrames::new(),
            feed: Feed::new(source.clone()),
            source,
            resource,
            fetched: Vec::new(),
            snapshots: Vec::new(),
        }
    }

    fn create(&self) {
        self.host.send(Command::Create {
            player: self.player,
            frames: self.frames.clone(),
        });
    }

    fn load(&self) {
        self.host.send(Command::Load {
            player: self.player,
            feed: self.feed.clone(),
        });
    }

    /// One engine tick: answer what the player waits for (a ranged server, a chunk at
    /// a time), and drain its snapshots.
    fn tick(&mut self) {
        let wants = self.source.take_wants(u64::MAX);
        for (start, end) in &wants {
            let total = self.resource.len() as u64;
            let end = (*end).min(start + CHUNK).min(total);
            self.source.set_info(Some("video/mp4".into()), total);
            if *start < end {
                self.source.add(
                    *start,
                    Bytes::copy_from_slice(&self.resource[*start as usize..end as usize]),
                );
            }
            self.fetched.push((*start, end));
        }
        if !wants.is_empty() {
            self.feed.bytes_changed();
        }
        self.snapshots.extend(self.host.drain(self.player));
    }

    /// Tick until `done` holds of the newest snapshot, for up to 5 s.
    fn until(&mut self, what: &str, done: impl Fn(&Snapshot) -> bool) -> Snapshot {
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(5) {
            self.tick();
            if let Some(last) = self.snapshots.last() {
                if done(last) {
                    return last.clone();
                }
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("{what}: never; snapshots {:?}", self.snapshots);
    }

    /// Tick until a frame comes, for up to 5 s.
    fn frame(&mut self) -> Option<RgbaFrame> {
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(5) {
            self.tick();
            if let Some(frame) = self.frames.pull() {
                return Some(frame);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        None
    }
}

/// The nearest of the fixture's colours (tools/media_fixtures) to `rgb`.
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

/// Commands run on the main thread and are answered by snapshots: create answers with
/// no item; load gives an item that reads ready, with the fixture's size and duration.
fn create_and_load_are_answered(host: &Arc<AvfHost>) {
    let mut rig = Rig::new(host, fixture("color-steps.mp4"));
    rig.create();
    let created = rig.until("create answered", |s| s.answered == 1);
    assert_eq!(created.status, ItemStatus::None);
    assert!(created.paused, "a new player is paused");
    rig.load();
    let ready = rig.until("ready", |s| {
        s.answered == 2 && matches!(s.status, ItemStatus::Ready | ItemStatus::Failed)
    });
    assert_eq!(ready.status, ItemStatus::Ready, "{:?}", rig.snapshots);
    assert_eq!(ready.natural_size, Some((64.0, 48.0)));
    let duration = ready.duration.expect("a duration");
    assert!((duration - 2.0).abs() < 0.01, "duration {duration}");
    assert!(ready.paused);
    assert_eq!(ready.current_time, 0.0);
    // While the item loaded, the host's timer read it again: more than one snapshot.
    assert!(
        rig.snapshots
            .iter()
            .any(|s| s.status == ItemStatus::Loading),
        "{:?}",
        rig.snapshots
    );
    host.send(Command::Destroy { player: rig.player });
}

/// P-net (§5.1): every byte the player read came through the feed, so through what the
/// engine fetched (rustkit-net and the shield in the engine): the asset is on a private
/// scheme Apple's loader cannot fetch.
fn every_byte_comes_through_the_feed(host: &Arc<AvfHost>) {
    let mut rig = Rig::new(host, fixture("color-steps.mp4"));
    rig.create();
    rig.load();
    rig.until("ready", |s| s.status == ItemStatus::Ready);
    assert!(!rig.fetched.is_empty());
    assert!(rig.source.held() > 0);
    host.send(Command::Destroy { player: rig.player });
}

/// Frames, step 1a (§3.3): a paused player that never played shows frame 0, pulled off
/// the main thread, as RGBA of the video's size. Its centre is frame 0's colour as
/// Chrome 148's decoder gives it.
fn the_first_frame_is_pulled_off_the_main_thread(host: &Arc<AvfHost>) {
    let receipt: serde_json::Value = serde_json::from_str(RECEIPT).unwrap();
    assert_eq!(receipt["chrome"], "148.0.7778.216");
    let chrome = &receipt["cases"][0]["frame0_centre"];
    let chrome = [0, 1, 2].map(|i| chrome[i].as_u64().unwrap() as u8);
    let mut rig = Rig::new(host, fixture("color-steps.mp4"));
    rig.create();
    rig.load();
    assert!(
        std::thread::current().name() != Some("main"),
        "the frame is pulled on the engine's thread"
    );
    let frame = rig.frame().expect("frame 0");
    assert_eq!((frame.width, frame.height), (64, 48));
    assert_eq!(frame.rgba.len(), 64 * 48 * 4);
    let at = ((24 * 64 + 32) * 4) as usize;
    let ours = [frame.rgba[at], frame.rgba[at + 1], frame.rgba[at + 2]];
    assert_eq!(nearest(ours), "red", "ours {ours:?}");
    assert_eq!(nearest(chrome), "red", "Chrome {chrome:?}");
    let off = (0..3).map(|i| ours[i].abs_diff(chrome[i])).max().unwrap();
    assert!(off <= 8, "ours {ours:?} vs Chrome 148 {chrome:?}");
    assert_eq!(frame.rgba[at + 3], 255, "opaque");
    // One frame while paused: nothing new until the time moves.
    assert_eq!(rig.frames.pull(), None);
    host.send(Command::Destroy { player: rig.player });
}

/// Bytes that are not media: the item fails, and says so in a snapshot.
fn bytes_that_are_not_media_fail_the_item(host: &Arc<AvfHost>) {
    let mut rig = Rig::new(host, b"this is not a media file ".repeat(40));
    rig.create();
    rig.load();
    let last = rig.until("failed", |s| {
        matches!(s.status, ItemStatus::Failed | ItemStatus::Ready)
    });
    assert_eq!(last.status, ItemStatus::Failed);
    host.send(Command::Destroy { player: rig.player });
}

/// Destroy drops the player: no snapshot comes after it, and a load sent after it is
/// not answered.
fn destroy_drops_the_player(host: &Arc<AvfHost>) {
    let mut rig = Rig::new(host, fixture("color-steps.mp4"));
    rig.create();
    rig.until("create answered", |s| s.answered == 1);
    host.send(Command::Destroy { player: rig.player });
    rig.load();
    let started = Instant::now();
    while started.elapsed() < Duration::from_millis(300) {
        rig.tick();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(rig.snapshots.len(), 1, "{:?}", rig.snapshots);
}

type Case = (&'static str, fn(&Arc<AvfHost>));

fn main() {
    let cases: Vec<Case> = vec![
        ("create_and_load_are_answered", create_and_load_are_answered),
        (
            "every_byte_comes_through_the_feed",
            every_byte_comes_through_the_feed,
        ),
        (
            "the_first_frame_is_pulled_off_the_main_thread",
            the_first_frame_is_pulled_off_the_main_thread,
        ),
        (
            "bytes_that_are_not_media_fail_the_item",
            bytes_that_are_not_media_fail_the_item,
        ),
        ("destroy_drops_the_player", destroy_drops_the_player),
    ];
    let filter: Vec<String> = std::env::args()
        .skip(1)
        .filter(|a| !a.starts_with('-'))
        .collect();
    std::thread::Builder::new()
        .name("engine".into())
        .spawn(move || {
            let host = Arc::new(AvfHost::new());
            let mut failed = 0;
            let mut ran = 0;
            for (name, case) in cases {
                if !filter.is_empty() && !filter.iter().any(|f| name.contains(f.as_str())) {
                    continue;
                }
                ran += 1;
                let host = host.clone();
                match std::panic::catch_unwind(move || case(&host)) {
                    Ok(()) => println!("test {name} ... ok"),
                    Err(_) => {
                        println!("test {name} ... FAILED");
                        failed += 1;
                    }
                }
            }
            println!(
                "\ntest result: {}. {} passed; {failed} failed",
                if failed == 0 { "ok" } else { "FAILED" },
                ran - failed
            );
            std::process::exit(if failed == 0 { 0 } else { 101 });
        })
        .expect("the engine thread");
    // The main thread drains the main queue, where the host runs.
    objc2_core_foundation::CFRunLoop::run();
}
