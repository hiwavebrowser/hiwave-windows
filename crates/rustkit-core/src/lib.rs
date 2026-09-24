//! # RustKit Core
//!
//! Core engine runtime for the RustKit browser engine.
//! Provides task scheduling, timers, navigation state management, and input events.
//!
//! ## Design Goals
//!
//! 1. **Deterministic event delivery**: Events fire in predictable order
//! 2. **Reliable lifecycle events**: start → commit → finish guaranteed
//! 3. **Timer accuracy**: setTimeout/setInterval equivalents
//! 4. **Structured logging**: Full tracing support
//! 5. **Platform-agnostic input**: Unified input event types

pub mod history;
pub mod input;
pub mod lifecycle;

pub use history::*;
pub use input::*;
pub use lifecycle::*;

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use thiserror::Error;
use tokio::sync::{mpsc, RwLock};
use tracing::{debug, error, info, trace};
use url::Url;

/// Unique identifier for a navigation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NavigationId(u64);

impl NavigationId {
    /// Create a new unique NavigationId.
    pub fn new() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(1);
        Self(COUNTER.fetch_add(1, Ordering::Relaxed))
    }

    /// Get the raw ID value.
    pub fn raw(&self) -> u64 {
        self.0
    }
}

impl Default for NavigationId {
    fn default() -> Self {
        Self::new()
    }
}

/// Unique identifier for a timer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TimerId(u64);

impl TimerId {
    /// Create a new unique TimerId.
    pub fn new() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(1);
        Self(COUNTER.fetch_add(1, Ordering::Relaxed))
    }

    /// Get the raw ID value.
    pub fn raw(&self) -> u64 {
        self.0
    }
}

impl Default for TimerId {
    fn default() -> Self {
        Self::new()
    }
}

/// Errors that can occur in the engine core.
#[derive(Error, Debug)]
pub enum CoreError {
    #[error("Invalid URL: {0}")]
    InvalidUrl(String),

    #[error("Navigation failed: {0}")]
    NavigationFailed(String),

    #[error("Timer error: {0}")]
    TimerError(String),

    #[error("Task queue error: {0}")]
    TaskQueueError(String),

    #[error("Engine not running")]
    NotRunning,
}

/// Navigation state for a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavigationState {
    /// No navigation in progress.
    Idle,
    /// Navigation started, waiting for response.
    Provisional,
    /// First bytes received, committed to navigate.
    Committed,
    /// Page fully loaded.
    Finished,
    /// Navigation failed.
    Failed,
}

/// Page load events emitted during navigation.
#[derive(Debug, Clone)]
pub enum LoadEvent {
    /// Navigation started.
    DidStartProvisionalLoad {
        navigation_id: NavigationId,
        url: Url,
    },
    /// First bytes received.
    DidCommitLoad {
        navigation_id: NavigationId,
        url: Url,
    },
    /// Page fully loaded.
    DidFinishLoad {
        navigation_id: NavigationId,
        url: Url,
    },
    /// Navigation failed.
    DidFailLoad {
        navigation_id: NavigationId,
        url: Url,
        error: String,
    },
    /// Progress update (0.0 - 1.0).
    DidUpdateProgress {
        navigation_id: NavigationId,
        progress: f64,
    },
}

/// Navigation request.
#[derive(Debug, Clone)]
pub struct NavigationRequest {
    /// Unique ID for this navigation.
    pub id: NavigationId,
    /// Target URL.
    pub url: Url,
    /// Whether this replaces the current history entry.
    pub replace_history: bool,
    /// Timestamp when navigation was requested.
    pub started_at: Instant,
}

impl NavigationRequest {
    /// Create a new navigation request.
    pub fn new(url: Url) -> Self {
        Self {
            id: NavigationId::new(),
            url,
            replace_history: false,
            started_at: Instant::now(),
        }
    }

