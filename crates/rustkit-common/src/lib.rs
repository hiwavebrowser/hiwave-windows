//! # RustKit Common
//!
//! Common utilities, error types, and logging configuration for the RustKit browser engine.
//!
//! ## Features
//!
//! - Unified error types with backtrace support
//! - Logging configuration and setup
//! - Retry and timeout utilities
//! - Result extension traits

use std::time::Duration;
use thiserror::Error;

pub mod logging;

pub mod retry;

/// `navigator.platform` for the OS the engine is built for, as Chromium
/// reports it ("MacIntel" on every Mac, Apple silicon included). One value
/// for the page (rustkit-bindings) and its workers (rustkit-worker).
pub const NAVIGATOR_PLATFORM: &str = if cfg!(target_os = "macos") {
    "MacIntel"
} else if cfg!(windows) {
    "Win32"
} else if cfg!(target_arch = "aarch64") {
    "Linux aarch64"
} else {
    "Linux x86_64"
};

pub use logging::{init_logging, LogConfig, LogFormat};
pub use retry::{retry_with_backoff, RetryConfig};

/// Unified error type for RustKit.
#[derive(Error, Debug)]
pub enum RustKitError {
    /// View-related errors.
    #[error("View error: {message}")]
    View {
        message: String,
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },

    /// Network-related errors.
    #[error("Network error: {message}")]
    Network {
        message: String,
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },

    /// DOM-related errors.
    #[error("DOM error: {message}")]
    Dom {
        message: String,
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },

    /// JavaScript errors.
    #[error("JS error: {message}")]
    JavaScript {
        message: String,
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },

    /// Rendering errors.
    #[error("Render error: {message}")]
    Render {
        message: String,
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },

    /// Layout errors.
    #[error("Layout error: {message}")]
    Layout {
        message: String,
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },

    /// Navigation errors.
    #[error("Navigation error: {message}")]
    Navigation {
        message: String,
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },

    /// Configuration errors.
    #[error("Config error: {message}")]
    Config {
        message: String,
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },

    /// I/O errors.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// Timeout errors.
    #[error("Operation timed out after {0:?}")]
    Timeout(Duration),

    /// Cancelled operation.
    #[error("Operation cancelled")]
    Cancelled,

    /// Resource not found.
    #[error("Resource not found: {0}")]
    NotFound(String),

    /// Invalid argument.
    #[error("Invalid argument: {0}")]
    InvalidArgument(String),

    /// Internal error (unexpected).
    #[error("Internal error: {message}")]
    Internal {
        message: String,
        backtrace: Option<backtrace::Backtrace>,
    },
}

impl RustKitError {
    /// Create a view error.
    pub fn view(message: impl Into<String>) -> Self {
        Self::View {
            message: message.into(),
            source: None,
        }
    }

