//! AVFoundation glue for MEDIA-AVF (`docs/design/MEDIA_AVF.md`).
//!
//! Step 1 added the dependency and `is_playable_extended_mime_type`. Step 2 adds
//! [`Probe`]: an `AVURLAsset` on a private URL scheme whose every byte comes from a
//! [`MediaSource`] the engine fills through rustkit-net (and so through the shield),
//! answered by an `AVAssetResourceLoaderDelegate` on a serial dispatch queue of our own
//! (§3.4). AVFoundation asks for byte ranges as it reads; what is not held yet becomes a
//! *want* the engine fetches by `Range`, a chunk at a time. So loading the metadata
//! fetches what AVFoundation reads for it (the header, and the `moov` atom wherever it
//! sits), never the whole resource. The probe loads `playable`, `duration` and `tracks`
//! and reports them: what `loadedmetadata` needs. It never plays.
//!
//! Threading: `AVURLAsset` and its resource loader are not main-thread only, so a probe
//! is created and polled on the engine's thread. Its delegate, and everything it keeps,
//! is touched only on the probe's own queue. (`AVPlayer` and `AVPlayerItem` are
//! main-thread only: step 3's `MediaHost`, §3.2.)

use bytes::Bytes;
use dispatch2::{DispatchQueue, DispatchRetained};
use objc2::rc::Retained;
use objc2::runtime::{NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{define_class, msg_send, AllocAnyThread, DefinedClass, Message};
use objc2_av_foundation::{
    AVAssetResourceLoader, AVAssetResourceLoaderDelegate, AVAssetResourceLoadingRequest,
    AVAsynchronousKeyValueLoading, AVMediaTypeAudio, AVMediaTypeVideo, AVURLAsset,
};
use objc2_foundation::{NSArray, NSData, NSString, NSURL};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// Whether AVFoundation can play `mime`, an extended MIME type (`video/mp4;
/// codecs="avc1.42E01E"`): `+[AVURLAsset isPlayableExtendedMIMEType:]`.
pub fn is_playable_extended_mime_type(mime: &str) -> bool {
    let mime = NSString::from_str(mime);
    unsafe { AVURLAsset::isPlayableExtendedMIMEType(&mime) }
}

/// The most one ranged fetch asks for. AVFoundation may ask for the rest of a resource;
/// it gets it a chunk at a time, as it reads.
pub const CHUNK: u64 = 1 << 20;

/// A media resource's bytes as the engine has fetched them so far, shared by the engine
/// (which fetches) and the probe's delegate (which answers AVFoundation from them).
/// Chunks are kept as rustkit-net returned them (`Bytes`): stored without a copy.
#[derive(Default)]
pub struct MediaSource {
    state: Mutex<SourceState>,
}

#[derive(Default)]
struct SourceState {
    content_type: Option<String>,
    total: Option<u64>,
    /// Fetched spans by start offset.
    chunks: BTreeMap<u64, Bytes>,
    /// Spans AVFoundation asked for and that are not held: [start, end).
    wants: Vec<(u64, u64)>,
    failed: bool,
}

impl MediaSource {
    pub fn new() -> Arc<MediaSource> {
        Arc::new(MediaSource::default())
    }

    /// The response's `Content-Type` and the resource's length, from the first response
    /// (`Content-Range`'s total, or a whole 200's length).
    pub fn set_info(&self, content_type: Option<String>, total: u64) {
        let mut s = self.state.lock().unwrap();
        if s.content_type.is_none() {
            s.content_type = content_type;
        }
        s.total.get_or_insert(total);
    }

    /// Keep `bytes`, fetched from `start`. A span whose end would overflow is not kept
    /// (the engine refuses such a response before this; this is the store's own guard).
    pub fn add(&self, start: u64, bytes: Bytes) {
        let mut s = self.state.lock().unwrap();
        if bytes.is_empty()
            || s.chunks.contains_key(&start)
            || start.checked_add(bytes.len() as u64).is_none()
        {
            return;
        }
        s.chunks.insert(start, bytes);
    }

    /// The fetch failed (refused, blocked, not found): every waiting request fails.
    pub fn fail(&self) {
        self.state.lock().unwrap().failed = true;
    }

    pub fn failed(&self) -> bool {
        self.state.lock().unwrap().failed
    }

    /// The spans AVFoundation waits for that the engine is to fetch now, taken (each is
    /// fetched once). A span that only continues what is held (AVFoundation reading on,
    /// sequentially) is left waiting once `sequential_budget` bytes are held: loading
    /// metadata, AVFoundation reads ahead far past what it needs, and only a seek (a span
    /// that starts where nothing is held, the `moov` atom at the end say) is still
    /// fetched past the budget. Playback (step 3) raises the budget.
    pub fn take_wants(&self, sequential_budget: u64) -> Vec<(u64, u64)> {
        let mut s = self.state.lock().unwrap();
        let held: u64 = s.chunks.values().map(|b| b.len() as u64).sum();
        let continues = |start: u64, s: &SourceState| {
            start > 0
                && s.chunks
                    .range(..start)
                    .next_back()
                    .is_some_and(|(a, b)| a.saturating_add(b.len() as u64) == start)
        };
        let (now, later): (Vec<(u64, u64)>, Vec<(u64, u64)>) = std::mem::take(&mut s.wants)
            .into_iter()
            .partition(|(start, _)| held < sequential_budget || !continues(*start, &s));
        s.wants = later;
        now
    }

    /// Bytes held.
    pub fn held(&self) -> usize {
        self.state
            .lock()
            .unwrap()
            .chunks
            .values()
            .map(Bytes::len)
            .sum()
    }

    /// The resource's length, once a response gave it.
    pub fn total(&self) -> Option<u64> {
        self.state.lock().unwrap().total
    }

    /// Whether every byte of the resource is held.
    pub fn complete(&self) -> bool {
        let s = self.state.lock().unwrap();
        let Some(total) = s.total else { return false };
        let mut at = 0u64;
        for (start, bytes) in &s.chunks {
            if *start > at {
                return false;
            }
            at = at.max(start.saturating_add(bytes.len() as u64));
        }
        at >= total
    }

    /// The held bytes from `offset`, at most `max` (within one chunk).
    fn read(&self, offset: u64, max: u64) -> Option<Bytes> {
        let s = self.state.lock().unwrap();
        let (start, bytes) = s
            .chunks
            .range(..=offset)
            .rev()
            .find(|(start, b)| offset < start.saturating_add(b.len() as u64))?;
        let from = (offset - start) as usize;
        let to = (from as u64 + max).min(bytes.len() as u64) as usize;
        Some(bytes.slice(from..to))
    }

    fn want(&self, start: u64, end: u64) {
        let mut s = self.state.lock().unwrap();
        if start < end && !s.wants.iter().any(|w| w.0 == start) {
            s.wants.push((start, end));
        }
    }

    fn info(&self) -> Option<(String, u64)> {
        let s = self.state.lock().unwrap();
        Some((s.content_type.clone().unwrap_or_default(), s.total?))
    }
}

/// The Uniform Type Identifier AVFoundation wants for a response's `Content-Type` (its
/// essence, parameters ignored). Unknown types are offered as MPEG-4, which is what
/// AVFoundation would sniff most media as; a wrong guess fails as unplayable.
fn uti_for(content_type: &str) -> &'static str {
    let essence = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    match essence.as_str() {
        "audio/mp4" | "audio/x-m4a" | "audio/aac" => "com.apple.m4a-audio",
        "audio/mpeg" | "audio/mp3" => "public.mp3",
        "audio/wav" | "audio/wave" | "audio/x-wav" => "com.microsoft.waveform-audio",
        "video/quicktime" => "com.apple.quicktime-movie",
        _ => "public.mpeg-4",
    }
}

