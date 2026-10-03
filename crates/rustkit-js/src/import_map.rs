//! Import maps (HTML §8.1.5.4, the parts a page can observe): `imports` and
//! `scopes`, resolved with the spec's longest-match rules. Pure data and URL
//! arithmetic, no JS: the module host asks `resolve` for every specifier.
//!
//! Stated limits: `integrity` is ignored; an import map that arrives after a
//! module has been resolved cannot change what was resolved (the module map
//! is keyed by URL, so nothing is re-resolved), and a later map never
//! overrides a key an earlier one defined.

use std::collections::BTreeMap;

use url::Url;

/// A specifier map: normalized key to its address (`None` is an explicit
/// block: `"x": null`).
type SpecifierMap = BTreeMap<String, Option<Url>>;

#[derive(Debug, Default, Clone)]
pub struct ImportMap {
    imports: SpecifierMap,
    /// Scope prefix (a URL string) to its specifier map.
    scopes: BTreeMap<String, SpecifierMap>,
}

/// "Parse a URL-like import specifier": an absolute URL, or a relative one
/// that starts with `/`, `./` or `../`.
fn url_like(specifier: &str, base: Option<&Url>) -> Option<Url> {
    if let Ok(url) = Url::parse(specifier) {
        return Some(url);
    }
    if specifier.starts_with('/') || specifier.starts_with("./") || specifier.starts_with("../") {
        return base?.join(specifier).ok();
    }
    None
}

fn parse_specifier_map(value: &serde_json::Value, base: &Url, warnings: &mut Vec<String>) -> SpecifierMap {
    let mut out = SpecifierMap::new();
    let Some(object) = value.as_object() else {
        return out;
    };
    for (key, address) in object {
        if key.is_empty() {
            warnings.push("an import map key is empty".into());
            continue;
        }
        let normalized = url_like(key, Some(base)).map(|u| u.to_string()).unwrap_or_else(|| key.clone());
        match address {
            serde_json::Value::Null => {
                out.insert(normalized, None);
            }
            serde_json::Value::String(text) => match url_like(text, Some(base)) {
                None => {
                    warnings.push(format!("the address of '{key}' is not a URL"));
                    out.insert(normalized, None);
                }
                Some(url) => {
                    if key.ends_with('/') && !url.as_str().ends_with('/') {
                        warnings.push(format!("'{key}' ends with a slash but its address does not"));
                        out.insert(normalized, None);
                    } else {
                        out.insert(normalized, Some(url));
                    }
                }
            },
            _ => {
                warnings.push(format!("the address of '{key}' is not a string"));
                out.insert(normalized, None);
            }
        }
    }
    out
}

impl ImportMap {
    /// Parse an import map document. `base` is the document URL. Problems
    /// with single entries are warnings (the entry is dropped); a document
    /// that is not a JSON object, or whose `imports` / `scopes` is not an
    /// object, is an error.
    pub fn parse(text: &str, base: &Url) -> Result<(ImportMap, Vec<String>), String> {
        let value: serde_json::Value = serde_json::from_str(text).map_err(|e| format!("the import map is not valid JSON: {e}"))?;
        let object = value.as_object().ok_or_else(|| "the import map must be a JSON object".to_string())?;
        let mut warnings = Vec::new();
        let mut map = ImportMap::default();
        if let Some(imports) = object.get("imports") {
            if !imports.is_object() {
                return Err("the 'imports' of an import map must be an object".into());
            }
            map.imports = parse_specifier_map(imports, base, &mut warnings);
        }
        if let Some(scopes) = object.get("scopes") {
            let scopes = scopes.as_object().ok_or_else(|| "the 'scopes' of an import map must be an object".to_string())?;
            for (prefix, inner) in scopes {
                if !inner.is_object() {
                    return Err(format!("the scope '{prefix}' must be an object"));
                }
                let Ok(scope_url) = base.join(prefix) else {
                    warnings.push(format!("the scope prefix '{prefix}' is not a URL"));
                    continue;
                };
                map.scopes.insert(scope_url.to_string(), parse_specifier_map(inner, base, &mut warnings));
            }
        }
        Ok((map, warnings))
    }

    /// Fold a later import map in. A key the map already has keeps its
    /// first address.
    pub fn merge(&mut self, later: ImportMap) {
        for (k, v) in later.imports {
            self.imports.entry(k).or_insert(v);
        }
        for (scope, entries) in later.scopes {
            let existing = self.scopes.entry(scope).or_default();
            for (k, v) in entries {
                existing.entry(k).or_insert(v);
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.imports.is_empty() && self.scopes.is_empty()
    }

    /// "Resolve a module specifier" (HTML §8.1.5.5): `referrer` is the URL of
    /// the importing module (or the document), `base` what a relative
    /// specifier resolves against.
    pub fn resolve(&self, specifier: &str, referrer: &Url, base: &Url) -> Result<Url, String> {
        let as_url = url_like(specifier, Some(base));
        let normalized = as_url.as_ref().map(|u| u.to_string()).unwrap_or_else(|| specifier.to_string());
        let referrer_text = referrer.as_str();
        // Scopes first: the most specific (longest) prefix of the referrer wins.
        let mut scopes: Vec<(&String, &SpecifierMap)> = self
            .scopes
            .iter()
            .filter(|(prefix, _)| prefix.as_str() == referrer_text || (prefix.ends_with('/') && referrer_text.starts_with(prefix.as_str())))
            .collect();
        scopes.sort_by(|a, b| b.0.len().cmp(&a.0.len()));
        for (_, map) in scopes {
            if let Some(found) = match_in(map, &normalized, specifier)? {
                return Ok(found);
            }
        }
        if let Some(found) = match_in(&self.imports, &normalized, specifier)? {
            return Ok(found);
        }
        match as_url {
            Some(url) => Ok(url),
            None => Err(format!(
                "Failed to resolve module specifier '{specifier}': relative references must start with \"/\", \"./\" or \"../\", or the specifier must be in an import map"
            )),
        }
    }
}

/// "Resolve an imports match": exact key, else the longest key that ends with
/// `/` and prefixes the specifier. `Ok(None)` is no match; a null address is
/// an error (the specifier is blocked).
fn match_in(map: &SpecifierMap, normalized: &str, original: &str) -> Result<Option<Url>, String> {
    if let Some(address) = map.get(normalized) {
        return address
            .clone()
            .map(Some)
            .ok_or_else(|| format!("Failed to resolve module specifier '{original}': it is blocked by the import map"));
    }
    let mut best: Option<(&String, &Option<Url>)> = None;
    for (key, address) in map {
        if key.ends_with('/') && normalized.starts_with(key.as_str()) && best.map_or(true, |(b, _)| key.len() > b.len()) {
            best = Some((key, address));
        }
    }
    match best {
        None => Ok(None),
        Some((key, address)) => {
            let base = address
                .as_ref()
                .ok_or_else(|| format!("Failed to resolve module specifier '{original}': it is blocked by the import map"))?;
            let rest = &normalized[key.len()..];
            base.join(rest)
                .map(Some)
                .map_err(|e| format!("Failed to resolve module specifier '{original}': {e}"))
        }
    }
}
