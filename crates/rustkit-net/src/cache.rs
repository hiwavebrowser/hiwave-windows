//! HTTP response caching for RustKit.
//!
//! Provides a memory cache for HTTP responses with LRU eviction.

use bytes::Bytes;
use http::{HeaderMap, StatusCode};
use std::collections::HashMap;
use std::sync::RwLock;
use std::time::{Duration, Instant};
use tracing::{debug, info, trace};
use url::Url;

/// Cache configuration.
#[derive(Debug, Clone)]
pub struct CacheConfig {
    /// Maximum cache size in bytes.
    pub max_size_bytes: usize,
    /// Default TTL for cached entries.
    pub default_ttl: Duration,
    /// Whether to respect Cache-Control headers.
    pub respect_cache_control: bool,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            max_size_bytes: 50 * 1024 * 1024, // 50 MB
            default_ttl: Duration::from_secs(300), // 5 minutes
            respect_cache_control: true,
        }
    }
}

/// A cached HTTP response.
#[derive(Debug, Clone)]
pub struct CachedResponse {
    /// Response status code.
    pub status: StatusCode,
    /// Response headers.
    pub headers: HeaderMap,
    /// Response body.
    pub body: Bytes,
    /// When this entry was cached.
    pub cached_at: Instant,
    /// When this entry expires.
    pub expires_at: Instant,
    /// Size of this entry in bytes.
    pub size: usize,
}

impl CachedResponse {
    /// Check if this entry is expired.
    pub fn is_expired(&self) -> bool {
        Instant::now() >= self.expires_at
    }
    
    /// Get the remaining TTL.
    pub fn remaining_ttl(&self) -> Duration {
        self.expires_at.saturating_duration_since(Instant::now())
    }
}

/// Cache key for HTTP requests.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct CacheKey {
    /// Request URL.
    pub url: String,
    /// Request method (only GET is cacheable).
    pub method: String,
}

impl CacheKey {
    pub fn new(url: &Url) -> Self {
        Self {
            url: url.to_string(),
            method: "GET".to_string(),
        }
    }
}

/// Memory cache entry with LRU tracking.
struct CacheEntry {
    response: CachedResponse,
    last_accessed: Instant,
}

/// Memory cache for HTTP responses.
pub struct MemoryCache {
    entries: RwLock<HashMap<CacheKey, CacheEntry>>,
    config: CacheConfig,
    current_size: RwLock<usize>,
    stats: RwLock<CacheStats>,
}

/// Cache statistics.
#[derive(Debug, Clone, Default)]
pub struct CacheStats {
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
    pub insertions: u64,
    pub total_bytes_served: u64,
}

impl CacheStats {
    pub fn hit_rate(&self) -> f64 {
        let total = self.hits + self.misses;
        if total == 0 {
            0.0
        } else {
            self.hits as f64 / total as f64
        }
    }
}

impl MemoryCache {
    /// Create a new memory cache with default configuration.
    pub fn new() -> Self {
        Self::with_config(CacheConfig::default())
    }
    
    /// Create a new memory cache with custom configuration.
    pub fn with_config(config: CacheConfig) -> Self {
        info!(
            max_size_mb = config.max_size_bytes / 1024 / 1024,
            default_ttl_secs = config.default_ttl.as_secs(),
            "Memory cache initialized"
        );
        
        Self {
            entries: RwLock::new(HashMap::new()),
            config,
            current_size: RwLock::new(0),
            stats: RwLock::new(CacheStats::default()),
        }
    }
    
    /// Get a cached response.
    pub fn get(&self, key: &CacheKey) -> Option<CachedResponse> {
        let mut entries = self.entries.write().ok()?;
        
        if let Some(entry) = entries.get_mut(key) {
            // Check if expired
            if entry.response.is_expired() {
                trace!(url = %key.url, "Cache entry expired");
                let size = entry.response.size;
                entries.remove(key);
                
                if let Ok(mut current) = self.current_size.write() {
                    *current = current.saturating_sub(size);
                }
                
                if let Ok(mut stats) = self.stats.write() {
                    stats.misses += 1;
                }
                
                return None;
            }
            
            // Update LRU
            entry.last_accessed = Instant::now();
            
            // Update stats
            if let Ok(mut stats) = self.stats.write() {
                stats.hits += 1;
                stats.total_bytes_served += entry.response.body.len() as u64;
            }
            
            debug!(
                url = %key.url,
                size = entry.response.size,
                remaining_ttl_secs = entry.response.remaining_ttl().as_secs(),
                "Cache hit"
            );
            
            return Some(entry.response.clone());
        }
        
        if let Ok(mut stats) = self.stats.write() {
            stats.misses += 1;
        }
        
        trace!(url = %key.url, "Cache miss");
        None
    }
    