struct LoaderIvars {
    source: Arc<MediaSource>,
    /// The requests AVFoundation waits on. Touched only on the probe's queue.
    pending: RefCell<Vec<Retained<AVAssetResourceLoadingRequest>>>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements; `RustKitMediaLoader` has no Drop.
    #[unsafe(super(NSObject))]
    #[name = "RustKitMediaLoader"]
    #[ivars = LoaderIvars]
    struct Loader;

    unsafe impl NSObjectProtocol for Loader {}

    unsafe impl AVAssetResourceLoaderDelegate for Loader {
        /// Every request waits for the bytes it names.
        #[unsafe(method(resourceLoader:shouldWaitForLoadingOfRequestedResource:))]
        fn should_wait(
            &self,
            _loader: &AVAssetResourceLoader,
            request: &AVAssetResourceLoadingRequest,
        ) -> bool {
            self.ivars().pending.borrow_mut().push(request.retain());
            self.service();
            true
        }

        #[unsafe(method(resourceLoader:didCancelLoadingRequest:))]
        fn did_cancel(
            &self,
            _loader: &AVAssetResourceLoader,
            request: &AVAssetResourceLoadingRequest,
        ) {
            self.ivars()
                .pending
                .borrow_mut()
                .retain(|r| !std::ptr::eq(&**r, request));
        }
    }
);

