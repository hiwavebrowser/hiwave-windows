//! `document.forms["id"]` in a loaded page: the page's own script, run by the load, reads its
//! form from `document.forms` by id and by name, and adds a listener to it (what MotionMark
//! 1.3.2's start page does with its options form before it can start).
//!
//! The values are the pinned Chrome's (CfT 148.0.7778.216, headless;
//! `docs/diagnostics/2026-11-15/named_collections_cft148.json`: `forms[id]`, `forms[name]`,
//! `forms.missing`, `keys`). The bindings' own tests (`named_collections_tests`) run the whole
//! probe.

use super::*;

const PAGE: &str = r#"<!doctype html><html><head><title>options</title></head><body>
<form id="benchmark-options" name="opts">
  <label><input type="radio" name="display" value="minimal" checked> minimal</label>
  <label><input type="radio" name="display" value="bar"> bar</label>
</form>
<img id="logo" name="mark" alt="">
<script>
window.__seen = [];
try {
  var byId = document.forms["benchmark-options"];
  byId.addEventListener("change", function (e) { window.__seen.push("change:" + e.target.value); }, true);
  window.__read = [Object.prototype.toString.call(byId), byId === document.forms.opts, byId === document.forms[0],
                   String(document.forms.nope), Object.keys(document.forms).join(" "),
                   document.images.mark === document.images.logo, document.images.logo === document.querySelector("img")].join(",");
} catch (e) { window.__read = "throws " + e.name; }
</script>
</body></html>"#;

fn read(engine: &mut Engine, view: EngineViewId, expr: &str) -> String {
    let value = engine.execute_script(view, &format!("String({expr})")).unwrap();
    value.strip_prefix("String(\"").and_then(|v| v.strip_suffix("\")")).unwrap_or(&value).to_string()
}

#[test]
fn a_loaded_pages_script_reads_its_form_from_document_forms_by_id_and_by_name() {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine.create_headless_view(Bounds { x: 0, y: 0, width: 800, height: 600 }).expect("view");
    engine.load_html(view, PAGE).expect("load_html");
    assert_eq!(read(&mut engine, view, "window.__read"), "[object HTMLFormElement],true,true,undefined,0,true,true");
    // The listener the page added to that form hears a change in it.
    engine
        .execute_script(view, "document.querySelector('input[value=bar]').dispatchEvent(new Event('change', { bubbles: true }))")
        .unwrap();
    assert_eq!(read(&mut engine, view, "window.__seen.join()"), "change:bar");
}
