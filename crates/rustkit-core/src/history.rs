//! History API implementation for RustKit.
//!
//! Implements the HTML5 History API:
//! - pushState(state, title, url)
//! - replaceState(state, title, url)
//! - back(), forward(), go(delta)
//! - popstate event
//! - hashchange event
//!
//! Also handles scroll restoration and session history.

use std::collections::HashMap;
use std::time::Instant;
use url::Url;

/// A history entry representing a point in the session history.
#[derive(Debug, Clone)]
pub struct HistoryEntry {
    /// Unique ID for this entry.
    pub id: u64,

    /// The URL of this entry.
    pub url: Url,

    /// The state object (from pushState/replaceState).
    pub state: Option<HistoryState>,

    /// The document title at the time of navigation.
    pub title: String,

    /// The scroll position to restore.
    pub scroll_position: ScrollPosition,

    /// Timestamp when this entry was created.
    pub created_at: Instant,

    /// Navigation type that created this entry.
    pub navigation_type: NavigationType,
}

impl HistoryEntry {
    /// Create a new history entry.
    pub fn new(url: Url, title: String) -> Self {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Self {
            id: COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            url,
            state: None,
            title,
            scroll_position: ScrollPosition::default(),
            created_at: Instant::now(),
            navigation_type: NavigationType::Navigate,
        }
    }

    /// Create entry with state.
    pub fn with_state(mut self, state: HistoryState) -> Self {
        self.state = Some(state);
        self
    }

    /// Set scroll position.
    pub fn with_scroll_position(mut self, scroll_x: f32, scroll_y: f32) -> Self {
        self.scroll_position = ScrollPosition { x: scroll_x, y: scroll_y };
        self
    }

    /// Set navigation type.
    pub fn with_navigation_type(mut self, nav_type: NavigationType) -> Self {
        self.navigation_type = nav_type;
        self
    }
}

/// Scroll position for scroll restoration.
#[derive(Debug, Clone, Copy, Default)]
pub struct ScrollPosition {
    pub x: f32,
    pub y: f32,
}

/// State object for pushState/replaceState.
/// In a real implementation this would be a JSON-serializable value.
#[derive(Debug, Clone, Default)]
pub enum HistoryState {
    /// Null state.
    #[default]
    Null,
    /// Boolean value.
    Bool(bool),
    /// Number value.
    Number(f64),
    /// String value.
    String(String),
    /// Array of values.
    Array(Vec<HistoryState>),
    /// Object (key-value pairs).
    Object(HashMap<String, HistoryState>),
}

impl HistoryState {
    /// Create a null state.
    pub fn null() -> Self {
        Self::Null
    }

    /// Create a string state.
    pub fn string(s: impl Into<String>) -> Self {
        Self::String(s.into())
    }

    /// Create an object state.
    pub fn object() -> Self {
        Self::Object(HashMap::new())
    }

    /// Check if this is null.
    pub fn is_null(&self) -> bool {
        matches!(self, Self::Null)
    }
}

/// Navigation type indicating how a navigation occurred.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NavigationType {
    /// Normal navigation (link click, location assignment).
    #[default]
    Navigate,
    /// Page reload.
    Reload,
    /// Back/forward navigation.
    BackForward,
    /// History traversal via go().
    Traverse,
    /// Replace navigation (location.replace, replaceState).
    Replace,
    /// Push state (pushState).
    PushState,
}

impl NavigationType {
    /// Check if this creates a new history entry.
    pub fn creates_entry(self) -> bool {
        matches!(self, NavigationType::Navigate | NavigationType::PushState)
    }

    /// Check if this is a history navigation.
    pub fn is_history_navigation(self) -> bool {
        matches!(self, NavigationType::BackForward | NavigationType::Traverse)
    }
}

/// Scroll restoration mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScrollRestoration {
    /// Automatically restore scroll position.
    #[default]
    Auto,
    /// Don't restore scroll position.
    Manual,
}

/// History event types.
#[derive(Debug, Clone)]
pub enum HistoryEvent {
    /// Popstate event - fired when navigating through history.
    PopState {
        /// The state object of the new entry.
        state: Option<HistoryState>,
        /// The URL of the new entry.
        url: Url,
    },

    /// Hashchange event - fired when the fragment identifier changes.
    HashChange {
        /// The old URL.
        old_url: Url,
        /// The new URL.
        new_url: Url,
    },

    /// Navigation requested (for interception).
    NavigationRequested {
        /// The destination URL.
        url: Url,
        /// Whether this is a same-document navigation.
        same_document: bool,
    },
}

/// The session history for a browsing context.
#[derive(Debug)]
pub struct SessionHistory {
    /// History entries.
    entries: Vec<HistoryEntry>,

    /// Current entry index.
    current_index: usize,

