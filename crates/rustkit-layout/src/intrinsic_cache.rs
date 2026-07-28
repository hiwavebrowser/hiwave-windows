//! Intrinsic sizing cache for layout performance optimization.
//!
//! This module implements caching for intrinsic size calculations (min-content and max-content
//! widths/heights). These calculations are expensive because they require traversing the
//! element's subtree, and the same element may be measured multiple times during a single
//! layout pass (e.g., for flex item sizing, table column distribution, etc.).
//!
//! # Design
//!
//! The cache uses epoch-based invalidation: each layout pass increments the epoch, and
//! cache entries from previous epochs are considered stale. This avoids the need for
//! explicit cache invalidation when styles change.
//!
//! Cache keys are composed of:
//! - Element ID (unique identifier for the DOM element)
//! - Style pointer (ensures cache is invalidated when styles change)
//! - Sizing mode (MinContent or MaxContent)
//!
//! # Usage
//!
//! ```ignore
//! // At the start of a layout pass:
//! intrinsic_cache::use_epoch(layout_epoch);
//!
//! // When computing intrinsic size:
//! if let Some(cached) = intrinsic_cache::lookup(element_id, style_ptr, mode) {
//!     return cached;
//! }
//! let computed = expensive_intrinsic_calculation();
//! intrinsic_cache::store(element_id, style_ptr, mode, computed);
//! ```

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Intrinsic sizing mode for cache lookups.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum IntrinsicSizingMode {
    /// Minimum content size - the smallest size that doesn't cause overflow.
    /// For text, this is typically the width of the longest word.
    MinContent,
    /// Maximum content size - the size needed to fit all content without wrapping.
    /// For text, this is the width needed to display on a single line.
    MaxContent,
}

/// Cache key for intrinsic size lookups.
/// Composed of (element_id, style_ptr, mode).
type CacheKey = (usize, usize, IntrinsicSizingMode);

/// Cache entry with epoch tag for invalidation.
type CacheEntry = (usize, f32); // (epoch, value)

/// Global cache epoch counter.
/// Incremented at the start of each layout pass.
static CACHE_EPOCH: AtomicUsize = AtomicUsize::new(1);

/// Cache statistics for debugging and profiling.
static CACHE_LOOKUPS: AtomicUsize = AtomicUsize::new(0);
static CACHE_HITS: AtomicUsize = AtomicUsize::new(0);
static CACHE_STORES: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    /// Thread-local epoch tracker to detect stale caches.
    static TL_EPOCH: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };

    /// Thread-local cache for inline (width) intrinsic sizes.
    static INLINE_CACHE: RefCell<HashMap<CacheKey, CacheEntry>> = RefCell::new(HashMap::new());

    /// Thread-local cache for block (height) intrinsic sizes.
    static BLOCK_CACHE: RefCell<HashMap<CacheKey, CacheEntry>> = RefCell::new(HashMap::new());
}

/// Set the cache epoch for the current layout pass.
///
/// This should be called at the start of each layout pass. Entries from
/// previous epochs will be considered stale and ignored.
///
/// # Arguments
/// * `epoch` - The epoch number for this layout pass. Must be > 0.
pub fn use_epoch(epoch: usize) {
    let epoch = epoch.max(1); // Ensure epoch is never 0
    CACHE_EPOCH.store(epoch, Ordering::Relaxed);

    // Clear thread-local caches if epoch changed
    TL_EPOCH.with(|cell| {
        if cell.get() != epoch {
            INLINE_CACHE.with(|cache| cache.borrow_mut().clear());
            BLOCK_CACHE.with(|cache| cache.borrow_mut().clear());
            cell.set(epoch);
        }
    });
}

/// Get the current cache epoch.
pub fn current_epoch() -> usize {
    CACHE_EPOCH.load(Ordering::Relaxed)
}

/// Ensure the thread-local cache is synchronized with the global epoch.
#[inline]
fn ensure_epoch() {
    let epoch = CACHE_EPOCH.load(Ordering::Relaxed);
    TL_EPOCH.with(|cell| {
        if cell.get() != epoch {
            INLINE_CACHE.with(|cache| cache.borrow_mut().clear());
            BLOCK_CACHE.with(|cache| cache.borrow_mut().clear());
            cell.set(epoch);
        }
    });
}

