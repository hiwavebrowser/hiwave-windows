//! # RustKit Image
//!
//! Image loading, decoding, and caching for the RustKit browser engine.
//!
//! This crate handles:
//! - Async image fetching from URLs
//! - Decoding of PNG, JPEG, GIF, WebP, BMP, and ICO formats
//! - Animated GIF support
//! - Memory and disk caching
//! - GPU texture management
//! - Lazy loading support

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use rustkit_codecs::{Decoded, ImageFormat, RgbaImage};
use thiserror::Error;
use tokio::sync::{mpsc, oneshot};
use tracing::debug;
use url::Url;

pub mod cache;
pub mod decode;
pub mod loader;

pub use cache::*;
pub use decode::*;
pub use loader::*;

/// Errors that can occur during image operations
#[derive(Error, Debug)]
pub enum ImageError {
    #[error("Failed to fetch image: {0}")]
    FetchError(String),

    #[error("Failed to decode image: {0}")]
    DecodeError(String),

    #[error("Unsupported image format: {0}")]
    UnsupportedFormat(String),

    #[error("Image too large: {width}x{height} exceeds maximum")]
    TooLarge { width: u32, height: u32 },

    #[error("Invalid image URL: {0}")]
    InvalidUrl(String),

    #[error("Network error: {0}")]
    NetworkError(#[from] rustkit_http::HttpError),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("Cache error: {0}")]
    CacheError(String),
}

/// Result type for image operations
pub type ImageResult<T> = Result<T, ImageError>;

/// Represents a loaded and decoded image
#[derive(Clone)]
pub struct LoadedImage {
    /// Original URL of the image
    pub url: Url,

    /// Natural width of the image
    pub natural_width: u32,

    /// Natural height of the image
    pub natural_height: u32,

    /// Image data (static or animated)
    pub data: ImageData,

    /// When this image was decoded
    pub decoded_at: Instant,

    /// Content type from HTTP response
    pub content_type: Option<String>,

    /// Whether this image is complete (loaded successfully)
    pub complete: bool,
}

impl LoadedImage {
    /// Create a new loaded image from decoded data
    pub fn new(url: Url, image: RgbaImage) -> Self {
        let natural_width = image.width();
        let natural_height = image.height();
        Self {
            url,
            natural_width,
            natural_height,
            data: ImageData::Static(image),
            decoded_at: Instant::now(),
            content_type: None,
            complete: true,
        }
    }

    /// Create an animated image
    pub fn animated(url: Url, frames: Vec<AnimationFrame>) -> Self {
        let (natural_width, natural_height) = if let Some(first) = frames.first() {
            (first.image.width(), first.image.height())
        } else {
            (0, 0)
        };

        Self {
            url,
            natural_width,
            natural_height,
            data: ImageData::Animated(AnimatedImage {
                frames,
                loop_count: 0, // Infinite
            }),
            decoded_at: Instant::now(),
            content_type: None,
            complete: true,
        }
    }

    /// Get the current frame to display
    pub fn current_frame(&self, elapsed: Duration) -> &RgbaImage {
        match &self.data {
            ImageData::Static(img) => img,
            ImageData::Animated(anim) => anim.frame_at(elapsed),
        }
    }

    /// Check if this image is animated
    pub fn is_animated(&self) -> bool {
        matches!(self.data, ImageData::Animated(_))
    }

    /// Get the aspect ratio
    pub fn aspect_ratio(&self) -> f64 {
        if self.natural_height == 0 {
            1.0
        } else {
            self.natural_width as f64 / self.natural_height as f64
        }
    }
}

/// Image data - either static or animated
#[derive(Clone)]
pub enum ImageData {
    /// Single static image
    Static(RgbaImage),

    /// Animated image with multiple frames
    Animated(AnimatedImage),
}

/// Animated image with frames
#[derive(Clone)]
pub struct AnimatedImage {
    /// All frames
    pub frames: Vec<AnimationFrame>,

