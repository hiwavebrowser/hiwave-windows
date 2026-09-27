//! Request interception for URL filtering and modification.

use crate::{Request, Url};
use std::sync::Arc;
use tracing::{debug, trace};

/// Action to take for an intercepted request.
#[derive(Debug, Clone)]
pub enum InterceptAction {
    /// Allow the request to proceed.
    Allow,
    /// Block the request.
    Block,
    /// Redirect to a different URL.
    Redirect(Url),
    /// Modify the request.
    Modify(Box<Request>),
}

/// Handler for intercepting requests.
pub trait InterceptHandler: Send + Sync {
    /// Called for each request. Return the action to take.
    fn intercept(&self, request: &Request) -> InterceptAction;
}

/// URL pattern for matching.
#[derive(Debug, Clone)]
pub struct UrlPattern {
    /// Pattern type.
    pub pattern_type: PatternType,
    /// Pattern string.
    pub pattern: String,
}

/// Type of URL pattern.
#[derive(Debug, Clone, Copy)]
pub enum PatternType {
    /// Exact URL match.
    Exact,
    /// Prefix match.
    Prefix,
    /// Suffix match (e.g., domain).
    Suffix,
    /// Contains substring.
    Contains,
    /// Regular expression.
    Regex,
}

impl UrlPattern {
    /// Create an exact match pattern.
    pub fn exact(url: &str) -> Self {
        Self {
            pattern_type: PatternType::Exact,
            pattern: url.to_string(),
        }
    }

    /// Create a prefix match pattern.
    pub fn prefix(prefix: &str) -> Self {
        Self {
            pattern_type: PatternType::Prefix,
            pattern: prefix.to_string(),
        }
    }

    /// Create a suffix match pattern (e.g., for domains).
    pub fn suffix(suffix: &str) -> Self {
        Self {
            pattern_type: PatternType::Suffix,
            pattern: suffix.to_string(),
        }
    }

    /// Create a contains pattern.
    pub fn contains(substring: &str) -> Self {
        Self {
            pattern_type: PatternType::Contains,
            pattern: substring.to_string(),
        }
    }

    /// Check if a URL matches this pattern.
    pub fn matches(&self, url: &Url) -> bool {
        let url_str = url.as_str();
        match self.pattern_type {
            PatternType::Exact => url_str == self.pattern,
            PatternType::Prefix => url_str.starts_with(&self.pattern),
            PatternType::Suffix => url_str.ends_with(&self.pattern),
            PatternType::Contains => url_str.contains(&self.pattern),
            PatternType::Regex => {
                // Simplified: would use regex crate in production
                url_str.contains(&self.pattern)
            }
        }
    }
}

/// Rule for intercepting requests.
#[derive(Debug, Clone)]
pub struct InterceptRule {
    /// Pattern to match.
    pub pattern: UrlPattern,
    /// Action to take.
    pub action: RuleAction,
    /// Priority (higher = first).
    pub priority: i32,
}

/// Action for a rule.
#[derive(Debug, Clone)]
pub enum RuleAction {
    /// Allow the request.
    Allow,
    /// Block the request.
    Block,
    /// Redirect to URL.
    Redirect(String),
}

/// Request interceptor with configurable rules.
pub struct RequestInterceptor {
    rules: Vec<InterceptRule>,
    default_action: InterceptAction,
    handlers: Vec<Arc<dyn InterceptHandler>>,
}

impl RequestInterceptor {
    /// Create a new interceptor.
    pub fn new() -> Self {
        Self {
            rules: Vec::new(),
            default_action: InterceptAction::Allow,
            handlers: Vec::new(),
        }
    }

    /// Set the default action for non-matching requests.
    pub fn set_default_action(&mut self, action: InterceptAction) {
        self.default_action = action;
    }

    /// Add a rule.
    pub fn add_rule(&mut self, rule: InterceptRule) {
        self.rules.push(rule);
        // Sort by priority (descending)
        self.rules.sort_by(|a, b| b.priority.cmp(&a.priority));
    }

    /// Remove rules matching a pattern.
    pub fn remove_rules(&mut self, pattern: &str) {
        self.rules.retain(|r| r.pattern.pattern != pattern);
    }

    /// Add a custom handler.
    pub fn add_handler(&mut self, handler: Arc<dyn InterceptHandler>) {
        self.handlers.push(handler);
    }