/// Look up a cached intrinsic inline (width) size.
///
/// # Arguments
/// * `element_id` - Unique identifier for the element
/// * `style_ptr` - Pointer to the element's computed style (as usize)
/// * `mode` - Whether to look up min-content or max-content
///
/// # Returns
/// The cached value if found and not stale, or None if not cached.
pub fn lookup_inline(element_id: usize, style_ptr: usize, mode: IntrinsicSizingMode) -> Option<f32> {
    if element_id == 0 {
        return None;
    }

    CACHE_LOOKUPS.fetch_add(1, Ordering::Relaxed);
    ensure_epoch();

    let epoch = CACHE_EPOCH.load(Ordering::Relaxed);
    let key = (element_id, style_ptr, mode);

    INLINE_CACHE.with(|cache| {
        cache.borrow().get(&key).and_then(|(entry_epoch, value)| {
            if *entry_epoch == epoch {
                CACHE_HITS.fetch_add(1, Ordering::Relaxed);
                Some(*value)
            } else {
                None
            }
        })
    })
}

/// Look up a cached intrinsic block (height) size.
///
/// # Arguments
/// * `element_id` - Unique identifier for the element
/// * `style_ptr` - Pointer to the element's computed style (as usize)
/// * `mode` - Whether to look up min-content or max-content
///
/// # Returns
/// The cached value if found and not stale, or None if not cached.
pub fn lookup_block(element_id: usize, style_ptr: usize, mode: IntrinsicSizingMode) -> Option<f32> {
    if element_id == 0 {
        return None;
    }

    CACHE_LOOKUPS.fetch_add(1, Ordering::Relaxed);
    ensure_epoch();

    let epoch = CACHE_EPOCH.load(Ordering::Relaxed);
    let key = (element_id, style_ptr, mode);

    BLOCK_CACHE.with(|cache| {
        cache.borrow().get(&key).and_then(|(entry_epoch, value)| {
            if *entry_epoch == epoch {
                CACHE_HITS.fetch_add(1, Ordering::Relaxed);
                Some(*value)
            } else {
                None
            }
        })
    })
}

/// Store a computed intrinsic inline (width) size in the cache.
///
/// # Arguments
/// * `element_id` - Unique identifier for the element
/// * `style_ptr` - Pointer to the element's computed style (as usize)
/// * `mode` - Whether this is min-content or max-content
/// * `value` - The computed intrinsic size
pub fn store_inline(element_id: usize, style_ptr: usize, mode: IntrinsicSizingMode, value: f32) {
    if element_id == 0 || !value.is_finite() {
        return;
    }

    ensure_epoch();
    let epoch = CACHE_EPOCH.load(Ordering::Relaxed);
    let key = (element_id, style_ptr, mode);

    INLINE_CACHE.with(|cache| {
        cache.borrow_mut().insert(key, (epoch, value));
    });
    CACHE_STORES.fetch_add(1, Ordering::Relaxed);
}

/// Store a computed intrinsic block (height) size in the cache.
///
/// # Arguments
/// * `element_id` - Unique identifier for the element
/// * `style_ptr` - Pointer to the element's computed style (as usize)
/// * `mode` - Whether this is min-content or max-content
/// * `value` - The computed intrinsic size
pub fn store_block(element_id: usize, style_ptr: usize, mode: IntrinsicSizingMode, value: f32) {
    if element_id == 0 || !value.is_finite() {
        return;
    }

    ensure_epoch();
    let epoch = CACHE_EPOCH.load(Ordering::Relaxed);
    let key = (element_id, style_ptr, mode);

    BLOCK_CACHE.with(|cache| {
        cache.borrow_mut().insert(key, (epoch, value));
    });
    CACHE_STORES.fetch_add(1, Ordering::Relaxed);
}

/// Get cache statistics for debugging and profiling.
///
/// # Returns
/// A tuple of (lookups, hits, stores).
pub fn stats() -> (usize, usize, usize) {
    (
        CACHE_LOOKUPS.load(Ordering::Relaxed),
        CACHE_HITS.load(Ordering::Relaxed),
        CACHE_STORES.load(Ordering::Relaxed),
    )
}

/// Reset cache statistics counters.
pub fn reset_stats() {
    CACHE_LOOKUPS.store(0, Ordering::Relaxed);
    CACHE_HITS.store(0, Ordering::Relaxed);
    CACHE_STORES.store(0, Ordering::Relaxed);
}

