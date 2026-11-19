//! # RustKit Media
//!
//! HTML5 Audio/Video element support for the RustKit browser engine.
//!
//! ## Features
//!
//! - **HTMLMediaElement**: Base interface for audio/video
//! - **Audio playback**: Via rodio audio backend
//! - **Video rendering**: Frame extraction for display
//! - **Media controls**: Play, pause, seek, volume
//! - **Media events**: play, pause, ended, timeupdate, etc.
//!
//! ## AVFoundation
//!
//! [`avf`] (macOS) is the glue to AVFoundation that MEDIA-AVF builds on
//! (`docs/design/MEDIA_AVF.md`).
//!
//! ## Architecture
//!
//! ```text
//! HTMLMediaElement (base)
//!     ├── HTMLAudioElement
//!     │      └── AudioPlayer (rodio)
//!     └── HTMLVideoElement
//!            └── VideoDecoder (TODO)
//! ```

#[cfg(target_os = "macos")]
pub mod avf;

use hashbrown::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use thiserror::Error;
use tokio::sync::mpsc;
use url::Url;

// ==================== Errors ====================

/// Errors that can occur in media operations.
#[derive(Error, Debug)]
pub enum MediaError {
    #[error("Media not supported: {0}")]
    NotSupported(String),

    #[error("Network error: {0}")]
    NetworkError(String),

    #[error("Decode error: {0}")]
    DecodeError(String),

    #[error("Playback error: {0}")]
    PlaybackError(String),

    #[error("Invalid state: {0}")]
    InvalidState(String),
}

// ==================== Media Types ====================

/// Unique identifier for a media element.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MediaId(u64);

impl MediaId {
    fn new() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(1);
        Self(COUNTER.fetch_add(1, Ordering::Relaxed))
    }
}

/// Media network state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NetworkState {
    #[default]
    Empty,
    Idle,
    Loading,
    NoSource,
}

/// Media ready state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum ReadyState {
    #[default]
    HaveNothing,
    HaveMetadata,
    HaveCurrentData,
    HaveFutureData,
    HaveEnoughData,
}

/// Media preload attribute.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Preload {
    None,
    Metadata,
    #[default]
    Auto,
}

/// Text track kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextTrackKind {
    Subtitles,
    Captions,
    Descriptions,
    Chapters,
    Metadata,
}

/// Text track cue.
#[derive(Debug, Clone)]
pub struct TextTrackCue {
    pub id: String,
    pub start_time: f64,
    pub end_time: f64,
    pub text: String,
    pub pause_on_exit: bool,
}

/// Text track.
#[derive(Debug, Clone)]
pub struct TextTrack {
    pub id: String,
    pub kind: TextTrackKind,
    pub label: String,
    pub language: String,
    pub cues: Vec<TextTrackCue>,
    pub active_cue_index: Option<usize>,
}

/// Time range (for buffered, seekable, played).
#[derive(Debug, Clone, Default)]
pub struct TimeRanges {
    ranges: Vec<(f64, f64)>,
}

impl TimeRanges {
    pub fn new() -> Self {
        Self { ranges: Vec::new() }
    }

    pub fn add(&mut self, start: f64, end: f64) {
        self.ranges.push((start, end));
        self.normalize();
    }

    fn normalize(&mut self) {
        self.ranges.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        // Merge overlapping ranges
        let mut merged: Vec<(f64, f64)> = Vec::new();
        for range in &self.ranges {
            if let Some(last) = merged.last_mut() {
                if range.0 <= last.1 {
                    last.1 = last.1.max(range.1);
                    continue;
                }
            }
            merged.push(*range);
        }
        self.ranges = merged;
    }

    pub fn length(&self) -> usize {
        self.ranges.len()
    }

    pub fn start(&self, index: usize) -> Option<f64> {
        self.ranges.get(index).map(|r| r.0)
    }

    pub fn end(&self, index: usize) -> Option<f64> {
        self.ranges.get(index).map(|r| r.1)
    }
}

// ==================== Media Events ====================

