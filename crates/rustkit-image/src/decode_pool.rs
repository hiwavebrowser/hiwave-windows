//! The decode pool (threads plan, step D1).
//!
//! Two `rustkit-decode` threads behind a bounded queue of
//! [`DECODE_QUEUE`] jobs. Until this pool a fetched image was decoded on the
//! page thread inside the future that fetched it, so eBay's 84 images
//! decoded one after another between script and layout. Now the page
//! thread hands the bytes over and goes on; the future that fetched them
//! wakes when the pixels come back.
//!
//! The rules:
//! - Only plain data crosses: bytes in; pixels and size (or the error) out.
//!   No `Rc`, no engine or DOM type, no `Arc` to page state.
//! - The page thread never blocks on the pool. A full queue is not a wait:
//!   [`DecodePool::try_submit`] hands the bytes back and the caller decodes
//!   them inline, as before the pool.
//! - Every job carries its navigation ([`DecodeNav`]). A job whose
//!   navigation was superseded before it ran is not decoded, and a result
//!   that arrives after its navigation was superseded is dropped by the
//!   receiver, never cached.
//! - The threads start on the first job and end when the pool is dropped.
//!   The pool belongs to an `ImageManager`, so each engine has its own: a
//!   second engine (the opt-in RustKit chrome) starts two more threads.

use std::collections::HashMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, OnceLock};

use tokio::sync::oneshot;
use tracing::warn;

use crate::{DecodedPixels, ImageError, ImageResult};

/// Decode threads per pool.
pub const DECODE_THREADS: usize = 2;

/// Jobs the queue holds before the page thread decodes inline instead.
pub const DECODE_QUEUE: usize = 64;

/// The navigation a decode belongs to: the view's id and the view's
/// navigation generation when the image was asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DecodeNav {
    pub view: u64,
    pub nav_id: u64,
}

/// What comes back from a decode thread.
pub enum DecodeReply {
    Decoded(ImageResult<DecodedPixels>),
    /// The job's navigation was superseded before it ran: not decoded.
    Superseded,
}

/// Counters, for tests and the diagnostic snapshot.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DecodeStats {
    /// Jobs a decode thread took.
    pub pooled: u64,
    /// Decodes the page thread did itself because the queue was full.
    pub inline: u64,
    /// Jobs or results dropped for a superseded navigation.
    pub superseded: u64,
}

#[derive(Default)]
struct Counters {
    pooled: AtomicU64,
    inline: AtomicU64,
    superseded: AtomicU64,
}

struct DecodeJob {
    nav: DecodeNav,
    bytes: Vec<u8>,
    max_dimensions: (u32, u32),
    reply: oneshot::Sender<DecodeReply>,
}

/// The latest navigation of each view that has started one. A view with no
/// entry has never navigated, so every job for it is current.
#[derive(Default)]
struct NavTable(Mutex<HashMap<u64, u64>>);

impl NavTable {
    fn is_current(&self, nav: DecodeNav) -> bool {
        let table = self.0.lock().unwrap_or_else(|e| e.into_inner());
        table
            .get(&nav.view)
            .is_none_or(|&latest| latest == nav.nav_id)
    }
}

/// See the module documentation.
#[derive(Default)]
pub struct DecodePool {
    /// `None` inside: the threads could not be spawned; every decode is
    /// inline.
    queue: OnceLock<Option<SyncSender<DecodeJob>>>,
    navs: Arc<NavTable>,
    counters: Arc<Counters>,
}

impl DecodePool {
    pub fn new() -> Self {
        Self::default()
    }

    /// `view` has started navigation `nav_id`: jobs and results for its
    /// earlier navigations are dropped from now on.
    pub fn begin_navigation(&self, view: u64, nav_id: u64) {
        let mut table = self.navs.0.lock().unwrap_or_else(|e| e.into_inner());
        table.insert(view, nav_id);
    }

    /// `view` is gone: drop its entry. Its in-flight fetches died with it.
    pub fn forget_view(&self, view: u64) {
        let mut table = self.navs.0.lock().unwrap_or_else(|e| e.into_inner());
        table.remove(&view);
    }

    /// Whether `nav` is still its view's latest navigation.
    pub fn is_current(&self, nav: DecodeNav) -> bool {
        self.navs.is_current(nav)
    }

    pub fn stats(&self) -> DecodeStats {
        DecodeStats {
            pooled: self.counters.pooled.load(Ordering::Relaxed),
            inline: self.counters.inline.load(Ordering::Relaxed),
            superseded: self.counters.superseded.load(Ordering::Relaxed),
        }
    }