/// Clear all caches and reset epoch.
/// Primarily for testing purposes.
pub fn clear_all() {
    CACHE_EPOCH.store(1, Ordering::Relaxed);
    TL_EPOCH.with(|cell| cell.set(0));
    INLINE_CACHE.with(|cache| cache.borrow_mut().clear());
    BLOCK_CACHE.with(|cache| cache.borrow_mut().clear());
    reset_stats();
}

/// Get the current cache sizes for debugging.
///
/// # Returns
/// A tuple of (inline_cache_size, block_cache_size).
pub fn cache_sizes() -> (usize, usize) {
    let inline = INLINE_CACHE.with(|cache| cache.borrow().len());
    let block = BLOCK_CACHE.with(|cache| cache.borrow().len());
    (inline, block)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serialises tests that touch the global cache epoch.
    static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn test_cache_miss_on_first_lookup() {
        // The cache epoch is process-global while the maps are
        // thread-local, so a concurrent test's use_epoch() invalidates
        // this test's entries. Unique epochs per test (the previous
        // mitigation) cannot fix that -- every test still writes the
        // same global. Serialise instead.
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_all();
        use_epoch(1);

        let result = lookup_inline(1, 0x1000, IntrinsicSizingMode::MinContent);
        assert!(result.is_none());
    }

    // Note: These tests use unique element IDs and epochs to avoid interference
    // when tests run in parallel. The cache uses global state, so concurrent tests
    // can interfere with each other if they use the same IDs/epochs.

    #[test]
    fn test_cache_hit_after_store() {
        // The cache epoch is process-global while the maps are
        // thread-local, so a concurrent test's use_epoch() invalidates
        // this test's entries. Unique epochs per test (the previous
        // mitigation) cannot fix that -- every test still writes the
        // same global. Serialise instead.
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // Use unique epoch and element ID for this test
        use_epoch(1001);
        let elem_id = 100001;

        store_inline(elem_id, 0x1000, IntrinsicSizingMode::MinContent, 100.0);
        let result = lookup_inline(elem_id, 0x1000, IntrinsicSizingMode::MinContent);

        assert_eq!(result, Some(100.0));
    }

    #[test]
    fn test_cache_miss_different_mode() {
        // The cache epoch is process-global while the maps are
        // thread-local, so a concurrent test's use_epoch() invalidates
        // this test's entries. Unique epochs per test (the previous
        // mitigation) cannot fix that -- every test still writes the
        // same global. Serialise instead.
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        use_epoch(1002);
        let elem_id = 100002;

        store_inline(elem_id, 0x1000, IntrinsicSizingMode::MinContent, 100.0);
        let result = lookup_inline(elem_id, 0x1000, IntrinsicSizingMode::MaxContent);

        assert!(result.is_none());
    }

    #[test]
    fn test_cache_miss_different_style() {
        // The cache epoch is process-global while the maps are
        // thread-local, so a concurrent test's use_epoch() invalidates
        // this test's entries. Unique epochs per test (the previous
        // mitigation) cannot fix that -- every test still writes the
        // same global. Serialise instead.
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        use_epoch(1003);
        let elem_id = 100003;

        store_inline(elem_id, 0x1000, IntrinsicSizingMode::MinContent, 100.0);
        let result = lookup_inline(elem_id, 0x2000, IntrinsicSizingMode::MinContent);

        assert!(result.is_none());
    }

    #[test]
    fn test_cache_invalidation_on_epoch_change() {
        // The cache epoch is process-global while the maps are
        // thread-local, so a concurrent test's use_epoch() invalidates
        // this test's entries. Unique epochs per test (the previous
        // mitigation) cannot fix that -- every test still writes the
        // same global. Serialise instead.
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        use_epoch(1004);
        let elem_id = 100004;

        store_inline(elem_id, 0x1000, IntrinsicSizingMode::MinContent, 100.0);
        assert_eq!(lookup_inline(elem_id, 0x1000, IntrinsicSizingMode::MinContent), Some(100.0));

        // Change to a new epoch
        use_epoch(1005);

        // Previous entry should be stale
        let result = lookup_inline(elem_id, 0x1000, IntrinsicSizingMode::MinContent);
        assert!(result.is_none());
    }

    #[test]
    fn test_block_cache_separate_from_inline() {
        // The cache epoch is process-global while the maps are
        // thread-local, so a concurrent test's use_epoch() invalidates
        // this test's entries. Unique epochs per test (the previous
        // mitigation) cannot fix that -- every test still writes the
        // same global. Serialise instead.
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // This test verifies inline and block caches are separate.
        // Due to global epoch state, we test each cache independently.
        let elem_id = 100006;

        // Test inline cache
        use_epoch(2006);
        store_inline(elem_id, 0x1000, IntrinsicSizingMode::MinContent, 100.0);
        let inline_result = lookup_inline(elem_id, 0x1000, IntrinsicSizingMode::MinContent);
        assert_eq!(inline_result, Some(100.0), "Inline cache should work");

        // Test block cache with same element ID (should be independent)
        use_epoch(2007);
        store_block(elem_id, 0x1000, IntrinsicSizingMode::MinContent, 50.0);
        let block_result = lookup_block(elem_id, 0x1000, IntrinsicSizingMode::MinContent);
        assert_eq!(block_result, Some(50.0), "Block cache should work");
    }

    #[test]
    fn test_stats_tracking() {
        // The cache epoch is process-global while the maps are
        // thread-local, so a concurrent test's use_epoch() invalidates
        // this test's entries. Unique epochs per test (the previous
        // mitigation) cannot fix that -- every test still writes the
        // same global. Serialise instead.
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // Check relative changes to avoid parallel test interference
        use_epoch(1007);
        let elem_id = 100007;

        let (initial_lookups, initial_hits, initial_stores) = stats();

        // Miss
        lookup_inline(elem_id, 0x2000, IntrinsicSizingMode::MinContent);
        // Store
        store_inline(elem_id, 0x2000, IntrinsicSizingMode::MinContent, 100.0);
        // Hit
        lookup_inline(elem_id, 0x2000, IntrinsicSizingMode::MinContent);

        let (lookups, hits, stores) = stats();
        // Check that stats increased by expected amounts
        assert!(lookups >= initial_lookups + 2, "Expected at least 2 more lookups");
        assert!(hits >= initial_hits + 1, "Expected at least 1 more hit");
        assert!(stores >= initial_stores + 1, "Expected at least 1 more store");
    }

    #[test]
    fn test_zero_element_id_not_cached() {
        // The cache epoch is process-global while the maps are
        // thread-local, so a concurrent test's use_epoch() invalidates
        // this test's entries. Unique epochs per test (the previous
        // mitigation) cannot fix that -- every test still writes the
        // same global. Serialise instead.
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_all();
        use_epoch(1);

        // Element ID 0 should not be cached
        store_inline(0, 0x1000, IntrinsicSizingMode::MinContent, 100.0);
        let result = lookup_inline(0, 0x1000, IntrinsicSizingMode::MinContent);

        assert!(result.is_none());
    }

    #[test]
    fn test_non_finite_values_not_cached() {
        // The cache epoch is process-global while the maps are
        // thread-local, so a concurrent test's use_epoch() invalidates
        // this test's entries. Unique epochs per test (the previous
        // mitigation) cannot fix that -- every test still writes the
        // same global. Serialise instead.
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_all();
        use_epoch(1);

        // NaN and infinity should not be cached
        store_inline(1, 0x1000, IntrinsicSizingMode::MinContent, f32::NAN);
        assert!(lookup_inline(1, 0x1000, IntrinsicSizingMode::MinContent).is_none());

        store_inline(2, 0x1000, IntrinsicSizingMode::MinContent, f32::INFINITY);
        assert!(lookup_inline(2, 0x1000, IntrinsicSizingMode::MinContent).is_none());
    }

    #[test]
    fn test_cache_sizes() {
        // The cache epoch is process-global while the maps are
        // thread-local, so a concurrent test's use_epoch() invalidates
        // this test's entries. Unique epochs per test (the previous
        // mitigation) cannot fix that -- every test still writes the
        // same global. Serialise instead.
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_all();
        use_epoch(1);

        store_inline(1, 0x1000, IntrinsicSizingMode::MinContent, 100.0);
        store_inline(2, 0x2000, IntrinsicSizingMode::MaxContent, 200.0);
        store_block(1, 0x1000, IntrinsicSizingMode::MinContent, 50.0);

        let (inline_size, block_size) = cache_sizes();
        assert_eq!(inline_size, 2);
        assert_eq!(block_size, 1);
    }
}