    /// Scroll restoration mode.
    scroll_restoration: ScrollRestoration,

    /// Event sender.
    event_sender: Option<tokio::sync::mpsc::UnboundedSender<HistoryEvent>>,
}

impl SessionHistory {
    /// Create a new session history.
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            current_index: 0,
            scroll_restoration: ScrollRestoration::Auto,
            event_sender: None,
        }
    }

    /// Create with an event sender.
    pub fn with_event_sender(mut self, sender: tokio::sync::mpsc::UnboundedSender<HistoryEvent>) -> Self {
        self.event_sender = Some(sender);
        self
    }

    /// Get the number of entries.
    pub fn length(&self) -> usize {
        self.entries.len()
    }

    /// Get the current entry.
    pub fn current_entry(&self) -> Option<&HistoryEntry> {
        self.entries.get(self.current_index)
    }

    /// Get the current entry mutably.
    pub fn current_entry_mut(&mut self) -> Option<&mut HistoryEntry> {
        self.entries.get_mut(self.current_index)
    }

    /// Get the current URL.
    pub fn current_url(&self) -> Option<&Url> {
        self.current_entry().map(|e| &e.url)
    }

    /// Get the current state.
    pub fn current_state(&self) -> Option<&HistoryState> {
        self.current_entry().and_then(|e| e.state.as_ref())
    }

    /// Get scroll restoration mode.
    pub fn scroll_restoration(&self) -> ScrollRestoration {
        self.scroll_restoration
    }

    /// Set scroll restoration mode.
    pub fn set_scroll_restoration(&mut self, mode: ScrollRestoration) {
        self.scroll_restoration = mode;
    }

    /// Push a new entry (for regular navigation).
    pub fn push(&mut self, entry: HistoryEntry) {
        // Truncate forward history
        if self.current_index + 1 < self.entries.len() {
            self.entries.truncate(self.current_index + 1);
        }

        self.entries.push(entry);
        self.current_index = self.entries.len() - 1;
    }

    /// Push state (History API).
    pub fn push_state(&mut self, state: Option<HistoryState>, title: String, url: Option<Url>) -> Result<(), HistoryError> {
        let base_url = self.current_url()
            .ok_or(HistoryError::NoCurrentEntry)?;

        let new_url = match url {
            Some(u) => {
                // Validate same-origin
                if !is_same_origin(&u, base_url) {
                    return Err(HistoryError::SecurityError(
                        "Cannot pushState to different origin".into()
                    ));
                }
                u
            }
            None => base_url.clone(),
        };

        let mut entry = HistoryEntry::new(new_url, title)
            .with_navigation_type(NavigationType::PushState);

        if let Some(s) = state {
            entry = entry.with_state(s);
        }

        self.push(entry);
        Ok(())
    }

    /// Replace state (History API).
    pub fn replace_state(&mut self, state: Option<HistoryState>, title: String, url: Option<Url>) -> Result<(), HistoryError> {
        let base_url = self.current_url()
            .ok_or(HistoryError::NoCurrentEntry)?;

        let new_url = match url {
            Some(u) => {
                // Validate same-origin
                if !is_same_origin(&u, base_url) {
                    return Err(HistoryError::SecurityError(
                        "Cannot replaceState to different origin".into()
                    ));
                }
                u
            }
            None => base_url.clone(),
        };

        if let Some(entry) = self.current_entry_mut() {
            entry.url = new_url;
            entry.title = title;
            entry.state = state;
            entry.navigation_type = NavigationType::Replace;
        }

        Ok(())
    }

    /// Check if can go back.
    pub fn can_go_back(&self) -> bool {
        self.current_index > 0
    }

    /// Check if can go forward.
    pub fn can_go_forward(&self) -> bool {
        self.current_index + 1 < self.entries.len()
    }

    /// Go back one entry.
    pub fn back(&mut self) -> Option<&HistoryEntry> {
        self.go(-1)
    }

    /// Go forward one entry.
    pub fn forward(&mut self) -> Option<&HistoryEntry> {
        self.go(1)
    }

    /// Go by delta entries (negative = back, positive = forward).
    pub fn go(&mut self, delta: i32) -> Option<&HistoryEntry> {
        let new_index = if delta >= 0 {
            self.current_index.checked_add(delta as usize)?
        } else {
            self.current_index.checked_sub((-delta) as usize)?
        };

        if new_index >= self.entries.len() {
            return None;
        }

        let old_entry = self.current_entry().cloned();
        self.current_index = new_index;

        // Fire popstate event
        if let (Some(sender), Some(new_entry)) = (&self.event_sender, self.current_entry()) {
            let _ = sender.send(HistoryEvent::PopState {
                state: new_entry.state.clone(),
                url: new_entry.url.clone(),
            });

            // Check for hashchange
            if let Some(old) = old_entry {
                if old.url.host() == new_entry.url.host()
                    && old.url.path() == new_entry.url.path()
                    && old.url.query() == new_entry.url.query()
                    && old.url.fragment() != new_entry.url.fragment()
                {
                    let _ = sender.send(HistoryEvent::HashChange {
                        old_url: old.url,
                        new_url: new_entry.url.clone(),
                    });
                }
            }
        }

        self.current_entry()
    }

    /// Navigate to a new URL.
    pub fn navigate(&mut self, url: Url, nav_type: NavigationType, title: String) {
        let old_url = self.current_url().cloned();

        match nav_type {
            NavigationType::Navigate | NavigationType::PushState => {
                let entry = HistoryEntry::new(url.clone(), title)
                    .with_navigation_type(nav_type);
                self.push(entry);
            }
            NavigationType::Replace => {
                if let Some(entry) = self.current_entry_mut() {
                    entry.url = url.clone();
                    entry.title = title;
                    entry.navigation_type = nav_type;
                }
            }
            NavigationType::Reload => {
                // No history change on reload
            }
            NavigationType::BackForward | NavigationType::Traverse => {
                // Handled by go()
            }
        }

        // Check for hashchange
        if let (Some(sender), Some(old)) = (&self.event_sender, old_url) {
            if old.host() == url.host()
                && old.path() == url.path()
                && old.query() == url.query()
                && old.fragment() != url.fragment()
            {
                let _ = sender.send(HistoryEvent::HashChange {
                    old_url: old,
                    new_url: url,
                });
            }
        }
    }

    /// Save scroll position for current entry.
    pub fn save_scroll_position(&mut self, x: f32, y: f32) {
        if let Some(entry) = self.current_entry_mut() {
            entry.scroll_position = ScrollPosition { x, y };
        }
    }

    /// Get entries for debugging.
    pub fn entries(&self) -> &[HistoryEntry] {
        &self.entries
    }

    /// Get current index.
    pub fn current_index(&self) -> usize {
        self.current_index
    }
}

