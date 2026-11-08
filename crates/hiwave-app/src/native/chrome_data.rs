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
