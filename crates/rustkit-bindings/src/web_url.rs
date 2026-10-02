//! `URL` and `URLSearchParams` for page script.
//!
//! The parser and the component setters are the `url` crate's (the same one
//! the network stack resolves requests with), exposed to JS as two host
//! functions. `web_url.js` is the object layer over them. The host functions
//! are deleted from the global scope once the JS has captured them.

use rustkit_js::{JsError, JsRuntime, JsValue};
use url::Url;

fn string_arg(args: &[JsValue], index: usize) -> Option<&str> {
    match args.get(index) {
        Some(JsValue::String(s)) => Some(s.as_str()),
        _ => None,
    }
}

/// The URL's components as the JSON object `web_url.js` reads.
fn components(url: &Url) -> String {
    let host = url.host_str().unwrap_or("");
    let port = url.port().map(|p| p.to_string()).unwrap_or_default();
    let host_with_port = if port.is_empty() {
        host.to_string()
    } else {
        format!("{host}:{port}")
    };
    serde_json::json!({
        "href": url.as_str(),
        "origin": url.origin().ascii_serialization(),
        "protocol": format!("{}:", url.scheme()),
        "username": url.username(),
        "password": url.password().unwrap_or(""),
        "host": host_with_port,
        "hostname": host,
        "port": port,
        "pathname": url.path(),
        "search": match url.query() { Some(q) if !q.is_empty() => format!("?{q}"), _ => String::new() },
        "hash": match url.fragment() { Some(f) if !f.is_empty() => format!("#{f}"), _ => String::new() },
    })
    .to_string()
}

/// `new URL(input, base)`: the components, or `None` when it does not parse.
fn parse(input: &str, base: Option<&str>) -> Option<Url> {
    match base {
        Some(b) => Url::parse(b).ok()?.join(input).ok(),
        None => Url::parse(input).ok(),
    }
}

/// Apply one component setter. A value the URL Standard would reject leaves
/// the URL as it was, which is what the setters return.
fn update(href: &str, field: &str, value: &str) -> Option<Url> {
    let mut url = Url::parse(href).ok()?;
    match field {
        "href" => return Url::parse(value).ok(),
        "protocol" => {
            let scheme = value.split(':').next().unwrap_or("");
            let _ = url.set_scheme(scheme);
        }
        "username" => {
            let _ = url.set_username(value);
        }
        "password" => {
            let _ = url.set_password(if value.is_empty() { None } else { Some(value) });
        }
        "host" => {
            let (h, p) = match value.rsplit_once(':') {
                Some((h, p)) if !h.ends_with(']') || h.starts_with('[') => (h, Some(p)),
                _ => (value, None),
            };
            if url.set_host(if h.is_empty() { None } else { Some(h) }).is_ok() {
                if let Some(p) = p {
                    let _ = url.set_port(p.parse::<u16>().ok());
                }
            }
        }
        "hostname" => {
            let _ = url.set_host(if value.is_empty() { None } else { Some(value) });
        }
        "port" => {
            if value.is_empty() {
                let _ = url.set_port(None);
            } else if let Ok(p) = value.trim().parse::<u16>() {
                let _ = url.set_port(Some(p));
            }
        }
        "pathname" => url.set_path(value),
        "search" => {
            let q = value.strip_prefix('?').unwrap_or(value);
            url.set_query(if q.is_empty() { None } else { Some(q) });
        }
        "hash" => {
            let f = value.strip_prefix('#').unwrap_or(value);
            url.set_fragment(if f.is_empty() { None } else { Some(f) });
        }
        _ => {}
    }
    Some(url)
}

pub(crate) fn install(runtime: &mut JsRuntime) -> Result<(), JsError> {
    runtime.register_host_function(
        "__rustkit_url_parse",
        2,
        Box::new(|args| {
            let Some(input) = string_arg(args, 0) else { return JsValue::Null };
            match parse(input, string_arg(args, 1)) {
                Some(url) => JsValue::String(components(&url)),
                None => JsValue::Null,
            }
        }),
    )?;
    runtime.register_host_function(
        "__rustkit_url_update",
        3,
        Box::new(|args| {
            let (Some(href), Some(field), Some(value)) =
                (string_arg(args, 0), string_arg(args, 1), string_arg(args, 2))
            else {
                return JsValue::Null;
            };
            match update(href, field, value) {
                Some(url) => JsValue::String(components(&url)),
                None => JsValue::Null,
            }
        }),
    )?;
    runtime.evaluate_script(include_str!("web_url.js")).map(|_| ())
}
