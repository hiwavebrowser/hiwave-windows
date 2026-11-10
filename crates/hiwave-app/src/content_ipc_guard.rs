//! IPC hardening: content pages can only reach the commands they need. The analytics commands
//! are answered for the app's own report page and for nothing else.

/// The report page's address: the active tab holds it only while the app itself is showing the
/// report (the app sets it before loading the page; a page cannot).
pub(crate) const REPORT_URL: &str = "hiwave://report";

/// Whether a content-page analytics request is answered: only when the active tab is the report.
pub(crate) fn analytics_allowed(active_tab_url: Option<&str>) -> bool {
    active_tab_url == Some(REPORT_URL)
}

#[cfg(test)]
mod tests {
    use super::*;
    const MAIN: &str = include_str!("main.rs");

    #[test]
    fn only_the_report_page_gets_analytics() {
        assert!(analytics_allowed(Some("hiwave://report")));
        assert!(!analytics_allowed(None));
        assert!(!analytics_allowed(Some("https://example.com/")));
        assert!(!analytics_allowed(Some("hiwave://about")));
        assert!(!analytics_allowed(Some("https://example.com/hiwave://report")));
    }

    #[test]
    fn the_content_analytics_arm_is_gated_and_does_not_log_the_message() {
        let start = MAIN.find("| IpcMessage::ExportAnalyticsData { .. } => {").expect("analytics arm");
        let arm = &MAIN[start..];
        let end = arm.find("_ => {}").expect("end of the arm");
        let arm = &arm[..end];
        assert!(arm.contains("analytics_allowed"), "the arm must check the gate:\n{arm}");
        assert!(!arm.contains("{:?}\", msg"), "the arm must not log the message:\n{arm}");
    }
}
