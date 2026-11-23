//! Registered custom properties in the cascade (TRANS-F, CSS Properties
//! and Values API Level 1 §2.4, §3).
//!
//! The document's registry (`@property` rules of its sheets, then
//! `CSS.registerProperty()` ones) is made with the prepared sheets and
//! installed for the build (`RegistryScope`). Each element's custom
//! properties then get, for every registered name:
//! - `inherits: false`: the initial value unless the element declares it;
//! - `inherits: true`: the parent's value, the initial value at the root;
//! - a declared value that does not match the syntax (or a reference cycle)
//!   is invalid at computed-value time: as `unset`, the inherited or the
//!   initial value;
//! - a declared value that matches is kept for the element's own `var()`s,
//!   and stored computed (`rgb()`, px, ...) once the element's font size is
//!   known (`compute_own`), so its children inherit and script reads the
//!   computed value.
//!
//! A registered property's running transition overrides the element's
//! value (`OverrideScope`), and the engine cascades the element again
//! with it so that what reads it (`background-image: linear-gradient(
//! var(--from), ...)`) moves too.

use rustkit_css::registered_property::ComputeContext;
use rustkit_css::{CustomProperties, PropertyRegistry};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

thread_local! {
    static REGISTRY: RefCell<Option<Arc<PropertyRegistry>>> = const { RefCell::new(None) };
    static OVERRIDE: RefCell<Option<HashMap<String, String>>> = const { RefCell::new(None) };
}

/// The registry of the build in progress, `None` when it has none.
pub(crate) fn current() -> Option<Arc<PropertyRegistry>> {
    REGISTRY.with(|r| r.borrow().clone())
}

/// Installs a build's registry until dropped (and puts back what was).
pub(crate) struct RegistryScope(Option<Arc<PropertyRegistry>>);

impl RegistryScope {
    pub(crate) fn install(registry: Option<Arc<PropertyRegistry>>) -> Self {
        RegistryScope(REGISTRY.with(|r| r.replace(registry)))
    }
}

impl Drop for RegistryScope {
    fn drop(&mut self) {
        let prev = self.0.take();
        REGISTRY.with(|r| *r.borrow_mut() = prev);
    }
}

/// An element's animated registered values, over its cascade, until
/// dropped.
pub(crate) struct OverrideScope(Option<HashMap<String, String>>);

impl OverrideScope {
    pub(crate) fn install(values: HashMap<String, String>) -> Self {
        OverrideScope(OVERRIDE.with(|o| o.replace(Some(values))))
    }
}

impl Drop for OverrideScope {
    fn drop(&mut self) {
        let prev = self.0.take();
        OVERRIDE.with(|o| *o.borrow_mut() = prev);
    }
}

/// Whether an override is installed: the cascade in progress is not the
/// element's plain one (no style sharing).
pub(crate) fn override_active() -> bool {
    OVERRIDE.with(|o| o.borrow().is_some())
}

/// The installed override's values (empty when none is installed).
pub(crate) fn override_values() -> Vec<(String, String)> {
    OVERRIDE.with(|o| {
        o.borrow()
            .as_ref()
            .map(|o| o.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default()
    })
}

/// The registry made from a document's prepared sheets (`None` when it
/// registers nothing).
pub(crate) fn registry_for(sheets: &[rustkit_css::Stylesheet]) -> Option<Arc<PropertyRegistry>> {
    let registry = PropertyRegistry::from_sheets(sheets);
    (!registry.is_empty()).then(|| Arc::new(registry))
}

/// The element's custom properties with the registrations applied (see
/// the module comment), and an installed override over them. `vars` is
/// the result of `element_custom_properties`; `inherited` the parent's set
/// (or the root's map); `declared` the element's `--*` declarations in
/// cascade order (`None` is `initial`).
pub(crate) fn fix_up(
    registry: Option<&PropertyRegistry>,
    vars: Arc<CustomProperties>,
    inherited: &Arc<CustomProperties>,
    declared: &[(&str, Option<&str>)],
    is_root: bool,
) -> Arc<CustomProperties> {
    let mut layer: HashMap<String, Option<String>> = HashMap::new();
    if let Some(registry) = registry {
        for reg in registry.iter() {
            let name = reg.name.as_str();
            let unset = || {
                if reg.inherits && !is_root {
                    inherited
                        .get(name)
                        .map(str::to_string)
                        .or_else(|| reg.initial.clone())
                } else {
                    reg.initial.clone()
                }
            };
            let want = match declared
                .iter()
                .rev()
                .find(|(n, _)| *n == name)
                .map(|(_, v)| *v)
            {
                Some(None) => reg.initial.clone(),
                Some(Some(raw)) => match raw.trim().to_ascii_lowercase().as_str() {
                    "inherit" => inherited
                        .get(name)
                        .map(str::to_string)
                        .or_else(|| reg.initial.clone()),
                    "unset" | "revert" | "revert-layer" => unset(),
                    "initial" => reg.initial.clone(),
                    _ => match vars.get(name) {
                        Some(v) if reg.syntax.matches(v) => Some(v.to_string()),
                        _ => unset(),
                    },
                },
                None => unset(),
            };
            if vars.get(name) != want.as_deref() {
                layer.insert(name.to_string(), want);
            }
        }
    }
    OVERRIDE.with(|o| {
        if let Some(values) = o.borrow().as_ref() {
            for (k, v) in values {
                layer.insert(k.clone(), Some(v.clone()));
            }
        }
    });
    if layer.is_empty() {
        vars
    } else {
        Arc::new(CustomProperties::over(&vars, layer))
    }
}

/// The element's declared registered values, computed (`red` to
/// `rgb(255, 0, 0)`, `1em` to px against `font_px`). An overridden value
/// is computed already.
pub(crate) fn compute_own(
    registry: Option<&PropertyRegistry>,
    vars: Arc<CustomProperties>,
    declared: &[(&str, Option<&str>)],
    font_px: f32,
    viewport: Option<(f32, f32)>,
) -> Arc<CustomProperties> {
    let Some(registry) = registry else {
        return vars;
    };
    let ctx = ComputeContext {
        font_px,
        root_font_px: rustkit_css::root_font_px(),
        viewport: viewport.unwrap_or((0.0, 0.0)),
        independent: false,
    };
    let mut layer: HashMap<String, Option<String>> = HashMap::new();
    OVERRIDE.with(|o| {
        let o = o.borrow();
        for (name, _) in declared {
            if layer.contains_key(*name) || o.as_ref().is_some_and(|o| o.contains_key(*name)) {
                continue;
            }
            let Some(reg) = registry.get(name) else {
                continue;
            };
            let Some(value) = vars.get(name) else {
                continue;
            };
            if let Some(typed) = reg.syntax.compute(value, &ctx) {
                let css = typed.to_css();
                if css != value {
                    layer.insert(name.to_string(), Some(css));
                }
            }
        }
    });
    if layer.is_empty() {
        vars
    } else {
        Arc::new(CustomProperties::over(&vars, layer))
    }
}

/// The font size `em` resolves against on an element whose cascaded
/// `font-size` is `font_size`, its parent's being `parent_px`.
pub(crate) fn font_px(font_size: &rustkit_css::Length, parent_px: f32) -> f32 {
    use rustkit_css::Length;
    match font_size {
        Length::Px(px) => *px,
        Length::Em(em) => em * parent_px,
        Length::Percent(p) => p / 100.0 * parent_px,
        Length::Rem(r) => r * rustkit_css::root_font_px(),
        Length::Zero => 0.0,
        _ => parent_px,
    }
}
