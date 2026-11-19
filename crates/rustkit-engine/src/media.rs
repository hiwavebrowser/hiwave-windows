//! Media elements' resource loading (MEDIA-AVF Milestone 1 step 2,
//! `docs/design/MEDIA_AVF.md` §3.2-3.5): HTML §4.8.11.5's load algorithm, up to the
//! metadata. Nothing plays yet (steps 3 and 4).
//!
//! Each `<video>` / `<audio>` of a view's document is tracked by raw NodeId in its
//! [`MediaElements`] (the engine side of `docs/design/MEDIA_AVF.md` §3.2: the main-thread
//! `MediaHost` that will own players comes in step 3). The live loop syncs them with the
//! document after each layout and after `load()` calls, and an element whose resource changed (its `src`, its first
//! acceptable `<source>`, or a `load()`) runs the algorithm again:
//!
//! 1. `loadstart`. With `preload=none` the element then idles (`suspend`) and fetches
//!    nothing.
//! 2. An AVFoundation probe (`rustkit_media::avf::Probe`) starts reading the resource.
//!    What it reads and is not held becomes a *want*, fetched through rustkit-net (so
//!    through the shield) with `Range`, at most [`CHUNK`] per request, as AVFoundation
//!    reads: loading metadata fetches the header and the `moov` atom (wherever it
//!    sits), not the resource. Sequential read-ahead stops at [`METADATA_BUDGET`] held;
//!    a seek is still fetched. A server that ignores `Range` answers 200 with the whole
//!    body, refused past [`WHOLE_CAP`]; an element never holds more than [`WHOLE_CAP`]
//!    (`MEDIA_ERR_NETWORK`). A failed, blocked or 404 fetch is
//!    `MEDIA_ERR_SRC_NOT_SUPPORTED` (Chrome 148 reports 4 for a 404).
//! 3. `progress` and `suspend` when the first bytes come.
//! 4. Metadata: `durationchange`, `resize` (video), `loadedmetadata` at
//!    HAVE_METADATA. When every byte is in hand (a small resource), `loadeddata`,
//!    `canplay`, `canplaythrough` at HAVE_ENOUGH_DATA follow. The probe and its bytes
//!    are then let go: nothing plays in step 2.
//!
//! Removing an element, or giving it another resource, drops its fetches (the requests
//! are cancelled) and its bytes.
//!
//! The state each event carries is the element's as of that event (pushed with it to
//! the getters in dom.rs). The order is Chrome 148's
//! (`docs/diagnostics/2026-11-18/media_load_cft148.json`).

use rustkit_dom::{Document, Node, NodeType};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

/// The display-list key of a video's box once its size is known (its frames paint
/// under it from step 3; nothing paints yet).
pub(crate) const VIDEO_KEY_PREFIX: &str = "rustkit-video:";

/// `MEDIA_ERR_NETWORK`.
const NETWORK: u16 = 2;
/// `MEDIA_ERR_SRC_NOT_SUPPORTED`.
const SRC_NOT_SUPPORTED: u16 = 4;

pub(crate) use rustkit_media::avf::CHUNK;

/// Sequential bytes the metadata may read ahead (an MP3's ID3 tag with cover art can be
/// a few MB): see `MediaSource::take_wants`.
pub(crate) const METADATA_BUDGET: u64 = 8 << 20;

/// The most an element holds, and the largest whole body (a server that ignores
/// `Range`) it accepts: past it, `MEDIA_ERR_NETWORK`.
pub(crate) const WHOLE_CAP: u64 = 64 << 20;

thread_local! {
    /// Tests lower [`WHOLE_CAP`] to reach it with a small resource.
    pub(crate) static CAP_FOR_TESTS: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) };
}

pub(crate) fn whole_cap() -> u64 {
    CAP_FOR_TESTS.with(|c| c.get()).unwrap_or(WHOLE_CAP)
}

/// What a media element's getters read (pushed as JSON, dom.rs `MEDIA_INITIAL`).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct MediaState {
    pub(crate) ready_state: u8,
    pub(crate) network_state: u8,
    pub(crate) duration: Option<f64>,
    pub(crate) current_src: String,
    pub(crate) error: Option<u16>,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) buffered: Vec<(f64, f64)>,
    pub(crate) seekable: Vec<(f64, f64)>,
}

