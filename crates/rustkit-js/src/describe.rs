//! Describing a script value for diagnostics without running any of the
//! page's code: no `toString`, no getter, no Proxy trap. Primitives are
//! shown as they are; an error by its own `name` and `message` data
//! properties and the engine's own record of its stack; any other object by a
//! fixed tag, as a browser console's text form does.

use boa_engine::builtins::error::Error;
use boa_engine::property::PropertyKey;
use boa_engine::{js_string, Context, JsObject, JsValue};

/// How far up the prototype chain an error's `name` is looked for.
const MAX_PROTOTYPE_DEPTH: usize = 16;

/// `key` as an own data property of `object`: never a getter, never a trap
/// (a Proxy keeps no own properties of its own).
fn data_property(object: &JsObject, key: &str) -> Option<JsValue> {
    let key = PropertyKey::from(js_string!(key));
    object.borrow().properties().get(&key).and_then(|d| d.value().cloned())
}

/// `key` as a data property of `object` or of a prototype.
fn inherited_data_property(object: &JsObject, key: &str) -> Option<JsValue> {
    let mut current = Some(object.clone());
    for _ in 0..MAX_PROTOTYPE_DEPTH {
        let object = current?;
        if let Some(value) = data_property(&object, key) {
            return Some(value);
        }
        current = object.prototype();
    }
    None
}

fn string_of(value: &JsValue) -> Option<String> {
    value.as_string().map(|s| s.to_std_string_escaped())
}

/// A primitive's text; `None` for an object.
fn primitive_text(value: &JsValue, context: &mut Context) -> Option<String> {
    if value.is_object() {
        return None;
    }
    if let Some(symbol) = value.as_symbol() {
        let description = symbol.description().map(|d| d.to_std_string_escaped()).unwrap_or_default();
        return Some(format!("Symbol({description})"));
    }
    // Strings, numbers, booleans, null, undefined, BigInt: ToString runs no
    // script for these.
    value.to_string(context).ok().map(|s| s.to_std_string_escaped())
}

/// An error object's `name`, from data properties only.
pub(crate) fn error_name(object: &JsObject) -> Option<String> {
    inherited_data_property(object, "name").and_then(|v| string_of(&v))
}

/// An error object's `message`, from data properties only.
pub(crate) fn error_message(object: &JsObject) -> Option<String> {
    inherited_data_property(object, "message").and_then(|v| string_of(&v))
}

/// An error object's `Name: message` (or `Name`), from data properties only.
fn error_head(object: &JsObject) -> String {
    let name = inherited_data_property(object, "name")
        .and_then(|v| string_of(&v))
        .unwrap_or_else(|| "Error".to_string());
    let message = inherited_data_property(object, "message")
        .and_then(|v| string_of(&v))
        .unwrap_or_default();
    match message.is_empty() {
        true => name,
        false => format!("{name}: {message}"),
    }
}

/// An error object, or an object made from a constructor whose prototype
/// is the realm's `Error.prototype` (the bindings' `DOMException`). Reads
/// internal prototype links only: no Proxy trap runs.
pub(crate) fn is_error_like(object: &JsObject, context: &Context) -> bool {
    if object.is::<Error>() {
        return true;
    }
    let error_prototype = context.intrinsics().constructors().error().prototype();
    let mut current = object.prototype();
    for _ in 0..MAX_PROTOTYPE_DEPTH {
        let Some(prototype) = current else { return false };
        if JsObject::equals(&prototype, &error_prototype) {
            return true;
        }
        current = prototype.prototype();
    }
    false
}

/// A thrown value's text for the script log and the Inspector. An error
/// object with string `name` and `message` is Boa's own text (`Name:
/// message (path:line:col)` and its frames); any other value is described
/// as a console line describes it (`[object Object]`, never its
/// properties), followed by the frames Boa recorded where it was thrown.
pub(crate) fn error_text(error: &boa_engine::JsError, context: &mut Context) -> String {
    let full = error.to_string();
    let Some(value) = error.as_opaque() else {
        return full;
    };
    if let Some(object) = value.as_object() {
        let strings = ["name", "message"]
            .iter()
            .all(|key| inherited_data_property(&object, key).is_none_or(|v| v.is_string()));
        if object.is::<Error>() && strings {
            return full;
        }
    }
    // Boa's text is the value's display, then one `\n    at ...` line per
    // frame: keep the frames only.
    let shown = value.display().to_string();
    let frames = full.strip_prefix(shown.as_str()).unwrap_or("");
    format!("{}{frames}", text(value, context))
}

