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
    /// Master switch. False means nothing is stored or served at all.
    ///
    /// Private browsing and deterministic tests both need this and neither
    /// had it: MemoryCache was unconditional, so the only way to not have a
    /// cache was to not have a ResourceLoader.
    pub enabled: bool,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            max_size_bytes: 50 * 1024 * 1024, // 50 MB
            default_ttl: Duration::from_secs(300), // 5 minutes
            respect_cache_control: true,
            enabled: true,
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
    
    /// The TTL to apply when a response carries no usable `Cache-Control`.
    ///
    /// Exposed because the cache owns this policy and the loader has to ask
    /// for it. Before 2026-07-29 the loader instead reached for
    /// `LoaderConfig::default_timeout` — a NETWORK REQUEST TIMEOUT — so every
    /// header-less response was cached for 30s while `CacheConfig::default_ttl`
    /// (300s) was logged at startup and read by nothing.
    pub fn default_ttl(&self) -> Duration {
        self.config.default_ttl
    }

    /// Whether this cache is switched on at all.
    pub fn enabled(&self) -> bool {
        self.config.enabled
    }

    /// Whether `Cache-Control` should be honoured.
    ///
    /// Also previously unread: the flag existed, defaulted to true, and no
    /// code path consulted it, so setting it false changed nothing.
    pub fn respects_cache_control(&self) -> bool {
        self.config.respect_cache_control
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

/// Why a response may not be cached, or None if it may.
///
/// Eligibility is decided BEFORE any TTL question. A response can carry a
/// perfectly good `max-age` and still be ineligible — that is the distinction
/// the loader previously did not draw at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ineligible {
    /// The cache is switched off (private browsing, tests).
    Disabled,
    /// The request carried credentials. Caching it risks serving one user's
    /// authenticated response to another context.
    Credentialed,
    /// `Cache-Control: private` — not for a cache shared across contexts.
    MarkedPrivate,
    /// The response varies on request headers this cache does not key on.
    /// Refusing is the conservative choice: honouring `Vary` properly means
    /// folding the named headers into the key, which is a key-shape change.
    VariesOnRequestHeaders(String),
}

/// Decide whether a response may be stored at all.
pub fn cache_eligibility(
    enabled: bool,
    request_headers: &HeaderMap,
    response_headers: &HeaderMap,
) -> Option<Ineligible> {
    if !enabled {
        return Some(Ineligible::Disabled);
    }
    if request_headers.contains_key("authorization") {
        return Some(Ineligible::Credentialed);
    }
    if let Some(cc) = response_headers.get("cache-control").and_then(|v| v.to_str().ok()) {
        if cc.split(',').any(|d| d.trim().eq_ignore_ascii_case("private")) {
            return Some(Ineligible::MarkedPrivate);
        }
    }
    if let Some(vary) = response_headers.get("vary").and_then(|v| v.to_str().ok()) {
        let vary = vary.trim();
        if !vary.is_empty() {
            return Some(Ineligible::VariesOnRequestHeaders(vary.to_string()));
        }
    }
    None
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
    fn test_vary_makes_most_real_responses_ineligible() {
        // This is why the early-return bug mattered so much. Almost every real
        // server sends `Vary: Accept-Encoding`, so the ineligible branch is the
        // COMMON path, not an edge case. A field mismatch there hits nearly
        // every page load rather than a rare one.
        let mut resp = HeaderMap::new();
        resp.insert("vary", HeaderValue::from_static("Accept-Encoding"));
        assert!(
            matches!(
                cache_eligibility(true, &HeaderMap::new(), &resp),
                Some(Ineligible::VariesOnRequestHeaders(_))
            ),
            "Vary: Accept-Encoding must be ineligible — and it is the common case"
        );
    }

    #[test]
    fn test_credentialed_requests_are_not_cached() {
        let mut req = HeaderMap::new();
        req.insert("authorization", HeaderValue::from_static("Bearer secret"));
        assert_eq!(
            cache_eligibility(true, &req, &HeaderMap::new()),
            Some(Ineligible::Credentialed)
        );
    }

    #[test]
    fn test_cache_control_private_is_not_cached() {
        let mut resp = HeaderMap::new();
        resp.insert("cache-control", HeaderValue::from_static("private, max-age=600"));
        assert_eq!(
            cache_eligibility(true, &HeaderMap::new(), &resp),
            Some(Ineligible::MarkedPrivate)
        );
        // A perfectly good max-age does NOT rescue it — eligibility is decided
        // before freshness.
        assert!(parse_cache_control(&resp).is_some());
    }

    #[test]
    fn test_vary_is_refused_because_the_key_does_not_record_it() {
        let mut resp = HeaderMap::new();
        resp.insert("vary", HeaderValue::from_static("Accept-Language"));
        assert_eq!(
            cache_eligibility(true, &HeaderMap::new(), &resp),
            Some(Ineligible::VariesOnRequestHeaders("Accept-Language".into()))
        );
    }

    #[test]
    fn test_disabled_cache_stores_nothing() {
        assert_eq!(
            cache_eligibility(false, &HeaderMap::new(), &HeaderMap::new()),
            Some(Ineligible::Disabled)
        );
        let off = MemoryCache::with_config(CacheConfig { enabled: false, ..CacheConfig::default() });
        assert!(!off.enabled());
    }

    #[test]
    fn test_ordinary_response_is_eligible() {
        let mut resp = HeaderMap::new();
        resp.insert("cache-control", HeaderValue::from_static("max-age=600"));
        assert_eq!(cache_eligibility(true, &HeaderMap::new(), &resp), None);
    }

    #[test]
    fn test_default_ttl_is_the_caches_own_policy() {
        // REGRESSION: default_ttl had three references — declaration, default
        // value, and a startup log line — and was read by no code path. The
        // loader fell back to LoaderConfig::default_timeout (30s, a network
        // timeout) instead. The accessor is what makes the field real.
        let cache = MemoryCache::new();
        assert_eq!(cache.default_ttl(), Duration::from_secs(300));

        let custom = MemoryCache::with_config(CacheConfig {
            default_ttl: Duration::from_secs(42),
            ..CacheConfig::default()
        });
        assert_eq!(custom.default_ttl(), Duration::from_secs(42));
    }

    #[test]
    fn test_respect_cache_control_is_readable() {
        // REGRESSION: this flag had exactly two references, both in its own
        // declaration. Setting it false changed nothing anywhere.
        assert!(MemoryCache::new().respects_cache_control());

        let off = MemoryCache::with_config(CacheConfig {
            respect_cache_control: false,
            ..CacheConfig::default()
        });
        assert!(!off.respects_cache_control());
    }

    #[test]
    fn test_no_store_is_not_cacheable() {
        // Guards the behaviour that IS correct today, so a refactor of the
        // TTL path cannot quietly lose it: no-store/no-cache map to ZERO,
        // and the loader stores only when ttl > ZERO.
        let mut headers = HeaderMap::new();
        headers.insert("cache-control", HeaderValue::from_static("no-store"));
        assert_eq!(parse_cache_control(&headers), Some(Duration::ZERO));

        headers.insert("cache-control", HeaderValue::from_static("no-cache"));
        assert_eq!(parse_cache_control(&headers), Some(Duration::ZERO));
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

