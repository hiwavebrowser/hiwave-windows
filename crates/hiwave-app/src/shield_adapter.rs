//! Adapter to connect hiwave-shield to rustkit-net's request interception.
//!
//! This module bridges the gap between the ad blocking engine (hiwave-shield)
//! and RustKit's network layer (rustkit-net), allowing sub-resource requests
//! to be filtered by the shield.
//!
//! Note: The main hiwave-shield uses Brave's adblock engine which is not Send+Sync.
//! For RustKit's async network layer, we use a simple domain-based filter that
//! mirrors the most common blocking rules. Full adblock filtering still happens
//! at the navigation level.

use hiwave_shield::ResourceType as ShieldResourceType;
use rustkit_net::{InterceptAction, InterceptHandler, Request};
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use tracing::{debug, trace};

/// Blocked domains for ad/tracker blocking.
/// These are the most common ad and tracking domains.
const BLOCKED_DOMAINS: &[&str] = &[
    "doubleclick.net",
    "googlesyndication.com",
    "googleadservices.com",
    "adtrafficquality.google",
    "ads.twitter.com",
    "facebook.com/tr",
    "connect.facebook.net",
    "tr.snapchat.com",
    "amazon-adsystem.com",
    "criteo.com",
    "adnxs.com",
    "adsrvr.org",
    "adroll.com",
    "taboola.com",
    "outbrain.com",
    "rubiconproject.com",
    "openx.net",
    "pubmatic.com",
    "scorecardresearch.com",
    "chartbeat.com",
    "segment.io",
    "segment.com",
    "mixpanel.com",
    "hotjar.com",
    "fullstory.com",
    "googletagmanager.com",
];

/// Thread-safe adapter that implements rustkit-net's InterceptHandler.
pub struct ShieldInterceptHandler {
    /// Whether blocking is enabled.
    enabled: Arc<AtomicBool>,
    /// Counter for blocked requests.
    blocked_count: Arc<AtomicU64>,
    /// Set of blocked domain patterns.
    blocked_domains: HashSet<String>,
    /// Callback to notify when a request is blocked (for UI updates).
    on_blocked: Option<Box<dyn Fn(&str) + Send + Sync>>,
}

impl ShieldInterceptHandler {
    /// Create a new shield intercept handler with default blocked domains.
    pub fn new() -> Self {
        let blocked_domains: HashSet<String> = BLOCKED_DOMAINS
            .iter()
            .map(|s| s.to_string())
            .collect();

        Self {
            enabled: Arc::new(AtomicBool::new(true)),
            blocked_count: Arc::new(AtomicU64::new(0)),
            blocked_domains,
            on_blocked: None,
        }
    }

    /// Create with a shared counter for tracking blocked requests.
    pub fn with_counter(blocked_count: Arc<AtomicU64>) -> Self {
        let blocked_domains: HashSet<String> = BLOCKED_DOMAINS
            .iter()
            .map(|s| s.to_string())
            .collect();

        Self {
            enabled: Arc::new(AtomicBool::new(true)),
            blocked_count,
            blocked_domains,
            on_blocked: None,
        }
    }

    /// Set whether blocking is enabled.
    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Relaxed);
    }

    /// Get the blocked request count.
    pub fn blocked_count(&self) -> u64 {
        self.blocked_count.load(Ordering::Relaxed)
    }

    /// Get the counter Arc for sharing.
    pub fn counter(&self) -> Arc<AtomicU64> {
        Arc::clone(&self.blocked_count)
    }

    /// Set a callback to be called when a request is blocked.
    pub fn with_on_blocked<F>(mut self, callback: F) -> Self
    where
        F: Fn(&str) + Send + Sync + 'static,
    {
        self.on_blocked = Some(Box::new(callback));
        self
    }

    /// Check if a host should be blocked.
    fn should_block_host(&self, host: &str) -> bool {
        let host_lower = host.to_lowercase();
        for domain in &self.blocked_domains {
            if host_lower == *domain || host_lower.ends_with(&format!(".{}", domain)) {
                return true;
            }
        }
        false
    }

    /// Convert HTTP method and URL to a shield ResourceType.
    fn guess_resource_type(request: &Request) -> ShieldResourceType {
        let url_str = request.url.as_str().to_lowercase();
        let path = request.url.path().to_lowercase();

        // Check file extension
        if path.ends_with(".js") || path.ends_with(".mjs") {
            return ShieldResourceType::Script;
        }
        if path.ends_with(".css") {
            return ShieldResourceType::Stylesheet;
        }
        if path.ends_with(".png")
            || path.ends_with(".jpg")
            || path.ends_with(".jpeg")
            || path.ends_with(".gif")
            || path.ends_with(".webp")
            || path.ends_with(".svg")
            || path.ends_with(".ico")
        {
            return ShieldResourceType::Image;
        }
        if path.ends_with(".woff")
            || path.ends_with(".woff2")
            || path.ends_with(".ttf")
            || path.ends_with(".otf")
            || path.ends_with(".eot")
        {
            return ShieldResourceType::Font;
        }
        if path.ends_with(".mp4")
            || path.ends_with(".webm")
            || path.ends_with(".mp3")
            || path.ends_with(".ogg")
        {
            return ShieldResourceType::Media;
        }

        // Check common ad/tracker patterns in URL
        if url_str.contains("/pixel")
            || url_str.contains("/beacon")
            || url_str.contains("/track")
            || url_str.contains("/analytics")
            || url_str.contains("/collect")
        {
            return ShieldResourceType::Xhr;
        }

        // Check Accept header for hints
        if let Some(accept) = request.headers.get("accept") {
            if let Ok(accept_str) = accept.to_str() {
                if accept_str.contains("application/json")
                    || accept_str.contains("application/xml")
                {
                    return ShieldResourceType::Xhr;
                }
                if accept_str.contains("image/") {
                    return ShieldResourceType::Image;
                }
                if accept_str.contains("text/css") {
                    return ShieldResourceType::Stylesheet;
                }
            }
        }

        ShieldResourceType::Other
    }
}

