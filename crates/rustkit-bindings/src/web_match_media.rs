//! `window.matchMedia` for page script, over the engine's own media query
//! evaluator (rustkit-css `media.rs`): the one that decides `@media` rules
//! in style sheets, so a script and a sheet agree on every query.
//!
//! Two host functions: `__rustkit_media_serialize(query)` answers the
//! `MediaQueryList.media` text (an invalid query is `not all`), and
//! `__rustkit_media_matches(media, width, height)` evaluates that text
//! against a viewport. `web_match_media.js` is the object layer: the
//! `MediaQueryList` instances, their `change` events after a resize
//! ([`crate::DomBindings::fire_media_query_changes`]) and the legacy
//! `addListener`/`removeListener`.
//!
//! The environment the evaluator answers for is the board's: device pixel
//! ratio 1, light colour scheme, a fine hover-capable pointer, no
//! reduced-motion preference. Only the viewport size is live.

use rustkit_js::{JsError, JsRuntime, JsValue};

fn string_arg(args: &[JsValue], i: usize) -> &str {
    match args.get(i) {
        Some(JsValue::String(s)) => s.as_str(),
        _ => "",
    }
}

fn length_arg(args: &[JsValue], i: usize) -> f32 {
    match args.get(i) {
        Some(JsValue::Number(n)) if n.is_finite() && *n >= 0.0 => *n as f32,
        _ => 0.0,
    }
}

pub(crate) fn install(runtime: &mut JsRuntime) -> Result<(), JsError> {
    runtime.register_host_function(
        "__rustkit_media_serialize",
        1,
        Box::new(|args| JsValue::String(rustkit_css::media_query_list_serialize(string_arg(args, 0)))),
    )?;
    runtime.register_host_function(
        "__rustkit_media_matches",
        3,
        Box::new(|args| {
            JsValue::Boolean(rustkit_css::media_query_list_matches(
                string_arg(args, 0),
                length_arg(args, 1),
                length_arg(args, 2),
            ))
        }),
    )?;
    runtime
        .evaluate_with_host(include_str!("web_match_media.js"))
        .map(|_| ())
}