    /// Create a view error with source.
    pub fn view_with_source<E: std::error::Error + Send + Sync + 'static>(
        message: impl Into<String>,
        source: E,
    ) -> Self {
        Self::View {
            message: message.into(),
            source: Some(Box::new(source)),
        }
    }

    /// Create a network error.
    pub fn network(message: impl Into<String>) -> Self {
        Self::Network {
            message: message.into(),
            source: None,
        }
    }

    /// Create a network error with source.
    pub fn network_with_source<E: std::error::Error + Send + Sync + 'static>(
        message: impl Into<String>,
        source: E,
    ) -> Self {
        Self::Network {
            message: message.into(),
            source: Some(Box::new(source)),
        }
    }

    /// Create a DOM error.
    pub fn dom(message: impl Into<String>) -> Self {
        Self::Dom {
            message: message.into(),
            source: None,
        }
    }

    /// Create a JavaScript error.
    pub fn javascript(message: impl Into<String>) -> Self {
        Self::JavaScript {
            message: message.into(),
            source: None,
        }
    }

    /// Create a render error.
    pub fn render(message: impl Into<String>) -> Self {
        Self::Render {
            message: message.into(),
            source: None,
        }
    }

    /// Create a layout error.
    pub fn layout(message: impl Into<String>) -> Self {
        Self::Layout {
            message: message.into(),
            source: None,
        }
    }

    /// Create a navigation error.
    pub fn navigation(message: impl Into<String>) -> Self {
        Self::Navigation {
            message: message.into(),
            source: None,
        }
    }

    /// Create an internal error with backtrace.
    pub fn internal(message: impl Into<String>) -> Self {
        Self::Internal {
            message: message.into(),
            backtrace: Some(backtrace::Backtrace::new()),
        }
    }

    /// Check if this error is retryable.
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            RustKitError::Network { .. } | RustKitError::Timeout(_) | RustKitError::Io(_)
        )
    }

    /// Get the error category for metrics.
    pub fn category(&self) -> &'static str {
        match self {
            RustKitError::View { .. } => "view",
            RustKitError::Network { .. } => "network",
            RustKitError::Dom { .. } => "dom",
            RustKitError::JavaScript { .. } => "javascript",
            RustKitError::Render { .. } => "render",
            RustKitError::Layout { .. } => "layout",
            RustKitError::Navigation { .. } => "navigation",
            RustKitError::Config { .. } => "config",
            RustKitError::Io(_) => "io",
            RustKitError::Timeout(_) => "timeout",
            RustKitError::Cancelled => "cancelled",
            RustKitError::NotFound(_) => "not_found",
            RustKitError::InvalidArgument(_) => "invalid_argument",
            RustKitError::Internal { .. } => "internal",
        }
    }
}

/// Result type alias for RustKit operations.
pub type Result<T> = std::result::Result<T, RustKitError>;

/// Extension trait for Result.
pub trait ResultExt<T> {
    /// Add context to an error.
    fn context(self, message: impl Into<String>) -> Result<T>;

    /// Convert to a different error type.
    fn map_err_to<E: Into<RustKitError>>(self, f: impl FnOnce() -> E) -> Result<T>;
}

impl<T, E: std::error::Error + Send + Sync + 'static> ResultExt<T> for std::result::Result<T, E> {
    fn context(self, message: impl Into<String>) -> Result<T> {
        self.map_err(|e| RustKitError::Internal {
            message: format!("{}: {}", message.into(), e),
            backtrace: Some(backtrace::Backtrace::new()),
        })
    }

    fn map_err_to<E2: Into<RustKitError>>(self, f: impl FnOnce() -> E2) -> Result<T> {
        self.map_err(|_| f().into())
    }
}

/// Extension trait for Option.
pub trait OptionExt<T> {
    /// Convert None to a NotFound error.
    fn ok_or_not_found(self, resource: impl Into<String>) -> Result<T>;
}

impl<T> OptionExt<T> for Option<T> {
    fn ok_or_not_found(self, resource: impl Into<String>) -> Result<T> {
        self.ok_or_else(|| RustKitError::NotFound(resource.into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_categories() {
        assert_eq!(RustKitError::view("test").category(), "view");
        assert_eq!(RustKitError::network("test").category(), "network");
        assert_eq!(
            RustKitError::Timeout(Duration::from_secs(1)).category(),
            "timeout"
        );
    }

    #[test]
    fn test_retryable() {
        assert!(RustKitError::network("test").is_retryable());
        assert!(RustKitError::Timeout(Duration::from_secs(1)).is_retryable());
        assert!(!RustKitError::view("test").is_retryable());
        assert!(!RustKitError::Cancelled.is_retryable());
    }

    #[test]
    fn test_option_ext() {
        let some: Option<i32> = Some(42);
        assert_eq!(some.ok_or_not_found("test").unwrap(), 42);

        let none: Option<i32> = None;
        assert!(matches!(
            none.ok_or_not_found("test"),
            Err(RustKitError::NotFound(_))
        ));
    }
}