    /// Number of times to loop (0 = infinite)
    pub loop_count: u32,
}

impl AnimatedImage {
    /// Get the frame at a given elapsed time
    pub fn frame_at(&self, elapsed: Duration) -> &RgbaImage {
        if self.frames.is_empty() {
            panic!("AnimatedImage has no frames");
        }

        let total_duration: u64 = self.frames.iter().map(|f| f.delay_ms as u64).sum();
        if total_duration == 0 {
            return &self.frames[0].image;
        }

        let elapsed_ms = elapsed.as_millis() as u64 % total_duration;
        let mut cumulative = 0u64;

        for frame in &self.frames {
            cumulative += frame.delay_ms as u64;
            if elapsed_ms < cumulative {
                return &frame.image;
            }
        }

        &self.frames.last().unwrap().image
    }

    /// Get the total animation duration
    pub fn total_duration(&self) -> Duration {
        let total_ms: u64 = self.frames.iter().map(|f| f.delay_ms as u64).sum();
        Duration::from_millis(total_ms)
    }
}

/// A single animation frame
#[derive(Clone)]
pub struct AnimationFrame {
    /// The frame image
    pub image: RgbaImage,

    /// Delay before showing next frame (in milliseconds)
    pub delay_ms: u32,
}

/// Image loading state for tracking progress
#[derive(Clone, Debug)]
pub enum LoadingState {
    /// Not started
    Pending,

    /// Currently loading
    Loading {
        bytes_loaded: usize,
        bytes_total: Option<usize>,
    },

    /// Decoding the image
    Decoding,

    /// Successfully loaded
    Complete,

    /// Failed to load
    Error(String),
}

/// Request for loading an image
#[derive(Debug)]
pub struct ImageRequest {
    /// URL to load
    pub url: Url,

    /// Whether to use cache
    pub use_cache: bool,

    /// Priority (higher = load sooner)
    pub priority: u8,

    /// Whether this is a lazy load (defer if offscreen)
    pub lazy: bool,

    /// Desired width hint for srcset selection
    pub width_hint: Option<u32>,
}

impl ImageRequest {
    /// Create a simple request for a URL
    pub fn new(url: Url) -> Self {
        Self {
            url,
            use_cache: true,
            priority: 5,
            lazy: false,
            width_hint: None,
        }
    }

    /// Set lazy loading
    pub fn lazy(mut self, lazy: bool) -> Self {
        self.lazy = lazy;
        self
    }

    /// Set priority
    pub fn priority(mut self, priority: u8) -> Self {
        self.priority = priority;
        self
    }

    /// Set width hint for responsive images
    pub fn width_hint(mut self, width: u32) -> Self {
        self.width_hint = Some(width);
        self
    }
}

/// The main image manager that handles loading and caching
pub struct ImageManager {
    /// Memory cache for decoded images
    cache: Arc<RwLock<ImageCache>>,

    /// HTTP client for fetching images
    client: rustkit_http::Client,

    /// Pending loads
    #[allow(clippy::type_complexity)]
    pending: Arc<RwLock<HashMap<Url, Vec<oneshot::Sender<ImageResult<Arc<LoadedImage>>>>>>>,

    /// Channel for sending load requests
    request_tx: mpsc::Sender<ImageRequest>,

    /// Maximum image dimensions
    max_dimensions: (u32, u32),

    /// Maximum memory cache size in bytes
    #[allow(dead_code)]
    max_cache_bytes: usize,
}

impl ImageManager {
    /// Create a new image manager
    pub fn new() -> Self {
        let (request_tx, _request_rx) = mpsc::channel::<ImageRequest>(100);

        Self {
            cache: Arc::new(RwLock::new(ImageCache::new(100))),
            client: rustkit_http::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .expect("Failed to create HTTP client"),
            pending: Arc::new(RwLock::new(HashMap::new())),
            request_tx,
            max_dimensions: (16384, 16384),
            max_cache_bytes: 256 * 1024 * 1024, // 256MB
        }
    }