impl MediaState {
    fn initial() -> Self {
        MediaState {
            ready_state: 0,
            network_state: 0,
            duration: None,
            current_src: String::new(),
            error: None,
            width: 0,
            height: 0,
            buffered: Vec::new(),
            seekable: Vec::new(),
        }
    }

    pub(crate) fn json(&self) -> String {
        serde_json::json!({
            "rs": self.ready_state,
            "ns": self.network_state,
            "duration": self.duration,
            "src": self.current_src,
            "error": self.error,
            "w": self.width,
            "h": self.height,
            "buffered": self.buffered.iter().map(|(a, b)| [a, b]).collect::<Vec<_>>(),
            "seekable": self.seekable.iter().map(|(a, b)| [a, b]).collect::<Vec<_>>(),
        })
        .to_string()
    }
}

/// `preload`, as the element's resource loading reads it (HTML §4.8.11.2: missing or
/// invalid is `metadata` here, as in Chrome; empty is `auto`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Preload {
    None,
    Metadata,
    Auto,
}

impl Preload {
    fn of(value: Option<&str>) -> Preload {
        match value.map(|v| v.trim().to_ascii_lowercase()) {
            Some(v) if v == "none" => Preload::None,
            Some(v) if v.is_empty() || v == "auto" => Preload::Auto,
            _ => Preload::Metadata,
        }
    }
}

enum Stage {
    /// Nothing to load (no source), or waiting (`preload=none`).
    Idle,
    /// The probe reads, and its wants are fetched by range.
    Loading {
        source: Arc<rustkit_media::avf::MediaSource>,
        probe: rustkit_media::avf::Probe,
        /// Whether `progress` / `suspend` fired (the first bytes came).
        announced: bool,
        url: url::Url,
    },
    /// Metadata in, or an error: nothing more until the resource changes.
    Done,
}

/// One media element.
pub(crate) struct MediaElement {
    video: bool,
    /// The resource the element selected (absolute), with its `preload`.
    selected: Option<(String, Preload)>,
    /// Bumped each time the load algorithm runs: a fetch of an earlier run is dropped.
    serial: u64,
    stage: Stage,
    pub(crate) state: MediaState,
}

/// A ranged fetch's outcome, for the element and run that asked.
pub(crate) struct MediaFetched {
    pub(crate) node: usize,
    pub(crate) serial: u64,
    pub(crate) span: Result<FetchedSpan, FetchFailed>,
}

/// Bytes a response brought: from `start`, of a resource `total` long.
pub(crate) struct FetchedSpan {
    pub(crate) start: u64,
    pub(crate) total: u64,
    pub(crate) content_type: Option<String>,
    pub(crate) bytes: bytes::Bytes,
}

pub(crate) enum FetchFailed {
    /// Refused, blocked, not found, or not media: `MEDIA_ERR_SRC_NOT_SUPPORTED`.
    NotSupported,
    /// Too large to hold (`WHOLE_CAP`): `MEDIA_ERR_NETWORK`.
    TooLarge,
    /// A response whose range does not add up (not where it was asked, past its total,
    /// or overflowing): `MEDIA_ERR_NETWORK`.
    BadResponse,
}

/// The response to a ranged GET from `start`, read into a span. A 206 gives its
/// `Content-Range`; a 200 (a server that ignores `Range`) is the whole resource.
pub(crate) async fn read_span(
    response: rustkit_net::Response,
    start: u64,
) -> Result<FetchedSpan, FetchFailed> {
    let content_type = response.content_type.as_ref().map(|m| m.to_string());
    let status = response.status.as_u16();
    let content_range = response
        .headers
        .get("content-range")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let bytes = response
        .bytes()
        .await
        .map_err(|_| FetchFailed::NotSupported)?;
    let (start, total) = span_of(status, content_range.as_deref(), bytes.len() as u64, start)?;
    Ok(FetchedSpan {
        start,
        total,
        content_type,
        bytes,
    })
}

