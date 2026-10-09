//! Node APIs the 2026-10-03 web API census found missing: `Element.attributes`
//! (NamedNodeMap/Attr) and `getAttributeNode`, `document.activeElement` with
//! `focus()`/`blur()`, `createElementNS` and the `*AttributeNS` methods, and
//! `document.createEvent`. The script half is `node_apis.js`; this file adds
//! the one host function it needs, for element namespaces.
//!
//! Namespaces are stored, not acted on: layout and paint read tag names
//! only, so an SVG element made here draws exactly as `createElement` would.

use super::{adopt_template_contents, is_valid_name, node_id, string_arg, SharedDomHost};
use rustkit_dom::NodeType;
use rustkit_js::{JsError, JsRuntime, JsValue};

const XML_NS: &str = "http://www.w3.org/XML/1998/namespace";
const XMLNS_NS: &str = "http://www.w3.org/2000/xmlns/";

/// DOM §1.4 "validate and extract" for `namespace` (empty for null) and
/// `qualified`: `Err` is the DOMException name to throw.
fn validate_and_extract(namespace: &str, qualified: &str) -> Result<(), &'static str> {
    if !is_valid_name(qualified) {
        return Err("InvalidCharacterError");
    }
    let prefix = match qualified.split_once(':') {
        Some((p, local)) if p.is_empty() || local.is_empty() || local.contains(':') => {
            return Err("InvalidCharacterError")
        }
        Some((p, _)) => Some(p),
        None => None,
    };
    let xmlns = qualified == "xmlns" || prefix == Some("xmlns");
    if (prefix.is_some() && namespace.is_empty())
        || (prefix == Some("xml") && namespace != XML_NS)
        || (xmlns != (namespace == XMLNS_NS))
    {
        return Err("NamespaceError");
    }
    Ok(())
}

pub(super) fn install(runtime: &mut JsRuntime, host: &SharedDomHost) -> Result<(), JsError> {
    // ns(gen, op, a, b): "create" makes a detached element in namespace `a`
    // named `b` and answers its id; "check" only validates `a`/`b`; "of"
    // answers element `a`'s namespace, null when it has none. A string
    // answer from "create"/"check" is the DOMException name to throw.
    let h = host.clone();
    runtime.register_host_function(
        "__rustkit_dom_ns",
        4,
        Box::new(move |args| {
            let host = h.borrow();
            let Some(document) = args.first().and_then(|g| host.document_for(g)) else {
                return JsValue::Null;
            };
            let namespace = string_arg(args, 2).unwrap_or("");
            let qualified = string_arg(args, 3).unwrap_or("");
            match string_arg(args, 1) {
                Some("of") => match host.node_at(args, 2).map(|n| n.node_type.clone()) {
                    Some(NodeType::Element { namespace, .. }) if !namespace.is_empty() => {
                        JsValue::String(namespace)
                    }
                    _ => JsValue::Null,
                },
                Some(op @ ("create" | "check")) => {
                    if let Err(name) = validate_and_extract(namespace, qualified) {
                        return JsValue::String(name.to_string());
                    }
                    if op == "check" {
                        return JsValue::Null;
                    }
                    let node = document.create_node(NodeType::Element {
                        tag_name: qualified.to_string(),
                        namespace: namespace.to_string(),
                        attributes: Default::default(),
                    });
                    adopt_template_contents(document, &node, &mut host.templates.borrow_mut());
                    node_id(Some(node))
                }
                _ => JsValue::Null,
            }
        }),
    )?;
    runtime.evaluate_script(include_str!("node_apis.js"))?;
    Ok(())
}