impl Default for SessionHistory {
    fn default() -> Self {
        Self::new()
    }
}

/// History API errors.
#[derive(Debug, Clone, thiserror::Error)]
pub enum HistoryError {
    #[error("Security error: {0}")]
    SecurityError(String),

    #[error("No current entry")]
    NoCurrentEntry,

    #[error("Invalid URL: {0}")]
    InvalidUrl(String),
}

/// Check if two URLs have the same origin.
fn is_same_origin(a: &Url, b: &Url) -> bool {
    a.scheme() == b.scheme() && a.host() == b.host() && a.port() == b.port()
}

/// Location object (window.location).
#[derive(Debug, Clone)]
pub struct Location {
    url: Url,
}

impl Location {
    /// Create a new location from a URL.
    pub fn new(url: Url) -> Self {
        Self { url }
    }

    /// Get the full URL as a string.
    pub fn href(&self) -> String {
        self.url.to_string()
    }

    /// Get the URL.
    pub fn url(&self) -> &Url {
        &self.url
    }

    /// Get the protocol (e.g., "https:").
    pub fn protocol(&self) -> String {
        format!("{}:", self.url.scheme())
    }

    /// Get the host (hostname:port).
    pub fn host(&self) -> String {
        match (self.url.host_str(), self.url.port()) {
            (Some(host), Some(port)) => format!("{}:{}", host, port),
            (Some(host), None) => host.to_string(),
            _ => String::new(),
        }
    }

    /// Get the hostname.
    pub fn hostname(&self) -> String {
        self.url.host_str().unwrap_or("").to_string()
    }

    /// Get the port.
    pub fn port(&self) -> String {
        self.url.port().map(|p| p.to_string()).unwrap_or_default()
    }

    /// Get the pathname.
    pub fn pathname(&self) -> String {
        self.url.path().to_string()
    }

    /// Get the search string (including ?).
    pub fn search(&self) -> String {
        self.url.query().map(|q| format!("?{}", q)).unwrap_or_default()
    }

    /// Get the hash (including #).
    pub fn hash(&self) -> String {
        self.url.fragment().map(|f| format!("#{}", f)).unwrap_or_default()
    }

    /// Get the origin.
    pub fn origin(&self) -> String {
        self.url.origin().ascii_serialization()
    }