    /// Load an image from a URL
    pub async fn load(&self, url: Url) -> ImageResult<Arc<LoadedImage>> {
        // Check cache first
        if let Some(cached) = self.cache.read().unwrap().get(&url) {
            debug!("Image cache hit: {}", url);
            return Ok(cached);
        }

        // Check if already loading
        let already_loading = {
            let pending = self.pending.read().unwrap();
            pending.contains_key(&url)
        };

        if already_loading {
            debug!("Image already loading: {}", url);
            // Add ourselves to the waiting list
            let (tx, rx) = oneshot::channel();
            self.pending.write().unwrap().entry(url.clone()).or_default().push(tx);
            return rx.await.map_err(|_| ImageError::FetchError("Load cancelled".into()))?;
        }

        // Start loading
        debug!("Starting image load: {}", url);
        self.pending.write().unwrap().insert(url.clone(), vec![]);

        let result = self.fetch_and_decode(url.clone()).await;

        // Notify waiters and cache result
        let waiters = self.pending.write().unwrap().remove(&url).unwrap_or_default();
        
        match &result {
            Ok(image) => {
                self.cache.write().unwrap().insert(url.clone(), image.clone());
                for waiter in waiters {
                    let _ = waiter.send(Ok(image.clone()));
                }
            }
            Err(e) => {
                let err_msg = e.to_string();
                for waiter in waiters {
                    let _ = waiter.send(Err(ImageError::FetchError(err_msg.clone())));
                }
            }
        }

        result
    }

    /// Fetch and decode an image
    async fn fetch_and_decode(&self, url: Url) -> ImageResult<Arc<LoadedImage>> {
        // Handle data URLs
        if url.scheme() == "data" {
            return self.decode_data_url(&url);
        }

        // Fetch the image using rustkit-http
        let response = self.client.get(url.as_str()).await?;

        if !response.is_success() {
            return Err(ImageError::FetchError(format!(
                "HTTP {} for {}",
                response.status,
                url
            )));
        }

        let content_type = response.content_type().map(|s| s.to_string());

        // Decode the image
        let mut loaded = self.decode_bytes(&url, &response.body)?;
        loaded.content_type = content_type;

        Ok(Arc::new(loaded))
    }

    /// Decode bytes through the real engine path. Test-only surface so a
    /// codec fix can be proven to REACH the engine rather than merely
    /// existing in its own crate — the orphan shape this codebase keeps
    /// producing. Calls the same private decoder production uses.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn decode_bytes_for_test(&self, url: &Url, bytes: &[u8]) -> ImageResult<LoadedImage> {
        self.decode_bytes(url, bytes)
    }

    /// Decode image from bytes
    fn decode_bytes(&self, url: &Url, bytes: &[u8]) -> ImageResult<LoadedImage> {
        // Guess format from bytes
        let format = rustkit_codecs::detect_format(bytes)
            .unwrap_or(ImageFormat::Unknown);

        if format == ImageFormat::Unknown {
            return Err(ImageError::DecodeError("Unknown image format".into()));
        }

        // Handle animated GIFs specially
        if format == ImageFormat::Gif {
            return self.decode_gif(url, bytes);
        }

        // Decode static image
        let decoded = rustkit_codecs::decode_any(bytes)
            .map_err(|e| ImageError::DecodeError(e.to_string()))?;
        let img = match decoded {
            Decoded::Static(img) => img,
            Decoded::Animated(frames) => {
                // Some formats may be treated as animated later; for now, take first frame.
                frames
                    .into_iter()
                    .next()
                    .map(|f| f.image)
                    .ok_or_else(|| ImageError::DecodeError("Animated image had no frames".into()))?
            }
        };

        // Check dimensions
        let (width, height) = (img.width(), img.height());
        if width > self.max_dimensions.0 || height > self.max_dimensions.1 {
            return Err(ImageError::TooLarge { width, height });
        }

        Ok(LoadedImage::new(url.clone(), img))
    }