    /// Store a response in the cache.
    pub fn put(&self, key: CacheKey, response: CachedResponse) {
        // Check if response is too large
        if response.size > self.config.max_size_bytes / 2 {
            debug!(
                url = %key.url,
                size = response.size,
                "Response too large to cache"
            );
            return;
        }
        
        // Evict if needed
        self.evict_if_needed(response.size);
        
        // Insert
        if let Ok(mut entries) = self.entries.write() {
            // Remove old entry if exists
            if let Some(old) = entries.get(&key) {
                if let Ok(mut current) = self.current_size.write() {
                    *current = current.saturating_sub(old.response.size);
                }
            }
            
            let size = response.size;
            entries.insert(
                key.clone(),
                CacheEntry {
                    response,
                    last_accessed: Instant::now(),
                },
            );
            
            if let Ok(mut current) = self.current_size.write() {
                *current += size;
            }
            
            if let Ok(mut stats) = self.stats.write() {
                stats.insertions += 1;
            }
            
            debug!(url = %key.url, size, "Cached response");
        }
    }
    
    /// Evict entries to make room for a new entry.
    fn evict_if_needed(&self, needed: usize) {
        let mut entries = match self.entries.write() {
            Ok(e) => e,
            Err(_) => return,
        };
        
        let current = self.current_size.read().map(|s| *s).unwrap_or(0);
        
        if current + needed <= self.config.max_size_bytes {
            return;
        }
        
        // Collect entries sorted by last access time
        let mut by_access: Vec<_> = entries
            .iter()
            .map(|(k, v)| (k.clone(), v.last_accessed, v.response.size))
            .collect();
        
        by_access.sort_by_key(|(_k, accessed, _size)| *accessed);
        
        let mut freed = 0;
        let mut to_remove = Vec::new();
        
        for (key, _accessed, size) in by_access {
            if current + needed - freed <= self.config.max_size_bytes {
                break;
            }
            
            to_remove.push(key);
            freed += size;
        }
        
        for key in &to_remove {
            entries.remove(key);
        }
        
        if let Ok(mut current_size) = self.current_size.write() {
            *current_size = current_size.saturating_sub(freed);
        }
        
        if let Ok(mut stats) = self.stats.write() {
            stats.evictions += to_remove.len() as u64;
        }
        
        if !to_remove.is_empty() {
            debug!(evicted = to_remove.len(), freed_bytes = freed, "Evicted cache entries");
        }
    }
    
    /// Remove a specific entry from the cache.
    pub fn remove(&self, key: &CacheKey) -> bool {
        if let Ok(mut entries) = self.entries.write() {
            if let Some(entry) = entries.remove(key) {
                if let Ok(mut current) = self.current_size.write() {
                    *current = current.saturating_sub(entry.response.size);
                }
                return true;
            }
        }
        false
    }
    
    /// Clear all cached entries.
    pub fn clear(&self) {
        if let Ok(mut entries) = self.entries.write() {
            entries.clear();
        }
        if let Ok(mut current) = self.current_size.write() {
            *current = 0;
        }
        info!("Cache cleared");
    }
    
    /// Get current cache size in bytes.
    pub fn size(&self) -> usize {
        self.current_size.read().map(|s| *s).unwrap_or(0)
    }
    
    /// Get number of cached entries.
    pub fn len(&self) -> usize {
        self.entries.read().map(|e| e.len()).unwrap_or(0)
    }
    
    /// Check if cache is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    
    /// Get cache statistics.
    pub fn stats(&self) -> CacheStats {
        self.stats.read().map(|s| s.clone()).unwrap_or_default()
    }
    
