//! The HiWave Inspector page: an ordinary HTML/CSS/JS page that RustKit lays
//! out and paints, fed one element diagnostics dump (`diagnostics.rs`) and,
//! optionally, one `tools/inspector` difference result.
//!
//! The page lives with the other built-in pages
//! (`crates/hiwave-app/src/ui/inspector_rustkit.html`). This module only
//! prepares its input: it checks both schemas and writes the JSON into the
//! page's `application/json` element, escaped so that no string in it can
//! close that element. The page reads it with `JSON.parse` and puts every
//! string on screen through `textContent`.
//!
//! On demand only: nothing here runs unless a caller builds an Inspector.

use super::*;

/// The Inspector page before any input is injected.
pub const INSPECTOR_PAGE_TEMPLATE: &str = include_str!("../../hiwave-app/src/ui/inspector_rustkit.html");

/// Schema tag of an `inspector_diff.py` result (`tools/inspector`).
pub const INSPECTOR_DIFF_SCHEMA: &str = "hiwave-inspector-diff/1";

/// The two elements the input is written into; the template carries each
/// with `null`.
const INPUT_ELEMENT: &str = r#"<script type="application/json" id="hiwave-inspector-input">null</script>"#;
const DUMP_ELEMENT: &str = r#"<script type="application/json" id="hiwave-inspector-dump">null</script>"#;

/// Why an Inspector input was refused.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum InspectorInputError {
    #[error("{what} is not JSON: {reason}")]
    NotJson { what: &'static str, reason: String },
    #[error("{what} has schema {found:?}, expected {expected:?}")]
    WrongSchema { what: &'static str, expected: &'static str, found: Option<String> },
    #[error("the dump has no `elements` list")]
    NoElements,
}

/// The element click-to-inspect selects: the target view's element at a
/// point, named the way the Inspector's records name it.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct InspectorSelection {
    pub node_id: usize,
    pub selector: String,
}

fn parse(what: &'static str, json: &str, expected: &'static str) -> Result<serde_json::Value, InspectorInputError> {
    let v: serde_json::Value = serde_json::from_str(json)
        .map_err(|e| InspectorInputError::NotJson { what, reason: e.to_string() })?;
    let found = v.get("schema").and_then(|s| s.as_str());
    if found != Some(expected) {
        return Err(InspectorInputError::WrongSchema { what, expected, found: found.map(str::to_string) });
    }
    Ok(v)
}

/// JSON text that is safe inside a `<script type="application/json">`
/// element, appended to `out`; returns its length in UTF-16 code units (the
/// page's string offsets). `<`, `>` and `&` only occur inside JSON strings,
/// where their `\u` escapes mean the same characters, so `</script>` or
/// `<!--` in a class name cannot end the element. U+2028/U+2029 are escaped
/// too.
fn push_script_safe_json(out: &mut String, v: &serde_json::Value) -> usize {
    let text = serde_json::to_string(v).expect("a serde_json::Value always serialises");
    let mut units = 0;
    for c in text.chars() {
        let escaped = match c {
            '<' => "\\u003c",
            '>' => "\\u003e",
            '&' => "\\u0026",
            '\u{2028}' => "\\u2028",
            '\u{2029}' => "\\u2029",
            c => {
                out.push(c);
                units += c.len_utf16();
                continue;
            }
        };
        out.push_str(escaped);
        units += escaped.len();
    }
    units
}

fn push_raw(out: &mut String, s: &str) -> usize {
    out.push_str(s);
    s.encode_utf16().count()
}

/// The dump as script-safe JSON text, and one tree row per element:
/// `[start, end, node_id, depth, has_box, tag, id, classes]`, where
/// `start..end` is the element's record in that text, in UTF-16 units. The
/// page parses the rows when it loads and a record only when it is shown:
/// `JSON.parse` of a whole 2,000-element dump costs RustKit's script engine
/// over a second.
fn dump_text_and_rows(dump: &serde_json::Value) -> (String, Vec<serde_json::Value>) {
    let mut text = String::new();
    let mut at = push_raw(&mut text, "{");
    let mut rows = Vec::new();
    let obj = dump.as_object().expect("checked: the dump is an object");
    for (k, (key, value)) in obj.iter().enumerate() {
        if k > 0 {
            at += push_raw(&mut text, ",");
        }
        at += push_script_safe_json(&mut text, &serde_json::Value::String(key.clone()));
        at += push_raw(&mut text, ":");
        let Some(elements) = value.as_array().filter(|_| key == "elements") else {
            at += push_script_safe_json(&mut text, value);
            continue;
        };
        at += push_raw(&mut text, "[");
        for (i, e) in elements.iter().enumerate() {
            if i > 0 {
                at += push_raw(&mut text, ",");
            }
            let start = at;
            at += push_script_safe_json(&mut text, e);
            let id = &e["identity"];
            rows.push(serde_json::json!([
                start,
                at,
                id["node_id"],
                id["ancestry"].as_array().map_or(0, |a| a.len()),
                e["box"]["has_box"].as_bool().unwrap_or(false),
                id["tag"],
                id["id"],
                id["classes"],
            ]));
        }
        at += push_raw(&mut text, "]");
    }
    push_raw(&mut text, "}");
    (text, rows)
}