/// Media events.
#[derive(Debug, Clone)]
pub enum MediaEvent {
    /// Loading started.
    LoadStart,
    /// Progress in loading.
    Progress { loaded: u64, total: Option<u64> },
    /// Suspended loading.
    Suspend,
    /// Aborted loading.
    Abort,
    /// Error occurred.
    Error(String),
    /// All data emptied.
    Emptied,
    /// Stalled loading.
    Stalled,
    /// Metadata loaded (duration, dimensions).
    LoadedMetadata { duration: f64, width: u32, height: u32 },
    /// First frame loaded.
    LoadedData,
    /// Can start playing.
    CanPlay,
    /// Can play through without buffering.
    CanPlayThrough,
    /// Playback started.
    Playing,
    /// Waiting for data.
    Waiting,
    /// Seeking started.
    Seeking,
    /// Seeking completed.
    Seeked,
    /// Playback ended.
    Ended,
    /// Duration changed.
    DurationChange(f64),
    /// Time updated (periodic during playback).
    TimeUpdate(f64),
    /// Play requested.
    Play,
    /// Pause requested.
    Pause,
    /// Playback rate changed.
    RateChange(f64),
    /// Volume changed.
    VolumeChange { volume: f64, muted: bool },
}

// ==================== HTMLMediaElement ====================

/// Base HTMLMediaElement implementation.
#[derive(Debug)]
pub struct HTMLMediaElement {
    /// Unique ID.
    pub id: MediaId,
    
    /// Source URL.
    pub src: Option<Url>,
    
    /// Current source (from <source> children or src attribute).
    pub current_src: Option<Url>,
    
    /// Cross-origin setting.
    pub cross_origin: Option<String>,
    
    /// Network state.
    pub network_state: NetworkState,
    
    /// Ready state.
    pub ready_state: ReadyState,
    
    /// Preload setting.
    pub preload: Preload,
    
    /// Buffered time ranges.
    pub buffered: TimeRanges,
    
    /// Seekable time ranges.
    pub seekable: TimeRanges,
    
    /// Played time ranges.
    pub played: TimeRanges,
    
    /// Whether seeking.
    pub seeking: bool,
    
    /// Current playback position (seconds).
    pub current_time: f64,
    
    /// Duration (seconds).
    pub duration: f64,
    
    /// Whether paused.
    pub paused: bool,
    
    /// Default playback rate.
    pub default_playback_rate: f64,
    
    /// Current playback rate.
    pub playback_rate: f64,
    
    /// Whether ended.
    pub ended: bool,
    
    /// Autoplay attribute.
    pub autoplay: bool,
    
    /// Loop attribute.
    pub loop_: bool,
    
    /// Controls attribute (show native controls).
    pub controls: bool,
    
    /// Volume (0.0 - 1.0).
    pub volume: f64,
    
    /// Muted.
    pub muted: bool,
    
    /// Default muted.
    pub default_muted: bool,
    
    /// Text tracks (subtitles, captions).
    pub text_tracks: Vec<TextTrack>,
    
    /// Event sender.
    event_tx: mpsc::UnboundedSender<MediaEvent>,
    
    /// Error message.
    pub error: Option<String>,
}

impl HTMLMediaElement {
    /// Create a new media element.
    pub fn new() -> (Self, mpsc::UnboundedReceiver<MediaEvent>) {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        
        (Self {
            id: MediaId::new(),
            src: None,
            current_src: None,
            cross_origin: None,
            network_state: NetworkState::Empty,
            ready_state: ReadyState::HaveNothing,
            preload: Preload::Auto,
            buffered: TimeRanges::new(),
            seekable: TimeRanges::new(),
            played: TimeRanges::new(),
            seeking: false,
            current_time: 0.0,
            duration: f64::NAN,
            paused: true,
            default_playback_rate: 1.0,
            playback_rate: 1.0,
            ended: false,
            autoplay: false,
            loop_: false,
            controls: false,
            volume: 1.0,
            muted: false,
            default_muted: false,
            text_tracks: Vec::new(),
            event_tx,
            error: None,
        }, event_rx)
    }

