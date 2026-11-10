//! Autofill hardening: credentials are delivered only for a user-requested fill on the page's
//! own origin. These pins read the sources on purpose: the handler lives inside the content
//! WebView's IPC closure in `main.rs`, which a unit test cannot call.

#[cfg(test)]
mod tests {
    const MAIN: &str = include_str!("main.rs");
    const AUTOFILL_JS: &str = include_str!("ui/autofill.js");

    #[test]
    fn the_autofill_helper_has_no_message_listener() {
        assert!(
            !AUTOFILL_JS.contains("addEventListener('message'") && !AUTOFILL_JS.contains("addEventListener(\"message\""),
            "autofill.js must not listen for window messages"
        );
        assert!(!AUTOFILL_JS.contains("hiwave_autofill_credentials"), "no credentials message type is read by the page");
    }

    #[test]
    fn the_content_autofill_request_is_dropped() {
        // The content IPC arm must be a log-and-drop.
        let start = MAIN.find("IpcMessage::GetCredentialsForAutofill { ref domain } => {").expect("content arm");
        let arm = &MAIN[start..];
        let end = arm.find("// Handle focus mode from content").expect("end of the arm");
        let arm = &arm[..end];
        assert!(!arm.contains("handle_message"), "the arm must not reach the vault handler:\n{arm}");
        assert!(!arm.contains("SendAutofillCredentials"), "the arm must not deliver credentials:\n{arm}");
        assert!(!arm.contains("EvaluateContentScript"), "the arm must not script the page:\n{arm}");
    }

    #[test]
    fn the_app_posts_no_credentials_message() {
        assert!(
            !MAIN.contains("hiwave_autofill_credentials"),
            "main.rs must not post a credentials message into the page"
        );
        assert!(!MAIN.contains("UserEvent::SendAutofillCredentials {"), "nothing constructs the delivery event");
    }
}
