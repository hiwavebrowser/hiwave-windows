//! Image element support for the DOM
//!
//! Tracks image elements (`<img>`, `<picture>`, CSS background-image) and their loading states.

use std::collections::HashMap;
use crate::NodeId;

/// Loading state for an image
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageLoadingState {
    /// Not started loading
    Pending,

    /// Currently loading
    Loading,

    /// Successfully loaded
    Complete,

    /// Failed to load
    Error(String),
}

/// Represents an image element's attributes and state
#[derive(Debug, Clone)]
pub struct ImageElement {
    /// Node ID this image element belongs to
    pub node_id: NodeId,

    /// Primary source URL
    pub src: Option<String>,

    /// Alt text for accessibility
    pub alt: Option<String>,

    /// Explicit width attribute
    pub width: Option<u32>,

    /// Explicit height attribute
    pub height: Option<u32>,

    /// Srcset for responsive images
    pub srcset: Option<String>,

    /// Sizes attribute for responsive images
    pub sizes: Option<String>,

    /// Loading attribute (lazy or eager)
    pub loading: ImageLoading,

    /// Decoding attribute
    pub decoding: ImageDecoding,

    /// Crossorigin attribute
    pub crossorigin: Option<CrossOrigin>,

    /// Referrer policy
    pub referrer_policy: Option<String>,

    /// Current loading state
    pub loading_state: ImageLoadingState,

    /// Natural dimensions (set after loading)
    pub natural_width: Option<u32>,
    pub natural_height: Option<u32>,

    /// Whether the image is complete (loaded or errored)
    pub complete: bool,

    /// Current source URL being used (resolved from srcset if applicable)
    pub current_src: Option<String>,
}

impl ImageElement {
    /// Create a new image element from DOM attributes
    pub fn from_attributes(node_id: NodeId, attrs: &crate::AttrMap) -> Self {
        Self {
            node_id,
            src: attrs.get("src").cloned(),
            alt: attrs.get("alt").cloned(),
            width: attrs.get("width").and_then(|w| w.parse().ok()),
            height: attrs.get("height").and_then(|h| h.parse().ok()),
            srcset: attrs.get("srcset").cloned(),
            sizes: attrs.get("sizes").cloned(),
            loading: attrs
                .get("loading")
                .map(|l| ImageLoading::from_str(l))
                .unwrap_or_default(),
            decoding: attrs
                .get("decoding")
                .map(|d| ImageDecoding::from_str(d))
                .unwrap_or_default(),
            crossorigin: attrs.get("crossorigin").map(|s| CrossOrigin::from_str(s)),
            referrer_policy: attrs.get("referrerpolicy").cloned(),
            loading_state: ImageLoadingState::Pending,
            natural_width: None,
            natural_height: None,
            complete: false,
            current_src: None,
        }
    }

    /// Check if this image should be lazy loaded
    pub fn is_lazy(&self) -> bool {
        self.loading == ImageLoading::Lazy
    }

    /// Get the effective source URL to load
    pub fn effective_src(&self, viewport_width: u32, device_pixel_ratio: f64) -> Option<String> {
        // If srcset is present, select the best source
        if let Some(srcset) = &self.srcset {
            use rustkit_image::loader::{parse_srcset, select_srcset_entry};

            let entries = parse_srcset(srcset);
            if let Some(entry) = select_srcset_entry(&entries, viewport_width, device_pixel_ratio) {
                return Some(entry.url.clone());
            }
        }

        // Fall back to src
        self.src.clone()
    }

    /// Update loading state
    pub fn set_loading(&mut self) {
        self.loading_state = ImageLoadingState::Loading;
    }

    /// Mark as complete with dimensions
    pub fn set_complete(&mut self, natural_width: u32, natural_height: u32, current_src: String) {
        self.loading_state = ImageLoadingState::Complete;
        self.natural_width = Some(natural_width);
        self.natural_height = Some(natural_height);
        self.current_src = Some(current_src);
        self.complete = true;
    }

    /// Mark as error
    pub fn set_error(&mut self, error: String) {
        self.loading_state = ImageLoadingState::Error(error);
        self.complete = true;
    }