    /// Set source URL.
    pub fn set_src(&mut self, src: &str) -> Result<(), MediaError> {
        let url = Url::parse(src)
            .or_else(|_| Url::parse(&format!("file:///{}", src)))
            .map_err(|e| MediaError::InvalidState(e.to_string()))?;
        
        self.src = Some(url.clone());
        self.current_src = Some(url);
        self.network_state = NetworkState::Loading;
        self.ready_state = ReadyState::HaveNothing;
        
        let _ = self.event_tx.send(MediaEvent::LoadStart);
        
        Ok(())
    }

    /// Load the media.
    pub fn load(&mut self) {
        self.network_state = NetworkState::Loading;
        self.ready_state = ReadyState::HaveNothing;
        self.current_time = 0.0;
        self.paused = true;
        self.seeking = false;
        self.ended = false;
        self.error = None;
        
        let _ = self.event_tx.send(MediaEvent::LoadStart);
    }

    /// Check if can play type.
    pub fn can_play_type(&self, mime_type: &str) -> &'static str {
        match mime_type {
            // Audio formats
            "audio/mpeg" | "audio/mp3" => "probably",
            "audio/wav" | "audio/wave" => "probably",
            "audio/ogg" => "probably",
            "audio/flac" => "probably",
            "audio/webm" => "maybe",
            "audio/aac" => "maybe",
            
            // Video formats (limited support for now)
            "video/mp4" => "maybe",
            "video/webm" => "maybe",
            "video/ogg" => "maybe",
            
            _ => "",
        }
    }

    /// Play.
    pub fn play(&mut self) -> Result<(), MediaError> {
        if self.network_state == NetworkState::Empty {
            return Err(MediaError::InvalidState("No media loaded".to_string()));
        }
        
        self.paused = false;
        self.ended = false;
        
        let _ = self.event_tx.send(MediaEvent::Play);
        
        if self.ready_state >= ReadyState::HaveFutureData {
            let _ = self.event_tx.send(MediaEvent::Playing);
        }
        
        Ok(())
    }

    /// Pause.
    pub fn pause(&mut self) {
        if !self.paused {
            self.paused = true;
            let _ = self.event_tx.send(MediaEvent::Pause);
        }
    }

    /// Seek to time.
    pub fn seek(&mut self, time: f64) -> Result<(), MediaError> {
        if self.ready_state < ReadyState::HaveMetadata {
            return Err(MediaError::InvalidState("Not ready to seek".to_string()));
        }
        
        let time = time.max(0.0);
        let time = if self.duration.is_finite() {
            time.min(self.duration)
        } else {
            time
        };
        
        self.seeking = true;
        let _ = self.event_tx.send(MediaEvent::Seeking);
        
        self.current_time = time;
        
        self.seeking = false;
        let _ = self.event_tx.send(MediaEvent::Seeked);
        
        Ok(())
    }

    /// Set volume.
    pub fn set_volume(&mut self, volume: f64) -> Result<(), MediaError> {
        if !(0.0..=1.0).contains(&volume) {
            return Err(MediaError::InvalidState("Volume must be 0.0-1.0".to_string()));
        }
        
        self.volume = volume;
        let _ = self.event_tx.send(MediaEvent::VolumeChange {
            volume: self.volume,
            muted: self.muted,
        });
        
        Ok(())
    }

    /// Set muted.
    pub fn set_muted(&mut self, muted: bool) {
        self.muted = muted;
        let _ = self.event_tx.send(MediaEvent::VolumeChange {
            volume: self.volume,
            muted: self.muted,
        });
    }

    /// Set playback rate.
    pub fn set_playback_rate(&mut self, rate: f64) {
        self.playback_rate = rate;
        let _ = self.event_tx.send(MediaEvent::RateChange(rate));
    }

    /// Update time (called periodically during playback).
    pub fn update_time(&mut self, delta: f64) {
        if self.paused || self.ended {
            return;
        }
        
        let old_time = self.current_time;
        self.current_time += delta * self.playback_rate;
        
        if self.duration.is_finite() && self.current_time >= self.duration {
            self.current_time = self.duration;
            
            if self.loop_ {
                self.current_time = 0.0;
                let _ = self.event_tx.send(MediaEvent::TimeUpdate(self.current_time));
            } else {
                self.ended = true;
                self.paused = true;
                let _ = self.event_tx.send(MediaEvent::Ended);
            }
        } else {
            let _ = self.event_tx.send(MediaEvent::TimeUpdate(self.current_time));
        }
        
        // Update played ranges
        self.played.add(old_time, self.current_time);
    }

    /// Set metadata after loading.
    pub fn set_metadata(&mut self, duration: f64, width: u32, height: u32) {
        self.duration = duration;
        self.ready_state = ReadyState::HaveMetadata;
        self.seekable.add(0.0, duration);
        
        let _ = self.event_tx.send(MediaEvent::LoadedMetadata {
            duration,
            width,
            height,
        });
        let _ = self.event_tx.send(MediaEvent::DurationChange(duration));
    }

    /// Set ready to play.
    pub fn set_ready(&mut self) {
        self.ready_state = ReadyState::HaveEnoughData;
        self.network_state = NetworkState::Idle;
        
        let _ = self.event_tx.send(MediaEvent::LoadedData);
        let _ = self.event_tx.send(MediaEvent::CanPlay);
        let _ = self.event_tx.send(MediaEvent::CanPlayThrough);
        
        if self.autoplay {
            let _ = self.play();
        }
    }

    /// Set error.
    pub fn set_error(&mut self, error: &str) {
        self.error = Some(error.to_string());
        self.network_state = NetworkState::Idle;
        let _ = self.event_tx.send(MediaEvent::Error(error.to_string()));
    }

    /// Add buffered range.
    pub fn add_buffered(&mut self, start: f64, end: f64) {
        self.buffered.add(start, end);
        let _ = self.event_tx.send(MediaEvent::Progress {
            loaded: (end * 1000.0) as u64,
            total: if self.duration.is_finite() {
                Some((self.duration * 1000.0) as u64)
            } else {
                None
            },
        });
    }

    /// Get effective volume (considering muted).
    pub fn effective_volume(&self) -> f64 {
        if self.muted {
            0.0
        } else {
            self.volume
        }
    }
}

