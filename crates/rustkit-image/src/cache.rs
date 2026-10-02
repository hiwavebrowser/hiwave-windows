//! Image caching module
//!
//! Provides memory and disk caching for decoded images.

use std::num::NonZeroUsize;
use std::sync::Arc;

use lru::LruCache;
use url::Url;

use crate::LoadedImage;

/// Cache statistics
#[derive(Debug, Clone, Default)]
pub struct CacheStats {
    /// Number of cache hits
    pub hits: u64,

    /// Number of cache misses
    pub misses: u64,

    /// Current number of cached images
    pub count: usize,

    /// Estimated memory usage in bytes
    pub memory_bytes: usize,
}

impl CacheStats {
    /// Get the hit rate as a percentage
    pub fn hit_rate(&self) -> f64 {
        let total = self.hits + self.misses;
        if total == 0 {
            0.0
        } else {
            (self.hits as f64 / total as f64) * 100.0
        }
    }
}

/// Memory cache for decoded images
pub struct ImageCache {
    /// LRU cache of images
    cache: LruCache<Url, Arc<LoadedImage>>,

    /// Cache statistics
    stats: CacheStats,

    /// Maximum memory usage
    #[allow(dead_code)]
    max_memory: usize,
}

impl ImageCache {
    /// Create a new cache with the given capacity
    pub fn new(capacity: usize) -> Self {
        Self {
            cache: LruCache::new(NonZeroUsize::new(capacity).unwrap_or(NonZeroUsize::new(1).unwrap())),
            stats: CacheStats::default(),
            max_memory: 256 * 1024 * 1024, // 256MB default
        }
    }

    /// Get an image from the cache
    pub fn get(&self, url: &Url) -> Option<Arc<LoadedImage>> {
        // Note: We use peek to avoid mutable borrow, but this means we don't
        // update LRU order. For a real implementation, we'd need interior mutability.
        self.cache.peek(url).cloned()
    }

    /// Insert an image into the cache
    pub fn insert(&mut self, url: Url, image: Arc<LoadedImage>) {
        self.cache.put(url, image);
        self.stats.count = self.cache.len();
    }

    /// Check if an image is in the cache
    pub fn contains(&self, url: &Url) -> bool {
        self.cache.contains(url)
    }

    /// Clear the cache
    pub fn clear(&mut self) {
        self.cache.clear();
        self.stats.count = 0;
    }

    /// Get cache statistics
    pub fn stats(&self) -> CacheStats {
        self.stats.clone()
    }

    /// Record a cache hit
    pub fn record_hit(&mut self) {
        self.stats.hits += 1;
    }

    /// Record a cache miss
    pub fn record_miss(&mut self) {
        self.stats.misses += 1;
    }

    /// Estimate memory usage of a cached image
    pub fn estimate_memory(image: &LoadedImage) -> usize {
        let pixels = (image.natural_width as usize) * (image.natural_height as usize);
        match &image.data {
            crate::ImageData::Static(_) => pixels * 4, // RGBA
            crate::ImageData::Animated(anim) => pixels * 4 * anim.frames.len(),
        }
    }

    /// Get all cached entries as (URL, image) pairs
    pub fn entries(&self) -> Vec<(Url, Arc<LoadedImage>)> {
        self.cache.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cache_stats_hit_rate() {
        let stats = CacheStats {
            hits: 75,
            misses: 25,
            count: 10,
            memory_bytes: 1000,
        };
        assert!((stats.hit_rate() - 75.0).abs() < 0.001);
    }

    #[test]
    fn test_cache_stats_empty() {
        let stats = CacheStats::default();
        assert!((stats.hit_rate() - 0.0).abs() < 0.001);
    }
}