    /// Get aspect ratio hint for layout
    pub fn aspect_ratio(&self) -> Option<f64> {
        // Try natural dimensions first
        if let (Some(w), Some(h)) = (self.natural_width, self.natural_height) {
            if h > 0 {
                return Some(w as f64 / h as f64);
            }
        }

        // Fall back to explicit dimensions
        if let (Some(w), Some(h)) = (self.width, self.height) {
            if h > 0 {
                return Some(w as f64 / h as f64);
            }
        }

        None
    }
}

/// Loading attribute values
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ImageLoading {
    /// Load eagerly (default)
    #[default]
    Eager,

    /// Load lazily when near viewport
    Lazy,
}

impl ImageLoading {
    /// Parse from string
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "lazy" => ImageLoading::Lazy,
            _ => ImageLoading::Eager,
        }
    }
}

/// Decoding attribute values
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ImageDecoding {
    /// Decode synchronously
    Sync,

    /// Decode asynchronously
    Async,

    /// Let the browser decide (default)
    #[default]
    Auto,
}

impl ImageDecoding {
    /// Parse from string
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "sync" => ImageDecoding::Sync,
            "async" => ImageDecoding::Async,
            _ => ImageDecoding::Auto,
        }
    }
}

/// Crossorigin attribute values
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrossOrigin {
    /// Anonymous (no credentials)
    Anonymous,

    /// Use credentials
    UseCredentials,
}

impl CrossOrigin {
    /// Parse from string
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "use-credentials" => CrossOrigin::UseCredentials,
            _ => CrossOrigin::Anonymous,
        }
    }
}

/// Picture element with source children
#[derive(Debug, Clone)]
pub struct PictureElement {
    /// Node ID of the picture element
    pub node_id: NodeId,

    /// Source elements (in order)
    pub sources: Vec<PictureSource>,

    /// Fallback img element
    pub fallback: Option<ImageElement>,
}

impl PictureElement {
    /// Create a new picture element
    pub fn new(node_id: NodeId) -> Self {
        Self {
            node_id,
            sources: Vec::new(),
            fallback: None,
        }
    }

    /// Add a source element
    pub fn add_source(&mut self, source: PictureSource) {
        self.sources.push(source);
    }

    /// Set the fallback img element
    pub fn set_fallback(&mut self, img: ImageElement) {
        self.fallback = Some(img);
    }

    /// Select the best source for the current conditions
    pub fn select_source(
        &self,
        viewport_width: u32,
        _viewport_height: u32,
        device_pixel_ratio: f64,
    ) -> Option<String> {
        // Check each source in order
        for source in &self.sources {
            // TODO: Evaluate media query
            if source.media.is_some() {
                // For now, skip sources with media queries we can't evaluate
                continue;
            }

            // Check type if specified
            if let Some(media_type) = &source.media_type {
                // Check if we support this format
                if !is_supported_image_type(media_type) {
                    continue;
                }
            }

            // Select from srcset
            if let Some(srcset) = &source.srcset {
                use rustkit_image::loader::{parse_srcset, select_srcset_entry};

                let entries = parse_srcset(srcset);
                if let Some(entry) = select_srcset_entry(&entries, viewport_width, device_pixel_ratio)
                {
                    return Some(entry.url.clone());
                }
            }
        }

        // Fall back to img element
        self.fallback
            .as_ref()
            .and_then(|img| img.effective_src(viewport_width, device_pixel_ratio))
    }
}

/// A source element within a picture
#[derive(Debug, Clone)]
pub struct PictureSource {
    /// Node ID
    pub node_id: NodeId,

    /// srcset attribute
    pub srcset: Option<String>,

    /// sizes attribute
    pub sizes: Option<String>,

    /// media query
    pub media: Option<String>,

    /// MIME type
    pub media_type: Option<String>,
}