/// Where a response's `len` bytes sit, and the resource's length: a 206's
/// `Content-Range: bytes first-last/total`, or a 200's whole body. The numbers are the
/// server's: a 206 must start where it was asked to (`asked`), and its span must end
/// within its total without overflowing; anything else is a bad response
/// (`MEDIA_ERR_NETWORK`), never offset arithmetic that wraps.
pub(crate) fn span_of(
    status: u16,
    content_range: Option<&str>,
    len: u64,
    asked: u64,
) -> Result<(u64, u64), FetchFailed> {
    match status {
        206 => {
            let (range, total) = content_range
                .and_then(|v| v.rsplit_once('/'))
                .ok_or(FetchFailed::BadResponse)?;
            let first = range
                .trim()
                .strip_prefix("bytes")
                .and_then(|r| r.trim().split('-').next())
                .and_then(|f| f.trim().parse::<u64>().ok())
                .ok_or(FetchFailed::BadResponse)?;
            let total = total
                .trim()
                .parse::<u64>()
                .map_err(|_| FetchFailed::BadResponse)?;
            let end = first.checked_add(len).ok_or(FetchFailed::BadResponse)?;
            if first != asked || end > total {
                return Err(FetchFailed::BadResponse);
            }
            Ok((first, total))
        }
        200 => Ok((0, len)),
        _ => Err(FetchFailed::NotSupported),
    }
}

/// An event to fire, with the element's state as of it.
pub(crate) struct MediaEvent {
    pub(crate) node: usize,
    pub(crate) kind: &'static str,
    pub(crate) state: String,
}

/// A view's media elements.
#[derive(Default)]
pub(crate) struct MediaElements {
    elements: HashMap<usize, MediaElement>,
    events: Vec<MediaEvent>,
    /// The layout generation the document was last synced at.
    pub(crate) synced_for: Option<u64>,
}

/// A ranged fetch to start: `url`'s bytes [start, end) for `node`'s run `serial`.
pub(crate) struct MediaFetch {
    pub(crate) node: usize,
    pub(crate) serial: u64,
    pub(crate) url: url::Url,
    pub(crate) start: u64,
    pub(crate) end: u64,
}

fn is_tag(node: &Node, tag: &str) -> bool {
    node.tag_name().is_some_and(|t| t.eq_ignore_ascii_case(tag))
}

/// Every `<video>` and `<audio>` element of `document`, in tree order. The walk keeps
/// its own stack.
fn media_elements(document: &Document) -> Vec<Rc<Node>> {
    let mut out = Vec::new();
    let mut stack: Vec<Rc<Node>> = vec![document.root().clone()];
    while let Some(node) = stack.pop() {
        if matches!(node.node_type, NodeType::Element { .. })
            && (is_tag(&node, "video") || is_tag(&node, "audio"))
        {
            out.push(node.clone());
        }
        let children = node.children();
        stack.extend(children.into_iter().rev());
    }
    out
}

impl MediaElements {
    pub(crate) fn take_events(&mut self) -> Vec<MediaEvent> {
        std::mem::take(&mut self.events)
    }

    /// Whether the live loop has to keep turning for the media: a load is under way,
    /// or events wait.
    pub(crate) fn busy(&self) -> bool {
        !self.events.is_empty()
            || self
                .elements
                .values()
                .any(|e| matches!(e.stage, Stage::Loading { .. }))
    }

    /// Whether a fetch for `node`'s run `serial` is still wanted (the element is there,
    /// still loading that resource). The engine drops the others' requests.
    pub(crate) fn wants_fetch(&self, node: usize, serial: u64) -> bool {
        self.elements
            .get(&node)
            .is_some_and(|e| e.serial == serial && matches!(e.stage, Stage::Loading { .. }))
    }

    /// Bytes the elements hold, in all.
    #[cfg(test)]
    pub(crate) fn held(&self) -> usize {
        self.elements
            .values()
            .map(|e| match &e.stage {
                Stage::Loading { source, .. } => source.held(),
                _ => 0,
            })
            .sum()
    }

