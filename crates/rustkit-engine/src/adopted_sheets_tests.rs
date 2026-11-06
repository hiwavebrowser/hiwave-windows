//! Constructed stylesheets adopted by the document or a shadow root
//! (`adoptedStyleSheets`) enter the cascade, and Lit's `<?lit$…$>` template
//! markers survive the tokenizer. Lit adopts its styles this way and never
//! injects a `<style>`, so a Lit component's `:host` display was lost.
//! A small hand-made page, not Lit itself (re-cut of e1d795a4, Atlas
//! 2026-10-23).

use super::*;

fn read(engine: &mut Engine, view: EngineViewId, expr: &str) -> String {
    let value = engine.execute_script(view, &format!("String({expr})")).unwrap();
    value
        .strip_prefix("String(\"")
        .and_then(|v| v.strip_suffix("\")"))
        .unwrap_or(&value)
        .to_string()
}

const PAGE: &str = "<html><head><style>body{margin:0} x-card::part(label){height:11px}</style></head><body>\
    <div id=d style='height:10px'></div><x-card id=card></x-card></body></html>";

const SETUP: &str = "
    customElements.define('x-card', class extends HTMLElement {
      constructor() {
        super();
        var root = this.attachShadow({ mode: 'open' });
        root.innerHTML = '<span class=in part=label></span>';
        var sheet = new CSSStyleSheet();
        sheet.replaceSync(':host { display: inline-block; width: 50px; height: 30px } .in { display: block; width: 20px; height: 7px }');
        root.adoptedStyleSheets = [sheet];
      }
    });
    window.__doc = new CSSStyleSheet();
    window.__doc.replaceSync('#d { width: 123px }');
    var off = new CSSStyleSheet();
    off.replaceSync('#d { width: 999px }');
    off.disabled = true;
    document.adoptedStyleSheets = [window.__doc, off];
";

fn page() -> (Engine, EngineViewId) {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 800, height: 600 })
        .expect("view");
    engine.load_html(view, PAGE).expect("load_html");
    engine.execute_script(view, SETUP).expect("setup");
    (engine, view)
}

#[test]
fn a_documents_adopted_sheet_is_applied_and_follows_replace_sync() {
    let (mut engine, view) = page();
    assert_eq!(read(&mut engine, view, "document.getElementById('d').offsetWidth"), "123");
    // A later edit of an adopted sheet republishes it; a disabled sheet
    // adopted beside it stays out of the cascade.
    engine.execute_script(view, "window.__doc.replaceSync('#d { width: 77px }')").unwrap();
    assert_eq!(read(&mut engine, view, "document.getElementById('d').offsetWidth"), "77");
}

#[test]
fn a_shadow_roots_adopted_sheet_styles_its_host_and_tree() {
    let (mut engine, view) = page();
    let card = "document.getElementById('card')";
    assert_eq!(read(&mut engine, view, &format!("{card}.offsetWidth")), "50", ":host display and width");
    assert_eq!(read(&mut engine, view, &format!("{card}.offsetHeight")), "30");
    let inner = format!("{card}.shadowRoot.querySelector('.in')");
    assert_eq!(read(&mut engine, view, &format!("{inner}.offsetWidth")), "20", "the tree's own rules");
    // Named gap: `::part` is not understood yet, so the document's
    // `x-card::part(label) { height: 11px }` does not reach the span.
    assert_eq!(read(&mut engine, view, &format!("{inner}.offsetHeight")), "7");
}

#[test]
fn lit_template_markers_are_comments_that_keep_the_question_mark() {
    // HTML tokenizer: `<?` is a bogus comment and the `?` is its first
    // character. Lit finds its parts by matching the data `?lit$…$`.
    let (mut engine, view) = page();
    engine
        .execute_script(
            view,
            "var t = document.createElement('template');\
             t.innerHTML = '<?lit$1$><slot name=\"title\"></slot>';\
             window.__marker = t.content.firstChild;\
             window.__slot = t.content.childNodes[1];",
        )
        .unwrap();
    assert_eq!(read(&mut engine, view, "window.__marker.nodeType"), "8");
    assert_eq!(read(&mut engine, view, "window.__marker.data"), "?lit$1$");
    assert_eq!(read(&mut engine, view, "window.__slot.getAttribute('name')"), "title");
}