impl PictureSource {
    /// Create from attributes
    pub fn from_attributes(node_id: NodeId, attrs: &crate::AttrMap) -> Self {
        Self {
            node_id,
            srcset: attrs.get("srcset").cloned(),
            sizes: attrs.get("sizes").cloned(),
            media: attrs.get("media").cloned(),
            media_type: attrs.get("type").cloned(),
        }
    }
}

/// Check if an image MIME type is supported
fn is_supported_image_type(mime_type: &str) -> bool {
    matches!(
        mime_type.to_lowercase().as_str(),
        "image/png"
            | "image/jpeg"
            | "image/jpg"
            | "image/gif"
            | "image/webp"
            | "image/bmp"
            | "image/x-icon"
            | "image/vnd.microsoft.icon"
    )
}

/// Favicon link element
#[derive(Debug, Clone)]
pub struct FaviconLink {
    /// URL
    pub href: String,

    /// Rel value (icon, shortcut icon, apple-touch-icon, etc.)
    pub rel: String,

    /// Type (mime type)
    pub media_type: Option<String>,

    /// Sizes attribute (e.g., "32x32", "16x16 32x32")
    pub sizes: Option<String>,
}

impl FaviconLink {
    /// Create from link element attributes
    pub fn from_attributes(attrs: &crate::AttrMap) -> Option<Self> {
        let rel = attrs.get("rel")?;

        // Only process icon-related rels
        if !rel.split_whitespace().any(|r| {
            matches!(
                r.to_lowercase().as_str(),
                "icon" | "shortcut" | "apple-touch-icon" | "apple-touch-icon-precomposed"
            )
        }) {
            return None;
        }

        let href = attrs.get("href")?.clone();

        Some(Self {
            href,
            rel: rel.clone(),
            media_type: attrs.get("type").cloned(),
            sizes: attrs.get("sizes").cloned(),
        })
    }

    /// Parse sizes into dimensions
    pub fn parsed_sizes(&self) -> Vec<(u32, u32)> {
        let mut result = Vec::new();

        if let Some(sizes) = &self.sizes {
            for size in sizes.split_whitespace() {
                if size.to_lowercase() == "any" {
                    continue;
                }

                let size_lower = size.to_lowercase();
                let parts: Vec<&str> = size_lower.split('x').collect();
                if parts.len() == 2 {
                    if let (Ok(w), Ok(h)) = (parts[0].parse(), parts[1].parse()) {
                        result.push((w, h));
                    }
                }
            }
        }

        result
    }

    /// Get the best size for a target dimension
    pub fn best_size(&self, target: u32) -> Option<(u32, u32)> {
        let sizes = self.parsed_sizes();
        if sizes.is_empty() {
            return None;
        }

        // Find the smallest size that's >= target, or the largest available
        sizes
            .iter()
            .filter(|(w, _)| *w >= target)
            .min_by_key(|(w, _)| *w)
            .copied()
            .or_else(|| sizes.iter().max_by_key(|(w, _)| *w).copied())
    }
}

/// Manager for tracking image elements in a document
#[derive(Debug, Default)]
pub struct ImageElementManager {
    /// All img elements
    images: HashMap<NodeId, ImageElement>,

    /// All picture elements
    pictures: HashMap<NodeId, PictureElement>,

    /// Favicon links
    favicons: Vec<FaviconLink>,
}

impl ImageElementManager {
    /// Create a new manager
    pub fn new() -> Self {
        Self::default()
    }

    /// Register an img element
    pub fn register_image(&mut self, image: ImageElement) {
        self.images.insert(image.node_id, image);
    }

    /// Register a picture element
    pub fn register_picture(&mut self, picture: PictureElement) {
        self.pictures.insert(picture.node_id, picture);
    }

    /// Register a favicon link
    pub fn register_favicon(&mut self, favicon: FaviconLink) {
        self.favicons.push(favicon);
    }

    /// Get an image element by node ID
    pub fn get_image(&self, node_id: NodeId) -> Option<&ImageElement> {
        self.images.get(&node_id)
    }

    /// Get a mutable image element by node ID
    pub fn get_image_mut(&mut self, node_id: NodeId) -> Option<&mut ImageElement> {
        self.images.get_mut(&node_id)
    }

