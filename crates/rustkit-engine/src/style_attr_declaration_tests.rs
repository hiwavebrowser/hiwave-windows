//! Z plan CH2.E27: a `style` attribute is a declaration list (CSS Syntax 3
//! §5.3.10), so a `;` inside `url(…)`, a quoted string, a `()` block or a
//! comment does not end a declaration. The engine split the attribute at every
//! `;`, which cut `url(data:image/svg+xml;base64,…)` at its `;base64,` and
//! dropped every inline data-URL background or mask.
//!
//! Expected values are Chromium's answers on the same pages, written into
//! `tools/parity_oracle/style_attr_declaration_cases.json` by
//! `declaration_list_log.mjs --write`; none is typed by hand.

use super::*;

#[cfg(all(target_os = "macos", feature = "headless"))]
const CASES: &str = include_str!("../../../tools/parity_oracle/style_attr_declaration_cases.json");

#[cfg(all(target_os = "macos", feature = "headless"))]
fn eval(engine: &mut Engine, id: EngineViewId, script: &str) -> String {
    let value = engine.execute_script(id, script).expect("script");
    let inner = value
        .strip_prefix("String(\"")
        .and_then(|v| v.strip_suffix("\")"))
        .unwrap_or(&value);
    inner.replace("\\\"", "\"").replace("\\\\", "\\")
}

/// The cascade itself, with no script in the way: `getComputedStyle` lays the
/// inline text over the engine's rows, so the case table alone could pass on
/// a JS fix while paint still saw a cut declaration. Portable: no view.
#[test]
fn an_inline_data_url_reaches_the_computed_style() {
    let engine = Engine::new(EngineConfig::default()).expect("engine");
    let url = "data:image/svg+xml;base64,PHN2Zy8+";
    let attr = format!("background-image:url({url});mask-image:url('{url}');color:rgb(0, 128, 0)");
    let mut style = ComputedStyle::default();
    let vars: HashMap<String, String> = HashMap::new();
    engine.lctx.apply_inline_style(&mut style, None, &attr, &vars, &mut ChPending::default(), false);
    let lines = computed_style_lines(&LayoutBox::new(rustkit_layout::BoxType::Block, style));
    let row = |name: &str| {
        lines
            .lines()
            .find_map(|l| l.strip_prefix(name).and_then(|v| v.strip_prefix('\t')))
            .unwrap_or_else(|| panic!("no {name} row"))
            .to_string()
    };
    let want = format!("url(\"{url}\")");
    assert_eq!(row("background-image"), want);
    assert_eq!(row("mask-image"), want);
    assert_eq!(row("color"), "rgb(0, 128, 0)");
}

/// #972 R1: an unquoted url holding a quote or a `(` is a bad url. It ends
/// at its `)` (the next declaration survives) and the declaration is
/// dropped, as Chromium drops it: no background layer.
#[test]
fn an_inline_bad_url_is_dropped_and_the_next_declaration_applies() {
    let engine = Engine::new(EngineConfig::default()).expect("engine");
    for attr in [
        "background-image:url(data:image/svg+xml;utf8,it's);color:rgb(0, 128, 0)",
        "background-image:url(a(b.png);color:rgb(0, 128, 0)",
        "background:url(a(b.png) red;color:rgb(0, 128, 0)",
    ] {
        let mut style = ComputedStyle::default();
        let vars: HashMap<String, String> = HashMap::new();
        engine.lctx.apply_inline_style(&mut style, None, attr, &vars, &mut ChPending::default(), false);
        assert!(style.background_layers.is_empty(), "{attr}");
        let lines = computed_style_lines(&LayoutBox::new(rustkit_layout::BoxType::Block, style));
        assert!(lines.contains("\ncolor\trgb(0, 128, 0)"), "{attr}");
    }
}

/// CH2.E29: a rejected `background-image` leaves the image the cascade
/// already had (a stylesheet's, or an earlier declaration's), as Chromium
/// keeps it. It used to clear the layers: a bad inline url wiped the sheet's
/// image. The shorthand already dropped a bad url whole.
#[test]
fn a_rejected_background_image_keeps_the_earlier_image() {
    let engine = Engine::new(EngineConfig::default()).expect("engine");
    let url = "data:image/svg+xml;base64,PHN2Zy8+";
    for bad in ["url(a(b.png)", "url(it's)", "URL(a b.png)"] {
        for property in ["background-image", "background"] {
            let attr = format!("background-image:url({url});{property}:{bad}");
            let mut style = ComputedStyle::default();
            let vars: HashMap<String, String> = HashMap::new();
            engine.lctx.apply_inline_style(&mut style, None, &attr, &vars, &mut ChPending::default(), false);
            let lines = computed_style_lines(&LayoutBox::new(rustkit_layout::BoxType::Block, style));
            assert!(lines.contains(&format!("\nbackground-image\turl(\"{url}\")")), "{attr}");
        }
    }
}

#[cfg(all(target_os = "macos", feature = "headless"))]
#[test]
fn style_attribute_declarations_read_as_chromium() {
    let doc: serde_json::Value = serde_json::from_str(CASES).expect("cases");
    assert_eq!(doc["oracle"], "148.0.7778.216");
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let mut wrong = Vec::new();
    for c in doc["cases"].as_array().expect("cases") {
        let s = |k: &str| c[k].as_str().unwrap_or("").to_string();
        let id = engine.create_headless_view(Bounds::new(0, 0, 400, 300)).expect("view");
        engine.load_html(id, &s("html")).expect("load");
        let read = eval(&mut engine, id, &s("expr"));
        // A `gap` case reads wrong for a reason outside this rule, stated in
        // the case; its LAST field (after ` | `) must still match, and the
        // whole read must still differ, so the gap cannot go stale.
        let ok = match c["gap"].as_str() {
            Some(_) => {
                read != s("chrome")
                    && read.rsplit(" | ").next() == s("chrome").rsplit(" | ").next()
            }
            None => read == s("chrome"),
        };
        if !ok {
            wrong.push(format!("{}: read {read:?}, Chromium {:?}", s("name"), s("chrome")));
        }
        let _ = engine.destroy_view(id);
    }
    assert!(wrong.is_empty(), "{} wrong\n{}", wrong.len(), wrong.join("\n"));
}