    /// The ranged fetches the probes wait on, to start now.
    pub(crate) fn take_fetches(&mut self) -> Vec<MediaFetch> {
        let mut out = Vec::new();
        for (node, e) in &self.elements {
            let Stage::Loading { source, url, .. } = &e.stage else {
                continue;
            };
            for (start, end) in source.take_wants(METADATA_BUDGET) {
                let end = end
                    .min(start.saturating_add(CHUNK))
                    .min(source.total().unwrap_or(u64::MAX));
                if start < end {
                    out.push(MediaFetch {
                        node: *node,
                        serial: e.serial,
                        url: url.clone(),
                        start,
                        end,
                    });
                }
            }
        }
        out
    }

    /// Every video's display size whose metadata is in, by raw NodeId.
    pub(crate) fn video_sizes(&self) -> HashMap<usize, (u32, u32)> {
        self.elements
            .iter()
            .filter(|(_, e)| e.video && e.state.width > 0 && e.state.height > 0)
            .map(|(n, e)| (*n, (e.state.width, e.state.height)))
            .collect()
    }

    fn queue(&mut self, node: usize, kind: &'static str) {
        let state = self.elements[&node].state.json();
        self.events.push(MediaEvent { node, kind, state });
    }

    /// Bring the host in line with `document`: forget elements that left it, and run
    /// the load algorithm for each one whose selected resource changed or whose
    /// `load()` script called (`reload`). `resolve` makes a URL absolute against the
    /// document; `can_play` is `canPlayType`. The fetches the new loads ask for come
    /// from [`MediaElements::take_fetches`].
    pub(crate) fn sync(
        &mut self,
        document: &Document,
        reload: &[usize],
        resolve: &dyn Fn(&str) -> Option<url::Url>,
        can_play: &dyn Fn(&str) -> &'static str,
    ) {
        let mut alive = std::collections::HashSet::new();
        for node in media_elements(document) {
            let raw = node.id.raw();
            alive.insert(raw);
            let video = is_tag(&node, "video");
            let selected = Self::select(&node, resolve, can_play);
            let entry = self.elements.entry(raw).or_insert_with(|| MediaElement {
                video,
                selected: None,
                serial: 0,
                stage: Stage::Idle,
                state: MediaState::initial(),
            });
            let changed = entry.selected != selected || entry.serial == 0;
            if !(changed || reload.contains(&raw)) {
                continue;
            }
            if entry.serial == 0 && selected.is_none() {
                // Never had a resource: nothing to announce.
                entry.serial = 1;
                continue;
            }
            entry.serial += 1;
            entry.selected = selected.clone();
            entry.stage = Stage::Idle;
            entry.state = MediaState::initial();
            let Some((url, preload)) = selected else {
                continue;
            };
            entry.state.current_src = url.clone();
            if preload == Preload::None {
                // Chrome 148: `loadstart` already at NETWORK_IDLE, then `suspend`.
                entry.state.network_state = 1;
                self.queue(raw, "loadstart");
                self.queue(raw, "suspend");
                continue;
            }
            entry.state.network_state = 2;
            match url::Url::parse(&url) {
                Ok(url) => {
                    let source = rustkit_media::avf::MediaSource::new();
                    entry.stage = Stage::Loading {
                        probe: rustkit_media::avf::Probe::start(source.clone()),
                        source,
                        announced: false,
                        url,
                    };
                    self.queue(raw, "loadstart");
                }
                Err(_) => {
                    self.queue(raw, "loadstart");
                    self.fail(raw, SRC_NOT_SUPPORTED);
                }
            }
        }
        // An element that left the document: its probe, bytes and fetches go.
        self.elements.retain(|node, _| alive.contains(node));
    }

