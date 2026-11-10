//! IPC hardening: content pages can only reach the commands they need. The analytics commands are
//! answered for the app's own report page and for nothing else, and "the report page" is a
//! DOCUMENT, not a URL: the app embeds a fresh random nonce into the report HTML each time it
//! loads it, the page sends the nonce with every request, and a request is answered only when it
//! carries the nonce of the report most recently loaded. A page that is not that document does
//! not know the nonce, whatever it navigates to or sends first.

use std::sync::Mutex;

/// The placeholder in `ui/report.html` the app replaces with the nonce.
pub(crate) const NONCE_PLACEHOLDER: &str = "__HIWAVE_REPORT_NONCE__";

/// The nonce of the report page loaded last (`None` before any).
static REPORT_NONCE: Mutex<Option<String>> = Mutex::new(None);

/// A fresh nonce for a report load; the previous one stops being accepted.
pub(crate) fn new_report_nonce() -> String {
    use rand::Rng;
    let mut bytes = [0u8; 16];
    rand::rngs::OsRng.fill(&mut bytes);
    let nonce: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    if let Ok(mut current) = REPORT_NONCE.lock() {
        *current = Some(nonce.clone());
    }
    nonce
}

/// Whether a request's nonce is the current report's. Both must exist and be equal.
pub(crate) fn nonce_matches(message: Option<&str>, current: Option<&str>) -> bool {
    match (message, current) {
        (Some(a), Some(b)) if !a.is_empty() && a.len() == b.len() => {
            // No early exit on the first differing byte.
            a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
        }
        _ => false,
    }
}

/// Whether a content-page analytics request is answered.
pub(crate) fn analytics_allowed(message_nonce: Option<&str>) -> bool {
    let current = REPORT_NONCE.lock().ok().and_then(|c| c.clone());
    nonce_matches(message_nonce, current.as_deref())
}

/// The report HTML with this load's nonce embedded.
pub(crate) fn with_nonce(html: &str, nonce: &str) -> String {
    html.replace(NONCE_PLACEHOLDER, nonce)
}

#[cfg(test)]
mod tests {
    use super::*;
    const MAIN: &str = include_str!("main.rs");
    const REPORT: &str = include_str!("ui/report.html");

    #[test]
    fn a_nonce_matches_only_itself() {
        assert!(nonce_matches(Some("abc123"), Some("abc123")));
        assert!(!nonce_matches(None, Some("abc123")));
        assert!(!nonce_matches(Some("abc123"), None));
        assert!(!nonce_matches(None, None));
        assert!(!nonce_matches(Some("abc124"), Some("abc123")));
        assert!(!nonce_matches(Some("abc12"), Some("abc123")));
        assert!(!nonce_matches(Some(""), Some("")), "an empty nonce never matches");
    }

    #[test]
    fn each_report_load_gets_its_own_nonce_and_the_old_one_stops_working() {
        let first = new_report_nonce();
        assert_eq!(first.len(), 32);
        assert!(first.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(analytics_allowed(Some(&first)));
        let second = new_report_nonce();
        assert_ne!(first, second);
        assert!(!analytics_allowed(Some(&first)), "the previous load's nonce is no longer accepted");
        assert!(analytics_allowed(Some(&second)));
        assert!(!analytics_allowed(None), "a request with no nonce is dropped");
        assert!(!analytics_allowed(Some("hiwave://report")), "a URL is not a nonce");
    }

    #[test]
    fn the_report_page_carries_the_placeholder_and_sends_it_with_every_request() {
        assert!(REPORT.contains(NONCE_PLACEHOLDER), "the report HTML has the placeholder the app replaces");
        assert!(REPORT.contains("nonce: REPORT_NONCE"), "every report request carries the nonce");
        let html = with_nonce(REPORT, "deadbeef");
        assert!(!html.contains(NONCE_PLACEHOLDER) && html.contains("'deadbeef'"));
    }

    #[test]
    fn the_content_analytics_arm_checks_the_nonce_and_does_not_log_the_message() {
        let start = MAIN.find("| IpcMessage::ExportAnalyticsData { .. } => {").expect("analytics arm");
        let arm = &MAIN[start..];
        let end = arm.find("_ => {}").expect("end of the arm");
        let arm = &arm[..end];
        assert!(arm.contains("analytics_allowed"), "the arm must check the nonce:\n{arm}");
        assert!(!arm.contains("get_active_tab"), "the gate is not a URL:\n{arm}");
        assert!(!arm.contains("{:?}\", msg"), "the arm must not log the message:\n{arm}");
    }
}