    /// Create a navigation that replaces history.
    pub fn with_replace(mut self) -> Self {
        self.replace_history = true;
        self
    }
}

/// Navigation state machine for a single frame.
pub struct NavigationStateMachine {
    state: NavigationState,
    current_navigation: Option<NavigationRequest>,
    history: Vec<Url>,
    history_index: usize,
    event_sender: mpsc::UnboundedSender<LoadEvent>,
}

impl NavigationStateMachine {
    /// Create a new navigation state machine.
    pub fn new(event_sender: mpsc::UnboundedSender<LoadEvent>) -> Self {
        Self {
            state: NavigationState::Idle,
            current_navigation: None,
            history: Vec::new(),
            history_index: 0,
            event_sender,
        }
    }

    /// Start a new navigation.
    pub fn start_navigation(
        &mut self,
        request: NavigationRequest,
    ) -> Result<NavigationId, CoreError> {
        let nav_id = request.id;
        let url = request.url.clone();

        debug!(?nav_id, %url, "Starting navigation");

        self.state = NavigationState::Provisional;
        self.current_navigation = Some(request);

        let _ = self.event_sender.send(LoadEvent::DidStartProvisionalLoad {
            navigation_id: nav_id,
            url,
        });

        Ok(nav_id)
    }

    /// Mark navigation as committed (first bytes received).
    pub fn commit_navigation(&mut self) -> Result<(), CoreError> {
        if self.state != NavigationState::Provisional {
            return Err(CoreError::NavigationFailed(
                "Cannot commit: not in provisional state".into(),
            ));
        }

        let nav = self
            .current_navigation
            .as_ref()
            .ok_or_else(|| CoreError::NavigationFailed("No current navigation".into()))?;

        debug!(navigation_id = ?nav.id, "Navigation committed");

        self.state = NavigationState::Committed;

        let _ = self.event_sender.send(LoadEvent::DidCommitLoad {
            navigation_id: nav.id,
            url: nav.url.clone(),
        });

        Ok(())
    }

    /// Update load progress.
    pub fn update_progress(&mut self, progress: f64) -> Result<(), CoreError> {
        let nav = self
            .current_navigation
            .as_ref()
            .ok_or_else(|| CoreError::NavigationFailed("No current navigation".into()))?;

        trace!(navigation_id = ?nav.id, progress, "Progress update");

        let _ = self.event_sender.send(LoadEvent::DidUpdateProgress {
            navigation_id: nav.id,
            progress: progress.clamp(0.0, 1.0),
        });

        Ok(())
    }

    /// Mark navigation as finished.
    pub fn finish_navigation(&mut self) -> Result<(), CoreError> {
        if self.state != NavigationState::Committed {
            return Err(CoreError::NavigationFailed(
                "Cannot finish: not in committed state".into(),
            ));
        }

        let nav = self
            .current_navigation
            .take()
            .ok_or_else(|| CoreError::NavigationFailed("No current navigation".into()))?;

        info!(navigation_id = ?nav.id, elapsed_ms = ?nav.started_at.elapsed().as_millis(), "Navigation finished");

        // Update history
        if !nav.replace_history {
            // Truncate forward history if navigating from middle
            self.history.truncate(self.history_index + 1);
            self.history.push(nav.url.clone());
            self.history_index = self.history.len() - 1;
        } else if let Some(entry) = self.history.get_mut(self.history_index) {
            *entry = nav.url.clone();
        }

        self.state = NavigationState::Finished;

        let _ = self.event_sender.send(LoadEvent::DidFinishLoad {
            navigation_id: nav.id,
            url: nav.url,
        });

        // Return to idle after finish
        self.state = NavigationState::Idle;

        Ok(())
    }

