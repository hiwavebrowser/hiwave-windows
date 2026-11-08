//! What the address bar's text means, as a URL.
//!
//! The same rule the WebView2 shell applies to the chrome's `navigate`
//! message (main.rs): a full `http(s)` URL is used as typed, a host-looking
//! word gets `https://`, and anything else is a web search. Kept as plain
//! functions, away from the engine, so it can be tested without a window.

/// The search engine's query URL prefix (the default in `state.rs`).
const SEARCH_URL: &str = "https://duckduckgo.com/?q=";

/// The URL to load for `input`, or `None` when there is nothing to load:
/// empty text, or an `about:` / `hiwave://` address (those name the app's own
/// pages, which the native shell does not load through the network).
pub fn resolve(input: &str) -> Option<String> {
    let text = input.trim();
    if text.is_empty() {
        return None;
    }
    if text.starts_with("about:") || text.starts_with("hiwave://") {
        return None;
    }
    if text.starts_with("http://") || text.starts_with("https://") {
        return Some(text.to_string());
    }
    // A host-looking word: has a dot and no spaces ("example.com", "a.b/c?d=e").
    if text.contains('.') && !text.contains(char::is_whitespace) {
        return Some(format!("https://{text}"));
    }
    let query: String = url::form_urlencoded::byte_serialize(text.as_bytes()).collect();
    Some(format!("{SEARCH_URL}{query}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_full_url_is_used_as_typed() {
        assert_eq!(resolve("https://example.com/a?b=c").as_deref(), Some("https://example.com/a?b=c"));
        assert_eq!(resolve("http://localhost:8080").as_deref(), Some("http://localhost:8080"));
    }

    #[test]
    fn a_host_gets_https() {
        assert_eq!(resolve("example.com").as_deref(), Some("https://example.com"));
        assert_eq!(resolve("example.com/a/b?x=1").as_deref(), Some("https://example.com/a/b?x=1"));
    }

    #[test]
    fn plain_words_become_a_search() {
        assert_eq!(resolve("abc").as_deref(), Some("https://duckduckgo.com/?q=abc"));
        assert_eq!(resolve("rust lifetimes").as_deref(), Some("https://duckduckgo.com/?q=rust+lifetimes"));
    }

    #[test]
    fn text_with_a_dot_and_a_space_is_a_search_not_a_host() {
        let url = resolve("what is example.com").unwrap();
        assert!(url.starts_with("https://duckduckgo.com/?q="), "{url}");
    }

    #[test]
    fn search_text_is_escaped() {
        assert_eq!(resolve("a&b=c").as_deref(), Some("https://duckduckgo.com/?q=a%26b%3Dc"));
    }

    #[test]
    fn surrounding_whitespace_is_ignored() {
        assert_eq!(resolve("  example.com \n").as_deref(), Some("https://example.com"));
    }

    #[test]
    fn empty_and_app_addresses_load_nothing() {
        assert_eq!(resolve(""), None);
        assert_eq!(resolve("   "), None);
        assert_eq!(resolve("about:blank"), None);
        assert_eq!(resolve("hiwave://about"), None);
    }
}
