//! CLDR's likely-subtags table for `Intl.Locale.prototype.maximize` and
//! `minimize` (and the text direction and week data read through them).
//!
//! The table is data only (`likely_subtags.txt`, CLDR 48 under the Unicode
//! License v3, written by `scripts/cldr_likely_subtags.py`) and lives in the
//! binary once, not in every page's script heap: `web_intl.js` asks for one
//! key at a time through a host function it takes off the global object as
//! it loads. The algorithm over the table (UTS #35 §4.3) is in `web_intl.js`.

use rustkit_js::{JsError, JsRuntime, JsValue};
use std::sync::OnceLock;

static DATA: &str = include_str!("likely_subtags.txt");

/// `(key, value)` per data line, in key order.
fn table() -> &'static [(&'static str, &'static str)] {
    static TABLE: OnceLock<Vec<(&'static str, &'static str)>> = OnceLock::new();
    TABLE.get_or_init(|| {
        DATA.lines()
            .filter(|line| !line.starts_with('#'))
            .filter_map(|line| line.split_once('='))
            .collect()
    })
}

/// The likely `language-Script-REGION` for `key` (`zh`, `zh-TW`, `und-Arab`,
/// `und-Arab-PK`, ...), when CLDR has an entry for exactly that key.
pub(crate) fn likely_subtags(key: &str) -> Option<String> {
    let table = table();
    let (_, value) = table[table.binary_search_by(|(k, _)| (*k).cmp(key)).ok()?];
    // `ScrpRG` keeps the key's language; `lang-ScrpRG` names another.
    let (language, rest) = match value.split_once('-') {
        Some((language, rest)) => (language, rest),
        None => (key.split('-').next().unwrap_or(key), value),
    };
    let (script, region) = rest.split_at_checked(4)?;
    Some(format!("{language}-{script}-{region}"))
}

/// Define the lookup `web_intl.js` reads, on the host object (`__rk.likelySubtags`).
pub(crate) fn install(runtime: &mut JsRuntime) -> Result<(), JsError> {
    runtime.register_host_function(
        "__rustkit_likely_subtags",
        1,
        Box::new(|args| match args.first() {
            Some(JsValue::String(key)) => likely_subtags(key).map_or(JsValue::Undefined, JsValue::String),
            _ => JsValue::Undefined,
        }),
    )?;
    // Moved to the host object now, before any page script.
    runtime.evaluate_with_host("__rk.likelySubtags = globalThis.__rustkit_likely_subtags; delete globalThis.__rustkit_likely_subtags;")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_is_in_key_order_and_every_line_decodes() {
        let table = table();
        assert!(table.len() > 7000, "the table lost entries: {}", table.len());
        assert!(table.windows(2).all(|w| w[0].0 < w[1].0), "keys must ascend for the binary search");
        for (key, _) in table {
            let full = likely_subtags(key).unwrap_or_else(|| panic!("{key} does not decode"));
            let parts: Vec<&str> = full.split('-').collect();
            assert!(parts.len() == 3 && parts[1].len() == 4 && matches!(parts[2].len(), 2 | 3), "{key} -> {full}");
        }
    }

    #[test]
    fn lookups_are_exact_keys_only() {
        let get = |k: &str| likely_subtags(k);
        assert_eq!(get("en").as_deref(), Some("en-Latn-US"));
        assert_eq!(get("zh-TW").as_deref(), Some("zh-Hant-TW"));
        assert_eq!(get("und").as_deref(), Some("en-Latn-US"));
        assert_eq!(get("und-Arab").as_deref(), Some("ar-Arab-EG"));
        assert_eq!(get("und-Arab-PK").as_deref(), Some("ur-Arab-PK"));
        assert_eq!(get("und-419").as_deref(), Some("es-Latn-419"));
        assert_eq!(get("en-US"), None);
        assert_eq!(get("xx"), None);
        assert_eq!(get(""), None);
    }
}