    /// Get all image elements
    pub fn images(&self) -> impl Iterator<Item = &ImageElement> {
        self.images.values()
    }

    /// Get images that need loading (pending state)
    pub fn pending_images(&self) -> impl Iterator<Item = &ImageElement> {
        self.images
            .values()
            .filter(|img| img.loading_state == ImageLoadingState::Pending)
    }

    /// Get lazy images
    pub fn lazy_images(&self) -> impl Iterator<Item = &ImageElement> {
        self.images.values().filter(|img| img.is_lazy())
    }

    /// Get all favicons
    pub fn favicons(&self) -> &[FaviconLink] {
        &self.favicons
    }

    /// Get the best favicon for a target size
    pub fn best_favicon(&self, target_size: u32) -> Option<&FaviconLink> {
        // Prefer ICO or PNG
        self.favicons
            .iter()
            .filter(|f| {
                f.media_type
                    .as_ref()
                    .map(|t| {
                        t.contains("icon")
                            || t.contains("png")
                            || f.href.ends_with(".ico")
                            || f.href.ends_with(".png")
                    })
                    .unwrap_or(true)
            })
            .min_by_key(|f| {
                f.best_size(target_size)
                    .map(|(w, _)| (w as i32 - target_size as i32).abs())
                    .unwrap_or(1000)
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_image_element_from_attributes() {
        let mut attrs = crate::AttrMap::new();
        attrs.insert("src".to_string(), "image.png".to_string());
        attrs.insert("alt".to_string(), "A picture".to_string());
        attrs.insert("width".to_string(), "100".to_string());
        attrs.insert("height".to_string(), "200".to_string());
        attrs.insert("loading".to_string(), "lazy".to_string());

        let img = ImageElement::from_attributes(NodeId::new(1), &attrs);

        assert_eq!(img.src, Some("image.png".to_string()));
        assert_eq!(img.alt, Some("A picture".to_string()));
        assert_eq!(img.width, Some(100));
        assert_eq!(img.height, Some(200));
        assert!(img.is_lazy());
        assert!(!img.complete);
    }

    #[test]
    fn test_image_aspect_ratio() {
        let mut img = ImageElement::from_attributes(NodeId::new(1), &crate::AttrMap::new());
        img.width = Some(400);
        img.height = Some(200);

        assert!((img.aspect_ratio().unwrap() - 2.0).abs() < 0.001);
    }

    #[test]
    fn test_favicon_sizes_parsing() {
        let favicon = FaviconLink {
            href: "/favicon.png".to_string(),
            rel: "icon".to_string(),
            media_type: Some("image/png".to_string()),
            sizes: Some("16x16 32x32 64x64".to_string()),
        };

        let sizes = favicon.parsed_sizes();
        assert_eq!(sizes.len(), 3);
        assert!(sizes.contains(&(16, 16)));
        assert!(sizes.contains(&(32, 32)));
        assert!(sizes.contains(&(64, 64)));
    }

    #[test]
    fn test_favicon_best_size() {
        let favicon = FaviconLink {
            href: "/favicon.png".to_string(),
            rel: "icon".to_string(),
            media_type: None,
            sizes: Some("16x16 32x32 64x64".to_string()),
        };

        // Exact match
        assert_eq!(favicon.best_size(32), Some((32, 32)));

        // Prefer larger
        assert_eq!(favicon.best_size(24), Some((32, 32)));

        // Largest available
        assert_eq!(favicon.best_size(128), Some((64, 64)));
    }

    #[test]
    fn test_image_loading_state() {
        let mut img = ImageElement::from_attributes(NodeId::new(1), &crate::AttrMap::new());
        assert_eq!(img.loading_state, ImageLoadingState::Pending);

        img.set_loading();
        assert_eq!(img.loading_state, ImageLoadingState::Loading);

        img.set_complete(800, 600, "http://example.com/img.png".to_string());
        assert_eq!(img.loading_state, ImageLoadingState::Complete);
        assert_eq!(img.natural_width, Some(800));
        assert_eq!(img.natural_height, Some(600));
        assert!(img.complete);
    }
}