impl Default for HTMLMediaElement {
    fn default() -> Self {
        Self::new().0
    }
}

// ==================== Audio Player ====================

/// Audio player using rodio.
pub struct AudioPlayer {
    /// Media element state.
    pub element: HTMLMediaElement,
    
    /// Event receiver.
    event_rx: mpsc::UnboundedReceiver<MediaEvent>,
    
    // TODO: Add rodio stream/sink when audio feature is fully implemented
    // _stream: Option<rodio::OutputStream>,
    // sink: Option<rodio::Sink>,
}

impl AudioPlayer {
    /// Create a new audio player.
    pub fn new() -> Self {
        let (element, event_rx) = HTMLMediaElement::new();
        
        Self {
            element,
            event_rx,
        }
    }

    /// Get the event receiver.
    pub fn take_event_receiver(&mut self) -> mpsc::UnboundedReceiver<MediaEvent> {
        std::mem::replace(&mut self.event_rx, mpsc::unbounded_channel().1)
    }

    /// Load audio from URL.
    pub async fn load(&mut self, url: &str) -> Result<(), MediaError> {
        self.element.set_src(url)?;
        
        // For now, just simulate loading
        // Real implementation would fetch and decode audio
        tokio::time::sleep(Duration::from_millis(100)).await;
        
        // Set metadata (simulated)
        self.element.set_metadata(180.0, 0, 0); // 3 minutes, no video dimensions
        self.element.set_ready();
        
        Ok(())
    }
}

impl Default for AudioPlayer {
    fn default() -> Self {
        Self::new()
    }
}

// ==================== Video Player ====================

/// Video frame.
#[derive(Debug, Clone)]
pub struct VideoFrame {
    pub width: u32,
    pub height: u32,
    pub timestamp: f64,
    pub data: Vec<u8>, // RGBA
}