    /// Decode an animated GIF
    fn decode_gif(&self, url: &Url, bytes: &[u8]) -> ImageResult<LoadedImage> {
        let decoded_frames = rustkit_codecs::decode_gif(bytes)
            .map_err(|e| ImageError::DecodeError(e.to_string()))?;

        let mut frames = Vec::with_capacity(decoded_frames.len());
        for f in decoded_frames {
            // Check dimensions
            if f.image.width() > self.max_dimensions.0 || f.image.height() > self.max_dimensions.1 {
                return Err(ImageError::TooLarge {
                    width: f.image.width(),
                    height: f.image.height(),
                });
            }
            frames.push(AnimationFrame {
                image: f.image,
                delay_ms: f.delay_ms.max(10),
            });
        }

        if frames.is_empty() {
            return Err(ImageError::DecodeError("GIF has no frames".into()));
        }

        // Single frame = static image
        if frames.len() == 1 {
            let frame = frames.remove(0);
            return Ok(LoadedImage {
                url: url.clone(),
                natural_width: frame.image.width(),
                natural_height: frame.image.height(),
                data: ImageData::Static(frame.image),
                decoded_at: Instant::now(),
                content_type: Some("image/gif".into()),
                complete: true,
            });
        }

        Ok(LoadedImage::animated(url.clone(), frames))
    }

    /// Decode a data URL
    fn decode_data_url(&self, url: &Url) -> ImageResult<Arc<LoadedImage>> {
        let path = url.path();

        // Parse data URL: data:[<mediatype>][;base64],<data>
        let comma_pos = path.find(',')
            .ok_or_else(|| ImageError::InvalidUrl("Invalid data URL format".into()))?;

        let metadata = &path[..comma_pos];
        let data = &path[comma_pos + 1..];

        let is_base64 = metadata.contains("base64");
        let is_svg = metadata.contains("image/svg+xml");

        let bytes = if is_base64 {
            use base64::Engine;
            base64::engine::general_purpose::STANDARD.decode(data)
                .map_err(|e| ImageError::DecodeError(format!("Base64 decode error: {}", e)))?
        } else {
            // URL-encoded
            urlencoding::decode(data)
                .map_err(|e| ImageError::DecodeError(format!("URL decode error: {}", e)))?
                .into_owned()
                .into_bytes()
        };

        // Handle SVG data URLs specially
        if is_svg {
            let svg_text = String::from_utf8(bytes)
                .map_err(|e| ImageError::DecodeError(format!("SVG not valid UTF-8: {}", e)))?;
            return self.rasterize_svg(url, &svg_text);
        }

        let loaded = self.decode_bytes(url, &bytes)?;
        Ok(Arc::new(loaded))
    }

    /// Rasterize an SVG to pixels using a simple inline parser.
    /// This handles basic SVG shapes (rect, circle) with solid fills,
    /// which is sufficient for most image placeholder and test cases.
    fn rasterize_svg(&self, url: &Url, svg_text: &str) -> ImageResult<Arc<LoadedImage>> {
        // Parse SVG dimensions from attributes
        let (width, height) = parse_svg_dimensions(svg_text).unwrap_or((100, 100));
        let width = width.max(1);
        let height = height.max(1);

        // Create pixel buffer with transparent background
        let mut pixels = vec![0u8; (width * height * 4) as usize];

        // Parse and render rectangles
        for rect_match in find_svg_rects(svg_text) {
            let (rx, ry, rw, rh, fill_color) = rect_match;
            let x0 = (rx.max(0.0) as u32).min(width);
            let y0 = (ry.max(0.0) as u32).min(height);
            let x1 = ((rx + rw).max(0.0) as u32).min(width);
            let y1 = ((ry + rh).max(0.0) as u32).min(height);

            // Fill the rectangle
            for y in y0..y1 {
                for x in x0..x1 {
                    let idx = ((y * width + x) * 4) as usize;
                    if idx + 3 < pixels.len() {
                        pixels[idx] = fill_color.0;     // R
                        pixels[idx + 1] = fill_color.1; // G
                        pixels[idx + 2] = fill_color.2; // B
                        pixels[idx + 3] = fill_color.3; // A
                    }
                }
            }
        }

        // Parse and render circles
        for circle_match in find_svg_circles(svg_text) {
            let (cx, cy, r, fill_color) = circle_match;
            let r_sq = r * r;

            // Render circle with simple distance-based fill
            let x0 = ((cx - r).max(0.0) as u32).min(width);
            let y0 = ((cy - r).max(0.0) as u32).min(height);
            let x1 = ((cx + r).max(0.0) as u32).min(width);
            let y1 = ((cy + r).max(0.0) as u32).min(height);

            for y in y0..y1 {
                for x in x0..x1 {
                    let dx = x as f32 - cx;
                    let dy = y as f32 - cy;
                    if dx * dx + dy * dy <= r_sq {
                        let idx = ((y * width + x) * 4) as usize;
                        if idx + 3 < pixels.len() {
                            pixels[idx] = fill_color.0;
                            pixels[idx + 1] = fill_color.1;
                            pixels[idx + 2] = fill_color.2;
                            pixels[idx + 3] = fill_color.3;
                        }
                    }
                }
            }
        }

        let image = RgbaImage::from_rgba8(width, height, pixels)
            .map_err(|e| ImageError::DecodeError(format!("SVG rasterization failed: {}", e)))?;
        Ok(Arc::new(LoadedImage::new(url.clone(), image)))
    }