impl Loader {
    fn new(source: Arc<MediaSource>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(LoaderIvars {
            source,
            pending: RefCell::new(Vec::new()),
        });
        unsafe { msg_send![super(this), init] }
    }

    /// Answer what the held bytes can and ask the engine for the rest. On the queue.
    fn service(&self) {
        let source = &self.ivars().source;
        let failed = source.failed();
        self.ivars().pending.borrow_mut().retain(|request| unsafe {
            if failed {
                request.finishLoadingWithError(None);
                return false;
            }
            let Some((content_type, total)) = source.info() else {
                // Nothing is known until a first response: ask for the start.
                source.want(0, CHUNK);
                return true;
            };
            if let Some(info) = request.contentInformationRequest() {
                info.setContentType(Some(&NSString::from_str(uti_for(&content_type))));
                info.setContentLength(total as i64);
                info.setByteRangeAccessSupported(true);
            }
            let Some(data) = request.dataRequest() else {
                request.finishLoading();
                return false;
            };
            let end = if data.requestsAllDataToEndOfResource() {
                total
            } else {
                (data
                    .requestedOffset()
                    .saturating_add(data.requestedLength() as i64)
                    .max(0) as u64)
                    .min(total)
            };
            loop {
                let at = data.currentOffset().max(0) as u64;
                if at >= end {
                    request.finishLoading();
                    return false;
                }
                match source.read(at, end - at) {
                    Some(bytes) => data.respondWithData(&NSData::with_bytes(&bytes)),
                    None => {
                        source.want(at, end);
                        return true;
                    }
                }
            }
        });
    }
}

/// A resource loader over a [`MediaSource`] and the serial queue it runs on: what makes
/// an `AVURLAsset` read its bytes from the source (the probe's asset, and the player's
/// in the main-thread `MediaHost`, step 3). Each asset gets a feed of its own.
pub struct Feed {
    loader: Retained<Loader>,
    queue: DispatchRetained<DispatchQueue>,
}

// SAFETY: the loader's mutable state (its waiting requests) is touched only on `queue`:
// AVFoundation calls its delegate methods there, and `bytes_changed` services it there.
// From any other thread the loader is only retained, released and handed to
// `setDelegate:queue:` with that queue, which AVFoundation allows from any thread.
unsafe impl Send for Feed {}
unsafe impl Sync for Feed {}

/// Each asset's URL is its own (`rustkit-media://<kind>/<n>`).
static NEXT_ASSET: AtomicU64 = AtomicU64::new(1);

impl Feed {
    pub fn new(source: Arc<MediaSource>) -> Arc<Feed> {
        Arc::new(Feed {
            loader: Loader::new(source),
            queue: DispatchQueue::new("org.hiwave.rustkit-media.loader", None),
        })
    }

    /// An asset on a private URL scheme, so AVFoundation cannot load it without this
    /// feed's loader: every byte comes from the source (§3.4). `kind` names it in logs.
    pub(crate) fn asset(&self, kind: &str) -> Retained<AVURLAsset> {
        let id = NEXT_ASSET.fetch_add(1, Ordering::Relaxed);
        let url =
            NSURL::URLWithString(&NSString::from_str(&format!("rustkit-media://{kind}/{id}")))
                .expect("a valid URL");
        let asset = unsafe { AVURLAsset::URLAssetWithURL_options(&url, None) };
        unsafe {
            asset.resourceLoader().setDelegate_queue(
                Some(ProtocolObject::from_ref(&*self.loader)),
                Some(&self.queue),
            );
        }
        asset
    }

    /// Bytes came, or the fetch failed: answer the waiting requests, on the queue.
    pub fn bytes_changed(&self) {
        let loader = OnQueue(self.loader.clone());
        self.queue.exec_async(move || {
            let loader = loader;
            loader.0.service();
        });
    }
}

/// A feed's loader, handed to its queue. It is only ever used there (its delegate
/// methods run there too), which is what makes sending it sound.
struct OnQueue(Retained<Loader>);
// SAFETY: see above.
unsafe impl Send for OnQueue {}

/// What a [`Probe`] knows.
#[derive(Debug, Clone, PartialEq)]
pub enum ProbeState {
    /// The asset's values are still loading.
    Pending,
    /// The asset is playable. `video` is its first video track's display size; `None`
    /// for audio only.
    Ready {
        duration: f64,
        video: Option<(f64, f64)>,
        audio: bool,
    },
    /// Not media AVFoundation can play (or not media at all), or the fetch failed.
    Failed,
}

