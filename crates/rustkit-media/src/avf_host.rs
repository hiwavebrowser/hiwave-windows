//! The main-thread `MediaHost` (MEDIA-AVF Milestone 1 step 3, `docs/design/MEDIA_AVF.md`
//! §3.2 as amended 2026-11-19, and §3.3).
//!
//! `AVPlayer` and `AVPlayerItem` are main-thread only (`NS_SWIFT_UI_ACTOR`; objc2 makes
//! them `MainThreadOnly`). So every player lives here, on the main thread: [`AvfHost`]
//! dispatches each [`Command`] onto the main queue, where the host runs it against its
//! players, then reads the player and leaves a [`Snapshot`] for the engine to drain. While
//! an item's status is unknown, a short main-queue timer keeps reading it.
//!
//! There is no KVO and no callback into engine state: the host reads properties
//! (`status`, `timeControlStatus`, `currentTime`, `duration`, `presentationSize`,
//! `loadedTimeRanges`) and the engine decides what they mean.
//!
//! Frames (§3.3, step 1a): each player has an `AVPlayerItemVideoOutput` whose buffers are
//! IOSurface-backed BGRA. The output is `Send + Sync`, so it is handed to the player's
//! [`VideoFrames`] when the player is created, and the engine pulls from it on its own
//! thread, never on the main thread: `hasNewPixelBufferForItemTime` /
//! `copyPixelBufferForItemTime` at the item time for now, then a copy into RGBA.
//!
//! The main queue has to drain: in the app it is AppKit's main thread. A test needs a
//! `main` that runs the run loop (`tests/media_host.rs`, `harness = false`).

use crate::avf::Feed;
use crate::host::{
    bgra_to_rgba, Command, FramePuller, ItemStatus, MediaBackend, PlayerId, RgbaFrame, Snapshot,
};
use dispatch2::{DispatchQueue, DispatchTime};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{AllocAnyThread, MainThreadMarker};
use objc2_av_foundation::{
    AVPlayer, AVPlayerItem, AVPlayerItemStatus, AVPlayerItemVideoOutput, AVPlayerTimeControlStatus,
    AVURLAsset, NSValueAVFoundationExtensions,
};
use objc2_core_foundation::CFString;
use objc2_core_video::{
    kCVPixelBufferIOSurfacePropertiesKey, kCVPixelBufferMetalCompatibilityKey,
    kCVPixelBufferPixelFormatTypeKey, kCVPixelFormatType_32BGRA, CVPixelBufferGetBaseAddress,
    CVPixelBufferGetBytesPerRow, CVPixelBufferGetHeight, CVPixelBufferGetPixelFormatType,
    CVPixelBufferGetWidth, CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags,
    CVPixelBufferUnlockBaseAddress,
};
use objc2_foundation::{NSDictionary, NSNumber, NSString};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// How often the host reads a loading item.
const POLL: Duration = Duration::from_millis(20);

/// The snapshots waiting for the engine, by player. Shared by the host's main-thread
/// work (which adds) and the engine (which drains).
type Outbox = Arc<Mutex<HashMap<PlayerId, Vec<Snapshot>>>>;

/// The engine's handle on the main-thread host. Cheap to share; every view of an
/// engine can use one.
#[derive(Default)]
pub struct AvfHost {
    outbox: Outbox,
}

impl AvfHost {
    pub fn new() -> AvfHost {
        AvfHost::default()
    }
}

impl MediaBackend for AvfHost {
    fn send(&self, command: Command) {
        let outbox = self.outbox.clone();
        DispatchQueue::main().exec_async(move || {
            let mtm = MainThreadMarker::new().expect("the main queue runs on the main thread");
            run(command, &outbox, mtm);
        });
    }

    fn drain(&self, player: PlayerId) -> Vec<Snapshot> {
        self.outbox
            .lock()
            .unwrap()
            .remove(&player)
            .unwrap_or_default()
    }
}

