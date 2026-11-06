//! HTML drag and drop's script side (web_dnd.js, dom.rs `__rkFireDrag`):
//! the DataTransfer and its drag data store's protection modes, and the
//! DragEvent the engine's processing model fires.

use super::*;

const PAGE: &str = r#"<!DOCTYPE html><html><body>
<ul><li id="src" draggable="true">drag me</li></ul><div id="zone">zone</div>
<a id="link" href="https://example.test/page">link</a></body></html>"#;

fn bound() -> (DomBindings, Rc<Document>) {
    let bindings = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    let doc = Rc::new(Document::parse_html(PAGE).unwrap());
    bindings.set_document(doc.clone()).unwrap();
    (bindings, doc)
}

fn ev(bindings: &DomBindings, script: &str) -> String {
    match bindings.evaluate(script).unwrap() {
        JsValue::String(s) => s,
        JsValue::Boolean(b) => b.to_string(),
        JsValue::Number(n) => format!("{n}"),
        other => format!("{other:?}"),
    }
}

fn node(doc: &Document, id: &str) -> usize {
    doc.get_element_by_id(id)
        .unwrap_or_else(|| panic!("no #{id}"))
        .id
        .raw()
}

#[test]
fn a_constructed_data_transfer_holds_string_data() {
    let (b, doc) = bound();
    assert_eq!(
        ev(
            &b,
            "var t = new DataTransfer(); t.setData('text/plain', 'x'); t.setData('Text', 'y'); \
             t.setData('text/uri-list', '#c\\nhttps://a.test/\\nhttps://b.test/'); \
             [t.getData('text/plain'), t.getData('URL'), t.types.join(','), t.items.length, \
              t.items[0].kind, t.items[0].type, t.effectAllowed, t.dropEffect, \
              Object.prototype.toString.call(t), t.files.length].join('|')"
        ),
        "y|https://a.test/|text/plain,text/uri-list|2|string|text/plain|none|none|[object DataTransfer]|0"
    );
    assert_eq!(
        ev(
            &b,
            "var t = new DataTransfer(); t.setData('a', '1'); t.setData('b', '2'); t.clearData('a'); \
             var one = t.types.join(','); t.items.add('3', 'C'); var dup; \
             try { t.items.add('4', 'c'); dup = 'no'; } catch (e) { dup = e.name; } \
             t.dropEffect = 'bogus'; t.dropEffect = 'move'; t.effectAllowed = 'copyMove'; t.setDragImage(null, 0, 0); \
             var two = t.types.join(','); t.clearData(); \
             [one, two, dup, t.dropEffect, t.effectAllowed, t.types.length].join('|')"
        ),
        "b|b,c|NotSupportedError|move|copyMove|0"
    );
}

#[test]
fn a_drag_event_is_a_mouse_event_with_the_drags_data_transfer() {
    let (b, doc) = bound();
    ev(
        &b,
        "window.log = []; var src = document.getElementById('src'), zone = document.getElementById('zone'); \
         src.addEventListener('dragstart', function (e) { \
           e.dataTransfer.setData('text/plain', 'payload'); e.dataTransfer.effectAllowed = 'move'; \
           log.push(['start', e instanceof DragEvent, e instanceof MouseEvent, e.isTrusted, e.bubbles, e.cancelable, \
             e.dataTransfer instanceof DataTransfer, e.clientX].join(':')); }); \
         zone.addEventListener('dragover', function (e) { \
           e.dataTransfer.setData('text/plain', 'overwritten'); \
           log.push(['over', e.dataTransfer.getData('text/plain'), e.dataTransfer.types.join(','), \
             e.dataTransfer.dropEffect, e.dataTransfer.effectAllowed].join(':')); \
           e.dataTransfer.effectAllowed = 'copy'; e.preventDefault(); }); \
         zone.addEventListener('drop', function (e) { window.kept = e.dataTransfer; \
           log.push(['drop', e.dataTransfer.getData('text/plain'), e.dataTransfer.dropEffect].join(':')); \
           e.preventDefault(); }); \
         src.addEventListener('dragend', function (e) { \
           log.push(['end', e.cancelable, e.dataTransfer.dropEffect, e.dataTransfer.getData('text/plain')].join(':')); }); ''",
    );
    let (src, zone) = (node(&doc, "src"), node(&doc, "zone"));
    let at = MouseEventBindingData {
        client_x: 5.0,
        client_y: 6.0,
        buttons: 1,
        ..Default::default()
    };
    assert_eq!(
        b.fire_drag_event(src, "dragstart", &at, "none").unwrap(),
        (true, "none".to_string())
    );
    // Cancelled dragover: the operation its dropEffect gives ('move',
    // from effectAllowed 'move'); the store is protected there.
    assert_eq!(
        b.fire_drag_event(zone, "dragover", &at, "none").unwrap(),
        (false, "move".to_string())
    );
    assert_eq!(
        b.fire_drag_event(zone, "drop", &at, "move").unwrap(),
        (false, "move".to_string())
    );
    assert_eq!(
        b.fire_drag_event(src, "dragend", &at, "move").unwrap(),
        (true, "move".to_string())
    );
    assert_eq!(
        ev(&b, "log.join(' ')"),
        "start:true:true:true:true:true:true:5 over::text/plain:move:move drop:payload:move end:false:move:"
    );
    // After the drag, the store is gone: what a listener kept reads nothing.
    assert_eq!(
        ev(
            &b,
            "[kept.types.length, kept.getData('text/plain'), kept.items.length].join('|')"
        ),
        "0||0"
    );
}

#[test]
fn a_link_drag_carries_its_url_and_a_cancelled_dragstart_ends_the_store() {
    let (b, doc) = bound();
    ev(
        &b,
        "window.log = []; document.addEventListener('dragstart', function (e) { \
           log.push(e.dataTransfer.getData('text/uri-list') + '|' + e.dataTransfer.effectAllowed); window.dt = e.dataTransfer; \
           if (e.target.id === 'src') e.preventDefault(); }); ''",
    );
    let at = MouseEventBindingData::default();
    let link = node(&doc, "link");
    assert_eq!(
        b.fire_drag_event(link, "dragstart", &at, "none").unwrap().0,
        true
    );
    assert_eq!(
        ev(&b, "log.join(' ')"),
        "https://example.test/page|uninitialized"
    );
    let src = node(&doc, "src");
    assert_eq!(
        b.fire_drag_event(src, "dragstart", &at, "none").unwrap().0,
        false
    );
    assert_eq!(ev(&b, "String(dt.types.length)"), "0");
}