/// One resource's metadata, loaded by AVFoundation from bytes the engine fetches for it.
pub struct Probe {
    asset: Retained<AVURLAsset>,
    // Kept alive: the resource loader holds its delegate weakly.
    feed: Arc<Feed>,
    loaded: Arc<AtomicBool>,
    state: Option<ProbeState>,
}

impl Drop for Probe {
    /// The element went, or took another resource: AVFoundation stops asking.
    fn drop(&mut self) {
        unsafe { self.asset.cancelLoading() };
    }
}

impl Probe {
    /// Start loading the metadata of `source`'s resource. Nothing blocks: what
    /// AVFoundation reads shows up as `source`'s wants, and [`Probe::poll`] reports.
    pub fn start(source: Arc<MediaSource>) -> Probe {
        let feed = Feed::new(source);
        let asset = feed.asset("probe");
        let loaded = Arc::new(AtomicBool::new(false));
        let done = loaded.clone();
        let handler = block2::RcBlock::new(move || done.store(true, Ordering::Release));
        let keys = NSArray::from_retained_slice(&[
            NSString::from_str("playable"),
            NSString::from_str("duration"),
            NSString::from_str("tracks"),
        ]);
        unsafe { asset.loadValuesAsynchronouslyForKeys_completionHandler(&keys, Some(&handler)) };
        Probe {
            asset,
            feed,
            loaded,
            state: None,
        }
    }

    /// Bytes came, or the fetch failed: answer the waiting requests, on the queue.
    pub fn bytes_changed(&self) {
        self.feed.bytes_changed();
    }

    /// The probe's state now. Once out of `Pending` it does not change.
    pub fn poll(&mut self) -> ProbeState {
        if let Some(state) = &self.state {
            return state.clone();
        }
        if !self.loaded.load(Ordering::Acquire) {
            return ProbeState::Pending;
        }
        // The keys are loaded (or failed): these reads do not block.
        #[allow(deprecated)]
        let state = unsafe {
            let duration = self.asset.duration().seconds();
            if !self.asset.isPlayable() || !duration.is_finite() {
                ProbeState::Failed
            } else {
                let track = |kind: Option<&'static objc2_av_foundation::AVMediaType>| {
                    kind.and_then(|kind| self.asset.tracksWithMediaType(kind).firstObject())
                };
                let video = track(AVMediaTypeVideo).map(|t| {
                    let size = t.naturalSize();
                    // A rotated track is displayed rotated (the preferred transform).
                    let tr = t.preferredTransform();
                    let w = (size.width * tr.a + size.height * tr.c).abs();
                    let h = (size.width * tr.b + size.height * tr.d).abs();
                    (w, h)
                });
                ProbeState::Ready {
                    duration,
                    video,
                    audio: track(AVMediaTypeAudio).is_some(),
                }
            }
        };
        self.state = Some(state.clone());
        state
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The codecs of the MEDIA-AVF fixtures play; an unknown type does not.
    #[test]
    fn h264_and_aac_in_mp4_are_playable() {
        assert!(is_playable_extended_mime_type(
            "video/mp4; codecs=\"avc1.42E01E, mp4a.40.2\""
        ));
        assert!(is_playable_extended_mime_type(
            "audio/mp4; codecs=\"mp4a.40.2\""
        ));
        assert!(!is_playable_extended_mime_type("video/x-unknown"));
    }