    /// Mark navigation as failed.
    pub fn fail_navigation(&mut self, error: String) -> Result<(), CoreError> {
        let nav = self
            .current_navigation
            .take()
            .ok_or_else(|| CoreError::NavigationFailed("No current navigation".into()))?;

        error!(navigation_id = ?nav.id, %error, "Navigation failed");

        self.state = NavigationState::Failed;

        let _ = self.event_sender.send(LoadEvent::DidFailLoad {
            navigation_id: nav.id,
            url: nav.url,
            error,
        });

        // Return to idle after failure
        self.state = NavigationState::Idle;

        Ok(())
    }

    /// Get current state.
    pub fn state(&self) -> NavigationState {
        self.state
    }

    /// Check if navigation is in progress.
    pub fn is_loading(&self) -> bool {
        matches!(
            self.state,
            NavigationState::Provisional | NavigationState::Committed
        )
    }

    /// Get current URL.
    pub fn current_url(&self) -> Option<&Url> {
        self.history.get(self.history_index)
    }

    /// Check if can go back.
    pub fn can_go_back(&self) -> bool {
        self.history_index > 0
    }

    /// Check if can go forward.
    pub fn can_go_forward(&self) -> bool {
        self.history_index + 1 < self.history.len()
    }

    /// Go back in history.
    pub fn go_back(&mut self) -> Option<&Url> {
        if self.can_go_back() {
            self.history_index -= 1;
            self.history.get(self.history_index)
        } else {
            None
        }
    }

    /// Go forward in history.
    pub fn go_forward(&mut self) -> Option<&Url> {
        if self.can_go_forward() {
            self.history_index += 1;
            self.history.get(self.history_index)
        } else {
            None
        }
    }
}

/// Task priority levels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TaskPriority {
    /// Lowest priority (idle tasks).
    Idle = 0,
    /// Normal priority.
    Normal = 1,
    /// High priority (user input).
    High = 2,
    /// Highest priority (critical).
    Critical = 3,
}

/// A task to be executed.
pub type Task = Box<dyn FnOnce() + Send + 'static>;

/// Timer callback.
pub type TimerCallback = Box<dyn Fn() + Send + Sync + 'static>;

/// Timer configuration.
#[allow(dead_code)]
struct TimerEntry {
    id: TimerId,
    callback: Arc<TimerCallback>,
    interval: Option<Duration>,
    next_fire: Instant,
    cancelled: bool,
}

impl std::fmt::Debug for TimerEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TimerEntry")
            .field("id", &self.id)
            .field("interval", &self.interval)
            .field("next_fire", &self.next_fire)
            .field("cancelled", &self.cancelled)
            .finish_non_exhaustive()
    }
}

/// Task queue for scheduling work.
pub struct TaskQueue {
    sender: mpsc::UnboundedSender<(TaskPriority, Task)>,
    timers: Arc<RwLock<HashMap<TimerId, TimerEntry>>>,
}

impl TaskQueue {
    /// Create a new task queue.
    pub fn new() -> (Self, mpsc::UnboundedReceiver<(TaskPriority, Task)>) {
        let (sender, receiver) = mpsc::unbounded_channel();
        let queue = Self {
            sender,
            timers: Arc::new(RwLock::new(HashMap::new())),
        };
        (queue, receiver)
    }

    /// Post a task to the queue.
    pub fn post_task(&self, priority: TaskPriority, task: Task) -> Result<(), CoreError> {
        self.sender
            .send((priority, task))
            .map_err(|_| CoreError::TaskQueueError("Queue closed".into()))
    }

    /// Schedule a one-shot timer (setTimeout equivalent).
    pub fn set_timeout<F>(&self, callback: F, delay: Duration) -> TimerId
    where
        F: Fn() + Send + Sync + 'static,
    {
        let id = TimerId::new();
        let entry = TimerEntry {
            id,
            callback: Arc::new(Box::new(callback)),
            interval: None,
            next_fire: Instant::now() + delay,
            cancelled: false,
        };

        let timers = self.timers.clone();
        tokio::spawn(async move {
            timers.write().await.insert(id, entry);
        });

        trace!(?id, ?delay, "Timer scheduled");
        id
    }

