//! The host side of HTMLMediaElement (MEDIA-AVF M1.2, `docs/design/MEDIA_AVF.md` §3.5).
//!
//! The element's state lives in the engine, which pushes it with each event it fires
//! ([`crate::DomBindings::fire_media_event`]; the getters are in dom.rs). Script reaches
//! the engine through one host function, `__rk.mediaCall(gen, id, op, ...)`:
//! - `load`: the element asks to run its load algorithm again; the engine takes the
//!   requests with [`crate::DomBindings::take_media_loads`].
//! - `canPlayType`: answered by the engine's [`MediaCanPlay`] (AVFoundation's), `""`
//!   without one.

use crate::dom::{DomHost, SharedDomHost};
use rustkit_js::{JsError, JsRuntime, JsValue};
use std::cell::RefCell;
use std::rc::Rc;

/// `canPlayType`'s answer for a MIME type: `""`, `"maybe"` or `"probably"`.
pub type MediaCanPlay = Box<dyn Fn(&str) -> &'static str>;

#[derive(Default)]
pub(crate) struct MediaStore {
    /// Elements (raw NodeIds) whose `load()` script called, in call order.
    pub(crate) loads: Vec<usize>,
    pub(crate) can_play: Option<MediaCanPlay>,
}

pub(crate) type SharedMedia = Rc<RefCell<MediaStore>>;

pub(crate) fn install(
    runtime: &mut JsRuntime,
    store: &SharedMedia,
    host: &SharedDomHost,
) -> Result<(), JsError> {
    let (s, h) = (store.clone(), host.clone());
    runtime.register_host_function_by_op(
        "__rustkit_media",
        4,
        2,
        Box::new(move |args| {
            let op = match args.get(2) {
                Some(JsValue::String(op)) => op.as_str(),
                _ => return JsValue::Undefined,
            };
            match op {
                "load" => {
                    if let Some(node) = DomHost::node_of(&h, args) {
                        s.borrow_mut().loads.push(node);
                    }
                    JsValue::Undefined
                }
                "canPlayType" => {
                    let mime = match args.get(3) {
                        Some(JsValue::String(t)) => t.as_str(),
                        _ => "",
                    };
                    let answer = s.borrow().can_play.as_ref().map_or("", |f| f(mime));
                    JsValue::String(answer.into())
                }
                _ => JsValue::Undefined,
            }
        }),
    )?;
    runtime.evaluate_with_host(
        "__rk.mediaCall = globalThis.__rustkit_media; delete globalThis.__rustkit_media;",
    )?;
    Ok(())
}