/// Video player.
pub struct VideoPlayer {
    /// Media element state.
    pub element: HTMLMediaElement,
    
    /// Event receiver.
    event_rx: mpsc::UnboundedReceiver<MediaEvent>,
    
    /// Video dimensions.
    pub width: u32,
    pub height: u32,
    
    /// Current frame.
    pub current_frame: Option<VideoFrame>,
}

impl VideoPlayer {
    /// Create a new video player.
    pub fn new() -> Self {
        let (element, event_rx) = HTMLMediaElement::new();
        
        Self {
            element,
            event_rx,
            width: 0,
            height: 0,
            current_frame: None,
        }
    }

    /// Get the event receiver.
    pub fn take_event_receiver(&mut self) -> mpsc::UnboundedReceiver<MediaEvent> {
        std::mem::replace(&mut self.event_rx, mpsc::unbounded_channel().1)
    }

    /// Load video from URL.
    pub async fn load(&mut self, url: &str) -> Result<(), MediaError> {
        self.element.set_src(url)?;
        
        // For now, just simulate loading
        // Real implementation would fetch and decode video
        tokio::time::sleep(Duration::from_millis(100)).await;
        
        // Set metadata (simulated)
        self.width = 1920;
        self.height = 1080;
        self.element.set_metadata(120.0, self.width, self.height); // 2 minutes, 1080p
        self.element.set_ready();
        
        Ok(())
    }

    /// Get current frame for rendering.
    pub fn get_current_frame(&self) -> Option<&VideoFrame> {
        self.current_frame.as_ref()
    }
}

impl Default for VideoPlayer {
    fn default() -> Self {
        Self::new()
    }
}

// ==================== Media Manager ====================

/// Manages all media elements.
pub struct MediaManager {
    audio_players: HashMap<MediaId, AudioPlayer>,
    video_players: HashMap<MediaId, VideoPlayer>,
}

impl MediaManager {
    /// Create a new media manager.
    pub fn new() -> Self {
        Self {
            audio_players: HashMap::new(),
            video_players: HashMap::new(),
        }
    }

    /// Create a new audio element.
    pub fn create_audio(&mut self) -> MediaId {
        let player = AudioPlayer::new();
        let id = player.element.id;
        self.audio_players.insert(id, player);
        id
    }

    /// Create a new video element.
    pub fn create_video(&mut self) -> MediaId {
        let player = VideoPlayer::new();
        let id = player.element.id;
        self.video_players.insert(id, player);
        id
    }

    /// Get audio player.
    pub fn get_audio(&self, id: MediaId) -> Option<&AudioPlayer> {
        self.audio_players.get(&id)
    }

    /// Get audio player mutably.
    pub fn get_audio_mut(&mut self, id: MediaId) -> Option<&mut AudioPlayer> {
        self.audio_players.get_mut(&id)
    }

    /// Get video player.
    pub fn get_video(&self, id: MediaId) -> Option<&VideoPlayer> {
        self.video_players.get(&id)
    }

    /// Get video player mutably.
    pub fn get_video_mut(&mut self, id: MediaId) -> Option<&mut VideoPlayer> {
        self.video_players.get_mut(&id)
    }

    /// Remove audio element.
    pub fn remove_audio(&mut self, id: MediaId) -> Option<AudioPlayer> {
        self.audio_players.remove(&id)
    }

    /// Remove video element.
    pub fn remove_video(&mut self, id: MediaId) -> Option<VideoPlayer> {
        self.video_players.remove(&id)
    }

    /// Update all playing media (call each frame).
    pub fn update(&mut self, delta: f64) {
        for player in self.audio_players.values_mut() {
            player.element.update_time(delta);
        }
        for player in self.video_players.values_mut() {
            player.element.update_time(delta);
        }
    }
}