    pub(crate) fn note_inline(&self) {
        self.counters.inline.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn note_superseded(&self) {
        self.counters.superseded.fetch_add(1, Ordering::Relaxed);
    }

    /// Queue `bytes` for a decode thread. Never blocks: with the queue full
    /// (or no threads), the bytes come back for the caller to decode inline.
    pub fn try_submit(
        &self,
        nav: DecodeNav,
        bytes: Vec<u8>,
        max_dimensions: (u32, u32),
    ) -> Result<oneshot::Receiver<DecodeReply>, Vec<u8>> {
        let Some(queue) = self.queue.get_or_init(|| self.start()) else {
            return Err(bytes);
        };
        let (reply, receiver) = oneshot::channel();
        let job = DecodeJob {
            nav,
            bytes,
            max_dimensions,
            reply,
        };
        match queue.try_send(job) {
            Ok(()) => Ok(receiver),
            Err(TrySendError::Full(job) | TrySendError::Disconnected(job)) => Err(job.bytes),
        }
    }

    fn start(&self) -> Option<SyncSender<DecodeJob>> {
        let (sender, receiver) = sync_channel::<DecodeJob>(DECODE_QUEUE);
        let receiver = Arc::new(Mutex::new(receiver));
        let mut started = 0;
        for i in 0..DECODE_THREADS {
            let receiver = receiver.clone();
            let navs = self.navs.clone();
            let counters = self.counters.clone();
            let spawned = std::thread::Builder::new()
                .name(format!("rustkit-decode-{i}"))
                .spawn(move || decode_thread(&receiver, &navs, &counters));
            match spawned {
                Ok(_) => started += 1,
                Err(e) => warn!(error = %e, "Could not start a decode thread"),
            }
        }
        (started > 0).then_some(sender)
    }
}

/// Runs `decode` and turns a panic into [`ImageError::DecodeError`] so a
/// hostile codec cannot unwind the pool thread or the page thread's inline
/// path. Both call sites go through [`decode_pixels_caught`].
pub(crate) fn catch_decode_panic<T>(decode: impl FnOnce() -> ImageResult<T>) -> ImageResult<T> {
    catch_unwind(AssertUnwindSafe(decode))
        .unwrap_or_else(|_| Err(ImageError::DecodeError("decoder panicked".into())))
}

/// [`crate::decode_pixels`], with a codec that panics on hostile bytes
/// failing this image instead of unwinding the thread. Both the pool and
/// the page thread's inline decode (queue full) use it, so the two paths
/// fail the same way.
pub(crate) fn decode_pixels_caught(
    bytes: &[u8],
    max_dimensions: (u32, u32),
) -> ImageResult<DecodedPixels> {
    catch_decode_panic(|| crate::decode_pixels(bytes, max_dimensions))
}

fn decode_thread(receiver: &Mutex<Receiver<DecodeJob>>, navs: &NavTable, counters: &Counters) {
    loop {
        // The lock is held only while waiting for the next job.
        let job = match receiver.lock() {
            Ok(receiver) => receiver.recv(),
            Err(_) => return,
        };
        // Every sender is gone: the pool was dropped.
        let Ok(job) = job else { return };
        counters.pooled.fetch_add(1, Ordering::Relaxed);
        let reply = if navs.is_current(job.nav) {
            DecodeReply::Decoded(decode_pixels_caught(&job.bytes, job.max_dimensions))
        } else {
            counters.superseded.fetch_add(1, Ordering::Relaxed);
            DecodeReply::Superseded
        };
        // The receiver may be gone (its fetch was dropped); nothing to do.
        let _ = job.reply.send(reply);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ImageManager;
    use url::Url;

    /// 640 x 480 JPEG, ~88 KB: a decode that takes real time.
    const JPEG: &[u8] =
        include_bytes!("../../../websuite/js-ladder/01-mdn/32-image-gallery/images/pic4.jpg");

    const NAV: DecodeNav = DecodeNav { view: 1, nav_id: 1 };

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    fn url(i: usize) -> Url {
        Url::parse(&format!("http://example.test/{i}.jpg")).unwrap()
    }

    #[test]
    fn a_panicking_decode_becomes_a_decode_error() {
        // Without the catch, a codec panic kills the pool thread (or the
        // page thread on the inline overflow path). Both call sites share
        // catch_decode_panic so one pin covers the contract.
        let err = catch_decode_panic(|| -> ImageResult<()> {
            panic!("hostile codec");
        })
        .expect_err("a panic must become DecodeError");
        match err {
            ImageError::DecodeError(msg) => {
                assert!(msg.contains("panicked"), "{msg}");
            }
            other => panic!("expected DecodeError, got {other:?}"),
        }
        assert_eq!(
            catch_decode_panic(|| Ok::<_, ImageError>(7)).expect("Ok passes through"),
            7
        );
    }

    #[test]
    fn a_full_queue_hands_the_bytes_back_instead_of_blocking() {
        let pool = DecodePool::new();
        let started = std::time::Instant::now();
        let mut accepted = Vec::new();
        let mut refused = 0;
        for _ in 0..200 {
            match pool.try_submit(NAV, JPEG.to_vec(), (16384, 16384)) {
                Ok(reply) => accepted.push(reply),
                Err(bytes) => {
                    assert_eq!(bytes.len(), JPEG.len(), "the caller gets its bytes back");
                    refused += 1;
                }
            }
        }
        // 200 submissions take no decode time: none of them waited for a slot.
        assert!(
            started.elapsed() < std::time::Duration::from_secs(1),
            "{:?}",
            started.elapsed()
        );
        assert!(accepted.len() >= DECODE_QUEUE, "{}", accepted.len());
        assert!(refused > 0, "the queue holds {DECODE_QUEUE} jobs, not 200");
        runtime().block_on(async {
            for reply in accepted {
                match reply.await.expect("a decode thread answered") {
                    DecodeReply::Decoded(Ok(pixels)) => {
                        assert_eq!((pixels.width, pixels.height), (640, 480))
                    }
                    DecodeReply::Decoded(Err(e)) => panic!("{e}"),
                    DecodeReply::Superseded => panic!("nothing superseded it"),
                }
            }
        });
    }

    #[test]
    fn a_burst_over_the_queue_all_decodes_some_inline() {
        let manager = ImageManager::new();
        let burst = 3 * DECODE_QUEUE;
        let results = runtime().block_on(futures::future::join_all((0..burst).map(|i| {
            let manager = &manager;
            async move {
                manager
                    .decode_fetched(&url(i), Some("image/jpeg"), JPEG.to_vec(), NAV)
                    .await
            }
        })));
        for result in &results {
            let image = result.as_ref().expect("decoded");
            assert_eq!((image.natural_width, image.natural_height), (640, 480));
        }
        let stats = manager.decode_stats();
        assert_eq!(stats.pooled + stats.inline, burst as u64, "{stats:?}");
        assert!(
            stats.inline > 0,
            "the overflow decoded on the caller: {stats:?}"
        );
        assert!(stats.pooled >= DECODE_QUEUE as u64, "{stats:?}");
    }

    #[test]
    fn a_decode_for_a_superseded_navigation_is_dropped() {
        let manager = ImageManager::new();
        manager.begin_navigation(NAV.view, NAV.nav_id);
        let late = url(999);
        let urls: Vec<Url> = (0..8).map(url).collect();
        runtime().block_on(async {
            // Work ahead of it in the queue, so its result cannot be back
            // before the navigation moves on.
            let ahead: Vec<_> = urls
                .iter()
                .map(|u| manager.decode_fetched(u, None, JPEG.to_vec(), NAV))
                .collect();
            let mut ahead = Box::pin(futures::future::join_all(ahead));
            let mut old = Box::pin(manager.decode_fetched(&late, None, JPEG.to_vec(), NAV));
            assert!(futures::poll!(&mut ahead).is_pending());
            assert!(
                futures::poll!(&mut old).is_pending(),
                "submitted, not decoded yet"
            );
            manager.begin_navigation(NAV.view, NAV.nav_id + 1);
            assert!(matches!(old.await, Err(ImageError::Superseded)));
            ahead.await;
        });
        assert!(
            !manager.is_cached(&late),
            "a superseded result is never cached"
        );
        assert!(manager.decode_stats().superseded >= 1);

        // The view's new navigation decodes as usual; another view never
        // noticed.
        let next = DecodeNav {
            view: NAV.view,
            nav_id: NAV.nav_id + 1,
        };
        let other = DecodeNav { view: 2, nav_id: 7 };
        runtime().block_on(async {
            manager
                .decode_fetched(&late, None, JPEG.to_vec(), next)
                .await
                .expect("current");
            manager
                .decode_fetched(&url(1000), None, JPEG.to_vec(), other)
                .await
                .expect("other view");
        });
    }

    #[test]
    fn a_destroyed_view_leaves_no_entry() {
        let pool = DecodePool::new();
        pool.begin_navigation(5, 2);
        assert!(!pool.is_current(DecodeNav { view: 5, nav_id: 1 }));
        pool.forget_view(5);
        assert!(pool.navs.0.lock().unwrap().is_empty());
    }

    #[test]
    fn decode_errors_are_the_inline_ones() {
        let manager = ImageManager::new();
        let truncated = &JPEG[..JPEG.len() / 3];
        for (i, bytes) in [
            b"not an image".as_slice(),
            truncated,
            &[0x89, b'P', b'N', b'G'],
        ]
        .into_iter()
        .enumerate()
        {
            let inline = manager
                .insert_fetched(&url(i), Some("image/jpeg"), bytes)
                .err();
            let pooled = runtime()
                .block_on(manager.decode_fetched(&url(i), Some("image/jpeg"), bytes.to_vec(), NAV))
                .err();
            assert_eq!(
                inline.as_ref().map(ToString::to_string),
                pooled.as_ref().map(ToString::to_string),
                "case {i}"
            );
            assert!(!manager.is_cached(&url(i)), "case {i}");
        }
        assert!(manager.decode_stats().pooled > 0);

        // SVG by its type is handed back before any decode, as before.
        let svg = runtime().block_on(manager.decode_fetched(
            &url(50),
            Some("image/svg+xml"),
            b"<svg/>".to_vec(),
            NAV,
        ));
        assert!(matches!(svg, Err(ImageError::Svg(xml)) if xml == "<svg/>"));
    }
}
