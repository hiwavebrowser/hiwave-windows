//! Filter list management for Flow Shield
//!
//! Downloads and caches EasyList, EasyPrivacy, and other filter lists.

use std::fs;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

/// Known filter list sources
pub struct FilterListSource {
    pub name: &'static str,
    pub url: &'static str,
    pub filename: &'static str,
}

/// Standard filter lists
pub const FILTER_LISTS: &[FilterListSource] = &[
    FilterListSource {
        name: "EasyList",
        url: "https://easylist.to/easylist/easylist.txt",
        filename: "easylist.txt",
    },
    FilterListSource {
        name: "EasyPrivacy",
        url: "https://easylist.to/easylist/easyprivacy.txt",
        filename: "easyprivacy.txt",
    },
];

/// How often to check for filter list updates (24 hours)
const UPDATE_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// Manages downloading and caching of filter lists
pub struct FilterListManager {
    cache_dir: PathBuf,
}

impl FilterListManager {
    /// Create a new filter list manager
    pub fn new() -> Option<Self> {
        let data_dir = dirs::data_local_dir()?;
        let cache_dir = data_dir.join("hiwave").join("filter-lists");

        // Create cache directory if it doesn't exist
        if let Err(e) = fs::create_dir_all(&cache_dir) {
            log::error!("Failed to create filter list cache directory: {}", e);
            return None;
        }

        Some(Self { cache_dir })
    }

    /// Get the cache path for a filter list
    fn cache_path(&self, source: &FilterListSource) -> PathBuf {
        self.cache_dir.join(source.filename)
    }

    /// Check if a cached filter list needs updating
    fn needs_update(&self, source: &FilterListSource) -> bool {
        let path = self.cache_path(source);

        if !path.exists() {
            return true;
        }

        // Check file modification time
        match fs::metadata(&path) {
            Ok(metadata) => match metadata.modified() {
                Ok(modified) => match SystemTime::now().duration_since(modified) {
                    Ok(age) => age > UPDATE_INTERVAL,
                    Err(_) => true,
                },
                Err(_) => true,
            },
            Err(_) => true,
        }
    }

    /// Download a filter list from the source
    fn download(&self, source: &FilterListSource) -> Result<String, String> {
        log::info!(
            "Downloading filter list: {} from {}",
            source.name,
            source.url
        );

        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| format!("Failed to create HTTP client: {}", e))?;

        let response = client
            .get(source.url)
            .send()
            .map_err(|e| format!("Failed to download {}: {}", source.name, e))?;

        if !response.status().is_success() {
            return Err(format!(
                "Failed to download {}: HTTP {}",
                source.name,
                response.status()
            ));
        }

        let content = response
            .text()
            .map_err(|e| format!("Failed to read response body: {}", e))?;

        // Cache the downloaded content
        let path = self.cache_path(source);
        if let Err(e) = fs::write(&path, &content) {
            log::warn!("Failed to cache filter list {}: {}", source.name, e);
        } else {
            log::info!(
                "Cached filter list {} ({} bytes)",
                source.name,
                content.len()
            );
        }

        Ok(content)
    }

    /// Load a filter list from cache
    fn load_from_cache(&self, source: &FilterListSource) -> Option<String> {
        let path = self.cache_path(source);
        match fs::read_to_string(&path) {
            Ok(content) => {
                log::info!(
                    "Loaded cached filter list: {} ({} bytes)",
                    source.name,
                    content.len()
                );
                Some(content)
            }
            Err(e) => {
                log::warn!("Failed to load cached filter list {}: {}", source.name, e);
                None
            }
        }
    }

    /// Get a filter list, downloading if necessary
    pub fn get_filter_list(&self, source: &FilterListSource) -> Option<String> {
        // Try to load from cache first if it's fresh
        if !self.needs_update(source) {
            if let Some(content) = self.load_from_cache(source) {
                return Some(content);
            }
        }

        // Download fresh copy
        match self.download(source) {
            Ok(content) => Some(content),
            Err(e) => {
                log::error!("{}", e);
                // Fall back to cached version if download fails
                self.load_from_cache(source)
            }
        }
    }

    /// Get all configured filter lists combined
    pub fn get_all_filter_lists(&self) -> String {
        let mut combined = String::new();

        for source in FILTER_LISTS {
            if let Some(content) = self.get_filter_list(source) {
                combined.push_str(&content);
                combined.push('\n');
            }
        }

        combined
    }

    /// Update all filter lists (download fresh copies)
    pub fn update_all(&self) -> usize {
        let mut updated = 0;

        for source in FILTER_LISTS {
            if self.needs_update(source) && self.download(source).is_ok() {
                updated += 1;
            }
        }

        updated
    }

    /// Get the total number of rules across all cached lists
    pub fn count_cached_rules(&self) -> usize {
        let mut count = 0;

        for source in FILTER_LISTS {
            if let Some(content) = self.load_from_cache(source) {
                // Count non-empty, non-comment lines
                count += content
                    .lines()
                    .filter(|line| {
                        let line = line.trim();
                        !line.is_empty() && !line.starts_with('!')
                    })
                    .count();
            }
        }

        count
    }
}

impl Default for FilterListManager {
    fn default() -> Self {
        Self::new().expect("Failed to initialize filter list manager")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_manager_creation() {
        let manager = FilterListManager::new();
        assert!(manager.is_some());
    }

    #[test]
    fn test_cache_path() {
        if let Some(manager) = FilterListManager::new() {
            let path = manager.cache_path(&FILTER_LISTS[0]);
            assert!(path.ends_with("easylist.txt"));
        }
    }
}
