//! Autofill hardening: content pages no longer receive credentials. These pins read the sources
//! on purpose where the handler lives inside the content WebView's IPC closure in `main.rs`,
//! which a unit test cannot call; the shared dispatcher is called for real.

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

    const COMMANDS: &str = include_str!("ipc/commands.rs");

    #[test]
    fn the_dispatcher_source_has_no_credentials_handler() {
        // Present in both builds (the module is compiled out of the native build, its source is not).
        assert!(!COMMANDS.contains("fn handle_get_credentials_for_autofill"), "the credentials handler is gone");
        assert!(!COMMANDS.contains("get_credentials("), "nothing in the dispatcher reads credentials from the vault");
    }

    #[cfg(not(feature = "native-win32"))]
    #[test]
    fn the_shared_dispatcher_returns_no_credentials() {
        use crate::ipc::{commands::handle_message, IpcMessage, IpcResponse};
        use crate::state::{AppConfig, AppState};
        use std::sync::{Arc, Mutex};
        let dir = std::env::temp_dir().join(format!("hiwave_autofill_guard_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut config = AppConfig::default();
        config.data_dir = dir.clone();
        config.download_dir = dir.join("downloads");
        let state = Arc::new(Mutex::new(AppState::new(config).expect("state")));
        let response = handle_message(&state, IpcMessage::GetCredentialsForAutofill { domain: "example.com".to_string() });
        assert!(matches!(response, IpcResponse::Error { .. }), "the dispatcher must not answer with data: {response:?}");
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