    /// Preload an image without blocking
    pub fn preload(&self, url: Url) {
        let _ = self.request_tx.try_send(ImageRequest::new(url));
    }

    /// Clear the cache
    pub fn clear_cache(&self) {
        self.cache.write().unwrap().clear();
    }

    /// Get cache statistics
    pub fn cache_stats(&self) -> CacheStats {
        self.cache.read().unwrap().stats()
    }

    /// Check if an image is cached
    pub fn is_cached(&self, url: &Url) -> bool {
        self.cache.read().unwrap().contains(url)
    }

    /// Get a cached image if available
    pub fn get_cached(&self, url: &Url) -> Option<Arc<LoadedImage>> {
        self.cache.read().unwrap().get(url)
    }

    /// Load an image synchronously (blocking).
    /// This is primarily for parity testing where we need images to be loaded
    /// before capturing a frame.
    pub fn load_blocking(&self, url: Url) -> ImageResult<Arc<LoadedImage>> {
        // Check cache first
        if let Some(cached) = self.cache.read().unwrap().get(&url) {
            return Ok(cached);
        }

        // For data URLs, decode synchronously
        if url.scheme() == "data" {
            return self.decode_data_url(&url);
        }

        // For other URLs, we need to block on the async load
        // This uses a simple polling approach
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| ImageError::FetchError(format!("Runtime error: {}", e)))?;

        runtime.block_on(self.load(url))
    }

    /// Check if an image is loading
    pub fn is_loading(&self, url: &Url) -> bool {
        self.pending.read().unwrap().contains_key(url)
    }

    /// Get all cached images.
    /// Returns a vector of (URL, image) pairs for all images in the cache.
    pub fn get_all_cached(&self) -> Vec<(Url, Arc<LoadedImage>)> {
        self.cache.read().unwrap().entries()
    }
}

impl Default for ImageManager {
    fn default() -> Self {
        Self::new()
    }
}

/// CSS object-fit values
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ObjectFit {
    /// Fill the box, possibly distorting the image
    #[default]
    Fill,

    /// Scale to fit inside the box, preserving aspect ratio
    Contain,

    /// Scale to cover the box, preserving aspect ratio
    Cover,

    /// Don't scale the image
    None,

    /// Like `contain` but never scale up
    ScaleDown,
}

impl ObjectFit {
    /// Parse from CSS value
    pub fn from_css(value: &str) -> Option<Self> {
        match value.trim().to_lowercase().as_str() {
            "fill" => Some(ObjectFit::Fill),
            "contain" => Some(ObjectFit::Contain),
            "cover" => Some(ObjectFit::Cover),
            "none" => Some(ObjectFit::None),
            "scale-down" => Some(ObjectFit::ScaleDown),
            _ => None,
        }
    }