    /// The element's resource (HTML §4.8.11.5, resource selection): its `src`, else the
    /// first `<source>` child with a `src` whose `type` is empty or one `canPlayType`
    /// does not reject. With the `preload` it loads under.
    fn select(
        node: &Rc<Node>,
        resolve: &dyn Fn(&str) -> Option<url::Url>,
        can_play: &dyn Fn(&str) -> &'static str,
    ) -> Option<(String, Preload)> {
        let preload = Preload::of(node.get_attribute("preload"));
        if let Some(src) = node.get_attribute("src") {
            return resolve(src).map(|u| (u.to_string(), preload));
        }
        node.children().into_iter().find_map(|child| {
            if !is_tag(&child, "source") {
                return None;
            }
            let src = child.get_attribute("src")?;
            if let Some(kind) = child.get_attribute("type") {
                if !kind.trim().is_empty() && can_play(kind).is_empty() {
                    return None;
                }
            }
            resolve(src).map(|u| (u.to_string(), preload))
        })
    }

    fn fail(&mut self, node: usize, code: u16) {
        let Some(e) = self.elements.get_mut(&node) else {
            return;
        };
        // Its probe and bytes go with the stage.
        e.stage = Stage::Done;
        e.state.network_state = 3;
        e.state.error = Some(code);
        self.queue(node, "error");
    }

    /// A ranged fetch came back.
    pub(crate) fn fetched(&mut self, fetched: MediaFetched) {
        if !self.wants_fetch(fetched.node, fetched.serial) {
            return;
        }
        let node = fetched.node;
        let span = match fetched.span {
            Ok(span) => span,
            Err(FetchFailed::NotSupported) => return self.fail(node, SRC_NOT_SUPPORTED),
            Err(FetchFailed::TooLarge | FetchFailed::BadResponse) => {
                return self.fail(node, NETWORK)
            }
        };
        let cap = whole_cap();
        let e = self.elements.get_mut(&node).expect("checked");
        let Stage::Loading {
            source,
            probe,
            announced,
            ..
        } = &mut e.stage
        else {
            return;
        };
        if source.held() as u64 + span.bytes.len() as u64 > cap {
            return self.fail(node, NETWORK);
        }
        source.set_info(span.content_type, span.total);
        source.add(span.start, span.bytes);
        probe.bytes_changed();
        if !std::mem::replace(announced, true) {
            // Chrome 148: `progress` and `suspend` before the metadata events.
            e.state.network_state = 1;
            self.queue(node, "progress");
            self.queue(node, "suspend");
        }
    }

    /// Poll the probes. Returns whether a video's size changed (its box must be laid
    /// out again).
    pub(crate) fn poll(&mut self) -> bool {
        let mut resized = false;
        let nodes: Vec<usize> = self
            .elements
            .iter()
            .filter(|(_, e)| matches!(e.stage, Stage::Loading { .. }))
            .map(|(n, _)| *n)
            .collect();
        for node in nodes {
            let e = self.elements.get_mut(&node).expect("listed");
            let Stage::Loading { probe, source, .. } = &mut e.stage else {
                continue;
            };
            let complete = source.complete();
            match probe.poll() {
                rustkit_media::avf::ProbeState::Pending => {}
                rustkit_media::avf::ProbeState::Failed => self.fail(node, SRC_NOT_SUPPORTED),
                rustkit_media::avf::ProbeState::Ready {
                    duration, video, ..
                } => {
                    // Nothing plays in step 2: the probe and its bytes go.
                    e.stage = Stage::Done;
                    e.state.network_state = 1;
                    e.state.ready_state = 1;
                    e.state.duration = Some(duration);
                    e.state.seekable = vec![(0.0, duration)];
                    if complete {
                        e.state.buffered = vec![(0.0, duration)];
                    }
                    let sized = match (e.video, video) {
                        (true, Some((w, h))) if w > 0.0 && h > 0.0 => {
                            e.state.width = w.round() as u32;
                            e.state.height = h.round() as u32;
                            true
                        }
                        _ => false,
                    };
                    self.queue(node, "durationchange");
                    if sized {
                        resized = true;
                        self.queue(node, "resize");
                    }
                    self.queue(node, "loadedmetadata");
                    if complete {
                        // Every byte is in hand: the data is all there.
                        self.elements
                            .get_mut(&node)
                            .expect("listed")
                            .state
                            .ready_state = 4;
                        self.queue(node, "loadeddata");
                        self.queue(node, "canplay");
                        self.queue(node, "canplaythrough");
                    }
                }
            }
        }
        resized
    }
}