/// One player on the main thread.
struct HostPlayer {
    player: Retained<AVPlayer>,
    output: Retained<AVPlayerItemVideoOutput>,
    item: Option<Retained<AVPlayerItem>>,
    /// Kept alive with the item: the resource loader holds its delegate weakly.
    feed: Option<Arc<Feed>>,
    answered: u64,
    /// A timer read is queued.
    polling: bool,
}

thread_local! {
    /// The players, on the main thread (only the main queue's work touches this).
    static PLAYERS: RefCell<HashMap<PlayerId, HostPlayer>> = RefCell::new(HashMap::new());
}

/// The pixel-buffer attributes of §3.3: IOSurface-backed, Metal-compatible BGRA.
fn output_attributes() -> Retained<NSDictionary<NSString, AnyObject>> {
    // SAFETY: CFString and NSString are toll-free bridged; the keys are static.
    let key = |k: &'static CFString| -> &'static NSString {
        unsafe { &*(k as *const CFString as *const NSString) }
    };
    let (format, metal, surface) = unsafe {
        (
            key(kCVPixelBufferPixelFormatTypeKey),
            key(kCVPixelBufferMetalCompatibilityKey),
            key(kCVPixelBufferIOSurfacePropertiesKey),
        )
    };
    let format_value = NSNumber::new_u32(kCVPixelFormatType_32BGRA);
    let metal_value = NSNumber::new_bool(true);
    let surface_value: Retained<NSDictionary<NSString, AnyObject>> = NSDictionary::new();
    let values: [&AnyObject; 3] = [&format_value, &metal_value, &surface_value];
    NSDictionary::from_slices(&[format, metal, surface], &values)
}

fn run(command: Command, outbox: &Outbox, mtm: MainThreadMarker) {
    let id = command.player();
    match command {
        Command::Create { frames, .. } => {
            let player = unsafe { AVPlayer::new(mtm) };
            let output = unsafe {
                AVPlayerItemVideoOutput::initWithPixelBufferAttributes(
                    AVPlayerItemVideoOutput::alloc(),
                    Some(&output_attributes()),
                )
            };
            frames.attach(Box::new(OutputPuller {
                output: output.clone(),
            }));
            PLAYERS.with(|p| {
                p.borrow_mut().insert(
                    id,
                    HostPlayer {
                        player,
                        output,
                        item: None,
                        feed: None,
                        answered: 0,
                        polling: false,
                    },
                )
            });
        }
        Command::Load { feed, .. } => {
            PLAYERS.with(|p| {
                let mut players = p.borrow_mut();
                let Some(host) = players.get_mut(&id) else {
                    return;
                };
                let asset: Retained<AVURLAsset> = feed.asset("player");
                let item = unsafe { AVPlayerItem::playerItemWithAsset(&asset, mtm) };
                unsafe {
                    item.addOutput(&host.output);
                    host.player.replaceCurrentItemWithPlayerItem(Some(&item));
                }
                host.item = Some(item);
                host.feed = Some(feed);
            });
        }
        Command::Destroy { .. } => {
            let gone = PLAYERS.with(|p| p.borrow_mut().remove(&id));
            if let Some(host) = gone {
                unsafe {
                    host.player.pause();
                    host.player.replaceCurrentItemWithPlayerItem(None);
                }
            }
            outbox.lock().unwrap().remove(&id);
            return;
        }
    }
    PLAYERS.with(|p| {
        if let Some(host) = p.borrow_mut().get_mut(&id) {
            host.answered += 1;
        }
    });
    report(id, outbox);
}