    /// Calculate the image rectangle within a container
    pub fn compute_rect(
        &self,
        container_width: f64,
        container_height: f64,
        image_width: f64,
        image_height: f64,
        object_position: (f64, f64), // 0-1 range, default (0.5, 0.5)
    ) -> ImageRect {
        if image_width == 0.0 || image_height == 0.0 {
            return ImageRect::default();
        }

        let image_aspect = image_width / image_height;
        let container_aspect = container_width / container_height;

        let (draw_width, draw_height) = match self {
            ObjectFit::Fill => (container_width, container_height),

            ObjectFit::Contain => {
                if image_aspect > container_aspect {
                    // Image is wider - fit to width
                    (container_width, container_width / image_aspect)
                } else {
                    // Image is taller - fit to height
                    (container_height * image_aspect, container_height)
                }
            }

            ObjectFit::Cover => {
                if image_aspect > container_aspect {
                    // Image is wider - fit to height
                    (container_height * image_aspect, container_height)
                } else {
                    // Image is taller - fit to width
                    (container_width, container_width / image_aspect)
                }
            }

            ObjectFit::None => (image_width, image_height),

            ObjectFit::ScaleDown => {
                // Use contain but only if it would scale down
                if image_width <= container_width && image_height <= container_height {
                    (image_width, image_height)
                } else if image_aspect > container_aspect {
                    (container_width, container_width / image_aspect)
                } else {
                    (container_height * image_aspect, container_height)
                }
            }
        };

        // Position within container
        let x = (container_width - draw_width) * object_position.0;
        let y = (container_height - draw_height) * object_position.1;

        ImageRect {
            x,
            y,
            width: draw_width,
            height: draw_height,
        }
    }
}

/// Rectangle for drawing an image
#[derive(Debug, Clone, Default)]
pub struct ImageRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// CSS object-position parsing
#[derive(Debug, Clone, Default)]
pub struct ObjectPosition {
    pub x: f64, // 0-1 range
    pub y: f64, // 0-1 range
}

impl ObjectPosition {
    /// Default center position
    pub fn center() -> Self {
        Self { x: 0.5, y: 0.5 }
    }

    /// Parse from CSS value
    pub fn from_css(value: &str) -> Self {
        let parts: Vec<&str> = value.split_whitespace().collect();

        let parse_keyword_or_percentage = |s: &str| -> f64 {
            match s.to_lowercase().as_str() {
                "left" | "top" => 0.0,
                "center" => 0.5,
                "right" | "bottom" => 1.0,
                s if s.ends_with('%') => {
                    s.trim_end_matches('%').parse::<f64>().unwrap_or(50.0) / 100.0
                }
                _ => 0.5,
            }
        };

        match parts.len() {
            0 => Self::center(),
            1 => {
                let v = parse_keyword_or_percentage(parts[0]);
                Self { x: v, y: v }
            }
            _ => Self {
                x: parse_keyword_or_percentage(parts[0]),
                y: parse_keyword_or_percentage(parts[1]),
            },
        }
    }
}

// ==================== Simple SVG Parsing Helpers ====================

/// Parse SVG dimensions from the root element attributes.
fn parse_svg_dimensions(svg: &str) -> Option<(u32, u32)> {
    // Look for width and height attributes in the <svg> tag
    let svg_start = svg.find("<svg")?;
    let svg_end = svg[svg_start..].find('>')? + svg_start;
    let svg_attrs = &svg[svg_start..svg_end];

    let width = extract_svg_attr(svg_attrs, "width")
        .and_then(|s| s.trim_end_matches("px").parse::<f32>().ok())
        .map(|v| v as u32)?;

    let height = extract_svg_attr(svg_attrs, "height")
        .and_then(|s| s.trim_end_matches("px").parse::<f32>().ok())
        .map(|v| v as u32)?;

    Some((width, height))
}

/// Extract an attribute value from an SVG tag string.
fn extract_svg_attr(tag: &str, name: &str) -> Option<String> {
    // Look for name=' or name="
    let patterns = [format!("{}='", name), format!("{}=\"", name)];

    for pattern in &patterns {
        if let Some(start) = tag.find(pattern) {
            let rest = &tag[start + pattern.len()..];
            let quote = if pattern.ends_with('\'') { '\'' } else { '"' };
            if let Some(end) = rest.find(quote) {
                return Some(rest[..end].to_string());
            }
        }
    }
    None
}