/// `script_net::settle_live` over fetches kept with their element and run: the ones
/// that finished within `slice` (or at once), removed and returned.
pub(crate) async fn settle_keyed<T>(
    in_flight: &mut Vec<(usize, u64, crate::script_net::LiveFuture<T>)>,
    slice: std::time::Duration,
) -> Vec<T> {
    let mut done = Vec::new();
    if in_flight.is_empty() {
        return done;
    }
    let pause = tokio::time::sleep(slice);
    tokio::pin!(pause);
    std::future::poll_fn(|cx| {
        let mut i = 0;
        while i < in_flight.len() {
            match std::future::Future::poll(in_flight[i].2.as_mut(), cx) {
                std::task::Poll::Ready(outcome) => {
                    drop(in_flight.remove(i));
                    done.push(outcome);
                }
                std::task::Poll::Pending => i += 1,
            }
        }
        if !done.is_empty() {
            return std::task::Poll::Ready(());
        }
        std::future::Future::poll(pause.as_mut(), cx)
    })
    .await;
    done
}

/// `canPlayType` (HTML §4.8.11.3) from AVFoundation: `"probably"` for a type with
/// `codecs` AVFoundation plays, `"maybe"` for a container type it plays, `""`
/// otherwise. Chrome 148's answers for the pinned strings are in
/// `docs/diagnostics/2026-11-18/media_codecs_cft148.json`; RustKit differs where
/// AVFoundation does (WebM / VP9), and for HLS (`application/vnd.apple.mpegurl`),
/// which Pete kept out of Milestone 1.
pub(crate) fn can_play_type(mime: &str) -> &'static str {
    let mime = mime.trim();
    let essence = mime
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if essence.is_empty()
        || essence.contains("mpegurl")
        || !rustkit_media::avf::is_playable_extended_mime_type(mime)
    {
        return "";
    }
    if mime.to_ascii_lowercase().contains("codecs=") {
        "probably"
    } else {
        "maybe"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #1344 delta R1: a hostile `Content-Range` near u64::MAX is a bad response, not
    /// an overflow.
    #[test]
    fn a_content_range_that_overflows_is_a_bad_response() {
        let max = u64::MAX;
        let bad = |status, range: &str, len, asked| {
            matches!(
                span_of(status, Some(range), len, asked),
                Err(FetchFailed::BadResponse)
            )
        };
        assert!(
            bad(
                206,
                &format!("bytes {}-{}/{}", max - 2, max, max),
                16,
                max - 2
            ),
            "first + len overflows"
        );
        assert!(
            bad(206, &format!("bytes {}-{}/{}", max, max, max), 1, max),
            "first + len overflows"
        );
        assert!(
            bad(
                206,
                &format!("bytes 0-99/{}", 18_446_744_073_709_551_616u128),
                100,
                0
            ),
            "total past u64"
        );
        assert!(bad(206, "bytes 0-99/50", 100, 0), "past its total");
        assert!(
            bad(206, "bytes 100-199/1000", 100, 0),
            "not where it was asked"
        );
        assert!(bad(206, "bytes x-199/1000", 100, 0), "unreadable");
        assert!(
            matches!(span_of(206, None, 100, 0), Err(FetchFailed::BadResponse)),
            "no Content-Range"
        );
        assert!(matches!(
            span_of(206, Some("bytes 0-99/1000"), 100, 0),
            Ok((0, 1000))
        ));
        assert!(matches!(span_of(200, None, 1234, 0), Ok((0, 1234))));
        assert!(matches!(
            span_of(404, None, 0, 0),
            Err(FetchFailed::NotSupported)
        ));
    }

    #[test]
    fn preload_reads_as_chrome() {
        assert_eq!(Preload::of(None), Preload::Metadata);
        assert_eq!(Preload::of(Some("")), Preload::Auto);
        assert_eq!(Preload::of(Some("AUTO")), Preload::Auto);
        assert_eq!(Preload::of(Some("none")), Preload::None);
        assert_eq!(Preload::of(Some("bogus")), Preload::Metadata);
    }
}