    /// Schedule a repeating timer (setInterval equivalent).
    pub fn set_interval<F>(&self, callback: F, interval: Duration) -> TimerId
    where
        F: Fn() + Send + Sync + 'static,
    {
        let id = TimerId::new();
        let entry = TimerEntry {
            id,
            callback: Arc::new(Box::new(callback)),
            interval: Some(interval),
            next_fire: Instant::now() + interval,
            cancelled: false,
        };

        let timers = self.timers.clone();
        tokio::spawn(async move {
            timers.write().await.insert(id, entry);
        });

        trace!(?id, ?interval, "Interval scheduled");
        id
    }

    /// Cancel a timer.
    pub fn clear_timer(&self, id: TimerId) {
        let timers = self.timers.clone();
        tokio::spawn(async move {
            if let Some(entry) = timers.write().await.get_mut(&id) {
                entry.cancelled = true;
            }
        });

        trace!(?id, "Timer cancelled");
    }
}

impl Default for TaskQueue {
    fn default() -> Self {
        Self::new().0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_navigation_id_uniqueness() {
        let id1 = NavigationId::new();
        let id2 = NavigationId::new();
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_timer_id_uniqueness() {
        let id1 = TimerId::new();
        let id2 = TimerId::new();
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_navigation_state_transitions() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut nav = NavigationStateMachine::new(tx);

        // Start navigation
        let url = Url::parse("https://example.com").unwrap();
        let request = NavigationRequest::new(url.clone());
        let _nav_id = nav.start_navigation(request).unwrap();

        assert_eq!(nav.state(), NavigationState::Provisional);
        assert!(nav.is_loading());

        // Commit
        nav.commit_navigation().unwrap();
        assert_eq!(nav.state(), NavigationState::Committed);
        assert!(nav.is_loading());

        // Finish
        nav.finish_navigation().unwrap();
        assert_eq!(nav.state(), NavigationState::Idle);
        assert!(!nav.is_loading());

        // Check events
        let events: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        assert!(events.len() >= 3);
    }

    #[test]
    fn test_history_navigation() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut nav = NavigationStateMachine::new(tx);

        // Navigate to page 1
        let url1 = Url::parse("https://example.com/1").unwrap();
        nav.start_navigation(NavigationRequest::new(url1.clone()))
            .unwrap();
        nav.commit_navigation().unwrap();
        nav.finish_navigation().unwrap();

        // Navigate to page 2
        let url2 = Url::parse("https://example.com/2").unwrap();
        nav.start_navigation(NavigationRequest::new(url2.clone()))
            .unwrap();
        nav.commit_navigation().unwrap();
        nav.finish_navigation().unwrap();

        // Navigate to page 3
        let url3 = Url::parse("https://example.com/3").unwrap();
        nav.start_navigation(NavigationRequest::new(url3.clone()))
            .unwrap();
        nav.commit_navigation().unwrap();
        nav.finish_navigation().unwrap();

        // Current should be page 3
        assert_eq!(nav.current_url(), Some(&url3));
        assert!(nav.can_go_back());
        assert!(!nav.can_go_forward());

        // Go back to page 2
        assert_eq!(nav.go_back(), Some(&url2));
        assert!(nav.can_go_back());
        assert!(nav.can_go_forward());

        // Go back to page 1
        assert_eq!(nav.go_back(), Some(&url1));
        assert!(!nav.can_go_back());
        assert!(nav.can_go_forward());

        // Go forward to page 2
        assert_eq!(nav.go_forward(), Some(&url2));
    }

    #[test]
    fn test_task_priority_ordering() {
        assert!(TaskPriority::Critical > TaskPriority::High);
        assert!(TaskPriority::High > TaskPriority::Normal);
        assert!(TaskPriority::Normal > TaskPriority::Idle);
    }
}