/// The record a difference result names first: its `first_innermost` root
/// (the README's "start here"), else `first`, matched to a dump record by
/// oracle key, then by structural selector. `None` when the diff names no
/// root or the root's element is not in the dump (missing on our side, or
/// past a truncated dump's cap).
pub fn diff_root_record(dump: &serde_json::Value, diff: &serde_json::Value) -> Option<usize> {
    let root = [&diff["first_innermost"], &diff["first"]].into_iter().find(|r| r.is_object())?;
    let elements = dump["elements"].as_array()?;
    ["oracle_key", "selector"].iter().find_map(|key| {
        let want = root[*key].as_str()?;
        elements.iter().position(|e| e["identity"][*key].as_str() == Some(want))
    })
}

/// The Inspector page with `dump_json` (a `hiwave-element-diagnostics/1`
/// dump, as `Engine::all_element_diagnostics` or `parity-capture
/// --dump-diagnostics` writes it) and the optional `diff_json`
/// (`hiwave-inspector-diff/1`) injected. `select` is the node id the page
/// selects first; without it the page selects the diff's first divergence,
/// else `body`.
pub fn inspector_page_html(
    dump_json: &str,
    diff_json: Option<&str>,
    select: Option<usize>,
) -> Result<String, InspectorInputError> {
    let dump = parse("the diagnostics dump", dump_json, DIAGNOSTICS_SCHEMA)?;
    if !dump.get("elements").is_some_and(|e| e.is_array()) {
        return Err(InspectorInputError::NoElements);
    }
    let diff = diff_json.map(|d| parse("the difference result", d, INSPECTOR_DIFF_SCHEMA)).transpose()?;
    let diff_record = diff.as_ref().and_then(|d| diff_root_record(&dump, d));
    let (dump_text, rows) = dump_text_and_rows(&dump);
    let input = serde_json::json!({
        "schema": dump["schema"],
        "page": dump["page"],
        "total_elements": dump["total_elements"],
        "truncated": dump["truncated"],
        "rows": rows,
        "diff": diff,
        "diff_record": diff_record,
        "select": select,
    });
    let mut input_text = String::new();
    push_script_safe_json(&mut input_text, &input);
    debug_assert_eq!(INSPECTOR_PAGE_TEMPLATE.matches(INPUT_ELEMENT).count(), 1);
    debug_assert_eq!(INSPECTOR_PAGE_TEMPLATE.matches(DUMP_ELEMENT).count(), 1);
    Ok(INSPECTOR_PAGE_TEMPLATE
        .replacen(INPUT_ELEMENT, &INPUT_ELEMENT.replacen(">null<", &format!(">{input_text}<"), 1), 1)
        .replacen(DUMP_ELEMENT, &DUMP_ELEMENT.replacen(">null<", &format!(">{dump_text}<"), 1), 1))
}

/// The script a live Inspector view runs to move its selection.
pub fn inspector_select_script(node_id: usize) -> String {
    format!("window.hiwaveInspector && window.hiwaveInspector.selectNode({node_id})")
}

impl Engine {
    /// Click to inspect, without the window: the element of `target` under
    /// viewport point (`x`, `y`) in CSS px, found by the diagnostics point
    /// lookup (`elementFromPoint` rules: text selects its element, a point
    /// outside the viewport selects nothing). Pass `node_id` to the
    /// Inspector page as `select`, or run [`inspector_select_script`] in a
    /// live Inspector view. Read-only, as [`Engine::element_diagnostics`].
    pub fn inspector_selection_at(
        &self,
        target: EngineViewId,
        x: f32,
        y: f32,
    ) -> Result<Option<InspectorSelection>, EngineError> {
        let record = self.element_diagnostics(target, &DiagnosticTarget::Point { x, y }, &DiagnosticsOptions::default())?;
        Ok(record.map(|r| InspectorSelection { node_id: r.identity.node_id, selector: r.identity.selector }))
    }
}