/// Find all <rect> elements and extract their properties.
/// Returns (x, y, width, height, fill_color) tuples.
fn find_svg_rects(svg: &str) -> Vec<(f32, f32, f32, f32, (u8, u8, u8, u8))> {
    let mut rects = Vec::new();
    let mut pos = 0;

    while let Some(rect_start) = svg[pos..].find("<rect") {
        let rect_start = pos + rect_start;
        let rect_end = match svg[rect_start..].find("/>") {
            Some(e) => rect_start + e + 2,
            None => match svg[rect_start..].find('>') {
                Some(e) => rect_start + e + 1,
                None => break,
            },
        };

        let rect_tag = &svg[rect_start..rect_end];

        let x = extract_svg_attr(rect_tag, "x")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0.0);
        let y = extract_svg_attr(rect_tag, "y")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0.0);
        let width = extract_svg_attr(rect_tag, "width")
            .and_then(|s| s.trim_end_matches("px").parse().ok())
            .unwrap_or(0.0);
        let height = extract_svg_attr(rect_tag, "height")
            .and_then(|s| s.trim_end_matches("px").parse().ok())
            .unwrap_or(0.0);
        let fill = extract_svg_attr(rect_tag, "fill")
            .map(|s| parse_svg_color(&s))
            .unwrap_or((0, 0, 0, 255));

        rects.push((x, y, width, height, fill));
        pos = rect_end;
    }

    rects
}

/// Find all <circle> elements and extract their properties.
/// Returns (cx, cy, r, fill_color) tuples.
fn find_svg_circles(svg: &str) -> Vec<(f32, f32, f32, (u8, u8, u8, u8))> {
    let mut circles = Vec::new();
    let mut pos = 0;

    while let Some(circle_start) = svg[pos..].find("<circle") {
        let circle_start = pos + circle_start;
        let circle_end = match svg[circle_start..].find("/>") {
            Some(e) => circle_start + e + 2,
            None => match svg[circle_start..].find('>') {
                Some(e) => circle_start + e + 1,
                None => break,
            },
        };

        let circle_tag = &svg[circle_start..circle_end];

        let cx = extract_svg_attr(circle_tag, "cx")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0.0);
        let cy = extract_svg_attr(circle_tag, "cy")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0.0);
        let r = extract_svg_attr(circle_tag, "r")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0.0);
        let fill = extract_svg_attr(circle_tag, "fill")
            .map(|s| parse_svg_color(&s))
            .unwrap_or((0, 0, 0, 255));

        circles.push((cx, cy, r, fill));
        pos = circle_end;
    }

    circles
}