/// Read `id` and leave its snapshot; while its item is loading, read it again soon.
fn report(id: PlayerId, outbox: &Outbox) {
    let snapshot = PLAYERS.with(|p| {
        let mut players = p.borrow_mut();
        let host = players.get_mut(&id)?;
        let snapshot = read(id, host);
        let again = snapshot.status == ItemStatus::Loading && !host.polling;
        if again {
            host.polling = true;
        }
        Some((snapshot, again))
    });
    let Some((snapshot, again)) = snapshot else {
        return;
    };
    outbox.lock().unwrap().entry(id).or_default().push(snapshot);
    if again {
        let outbox = outbox.clone();
        let when = DispatchTime::try_from(POLL).unwrap_or(DispatchTime::NOW);
        let _ = DispatchQueue::main().after(when, move || {
            PLAYERS.with(|p| {
                if let Some(host) = p.borrow_mut().get_mut(&id) {
                    host.polling = false;
                }
            });
            report(id, &outbox);
        });
    }
}

fn finite(seconds: f64) -> Option<f64> {
    seconds.is_finite().then_some(seconds)
}

/// The player's properties now. Main thread.
fn read(id: PlayerId, host: &HostPlayer) -> Snapshot {
    let mut snapshot = Snapshot {
        player: id,
        answered: host.answered,
        paused: unsafe { host.player.timeControlStatus() } == AVPlayerTimeControlStatus::Paused,
        ..Snapshot::default()
    };
    let Some(item) = &host.item else {
        return snapshot;
    };
    unsafe {
        snapshot.status = match item.status() {
            AVPlayerItemStatus::ReadyToPlay => ItemStatus::Ready,
            AVPlayerItemStatus::Failed => ItemStatus::Failed,
            _ => ItemStatus::Loading,
        };
        snapshot.current_time = finite(item.currentTime().seconds()).unwrap_or(0.0);
        snapshot.duration = finite(item.duration().seconds());
        let size = item.presentationSize();
        if size.width > 0.0 && size.height > 0.0 {
            snapshot.natural_size = Some((size.width, size.height));
        }
        let ranges = item.loadedTimeRanges();
        snapshot.loaded = (0..ranges.count())
            .filter_map(|i| {
                let range = ranges.objectAtIndex(i).CMTimeRangeValue();
                let start = finite(range.start.seconds())?;
                let length = finite(range.duration.seconds())?;
                Some((start, start + length))
            })
            .collect();
    }
    snapshot
}

extern "C" {
    fn mach_absolute_time() -> u64;
}

/// A player's output, read off the main thread (§3.2: `AVPlayerItemVideoOutput` is
/// `NS_SWIFT_SENDABLE`).
struct OutputPuller {
    output: Retained<AVPlayerItemVideoOutput>,
}

impl FramePuller for OutputPuller {
    fn pull(&self) -> Option<RgbaFrame> {
        unsafe {
            // The item time to show now (the current time while paused).
            let now = self
                .output
                .itemTimeForMachAbsoluteTime(mach_absolute_time() as i64);
            if !self.output.hasNewPixelBufferForItemTime(now) {
                return None;
            }
            let buffer = self
                .output
                .copyPixelBufferForItemTime_itemTimeForDisplay(now, std::ptr::null_mut())?;
            if CVPixelBufferGetPixelFormatType(&buffer) != kCVPixelFormatType_32BGRA {
                return None;
            }
            if CVPixelBufferLockBaseAddress(&buffer, CVPixelBufferLockFlags::ReadOnly) != 0 {
                return None;
            }
            let width = CVPixelBufferGetWidth(&buffer);
            let height = CVPixelBufferGetHeight(&buffer);
            let stride = CVPixelBufferGetBytesPerRow(&buffer);
            let base = CVPixelBufferGetBaseAddress(&buffer) as *const u8;
            let frame =
                (!base.is_null() && width > 0 && height > 0 && stride >= width * 4).then(|| {
                    // SAFETY: the buffer is locked, and holds `height` rows of `stride` bytes.
                    let bytes = std::slice::from_raw_parts(base, stride * height);
                    RgbaFrame {
                        width: width as u32,
                        height: height as u32,
                        rgba: bgra_to_rgba(bytes, width, height, stride),
                    }
                });
            CVPixelBufferUnlockBaseAddress(&buffer, CVPixelBufferLockFlags::ReadOnly);
            frame
        }
    }
}
