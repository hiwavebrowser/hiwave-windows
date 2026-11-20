//! The engine's side of the main-thread `MediaHost` (MEDIA-AVF Milestone 1 step 3,
//! `docs/design/MEDIA_AVF.md` §3.2 and §3.3).
//!
//! `AVPlayer` and `AVPlayerItem` are main-thread only, so the players live on the main
//! thread, in a host that runs [`Command`]s and reports [`Snapshot`]s. This module is the
//! contract between that host and the engine, with no AVFoundation in it:
//!
//! - [`MediaBackend`]: the engine sends commands and drains each player's snapshots at
//!   its frame ticks. The real one is `avf_host::AvfHost` (feature `media-avf`); the
//!   engine's tests use a scripted fake.
//! - [`VideoFrames`]: a player's frame slot. The host attaches a [`FramePuller`] (the
//!   `AVPlayerItemVideoOutput`, which is `Send + Sync`) when it creates the player, and
//!   the engine pulls from it off the main thread.
//!
//! Nothing here calls back into the engine: the host only answers through the
//! snapshots the engine drains, so the engine decides every state change and event.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// A player, unique in the process (a view's elements and every view share the host).
pub type PlayerId = u64;

static NEXT_PLAYER: AtomicU64 = AtomicU64::new(1);

/// A new player id.
pub fn next_player_id() -> PlayerId {
    NEXT_PLAYER.fetch_add(1, Ordering::Relaxed)
}

/// What the engine asks of the host (§3.2). Step 3 has `Create`, `Load` and `Destroy`;
/// step 4 adds play, pause, seek, rate and volume.
pub enum Command {
    /// Make a player with no item yet, and its video output, attached to `frames`.
    Create {
        player: PlayerId,
        frames: Arc<VideoFrames>,
    },
    /// Give the player an item over `feed`'s resource: every byte it reads comes from
    /// the feed's source, which the engine fills through rustkit-net (§3.4).
    Load {
        player: PlayerId,
        feed: Arc<crate::avf::Feed>,
    },
    /// Drop the player, its item and its output. Not answered.
    Destroy { player: PlayerId },
}

impl Command {
    pub fn player(&self) -> PlayerId {
        match self {
            Command::Create { player, .. }
            | Command::Load { player, .. }
            | Command::Destroy { player } => *player,
        }
    }
}

/// The player's item, as the host read it (`AVPlayerItem.status`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ItemStatus {
    /// No item (created, not loaded).
    #[default]
    None,
    /// An item whose status is still unknown.
    Loading,
    /// `AVPlayerItemStatusReadyToPlay`.
    Ready,
    /// `AVPlayerItemStatusFailed`.
    Failed,
}

/// One player's state as the host read it (§3.2), sent after each command it runs and
/// on its timer while an item is loading. The host reads properties; it decides nothing.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Snapshot {
    pub player: PlayerId,
    /// How many of the player's commands the host has run (`Destroy` is never counted):
    /// a command is unanswered until a snapshot counts it.
    pub answered: u64,
    pub status: ItemStatus,
    pub current_time: f64,
    /// The item's duration, once known and finite.
    pub duration: Option<f64>,
    /// `timeControlStatus == Paused`.
    pub paused: bool,
    /// The item played to its end (`AVPlayerItemDidPlayToEndTimeNotification`, step 4).
    pub ended: bool,
    /// `presentationSize`, once the item has one.
    pub natural_size: Option<(f64, f64)>,
    /// `loadedTimeRanges`, in seconds.
    pub loaded: Vec<(f64, f64)>,
}

/// The main-thread host as the engine sees it.
pub trait MediaBackend: Send + Sync {
    /// Queue `command` to run on the host.
    fn send(&self, command: Command);
    /// The snapshots `player` sent since the last drain, oldest first.
    fn drain(&self, player: PlayerId) -> Vec<Snapshot>;
}

/// A decoded frame: `width` x `height` RGBA rows, not premultiplied (video is opaque).
#[derive(Clone, Debug, PartialEq)]
pub struct RgbaFrame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Where a player's frames come from: `pull` gives the frame to show now when it is a
/// new one. It is called off the main thread (§3.2).
pub trait FramePuller: Send + Sync {
    fn pull(&self) -> Option<RgbaFrame>;
}

/// A player's frame slot (§3.3): the host attaches the player's output, and the engine
/// pulls at its frame ticks. A frame can also be pushed (the engine tests' fake host).
#[derive(Default)]
pub struct VideoFrames {
    puller: Mutex<Option<Box<dyn FramePuller>>>,
    pushed: Mutex<Option<RgbaFrame>>,
}

impl VideoFrames {
    pub fn new() -> Arc<VideoFrames> {
        Arc::new(VideoFrames::default())
    }

    /// The host: this player's frames come from `puller` from now on.
    pub fn attach(&self, puller: Box<dyn FramePuller>) {
        *self.puller.lock().unwrap() = Some(puller);
    }

    /// Whether a puller is attached.
    pub fn attached(&self) -> bool {
        self.puller.lock().unwrap().is_some()
    }

    /// Hand the slot a frame (a fake host's).
    pub fn push(&self, frame: RgbaFrame) {
        *self.pushed.lock().unwrap() = Some(frame);
    }

    /// The new frame to show now, if there is one: a pushed one first, else the
    /// puller's. Only the newest is kept, so nothing queues.
    pub fn pull(&self) -> Option<RgbaFrame> {
        if let Some(frame) = self.pushed.lock().unwrap().take() {
            return Some(frame);
        }
        self.puller.lock().unwrap().as_ref().and_then(|p| p.pull())
    }
}

/// BGRA rows (`stride` bytes apart) to tight RGBA: the copy upload of step 1a.
pub fn bgra_to_rgba(bgra: &[u8], width: usize, height: usize, stride: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(width * height * 4);
    for y in 0..height {
        let row = &bgra[y * stride..y * stride + width * 4];
        for px in row.chunks_exact(4) {
            out.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bgra_rows_with_padding_become_tight_rgba() {
        // 2x2, stride 12 (4 bytes of padding a row).
        let bgra = [
            1, 2, 3, 255, 4, 5, 6, 255, 0, 0, 0, 0, //
            7, 8, 9, 255, 10, 11, 12, 255, 0, 0, 0, 0,
        ];
        assert_eq!(
            bgra_to_rgba(&bgra, 2, 2, 12),
            vec![3, 2, 1, 255, 6, 5, 4, 255, 9, 8, 7, 255, 12, 11, 10, 255]
        );
    }

    struct Once(Mutex<Option<RgbaFrame>>);
    impl FramePuller for Once {
        fn pull(&self) -> Option<RgbaFrame> {
            self.0.lock().unwrap().take()
        }
    }

    #[test]
    fn a_slot_gives_each_frame_once_newest_first() {
        let frames = VideoFrames::new();
        assert_eq!(frames.pull(), None);
        let frame = |n: u8| RgbaFrame {
            width: 1,
            height: 1,
            rgba: vec![n, n, n, 255],
        };
        frames.attach(Box::new(Once(Mutex::new(Some(frame(1))))));
        frames.push(frame(2));
        frames.push(frame(3));
        assert_eq!(frames.pull(), Some(frame(3)), "the newest pushed frame");
        assert_eq!(frames.pull(), Some(frame(1)), "then the puller's");
        assert_eq!(frames.pull(), None);
    }

    #[test]
    fn player_ids_are_unique() {
        let a = next_player_id();
        let b = next_player_id();
        assert_ne!(a, b);
    }
}