/// Parse an SVG color value to RGBA.
fn parse_svg_color(s: &str) -> (u8, u8, u8, u8) {
    let s = s.trim();

    // Hex colors
    if s.starts_with('#') {
        let hex = &s[1..];
        match hex.len() {
            3 => {
                let r = u8::from_str_radix(&hex[0..1].repeat(2), 16).unwrap_or(0);
                let g = u8::from_str_radix(&hex[1..2].repeat(2), 16).unwrap_or(0);
                let b = u8::from_str_radix(&hex[2..3].repeat(2), 16).unwrap_or(0);
                return (r, g, b, 255);
            }
            6 => {
                let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(0);
                let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(0);
                let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(0);
                return (r, g, b, 255);
            }
            _ => {}
        }
    }

    // URL-encoded hex (e.g., %23ff0000 for #ff0000)
    if s.starts_with("%23") {
        let hex = &s[3..];
        match hex.len() {
            6 => {
                let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(0);
                let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(0);
                let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(0);
                return (r, g, b, 255);
            }
            _ => {}
        }
    }

    // Named colors
    match s.to_lowercase().as_str() {
        "black" => (0, 0, 0, 255),
        "white" => (255, 255, 255, 255),
        "red" => (255, 0, 0, 255),
        "green" => (0, 128, 0, 255),
        "blue" => (0, 0, 255, 255),
        "yellow" => (255, 255, 0, 255),
        "cyan" => (0, 255, 255, 255),
        "magenta" => (255, 0, 255, 255),
        "gray" | "grey" => (128, 128, 128, 255),
        "orange" => (255, 165, 0, 255),
        "purple" => (128, 0, 128, 255),
        "transparent" => (0, 0, 0, 0),
        _ => (0, 0, 0, 255), // Default to black
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_svg_parsing() {
        let svg = r#"<svg xmlns='http://www.w3.org/2000/svg' width='100' height='100'><rect fill='%23e74c3c' width='100' height='100'/></svg>"#;
        let dims = parse_svg_dimensions(svg);
        assert_eq!(dims, Some((100, 100)));

        let rects = find_svg_rects(svg);
        assert_eq!(rects.len(), 1);
        assert_eq!(rects[0].2, 100.0); // width
        assert_eq!(rects[0].3, 100.0); // height
        // %23e74c3c is URL-encoded #e74c3c (tomato red)
        assert_eq!(rects[0].4, (231, 76, 60, 255));
    }

    #[test]
    fn test_object_fit_contain() {
        let fit = ObjectFit::Contain;
        let rect = fit.compute_rect(100.0, 100.0, 200.0, 100.0, (0.5, 0.5));
        assert!((rect.width - 100.0).abs() < 0.001);
        assert!((rect.height - 50.0).abs() < 0.001);
        assert!((rect.x - 0.0).abs() < 0.001);
        assert!((rect.y - 25.0).abs() < 0.001);
    }

    #[test]
    fn test_object_fit_cover() {
        let fit = ObjectFit::Cover;
        let rect = fit.compute_rect(100.0, 100.0, 200.0, 100.0, (0.5, 0.5));
        assert!((rect.width - 200.0).abs() < 0.001);
        assert!((rect.height - 100.0).abs() < 0.001);
        assert!((rect.x - -50.0).abs() < 0.001);
    }

    #[test]
    fn test_object_fit_fill() {
        let fit = ObjectFit::Fill;
        let rect = fit.compute_rect(100.0, 80.0, 200.0, 100.0, (0.5, 0.5));
        assert!((rect.width - 100.0).abs() < 0.001);
        assert!((rect.height - 80.0).abs() < 0.001);
    }

    #[test]
    fn test_object_position_parsing() {
        let pos = ObjectPosition::from_css("left top");
        assert!((pos.x - 0.0).abs() < 0.001);
        assert!((pos.y - 0.0).abs() < 0.001);

        let pos = ObjectPosition::from_css("center");
        assert!((pos.x - 0.5).abs() < 0.001);
        assert!((pos.y - 0.5).abs() < 0.001);

        let pos = ObjectPosition::from_css("75% 25%");
        assert!((pos.x - 0.75).abs() < 0.001);
        assert!((pos.y - 0.25).abs() < 0.001);
    }

    #[test]
    fn test_animated_image_frame_at() {
        let rgba1 = RgbaImage::new(10, 10);
        let rgba2 = RgbaImage::new(10, 10);
        let anim = AnimatedImage {
            frames: vec![
                AnimationFrame { image: rgba1, delay_ms: 100 },
                AnimationFrame { image: rgba2, delay_ms: 100 },
            ],
            loop_count: 0,
        };

        // At 0ms, should be frame 0
        let _ = anim.frame_at(Duration::from_millis(0));
        
        // At 150ms, should be frame 1
        let _ = anim.frame_at(Duration::from_millis(150));

        // At 250ms (past loop), should be frame 0 again
        let _ = anim.frame_at(Duration::from_millis(250));
    }

    #[test]
    fn test_object_fit_scale_down() {
        // Image smaller than container - don't scale
        let fit = ObjectFit::ScaleDown;
        let rect = fit.compute_rect(200.0, 200.0, 50.0, 50.0, (0.5, 0.5));
        assert!((rect.width - 50.0).abs() < 0.001);
        assert!((rect.height - 50.0).abs() < 0.001);

        // Image larger than container - scale down like contain
        let rect = fit.compute_rect(100.0, 100.0, 400.0, 200.0, (0.5, 0.5));
        assert!((rect.width - 100.0).abs() < 0.001);
        assert!((rect.height - 50.0).abs() < 0.001);
    }
}

