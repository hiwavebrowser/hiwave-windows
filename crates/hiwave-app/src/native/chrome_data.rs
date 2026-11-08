//! Data the chrome's panels show (history, downloads), as scripts for the chrome page.
//!
//! The WebView2 shell builds these inline in `main.rs` (`sync_history_to_chrome`,
//! `sync_downloads_to_chrome`); they are `cfg(not(native-win32))`, so the native
//! shell has its own. Kept as plain functions so what reaches the chrome can be
//! tested without a window.

use crate::state::AppState;

/// A call into the chrome page: `hiwaveChrome.<method>(<payload>)`, guarded for a
/// page that has not set `hiwaveChrome` up yet. The payload is serialised JSON, so
/// no text in it can become script.
pub fn chrome_call(method: &str, payload: &serde_json::Value) -> String {
    format!("if(window.hiwaveChrome) {{ hiwaveChrome.{method}({payload}); }}")
}

/// The history panel's data: every visit, newest first.
pub fn history_script(state: &AppState) -> String {
    let history = serde_json::to_value(state.visit_history_snapshot()).unwrap_or(serde_json::json!([]));
    chrome_call("updateHistory", &history)
}

/// Whether the user's blocklist covers `url`'s domain (a subdomain of a blocked domain
/// counts). One predicate for every way a page can be reached: the address bar, a link
/// click, a form, a script, and the content-thread page.
pub fn url_is_blocked(state: &AppState, url: &str) -> bool {
    state.user_blocklist.should_block(url)
}

/// The blocklist panel's data: the user's blocked domains, sorted so the list does not
/// reorder itself between calls (the set has no order).
pub fn blocklist_script(state: &AppState) -> String {
    let mut domains: Vec<&String> = state.user_blocklist.domains.iter().collect();
    domains.sort();
    chrome_call("updateBlocklist", &serde_json::json!(domains))
}

/// The downloads panel's data and the count of active downloads.
pub fn downloads_script(state: &AppState) -> String {
    let payload = serde_json::json!({
        "downloads": state.downloads_snapshot(),
        "active_count": state.active_download_count(),
    });
    chrome_call("updateDownloads", &payload)
}

/// Whether a page belongs in the history. The start page, blank tabs and the
/// browser's own addresses do not; neither do `javascript:` and `data:` URLs.
pub fn should_record_history_url(url: &str) -> bool {
    let lower = url.trim().to_ascii_lowercase();
    if lower.is_empty() {
        return false;
    }
    !(lower.starts_with("about:")
        || lower.starts_with("data:")
        || lower.starts_with("javascript:")
        || lower.starts_with("hiwave:"))
}

/// Decides which snapshots of the active tab are a new visit for the history.
///
/// Three things are not visits: switching tabs (the other tab's page is not a new page),
/// a page that is still loading (its title may still be the previous page's), and the same
/// address seen again. Pollux's R1 on #126 found the first two.
#[derive(Debug, Default)]
pub struct VisitTracker {
    last_url: Option<String>,
    last_tab: Option<u64>,
}

