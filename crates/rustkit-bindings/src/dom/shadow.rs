//! The host function behind web_shadow.js. Shadow roots live in the Rust
//! DOM (rustkit-dom's `shadow.rs`): attaching one, finding a host's root or
//! a root's host, and slot assignment are all answered from that one tree,
//! the same one style, layout and paint read.

use super::{id_list, node_id, string_arg, SharedDomHost};
use crate::DomDirty;
use rustkit_dom::{ShadowRootInit, ShadowRootMode};
use rustkit_js::{JsError, JsRuntime, JsValue};
use std::rc::Rc;

/// `shadow(gen, op, id, a, b)`, for node `id`:
/// - "attach": attach a shadow root to host `id` in mode `a` ("open" or
///   "closed"); `b` holds `d` for delegatesFocus and `m` for manual slot
///   assignment. Answers the root's id, or the DOMException name to throw.
/// - "root": the host's shadow root, whatever its mode, or null.
/// - "host": the shadow root's host, or null for any other node.
/// - "init": the shadow root's "mode delegatesFocus slotAssignment", as
///   in `open 0 named`, or null for any other node.
/// - "assigned": the slot's assigned nodes, as an id list.
/// - "slot": the slot the node is assigned to, or null.
/// - "any": has any node of this document a shadow root?
/// - "define": `customElements.define` defined the name `a` (`id` is the
///   document); elements of that name are `:defined` from now on.
pub(super) fn install(
    runtime: &mut JsRuntime,
    host: &SharedDomHost,
    dirty: &Rc<crate::DirtyMark>,
) -> Result<(), JsError> {
    let h = host.clone();
    let d = dirty.clone();
    runtime.register_host_function(
        "__rustkit_dom_shadow",
        5,
        Box::new(move |args| {
            let host = h.borrow();
            let Some(document) = args.first().and_then(|g| host.document_for(g)) else {
                return JsValue::Null;
            };
            let Some(node) = host.node_at(args, 2) else {
                return JsValue::Null;
            };
            match string_arg(args, 1) {
                Some("attach") => {
                    let mode = match string_arg(args, 3) {
                        Some("closed") => ShadowRootMode::Closed,
                        _ => ShadowRootMode::Open,
                    };
                    let flags = string_arg(args, 4).unwrap_or("");
                    let init = ShadowRootInit {
                        mode,
                        delegates_focus: flags.contains('d'),
                        manual_slot_assignment: flags.contains('m'),
                    };
                    match document.attach_shadow(&node, init) {
                        Ok(root) => {
                            // The host's children stop rendering, until
                            // slotted.
                            d.mark(DomDirty::Style);
                            node_id(Some(root))
                        }
                        Err(_) => JsValue::String("NotSupportedError".into()),
                    }
                }
                Some("root") => node_id(node.shadow_root()),
                Some("host") => node_id(node.shadow_host()),
                Some("init") => match node.shadow_root_init() {
                    Some(init) => JsValue::String(format!(
                        "{} {} {}",
                        match init.mode {
                            ShadowRootMode::Open => "open",
                            ShadowRootMode::Closed => "closed",
                        },
                        u8::from(init.delegates_focus),
                        if init.manual_slot_assignment {
                            "manual"
                        } else {
                            "named"
                        },
                    )),
                    None => JsValue::Null,
                },
                Some("assigned") => id_list(node.assigned_nodes()),
                Some("slot") => node_id(node.assigned_slot()),
                Some("any") => JsValue::Boolean(document.has_shadow_roots()),
                Some("define") => {
                    if let Some(name) = string_arg(args, 3) {
                        document.define_custom_element(name);
                        // `:defined` rules now match elements of the name.
                        d.mark(DomDirty::Style);
                    }
                    JsValue::Null
                }
                _ => JsValue::Null,
            }
        }),
    )?;
    runtime.evaluate_script(SHADOW_JS)?;
    Ok(())
}

/// Hands web_shadow.js the host function and the wrapper internals it
/// needs, under a name it deletes when it loads.
const SHADOW_JS: &str = r#"
(function (g) {
    var I = g.__rkNodeInternals, SH = __rustkit_dom_shadow;
    delete g.__rustkit_dom_shadow;
    function call(op, node, a, b) {
        var s = I.slotOf(node), gen = I.gen();
        return s.gen === gen ? SH(gen, op, s.id, a, b) : null;
    }
    function nodes(ids) {
        return typeof ids === 'string' && ids !== '' ? ids.split(' ').map(function (id) { return I.wrap(Number(id)); }) : [];
    }
    // A fragment the DOM links to a host is a ShadowRoot.
    I.setFragmentProto(function (id) {
        return g.ShadowRoot && SH(I.gen(), 'init', id) !== null ? g.ShadowRoot.prototype : g.DocumentFragment.prototype;
    });
    Object.defineProperty(g, '__rkShadowInternals', {
        value: {
            call: call,
            node: function (op, node, a, b) { return I.wrap(call(op, node, a, b)); },
            nodes: function (op, node) { return nodes(call(op, node)); }
        },
        configurable: true
    });
})(globalThis);
"#;
