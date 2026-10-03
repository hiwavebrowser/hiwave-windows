//! The Rust side of the script-network bridge (`web_net_bridge.js`).
//!
//! Bindings hold a queue and a table of callbacks and nothing else: no
//! sockets, no URL policy. The engine enables the bridge, drains the queue,
//! runs every request under its `FetchPolicy` (rustkit-net) and delivers the
//! outcome. A bindings instance that never called [`DomBindings::enable_net_bridge`]
//! has no `__rustkit_net`, so no network surface can exist on it.

use super::*;

/// A request a page script queued, as plain data (the policy types live in
/// rustkit-net, which bindings do not depend on). `body_b64` is the request
/// body, base64 encoded, so binary bodies survive the JSON hop.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct NetRequest {
    pub id: u64,
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body_b64: Option<String>,
    /// `cors`, `no-cors` or `same-origin`.
    pub mode: String,
    /// `omit`, `same-origin` or `include`.
    pub credentials: String,
    /// `follow`, `manual` or `error`.
    pub redirect: String,
    /// `fetch` or `xhr`.
    pub destination: String,
}

/// The outcome of one request, as the engine hands it back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetDelivery {
    Response {
        url: String,
        status: u16,
        status_text: String,
        headers: Vec<(String, String)>,
        body_b64: String,
        /// `basic`, `cors`, `opaque` or `opaqueredirect`.
        kind: String,
        redirected: bool,
    },
    /// A denial or network failure. Script only ever sees "it failed".
    Error(String),
}

impl DomBindings {
    /// Install the bridge. Only the engine calls this, and only when it will
    /// drain the queue under a policy.
    pub fn enable_net_bridge(&self) -> Result<(), BindingError> {
        self.runtime
            .borrow_mut()
            .evaluate_script(include_str!("web_net_bridge.js"))?;
        // The surfaces built on the bridge exist only where the bridge does.
        self.runtime
            .borrow_mut()
            .evaluate_script(include_str!("web_xhr.js"))?;
        self.runtime
            .borrow_mut()
            .evaluate_script(include_str!("web_fetch.js"))?;
        Ok(())
    }

    /// Whether [`Self::enable_net_bridge`] ran.
    pub fn net_bridge_enabled(&self) -> bool {
        matches!(
            self.runtime
                .borrow_mut()
                .evaluate_script("typeof window.__rustkit_net === 'object'"),
            Ok(JsValue::Boolean(true))
        )
    }

    /// Take every request queued since the last call, in the order the page
    /// made them. Empty when the bridge is not enabled.
    pub fn take_net_requests(&self) -> Vec<NetRequest> {
        let taken = self.runtime.borrow_mut().evaluate_script(
            "window.__rustkit_net ? window.__rustkit_net.take() : '[]'",
        );
        match taken {
            Ok(JsValue::String(json)) => serde_json::from_str(&json).unwrap_or_default(),
            _ => Vec::new(),
        }
    }

    /// Hand a request's outcome to the callback that waits for it. `false`
    /// when nothing waits (the page aborted it, or the bridge is off).
    pub fn deliver_net_response(&self, id: u64, outcome: NetDelivery) -> Result<bool, BindingError> {
        let payload = match outcome {
            NetDelivery::Response {
                url,
                status,
                status_text,
                headers,
                body_b64,
                kind,
                redirected,
            } => serde_json::json!({
                "id": id,
                "ok": true,
                "url": url,
                "status": status,
                "status_text": status_text,
                "headers": headers,
                "body_b64": body_b64,
                "kind": kind,
                "redirected": redirected,
            }),
            NetDelivery::Error(reason) => serde_json::json!({
                "id": id,
                "ok": false,
                "error": reason,
            }),
        };
        // A JSON document inside a JS string literal: quote it with serde.
        let quoted = serde_json::to_string(&payload.to_string()).unwrap_or_default();
        let delivered = self.runtime.borrow_mut().evaluate_script(&format!(
            "window.__rustkit_net ? window.__rustkit_net.deliver({quoted}) : false"
        ))?;
        Ok(matches!(delivered, JsValue::Boolean(true)))
    }

    /// Complete every request still waiting with an error (the engine ran out
    /// of rounds or budget). Returns how many were waiting.
    pub fn fail_pending_net_requests(&self, reason: &str) -> usize {
        let quoted = serde_json::to_string(reason).unwrap_or_default();
        match self.runtime.borrow_mut().evaluate_script(&format!(
            "window.__rustkit_net ? window.__rustkit_net.fail_all({quoted}) : 0"
        )) {
            Ok(JsValue::Number(n)) => n as usize,
            _ => 0,
        }
    }

    /// How many requests still wait for a delivery.
    pub fn pending_net_requests(&self) -> usize {
        match self
            .runtime
            .borrow_mut()
            .evaluate_script("window.__rustkit_net ? window.__rustkit_net.waiting() : 0")
        {
            Ok(JsValue::Number(n)) => n as usize,
            _ => 0,
        }
    }
}