    fn fixture(name: &str) -> Vec<u8> {
        std::fs::read(format!(
            "{}/../../fixtures/media/{name}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .expect("fixture")
    }

    /// Play the engine: answer the probe's wants from `resource` (ranged, as a server
    /// would), with a 1 MB sequential budget, for up to 5 s of wall time. Returns the
    /// state and every span fetched.
    fn settle(resource: &[u8], ranged: bool) -> (ProbeState, Vec<(u64, u64)>, Arc<MediaSource>) {
        let source = MediaSource::new();
        let mut probe = Probe::start(source.clone());
        let mut fetched = Vec::new();
        for _ in 0..500 {
            let wants = source.take_wants(1 << 20);
            for (start, end) in &wants {
                let (start, end) = if ranged {
                    (*start, (*end).min(start + CHUNK).min(resource.len() as u64))
                } else {
                    (0, resource.len() as u64)
                };
                source.set_info(Some("video/mp4".into()), resource.len() as u64);
                source.add(
                    start,
                    Bytes::copy_from_slice(&resource[start as usize..end as usize]),
                );
                fetched.push((start, end));
            }
            if !wants.is_empty() {
                probe.bytes_changed();
            }
            match probe.poll() {
                ProbeState::Pending => std::thread::sleep(std::time::Duration::from_millis(10)),
                done => return (done, fetched, source),
            }
        }
        (ProbeState::Pending, fetched, source)
    }

    /// Chrome 148 (media_load_cft148.json): duration 2, 64x48, and an audio track.
    #[test]
    fn a_probe_reads_the_fixtures_metadata() {
        match settle(&fixture("color-steps.mp4"), true).0 {
            ProbeState::Ready {
                duration,
                video,
                audio,
            } => {
                assert!((duration - 2.0).abs() < 0.01, "duration {duration}");
                assert_eq!(video, Some((64.0, 48.0)));
                assert!(audio);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn bytes_that_are_not_media_fail() {
        assert_eq!(
            settle(&b"this is not a media file ".repeat(40), true).0,
            ProbeState::Failed
        );
    }

    /// The metadata is read from what AVFoundation needs, not the whole resource: the
    /// fixture followed by an 8 MB `free` box (valid MP4, skipped by readers).
    /// AVFoundation would read on through the box; the sequential budget stops it.
    #[test]
    fn the_metadata_needs_only_what_avfoundation_reads() {
        let mut resource = fixture("color-steps.mp4");
        let pad: u32 = 8 << 20;
        resource.extend_from_slice(&pad.to_be_bytes());
        resource.extend_from_slice(b"free");
        resource.resize(resource.len() + pad as usize - 8, 0);
        let (state, fetched, source) = settle(&resource, true);
        assert!(matches!(state, ProbeState::Ready { .. }), "{state:?}");
        let held = source.held();
        assert!(
            held < (2 << 20),
            "held {held} of {} bytes: {fetched:?}",
            resource.len()
        );
        assert!(!source.complete());
    }

    /// The `moov` atom at the end, past 8 MB that metadata does not need (a `free` box
    /// put between the media data and it): AVFoundation seeks there, a span that does
    /// not continue anything held, which is fetched past the budget.
    #[test]
    fn a_moov_at_the_end_is_fetched_by_range() {
        let file = fixture("color-steps.mp4");
        // The top-level boxes: (offset, size, type).
        let mut at = 0usize;
        let mut moov = None;
        while at + 8 <= file.len() {
            let mut size = u32::from_be_bytes(file[at..at + 4].try_into().unwrap()) as usize;
            // `1`: a 64-bit size follows the type (AVAssetWriter's `mdat`).
            if size == 1 {
                size = u64::from_be_bytes(file[at + 8..at + 16].try_into().unwrap()) as usize;
            }
            if &file[at + 4..at + 8] == b"moov" {
                moov = Some(at);
            }
            if size < 8 {
                break;
            }
            at += size;
        }
        let moov = moov.expect("a moov box");
        let pad: u32 = 8 << 20;
        let mut resource = file[..moov].to_vec();
        resource.extend_from_slice(&pad.to_be_bytes());
        resource.extend_from_slice(b"free");
        resource.resize(resource.len() + pad as usize - 8, 0);
        resource.extend_from_slice(&file[moov..]);
        let (state, fetched, source) = settle(&resource, true);
        match state {
            ProbeState::Ready {
                duration, video, ..
            } => {
                assert!((duration - 2.0).abs() < 0.01);
                assert_eq!(video, Some((64.0, 48.0)));
            }
            other => panic!("{other:?}: {fetched:?}"),
        }
        let held = source.held();
        assert!(
            held < (3 << 20),
            "held {held} of {}: {fetched:?}",
            resource.len()
        );
    }

    /// #1344 delta R1: a span near u64::MAX is not kept and breaks no offset sum.
    #[test]
    fn a_span_that_would_overflow_is_not_kept() {
        let source = MediaSource::new();
        source.set_info(None, u64::MAX);
        source.add(u64::MAX - 1, Bytes::from_static(b"0123"));
        assert_eq!(source.held(), 0);
        source.add(0, Bytes::from_static(b"0123"));
        assert!(!source.complete());
        assert!(source.read(u64::MAX - 1, 10).is_none());
        assert_eq!(source.take_wants(0), Vec::new());
    }

    #[test]
    fn a_failed_fetch_fails_the_probe() {
        let source = MediaSource::new();
        let mut probe = Probe::start(source.clone());
        for _ in 0..500 {
            if !source.take_wants(1 << 20).is_empty() {
                source.fail();
                probe.bytes_changed();
            }
            match probe.poll() {
                ProbeState::Pending => std::thread::sleep(std::time::Duration::from_millis(10)),
                done => {
                    assert_eq!(done, ProbeState::Failed);
                    return;
                }
            }
        }
        panic!("the probe never settled");
    }
}