    /// Prune expired entries.
    pub fn prune_expired(&self) -> usize {
        let mut entries = match self.entries.write() {
            Ok(e) => e,
            Err(_) => return 0,
        };
        
        let now = Instant::now();
        let expired: Vec<_> = entries
            .iter()
            .filter(|(_k, v)| v.response.expires_at <= now)
            .map(|(k, v)| (k.clone(), v.response.size))
            .collect();
        
        let count = expired.len();
        let mut freed = 0;
        
        for (key, size) in expired {
            entries.remove(&key);
            freed += size;
        }
        
        if let Ok(mut current) = self.current_size.write() {
            *current = current.saturating_sub(freed);
        }
        
        if count > 0 {
            debug!(pruned = count, freed_bytes = freed, "Pruned expired entries");
        }
        
        count
    }
}

impl Default for MemoryCache {
    fn default() -> Self {
        Self::new()
    }
}

/// Parse Cache-Control header to determine TTL.
pub fn parse_cache_control(headers: &HeaderMap) -> Option<Duration> {
    let cc = headers.get("cache-control")?.to_str().ok()?;
    
    // Check for no-store or no-cache
    if cc.contains("no-store") || cc.contains("no-cache") {
        return Some(Duration::ZERO);
    }
    
    // Look for max-age
    for directive in cc.split(',') {
        let directive = directive.trim();
        if directive.starts_with("max-age=") {
            if let Ok(secs) = directive[8..].parse::<u64>() {
                return Some(Duration::from_secs(secs));
            }
        }
    }
    
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::HeaderValue;
    
    #[test]
    fn test_cache_put_get() {
        let cache = MemoryCache::new();
        let key = CacheKey::new(&Url::parse("https://example.com/test.css").unwrap());
        
        let response = CachedResponse {
            status: StatusCode::OK,
            headers: HeaderMap::new(),
            body: Bytes::from("body content"),
            cached_at: Instant::now(),
            expires_at: Instant::now() + Duration::from_secs(300),
            size: 12,
        };
        
        cache.put(key.clone(), response);
        
        let cached = cache.get(&key);
        assert!(cached.is_some());
        assert_eq!(cached.unwrap().body, Bytes::from("body content"));
    }
    
    #[test]
    fn test_cache_expiration() {
        let cache = MemoryCache::new();
        let key = CacheKey::new(&Url::parse("https://example.com/expired.css").unwrap());
        
        let response = CachedResponse {
            status: StatusCode::OK,
            headers: HeaderMap::new(),
            body: Bytes::from("expired"),
            cached_at: Instant::now() - Duration::from_secs(10),
            expires_at: Instant::now() - Duration::from_secs(5), // Already expired
            size: 7,
        };
        
        cache.put(key.clone(), response);
        
        // Should not return expired entry
        let cached = cache.get(&key);
        assert!(cached.is_none());
    }
    
    #[test]
    fn test_parse_cache_control() {
        let mut headers = HeaderMap::new();
        headers.insert("cache-control", HeaderValue::from_static("max-age=3600"));
        
        let ttl = parse_cache_control(&headers);
        assert_eq!(ttl, Some(Duration::from_secs(3600)));
        
        let mut headers = HeaderMap::new();
        headers.insert("cache-control", HeaderValue::from_static("no-store"));
        
        let ttl = parse_cache_control(&headers);
        assert_eq!(ttl, Some(Duration::ZERO));
    }
    
    #[test]
    fn test_cache_stats() {
        let cache = MemoryCache::new();
        let key = CacheKey::new(&Url::parse("https://example.com/stats.css").unwrap());
        
        // Miss
        let _ = cache.get(&key);
        
        // Put
        let response = CachedResponse {
            status: StatusCode::OK,
            headers: HeaderMap::new(),
            body: Bytes::from("stats"),
            cached_at: Instant::now(),
            expires_at: Instant::now() + Duration::from_secs(300),
            size: 5,
        };
        cache.put(key.clone(), response);
        
        // Hit
        let _ = cache.get(&key);
        
        let stats = cache.stats();
        assert_eq!(stats.misses, 1);
        assert_eq!(stats.insertions, 1);
        assert_eq!(stats.hits, 1);
    }
}