impl InterceptHandler for ShieldInterceptHandler {
    fn intercept(&self, request: &Request) -> InterceptAction {
        trace!(url = %request.url, "Shield checking request");

        if !self.enabled.load(Ordering::Relaxed) {
            return InterceptAction::Allow;
        }

        // Check if the host is in our blocked list
        let should_block = request.url.host_str()
            .map(|host| self.should_block_host(host))
            .unwrap_or(false);

        if should_block {
            // Increment counter
            self.blocked_count.fetch_add(1, Ordering::Relaxed);

            debug!(
                url = %request.url,
                "Shield blocked sub-resource request"
            );

            // Notify callback if set
            if let Some(ref callback) = self.on_blocked {
                callback(request.url.as_str());
            }

            InterceptAction::Block
        } else {
            InterceptAction::Allow
        }
    }
}

impl Default for ShieldInterceptHandler {
    fn default() -> Self {
        Self::new()
    }
}

/// Create a request interceptor with the default shield handler.
pub fn create_shield_interceptor() -> rustkit_net::RequestInterceptor {
    let handler = ShieldInterceptHandler::new();
    let mut interceptor = rustkit_net::RequestInterceptor::new();
    interceptor.add_handler(Arc::new(handler));
    interceptor
}

/// Create a request interceptor with a shared counter.
pub fn create_shield_interceptor_with_counter(
    counter: Arc<AtomicU64>,
) -> rustkit_net::RequestInterceptor {
    let handler = ShieldInterceptHandler::with_counter(counter);
    let mut interceptor = rustkit_net::RequestInterceptor::new();
    interceptor.add_handler(Arc::new(handler));
    interceptor
}

/// Create a request interceptor with a blocked callback.
pub fn create_shield_interceptor_with_callback<F>(
    on_blocked: F,
) -> rustkit_net::RequestInterceptor
where
    F: Fn(&str) + Send + Sync + 'static,
{
    let handler = ShieldInterceptHandler::new().with_on_blocked(on_blocked);
    let mut interceptor = rustkit_net::RequestInterceptor::new();
    interceptor.add_handler(Arc::new(handler));
    interceptor
}

#[cfg(test)]
mod tests {
    use super::*;
    use wry::http::Method;
    use rustkit_net::RequestId;
    use url::Url;

    fn test_request(url_str: &str) -> Request {
        Request {
            id: RequestId::new(),
            url: Url::parse(url_str).unwrap(),
            method: Method::GET,
            headers: Default::default(),
            body: None,
            timeout: None,
            credentials: Default::default(),
            referrer: None,
            referrer_policy: Default::default(),
        }
    }

    #[test]
    fn test_resource_type_detection() {
        let js_req = test_request("https://example.com/script.js");
        assert!(matches!(
            ShieldInterceptHandler::guess_resource_type(&js_req),
            ShieldResourceType::Script
        ));

        let css_req = test_request("https://example.com/style.css");
        assert!(matches!(
            ShieldInterceptHandler::guess_resource_type(&css_req),
            ShieldResourceType::Stylesheet
        ));

        let img_req = test_request("https://example.com/image.png");
        assert!(matches!(
            ShieldInterceptHandler::guess_resource_type(&img_req),
            ShieldResourceType::Image
        ));

        let track_req = test_request("https://example.com/pixel/track");
        assert!(matches!(
            ShieldInterceptHandler::guess_resource_type(&track_req),
            ShieldResourceType::Xhr
        ));
    }
}