impl Default for MediaManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_media_element_creation() {
        let (element, _rx) = HTMLMediaElement::new();
        assert!(element.paused);
        assert_eq!(element.volume, 1.0);
        assert_eq!(element.network_state, NetworkState::Empty);
    }

    #[test]
    fn test_set_volume() {
        let (mut element, _rx) = HTMLMediaElement::new();
        assert!(element.set_volume(0.5).is_ok());
        assert_eq!(element.volume, 0.5);
        
        assert!(element.set_volume(1.5).is_err());
    }

    #[test]
    fn test_time_ranges() {
        let mut ranges = TimeRanges::new();
        ranges.add(0.0, 10.0);
        ranges.add(20.0, 30.0);
        
        assert_eq!(ranges.length(), 2);
        assert_eq!(ranges.start(0), Some(0.0));
        assert_eq!(ranges.end(0), Some(10.0));
    }

    #[test]
    fn test_time_ranges_merge() {
        let mut ranges = TimeRanges::new();
        ranges.add(0.0, 10.0);
        ranges.add(5.0, 15.0);
        
        assert_eq!(ranges.length(), 1);
        assert_eq!(ranges.start(0), Some(0.0));
        assert_eq!(ranges.end(0), Some(15.0));
    }

    #[test]
    fn test_play_pause() {
        let (mut element, _rx) = HTMLMediaElement::new();
        element.network_state = NetworkState::Idle;
        element.ready_state = ReadyState::HaveEnoughData;
        
        assert!(element.play().is_ok());
        assert!(!element.paused);
        
        element.pause();
        assert!(element.paused);
    }

    #[test]
    fn test_seek() {
        let (mut element, _rx) = HTMLMediaElement::new();
        element.ready_state = ReadyState::HaveMetadata;
        element.duration = 100.0;
        
        assert!(element.seek(50.0).is_ok());
        assert_eq!(element.current_time, 50.0);
        
        // Clamp to duration
        assert!(element.seek(150.0).is_ok());
        assert_eq!(element.current_time, 100.0);
    }

    #[test]
    fn test_can_play_type() {
        let (element, _rx) = HTMLMediaElement::new();
        assert_eq!(element.can_play_type("audio/mpeg"), "probably");
        assert_eq!(element.can_play_type("video/mp4"), "maybe");
        assert_eq!(element.can_play_type("unknown/format"), "");
    }

    #[test]
    fn test_audio_player() {
        let player = AudioPlayer::new();
        assert!(player.element.paused);
    }

    #[test]
    fn test_video_player() {
        let player = VideoPlayer::new();
        assert_eq!(player.width, 0);
        assert_eq!(player.height, 0);
    }

    #[test]
    fn test_media_manager() {
        let mut manager = MediaManager::new();
        
        let audio_id = manager.create_audio();
        let video_id = manager.create_video();
        
        assert!(manager.get_audio(audio_id).is_some());
        assert!(manager.get_video(video_id).is_some());
        
        assert!(manager.remove_audio(audio_id).is_some());
        assert!(manager.get_audio(audio_id).is_none());
    }

    #[test]
    fn test_update_time() {
        let (mut element, _rx) = HTMLMediaElement::new();
        element.network_state = NetworkState::Idle;
        element.ready_state = ReadyState::HaveEnoughData;
        element.duration = 100.0;
        element.paused = false;
        
        element.update_time(1.0);
        assert_eq!(element.current_time, 1.0);
        
        element.update_time(1.0);
        assert_eq!(element.current_time, 2.0);
    }

    #[test]
    fn test_loop() {
        let (mut element, _rx) = HTMLMediaElement::new();
        element.network_state = NetworkState::Idle;
        element.ready_state = ReadyState::HaveEnoughData;
        element.duration = 10.0;
        element.paused = false;
        element.loop_ = true;
        element.current_time = 9.5;
        
        element.update_time(1.0);
        assert_eq!(element.current_time, 0.0);
        assert!(!element.ended);
    }

    #[test]
    fn test_ended() {
        let (mut element, _rx) = HTMLMediaElement::new();
        element.network_state = NetworkState::Idle;
        element.ready_state = ReadyState::HaveEnoughData;
        element.duration = 10.0;
        element.paused = false;
        element.loop_ = false;
        element.current_time = 9.5;
        
        element.update_time(1.0);
        assert!(element.ended);
        assert!(element.paused);
    }
}