    /// Block URLs matching a pattern.
    pub fn block(&mut self, pattern: UrlPattern) {
        self.add_rule(InterceptRule {
            pattern,
            action: RuleAction::Block,
            priority: 0,
        });
    }

    /// Allow URLs matching a pattern (override blocks).
    pub fn allow(&mut self, pattern: UrlPattern) {
        self.add_rule(InterceptRule {
            pattern,
            action: RuleAction::Allow,
            priority: 10, // Higher priority than blocks
        });
    }

    /// Redirect URLs matching a pattern.
    pub fn redirect(&mut self, pattern: UrlPattern, target: &str) {
        self.add_rule(InterceptRule {
            pattern,
            action: RuleAction::Redirect(target.to_string()),
            priority: 5,
        });
    }

    /// Intercept a request.
    pub async fn intercept(&self, request: &Request) -> InterceptAction {
        trace!(url = %request.url, "Intercepting request");

        // Check custom handlers first
        for handler in &self.handlers {
            let action = handler.intercept(request);
            match action {
                InterceptAction::Allow => continue,
                other => {
                    debug!(url = %request.url, action = ?other, "Handler intercepted");
                    return other;
                }
            }
        }

        // Check rules
        for rule in &self.rules {
            if rule.pattern.matches(&request.url) {
                let action = match &rule.action {
                    RuleAction::Allow => InterceptAction::Allow,
                    RuleAction::Block => InterceptAction::Block,
                    RuleAction::Redirect(target) => {
                        if let Ok(url) = Url::parse(target) {
                            InterceptAction::Redirect(url)
                        } else {
                            InterceptAction::Block
                        }
                    }
                };
                debug!(url = %request.url, pattern = %rule.pattern.pattern, action = ?action, "Rule matched");
                return action;
            }
        }

        // Default action
        self.default_action.clone()
    }
}

impl Default for RequestInterceptor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::Method;

    fn test_request(url: &str) -> Request {
        Request {
            id: crate::RequestId::new(),
            url: Url::parse(url).unwrap(),
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
    fn test_url_pattern_exact() {
        let pattern = UrlPattern::exact("https://example.com/");
        let url = Url::parse("https://example.com/").unwrap();
        assert!(pattern.matches(&url));

        let url2 = Url::parse("https://example.com/page").unwrap();
        assert!(!pattern.matches(&url2));
    }

    #[test]
    fn test_url_pattern_prefix() {
        let pattern = UrlPattern::prefix("https://example.com/");
        let url = Url::parse("https://example.com/page").unwrap();
        assert!(pattern.matches(&url));
    }

    #[test]
    fn test_url_pattern_suffix() {
        let pattern = UrlPattern::suffix(".com/");
        let url = Url::parse("https://example.com/").unwrap();
        assert!(pattern.matches(&url));
    }

    #[test]
    fn test_url_pattern_contains() {
        let pattern = UrlPattern::contains("tracking");
        let url = Url::parse("https://example.com/tracking/pixel").unwrap();
        assert!(pattern.matches(&url));
    }

    #[tokio::test]
    async fn test_interceptor_block() {
        let mut interceptor = RequestInterceptor::new();
        interceptor.block(UrlPattern::contains("ads"));

        let request = test_request("https://example.com/ads/banner");
        let action = interceptor.intercept(&request).await;
        assert!(matches!(action, InterceptAction::Block));
    }

    #[tokio::test]
    async fn test_interceptor_allow_override() {
        let mut interceptor = RequestInterceptor::new();
        interceptor.block(UrlPattern::prefix("https://example.com/"));
        interceptor.allow(UrlPattern::exact("https://example.com/allowed"));

        let request = test_request("https://example.com/allowed");
        let action = interceptor.intercept(&request).await;
        assert!(matches!(action, InterceptAction::Allow));

        let request2 = test_request("https://example.com/other");
        let action2 = interceptor.intercept(&request2).await;
        assert!(matches!(action2, InterceptAction::Block));
    }

    #[tokio::test]
    async fn test_interceptor_redirect() {
        let mut interceptor = RequestInterceptor::new();
        interceptor.redirect(UrlPattern::exact("https://old.com/"), "https://new.com/");

        let request = test_request("https://old.com/");
        let action = interceptor.intercept(&request).await;
        match action {
            InterceptAction::Redirect(url) => {
                assert_eq!(url.as_str(), "https://new.com/");
            }
            _ => panic!("Expected redirect"),
        }
    }
}
