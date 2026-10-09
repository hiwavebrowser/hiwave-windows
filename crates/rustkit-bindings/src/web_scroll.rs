//! Scroll position for page script: `window.scrollTo/scrollBy/scrollX/scrollY`,
//! `Element.scrollTop/scrollLeft/scrollIntoView` (web_scroll.js). The engine
//! owns the scroll offset; it publishes the offset and the maximum after a
//! layout or a user scroll (`DomBindings::set_scroll_state`), and takes the
//! last position script asked for when it settles
//! (`DomBindings::take_scroll_request`).

use rustkit_js::{JsError, JsRuntime, JsValue};
use std::cell::RefCell;
use std::rc::Rc;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct ScrollState {
    pub x: f32,
    pub y: f32,
    pub max_x: f32,
    pub max_y: f32,
    /// Where script last scrolled to, not yet applied by the engine.
    pub request: Option<(f32, f32)>,
}

pub(crate) type SharedScroll = Rc<RefCell<ScrollState>>;

fn number(args: &[JsValue], index: usize) -> Option<f32> {
    match args.get(index) {
        Some(JsValue::Number(n)) if n.is_finite() => Some(*n as f32),
        _ => None,
    }
}

pub(crate) fn install(runtime: &mut JsRuntime, state: &SharedScroll) -> Result<(), JsError> {
    // `__rustkit_scroll_state()`: "x y maxX maxY".
    let s = state.clone();
    runtime.register_host_function(
        "__rustkit_scroll_state",
        0,
        Box::new(move |_| {
            let s = s.borrow();
            JsValue::String(format!("{} {} {} {}", s.x, s.y, s.max_x, s.max_y))
        }),
    )?;
    // `__rustkit_scroll_to(x, y)`: scroll there (clamped to the document;
    // a non-number keeps that axis) and answer the position reached, "x y".
    let s = state.clone();
    runtime.register_host_function(
        "__rustkit_scroll_to",
        2,
        Box::new(move |args| {
            let mut s = s.borrow_mut();
            let x = number(args, 0).unwrap_or(s.x).clamp(0.0, s.max_x.max(0.0));
            let y = number(args, 1).unwrap_or(s.y).clamp(0.0, s.max_y.max(0.0));
            if (x, y) != (s.x, s.y) {
                s.x = x;
                s.y = y;
                s.request = Some((x, y));
            }
            JsValue::String(format!("{x} {y}"))
        }),
    )?;
    runtime.evaluate_script(include_str!("web_scroll.js")).map(|_| ())
}