    /// Set the URL (for navigation).
    pub fn set_url(&mut self, url: Url) {
        self.url = url;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_history_entry_creation() {
        let url = Url::parse("https://example.com/page").unwrap();
        let entry = HistoryEntry::new(url.clone(), "Test Page".into());

        assert_eq!(entry.url, url);
        assert_eq!(entry.title, "Test Page");
        assert!(entry.state.is_none());
    }

    #[test]
    fn test_history_push_state() {
        let mut history = SessionHistory::new();

        // Need an initial entry
        let url1 = Url::parse("https://example.com/").unwrap();
        history.push(HistoryEntry::new(url1, "Home".into()));

        // Push new state
        let url2 = Url::parse("https://example.com/page").unwrap();
        history.push_state(
            Some(HistoryState::string("test")),
            "Page".into(),
            Some(url2.clone()),
        ).unwrap();

        assert_eq!(history.length(), 2);
        assert_eq!(history.current_url(), Some(&url2));
    }

    #[test]
    fn test_history_replace_state() {
        let mut history = SessionHistory::new();

        let url1 = Url::parse("https://example.com/").unwrap();
        history.push(HistoryEntry::new(url1, "Home".into()));

        let url2 = Url::parse("https://example.com/page").unwrap();
        history.replace_state(
            Some(HistoryState::string("replaced")),
            "New Title".into(),
            Some(url2.clone()),
        ).unwrap();

        assert_eq!(history.length(), 1); // Still one entry
        assert_eq!(history.current_url(), Some(&url2));
        assert_eq!(history.current_entry().unwrap().title, "New Title");
    }

    #[test]
    fn test_history_back_forward() {
        let mut history = SessionHistory::new();

        let url1 = Url::parse("https://example.com/1").unwrap();
        let url2 = Url::parse("https://example.com/2").unwrap();
        let url3 = Url::parse("https://example.com/3").unwrap();

        history.push(HistoryEntry::new(url1.clone(), "Page 1".into()));
        history.push(HistoryEntry::new(url2.clone(), "Page 2".into()));
        history.push(HistoryEntry::new(url3.clone(), "Page 3".into()));

        assert_eq!(history.current_url(), Some(&url3));
        assert!(history.can_go_back());
        assert!(!history.can_go_forward());

        // Go back
        history.back();
        assert_eq!(history.current_url(), Some(&url2));
        assert!(history.can_go_forward());

        // Go back again
        history.back();
        assert_eq!(history.current_url(), Some(&url1));
        assert!(!history.can_go_back());

        // Go forward
        history.forward();
        assert_eq!(history.current_url(), Some(&url2));
    }

    #[test]
    fn test_history_go() {
        let mut history = SessionHistory::new();

        for i in 0..5 {
            let url = Url::parse(&format!("https://example.com/{}", i)).unwrap();
            history.push(HistoryEntry::new(url, format!("Page {}", i)));
        }

        assert_eq!(history.current_index(), 4);

        // Go back 2
        history.go(-2);
        assert_eq!(history.current_index(), 2);

        // Go forward 1
        history.go(1);
        assert_eq!(history.current_index(), 3);
    }

    #[test]
    fn test_cross_origin_security() {
        let mut history = SessionHistory::new();

        let url1 = Url::parse("https://example.com/").unwrap();
        history.push(HistoryEntry::new(url1, "Home".into()));

        // Try to push to different origin
        let url2 = Url::parse("https://evil.com/page").unwrap();
        let result = history.push_state(None, "Evil".into(), Some(url2));

        assert!(matches!(result, Err(HistoryError::SecurityError(_))));
    }

    #[test]
    fn test_location_properties() {
        let url = Url::parse("https://example.com:8080/path?query=1#hash").unwrap();
        let location = Location::new(url);

        assert_eq!(location.protocol(), "https:");
        assert_eq!(location.host(), "example.com:8080");
        assert_eq!(location.hostname(), "example.com");
        assert_eq!(location.port(), "8080");
        assert_eq!(location.pathname(), "/path");
        assert_eq!(location.search(), "?query=1");
        assert_eq!(location.hash(), "#hash");
        assert_eq!(location.origin(), "https://example.com:8080");
    }

    #[test]
    fn test_scroll_restoration() {
        let mut history = SessionHistory::new();

        let url = Url::parse("https://example.com/").unwrap();
        history.push(HistoryEntry::new(url, "Home".into()));

        history.save_scroll_position(100.0, 200.0);

        let entry = history.current_entry().unwrap();
        assert_eq!(entry.scroll_position.x, 100.0);
        assert_eq!(entry.scroll_position.y, 200.0);
    }

    #[test]
    fn test_navigation_type() {
        assert!(NavigationType::Navigate.creates_entry());
        assert!(NavigationType::PushState.creates_entry());
        assert!(!NavigationType::Replace.creates_entry());
        assert!(!NavigationType::Reload.creates_entry());

        assert!(NavigationType::BackForward.is_history_navigation());
        assert!(NavigationType::Traverse.is_history_navigation());
        assert!(!NavigationType::Navigate.is_history_navigation());
    }
}