/// A value as a console line shows it.
pub(crate) fn text(value: &JsValue, context: &mut Context) -> String {
    if let Some(text) = primitive_text(value, context) {
        return text;
    }
    let Some(object) = value.as_object() else {
        return String::new();
    };
    if is_error_like(&object, context) {
        error_head(&object)
    } else if object.is_array() {
        "[object Array]".to_string()
    } else if object.is_callable() {
        "[object Function]".to_string()
    } else {
        "[object Object]".to_string()
    }
}


/// A value as a console line shows it when its text is not to be kept:
/// a string (and an error's message) only as its length, a number only as
/// its type. `true`, `false`, `null` and `undefined` carry no page text and
/// are shown as they are; an object by its tag, as in [`text`].
pub(crate) fn redacted_text(value: &JsValue, context: &Context) -> String {
    if let Some(s) = value.as_string() {
        return format!("<string len={}>", s.to_std_string_escaped().chars().count());
    }
    if value.is_number() {
        return "<number>".to_string();
    }
    if value.is_bigint() {
        return "<bigint>".to_string();
    }
    if value.is_symbol() {
        return "<symbol>".to_string();
    }
    if let Some(b) = value.as_boolean() {
        return b.to_string();
    }
    if value.is_null() {
        return "null".to_string();
    }
    let Some(object) = value.as_object() else {
        return "undefined".to_string();
    };
    if is_error_like(&object, context) {
        let name = crate::diag::standard_name(error_name(&object).as_deref());
        let length = inherited_data_property(&object, "message")
            .and_then(|v| string_of(&v))
            .map_or(0, |m| m.chars().count());
        format!("<{name} message len={length}>")
    } else if object.is_array() {
        "[object Array]".to_string()
    } else if object.is_callable() {
        "[object Function]".to_string()
    } else {
        "[object Object]".to_string()
    }
}

/// A thrown or rejected value as an error report: its text and, for an
/// error object, its stack. `stack_getter` is the engine's own
/// `Error.prototype.stack` getter, taken before any page script ran.
pub(crate) fn error_report(
    value: &JsValue,
    stack_getter: Option<&JsObject>,
    context: &mut Context,
) -> (String, Option<String>) {
    let head = text(value, context);
    let stack = match (value.as_object(), stack_getter) {
        (Some(object), Some(getter)) if object.is::<Error>() => getter
            .call(value, &[], context)
            .ok()
            .and_then(|v| string_of(&v))
            .filter(|s| !s.trim().is_empty()),
        _ => None,
    };
    (head, stack)
}

/// The engine's `Error.prototype.stack` getter, as the realm built it.
pub(crate) fn stack_getter(context: &Context) -> Option<JsObject> {
    let prototype = context.intrinsics().constructors().error().prototype();
    let key = PropertyKey::from(js_string!("stack"));
    let descriptor = prototype.borrow().properties().get(&key)?;
    descriptor.get().and_then(|g| g.as_object())
}

/// A console line: each argument described, joined by spaces, capped.
pub(crate) fn join_line(args: &[JsValue], mut describe: impl FnMut(&JsValue) -> String) -> String {
    const MAX_ARGS: usize = 256;
    const MAX_CHARS: usize = 16 * 1024;
    let mut line = String::new();
    for (i, arg) in args.iter().take(MAX_ARGS).enumerate() {
        if i > 0 {
            line.push(' ');
        }
        line.push_str(&describe(arg));
        if line.len() > MAX_CHARS {
            let mut end = MAX_CHARS;
            while !line.is_char_boundary(end) {
                end -= 1;
            }
            line.truncate(end);
            break;
        }
    }
    line
}
