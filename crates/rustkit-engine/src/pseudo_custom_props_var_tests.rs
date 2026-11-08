//! CH2.E14: `var()` on `::before` / `::after`. The pseudo-element cascade
//! applied its declarations as written, so `background: var(--c)` reached
//! the value parser as the text `var(--c)` and was dropped, and the pseudo
//! carried no custom properties at all. chrome.html styles its parts with
//! custom properties, so every `::before`/`::after` there lost its colours
//! (#927 had to make the page band a real element). A pseudo-element's
//! parent is its originating element (CSS Pseudo 4 §4): it inherits that
//! element's custom properties, declares its own, and substitutes `var()`
//! at computed-value time with fallbacks; a reference with neither a value
//! nor a fallback makes the declaration invalid at computed-value time,
//! which is `unset` (CSS Variables 1 §3).
//!
//! Every expected value is Chromium 143.0.7499.4's on this page, from
//! `getComputedStyle(el, '::before')`.

use super::*;
use rustkit_layout::{Dimensions, Rect};

const PAGE: &str = r#"<!doctype html><style>
:root{--root:rgb(0,128,0)}
body{margin:0;width:400px}
#outer{--anc:rgb(255,0,0);--w:15px;color:rgb(1,2,3)}
#a{--t:"hi";font:16px monospace}
#a::before{content:"";display:block;height:10px;width:20px;background:var(--root)}
#b{--t:"hi"}
#b::before{content:var(--t);font:16px monospace}
#c::before{content:"";display:block;height:5px;width:calc(var(--w) * 2)}
#d::before{content:"";display:block;height:5px;width:var(--missing, 40px);background-color:var(--missing, rgb(0,0,255))}
#e::before{content:"";display:block;height:5px;width:50px;width:var(--missing);background-color:red;background-color:var(--missing);color:red;color:var(--missing)}
#f::before{content:"";display:block;height:5px;width:20px;background:var(--anc)}
#g::before{--c:rgb(9,9,9);content:"";display:block;height:5px;width:20px;background-color:var(--c)}
</style><body><div id=outer><div id=a></div><div id=b></div><div id=c></div><div id=d></div><div id=e></div><div id=f></div><div id=g></div></div></body>"#;

fn laid_out() -> LayoutBox {
    let e = Engine::new(EngineConfig::default()).expect("engine");
    let d = Document::parse_html(PAGE).expect("parse");
    let cb = Dimensions {
        content: Rect::new(0.0, 0.0, 800.0, 0.0),
        ..Default::default()
    };
    let mut root = e.build_layout_from_document(&d, &[]);
    root.layout_with_collapse(
        &cb,
        &mut rustkit_layout::MarginCollapseContext::new(),
        &mut rustkit_layout::FloatContext::new(),
    );
    root
}

fn by_id<'a>(b: &'a LayoutBox, id: &str) -> Option<&'a LayoutBox> {
    if b.identity.as_ref().is_some_and(|i| i.selector == format!("#{id}")) {
        return Some(b);
    }
    b.children.iter().find_map(|c| by_id(c, id))
}

/// The `::before` box of `#id`: its element has no other content.
fn before<'a>(root: &'a LayoutBox, id: &str) -> &'a LayoutBox {
    let el = by_id(root, id).unwrap_or_else(|| panic!("no box #{id}"));
    el.children.first().unwrap_or_else(|| panic!("#{id} has no ::before box"))
}

fn rgb(r: u8, g: u8, b: u8) -> rustkit_css::Color {
    rustkit_css::Color::new(r, g, b, 1.0)
}

const TRANSPARENT: rustkit_css::Color = rustkit_css::Color::TRANSPARENT;

#[test]
fn a_pseudo_paints_a_root_custom_property() {
    let root = laid_out();
    assert_eq!(before(&root, "a").style.background_color, rgb(0, 128, 0));
}

#[test]
fn a_pseudo_reads_a_custom_property_of_a_distant_ancestor() {
    let root = laid_out();
    assert_eq!(before(&root, "f").style.background_color, rgb(255, 0, 0));
}

#[test]
fn a_pseudo_declares_and_uses_its_own_custom_property() {
    let root = laid_out();
    assert_eq!(before(&root, "g").style.background_color, rgb(9, 9, 9));
}

#[test]
fn content_takes_a_string_from_a_custom_property() {
    let root = laid_out();
    let b = before(&root, "b");
    assert_eq!(b.style.content.as_deref(), Some("hi"));
}

#[test]
fn var_inside_calc_resolves_on_a_pseudo() {
    let root = laid_out();
    assert_eq!(before(&root, "c").dimensions.content.width, 30.0);
}

#[test]
fn a_missing_variable_takes_its_fallback_on_a_pseudo() {
    let root = laid_out();
    let d = before(&root, "d");
    assert_eq!(d.dimensions.content.width, 40.0);
    assert_eq!(d.style.background_color, rgb(0, 0, 255));
}

/// `width: 50px; width: var(--missing)` is `width: unset` = `auto` (400px
/// in a 400px body), not 50px; `background-color` goes back to transparent,
/// and the inherited `color` to the element's rgb(1,2,3), not red.
#[test]
fn a_missing_variable_without_fallback_is_unset_on_a_pseudo() {
    let root = laid_out();
    let e = before(&root, "e");
    assert_eq!(e.dimensions.content.width, 400.0);
    assert_eq!(e.style.background_color, TRANSPARENT);
    assert_eq!(e.style.color, rgb(1, 2, 3));
}

fn loaded() -> (Engine, EngineViewId) {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let id = engine
        .create_headless_view(Bounds::new(0, 0, 400, 300))
        .expect("headless view");
    engine.load_html(id, PAGE).expect("load_html");
    (engine, id)
}

fn before_read(engine: &mut Engine, id: EngineViewId, element: &str, property: &str) -> String {
    let script = format!(
        "getComputedStyle(document.getElementById('{element}'), '::before').getPropertyValue('{property}')"
    );
    let out = engine
        .execute_script(id, &format!("String(eval({script:?}))"))
        .expect("script");
    out.strip_prefix("String(\"")
        .and_then(|s| s.strip_suffix("\")"))
        .unwrap_or_else(|| panic!("{script} evaluated to {out}"))
        .replace("\\\"", "\"")
}

#[test]
fn get_computed_style_of_a_pseudo_reads_its_custom_properties_and_var_results() {
    let (mut e, id) = loaded();
    let cases = [
        ("a", "background-color", "rgb(0, 128, 0)"),
        ("a", "--root", "rgb(0,128,0)"),
        ("a", "--anc", "rgb(255,0,0)"),
        ("f", "background-color", "rgb(255, 0, 0)"),
        ("d", "background-color", "rgb(0, 0, 255)"),
        ("e", "background-color", "rgba(0, 0, 0, 0)"),
        ("e", "color", "rgb(1, 2, 3)"),
        ("b", "content", "\"hi\""),
        // The pseudo's own custom property, which its element does not have.
        ("g", "--c", "rgb(9,9,9)"),
        ("g", "background-color", "rgb(9, 9, 9)"),
        ("a", "--c", ""),
    ];
    let mut wrong = Vec::new();
    for (el, prop, chrome) in cases {
        let got = before_read(&mut e, id, el, prop);
        if got != chrome {
            wrong.push(format!("#{el}::before {prop}: chrome {chrome:?}, engine {got:?}"));
        }
    }
    assert!(wrong.is_empty(), "{} reads differ from Chrome:\n{}", wrong.len(), wrong.join("\n"));
}