impl VisitTracker {
    /// Whether this snapshot (`url`, `title` of the active tab `tab`) should be written to
    /// the history now.
    pub fn should_record(&mut self, tab: Option<u64>, url: &str, title: &str, loading: bool) -> bool {
        if tab != self.last_tab {
            // A different tab is showing: adopt its address, record nothing.
            self.last_tab = tab;
            self.last_url = Some(url.to_string());
            return false;
        }
        if loading || url.is_empty() || title.is_empty() || title == "New Tab" {
            return false;
        }
        if !should_record_history_url(url) || self.last_url.as_deref() == Some(url) {
            return false;
        }
        self.last_url = Some(url.to_string());
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_chrome_call_is_guarded_and_carries_json() {
        let call = chrome_call("updateHistory", &json!([{"url": "https://a.test/"}]));
        assert_eq!(
            call,
            r#"if(window.hiwaveChrome) { hiwaveChrome.updateHistory([{"url":"https://a.test/"}]); }"#
        );
    }

    #[test]
    fn text_in_the_payload_cannot_become_script() {
        // A page title is page-controlled. It goes through JSON, so a quote or a
        // closing paren is escaped inside a string, not a way out of the call.
        let title = r#"x"); window.ipc.postMessage("evil"); //"#;
        let call = chrome_call("updateHistory", &json!([{ "title": title }]));
        // The whole argument still parses as ONE JSON value, so the hostile text is
        // data inside a string and not a second statement.
        let open = call.find("updateHistory(").unwrap() + "updateHistory(".len();
        let close = call.rfind("); }").unwrap();
        let back: serde_json::Value = serde_json::from_str(&call[open..close]).expect("one JSON value");
        assert_eq!(back[0]["title"], json!(title));
    }

    #[test]
    fn a_navigation_is_a_visit_once() {
        let mut v = VisitTracker::default();
        assert!(!v.should_record(Some(1), "hiwave://about", "About HiWave", false), "the first snapshot only adopts");
        assert!(v.should_record(Some(1), "https://a.test/", "A", false));
        assert!(!v.should_record(Some(1), "https://a.test/", "A", false), "the same page again is not a new visit");
        assert!(v.should_record(Some(1), "https://b.test/", "B", false));
    }

    #[test]
    fn switching_tabs_does_not_record_visits() {
        let mut v = VisitTracker::default();
        v.should_record(Some(1), "https://a.test/", "A", false);
        // Tab 2 shows B, tab 1 shows A again: no navigation happened in either.
        assert!(!v.should_record(Some(2), "https://b.test/", "B", false));
        assert!(!v.should_record(Some(1), "https://a.test/", "A", false));
        assert!(!v.should_record(Some(2), "https://b.test/", "B", false));
        // A real navigation in the tab that is showing still counts.
        assert!(v.should_record(Some(2), "https://c.test/", "C", false));
    }

    #[test]
    fn a_page_that_is_still_loading_is_not_recorded_with_the_old_title() {
        let mut v = VisitTracker::default();
        v.should_record(Some(1), "https://a.test/", "A", false);
        // The new address is in but the title is still the previous page's.
        assert!(!v.should_record(Some(1), "https://b.test/", "A", true));
        // Once it has loaded, the pair is right and it is recorded.
        assert!(v.should_record(Some(1), "https://b.test/", "B", false));
    }

    #[test]
    fn a_failed_navigation_that_falls_back_is_not_a_visit() {
        let mut v = VisitTracker::default();
        v.should_record(Some(1), "https://a.test/", "A", false);
        assert!(!v.should_record(Some(1), "https://dead.test/", "A", true));
        // The address goes back to A when the load fails: A was already recorded.
        assert!(!v.should_record(Some(1), "https://a.test/", "A", false));
    }

    #[test]
    fn a_blocked_domain_and_its_subdomains_are_blocked_everything_else_is_not() {
        let mut s = temp_state("blocked");
        s.block_domain("blocked.test".to_string()).unwrap();
        assert!(url_is_blocked(&s, "http://blocked.test/"));
        assert!(url_is_blocked(&s, "https://sub.blocked.test/page?x=1"));
        assert!(!url_is_blocked(&s, "https://notblocked.test/"));
        assert!(!url_is_blocked(&s, "https://example.com/blocked.test"));
        assert!(!url_is_blocked(&s, "not a url"));
        s.unblock_domain("blocked.test").unwrap();
        assert!(!url_is_blocked(&s, "http://blocked.test/"));
    }

    #[test]
    fn pages_and_internal_addresses_are_recorded_or_not() {
        assert!(should_record_history_url("https://example.com/a?b=c"));
        assert!(should_record_history_url("http://localhost:8765/x.html"));
        assert!(should_record_history_url("file:///C:/a.html"));
        assert!(!should_record_history_url(""));
        assert!(!should_record_history_url("about:blank"));
        assert!(!should_record_history_url("hiwave://about"));
        assert!(!should_record_history_url("data:text/html,hi"));
        assert!(!should_record_history_url("JavaScript:alert(1)"));
    }
}
