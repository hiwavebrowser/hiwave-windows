//! The iframe `sandbox` attribute on a frame request (HTML "sandboxing": a frame's flags are the
//! attribute's when its document is created, by a navigation; a later change of the attribute
//! waits for the next navigation).
//!
//! A frame request carries the attribute's value as it was when the page asked for the
//! navigation. It is read from the document by the request's host function, not by page script:
//! what the page has put on the element or the prototypes does not change it, and neither does a
//! change of the attribute after the request.
//!
//! A frame in the parser's markup is asked for when the engine scans the document, which is
//! after the page's first scripts. The pinned Chrome (CfT 148.0.7778.216, headless,
//! `docs/diagnostics/2026-11-15/frame_sandbox_{parent.html,child.html,page.js,chrome.mjs,cft148.json}`,
//! rows `markup.*`) starts that navigation at the parse: an attribute a script removes at once
//! still counts (`mk_removed`). So the attribute as parsed counts for a markup frame's first
//! request, with what the element has then.

use super::*;

const PIN: &str = include_str!("../../../docs/diagnostics/2026-11-15/frame_sandbox_cft148.json");

fn page(markup: &str) -> (DomBindings, Rc<Document>) {
    let b = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    b.enable_frames().unwrap();
    let doc = Rc::new(Document::parse_html(&format!("<html><head></head><body>{markup}</body></html>")).unwrap());
    b.set_document(doc.clone()).unwrap();
    (b, doc)
}

fn run(b: &DomBindings, script: &str) {
    b.evaluate(script).unwrap_or_else(|e| panic!("{script}: {e}"));
}

/// The `sandbox` of each request with a URL, in the order made.
fn asked(b: &DomBindings) -> Vec<Option<String>> {
    b.take_frame_requests().into_iter().filter(|r| r.url.is_some()).map(|r| r.sandbox).collect()
}

fn some(value: &str) -> Option<String> {
    Some(value.to_string())
}

/// Chrome's row for a frame, by group and name: was a document reachable from the page.
fn chrome_has_document(group: &str, name: &str) -> bool {
    let pin: serde_json::Value = serde_json::from_str(PIN).unwrap();
    match pin[group][name]["contentDocument"].as_str() {
        Some("object") => true,
        Some("null") => false,
        other => panic!("{group}.{name}: no contentDocument row ({other:?})"),
    }
}

#[test]
fn a_request_carries_the_attribute_value() {
    let (b, _) = page("");
    run(
        &b,
        "['', 'allow-scripts', 'allow-same-origin', 'allow-scripts allow-same-origin', null].forEach(function (v) { \
           var f = document.createElement('iframe'); if (v !== null) f.setAttribute('sandbox', v); document.body.appendChild(f); });",
    );
    assert_eq!(
        asked(&b),
        vec![some(""), some("allow-scripts"), some("allow-same-origin"), some("allow-scripts allow-same-origin"), None]
    );
}

#[test]
fn a_frame_in_the_markup_carries_it_too() {
    let (b, _) = page("<iframe sandbox></iframe><iframe></iframe><iframe sandbox='allow-forms'></iframe>");
    b.scan_frames().unwrap();
    assert_eq!(asked(&b), vec![some(""), None, some("allow-forms")]);
}

/// The value is the one at the request. Chrome (`afterLoad.removed_changed`, `added_changed`):
/// the document made before the change stays as it was.
#[test]
fn a_change_after_the_request_does_not_change_it() {
    let (b, _) = page("");
    run(
        &b,
        "var a = document.createElement('iframe'); a.setAttribute('sandbox', ''); document.body.appendChild(a); \
         var n = document.createElement('iframe'); document.body.appendChild(n); \
         a.removeAttribute('sandbox'); n.setAttribute('sandbox', '');",
    );
    assert!(!chrome_has_document("afterLoad", "removed_changed") && chrome_has_document("afterLoad", "added_changed"));
    assert_eq!(asked(&b), vec![some(""), None]);
}

/// Chrome (`afterLoad.removed_navigated`, `added_navigated`): the next navigation takes the
/// attribute as it is then.
#[test]
fn the_next_navigation_takes_the_attribute_as_it_is_then() {
    let (b, _) = page("");
    run(
        &b,
        "var a = document.createElement('iframe'); a.setAttribute('sandbox', ''); document.body.appendChild(a); \
         var n = document.createElement('iframe'); document.body.appendChild(n);",
    );
    let _ = b.take_frame_requests();
    run(&b, "a.removeAttribute('sandbox'); n.setAttribute('sandbox', 'allow-scripts'); a.src = '/x.html'; n.src = '/y.html';");
    assert!(chrome_has_document("afterLoad", "removed_navigated") && !chrome_has_document("afterLoad", "added_navigated"));
    assert_eq!(asked(&b), vec![None, some("allow-scripts")]);
}

/// The request reads the document, not the page's functions: built-ins the page replaced and
/// accessors it defined are not asked.
#[test]
fn the_attribute_is_read_from_the_document_not_through_page_script() {
    let (b, _) = page("<iframe id=m sandbox></iframe>");
    run(
        &b,
        "window.ran = 0; function no() { ran++; return null; } function never() { ran++; return false; } \
         Element.prototype.getAttribute = no; Element.prototype.hasAttribute = never; \
         Element.prototype.getAttributeNode = no; Element.prototype.getAttributeNames = function () { ran++; return []; }; \
         Object.defineProperty(HTMLIFrameElement.prototype, 'sandbox', { get: no, set: no, configurable: true }); \
         Object.defineProperty(Element.prototype, 'attributes', { get: no, configurable: true }); \
         String.prototype.toLowerCase = function () { ran++; return 'src'; }; \
         var f = document.createElement('iframe'); \
         Object.defineProperty(f, 'sandbox', { get: no, set: no }); \
         Element.prototype.setAttribute.call(f, 'sandbox', 'allow-scripts'); \
         ran = 0; document.body.appendChild(f);",
    );
    b.scan_frames().unwrap();
    assert_eq!(asked(&b), vec![some("allow-scripts"), some("")]);
    assert!(matches!(b.evaluate("ran"), Ok(JsValue::Number(n)) if n == 0.0), "page functions ran for the request");
}

/// A markup frame's first request, made at the engine's scan: the attribute as parsed counts
/// (Chrome `markup.mk_removed`: no document), and one the page set since counts (Chrome
/// `markup.mk_added_src`: set, then navigated: no document).
#[test]
fn a_markup_frames_first_request_counts_the_attribute_as_parsed() {
    let (b, _) = page("<iframe id=r sandbox='allow-forms'></iframe><iframe id=s></iframe>");
    run(
        &b,
        "document.getElementById('r').removeAttribute('sandbox'); \
         var s = document.getElementById('s'); s.setAttribute('sandbox', ''); s.src = '/x.html';",
    );
    b.scan_frames().unwrap();
    assert!(!chrome_has_document("markup", "mk_removed") && !chrome_has_document("markup", "mk_added_src"));
    // `s` asked first (its `src` was set by the script), `r` at the scan.
    assert_eq!(asked(&b), vec![some(""), some("allow-forms")]);
}

/// Only the first: once a markup frame has been asked for, the attribute as parsed is spent
/// (Chrome `afterLoad.removed_navigated`).
#[test]
fn the_attribute_as_parsed_counts_once() {
    let (b, _) = page("<iframe id=r sandbox></iframe>");
    b.scan_frames().unwrap();
    assert_eq!(asked(&b), vec![some("")]);
    run(&b, "var r = document.getElementById('r'); r.removeAttribute('sandbox'); r.src = '/x.html';");
    assert_eq!(asked(&b), vec![None]);
}

/// The same when the document was bound before frames were turned on.
#[test]
fn a_document_bound_before_frames_were_on_counts_its_markup_too() {
    let b = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    b.set_document(Rc::new(Document::parse_html("<html><head></head><body><iframe id=r sandbox></iframe></body></html>").unwrap()))
        .unwrap();
    b.enable_frames().unwrap();
    run(&b, "document.getElementById('r').removeAttribute('sandbox');");
    b.scan_frames().unwrap();
    assert_eq!(asked(&b), vec![some("")]);
}

/// Known difference, recorded: a markup frame the page's first script gives the attribute to,
/// without navigating it. Chrome (`markup.mk_added`) had started the navigation at the parse,
/// so the frame has its document; here the request is made at the scan and carries the
/// attribute.
#[test]
fn known_difference_an_attribute_set_on_a_markup_frame_before_the_scan_counts() {
    let (b, _) = page("<iframe id=s></iframe>");
    run(&b, "document.getElementById('s').setAttribute('sandbox', '');");
    b.scan_frames().unwrap();
    assert!(chrome_has_document("markup", "mk_added"), "Chrome's row changed: revisit this difference");
    assert_eq!(asked(&b), vec![some("")]);
}

/// A frame that left the document: the request names no navigation and carries nothing.
#[test]
fn a_removed_frames_request_carries_nothing() {
    let (b, _) = page("");
    run(&b, "var f = document.createElement('iframe'); f.setAttribute('sandbox', ''); document.body.appendChild(f); f.remove();");
    let requests = b.take_frame_requests();
    assert_eq!(requests.len(), 2);
    assert_eq!((requests[1].url.clone(), requests[1].sandbox.clone()), (None, None));
}

// ── Frames that come into the document as markup, as copies, or from elsewhere ──
//
// Chrome (`inserted.*`): the navigation is asked for when the frame goes into the document,
// with the attribute as it is then; taking the attribute off in the next statement changes
// nothing (`*_removed` rows are as `*_empty`).

/// Three frames by `put(id, markup)`: without the attribute, with it, and with it taken off in
/// the next statement.
const THREE: &str = "function markup(id, sandbox) { return '<iframe id=' + id + (sandbox ? ' sandbox' : '') + ' src=\"/' + id + '.html\"></iframe>'; } \
    function holder() { return document.body.appendChild(document.createElement('div')); } \
    function three(put) { [['none', false, false], ['empty', true, false], ['removed', true, true]].forEach(function (c) { \
        var f = put(c[0], markup(c[0], c[1])) || document.getElementById(c[0]); \
        if (c[2]) f.removeAttribute('sandbox'); }); }";

/// What the three frames' requests carried, and Chrome's rows for the same three.
fn three(put: &str, pinned: &str) -> Vec<Option<String>> {
    let (b, _) = page("");
    b.scan_frames().unwrap();
    run(&b, &format!("{THREE} three({put});"));
    assert!(chrome_has_document("inserted", &format!("{pinned}_none")));
    assert!(!chrome_has_document("inserted", &format!("{pinned}_empty")) && !chrome_has_document("inserted", &format!("{pinned}_removed")));
    asked(&b)
}

fn none_empty_empty() -> Vec<Option<String>> {
    vec![None, some(""), some("")]
}

#[test]
fn a_frame_set_as_inner_html_is_asked_for_with_its_attribute() {
    assert_eq!(three("function (id, m) { holder().innerHTML = m; }", "inner"), none_empty_empty());
}

#[test]
fn a_frame_inserted_as_adjacent_html_is_asked_for_with_its_attribute() {
    assert_eq!(three("function (id, m) { holder().insertAdjacentHTML('beforeend', m); }", "adjacent"), none_empty_empty());
}

#[test]
fn a_frame_set_as_outer_html_is_asked_for_with_its_attribute() {
    assert_eq!(
        three("function (id, m) { holder().appendChild(document.createElement('p')).outerHTML = m; }", "outer"),
        none_empty_empty()
    );
}

#[test]
fn a_frame_inserted_through_a_fragment_is_asked_for_with_its_attribute() {
    assert_eq!(
        three("function (id, m) { var t = document.createElement('template'); t.innerHTML = m; holder().appendChild(t.content); }", "fragment"),
        none_empty_empty()
    );
}

/// Made in a subtree outside the document: asked for when the subtree goes in.
#[test]
fn a_frame_is_asked_for_when_its_subtree_goes_into_the_document() {
    assert_eq!(
        three("function (id, m) { var d = document.createElement('div'); d.innerHTML = m; document.body.appendChild(d); }", "connected"),
        none_empty_empty()
    );
    // The attribute taken off before the subtree goes in: Chrome `inserted.connected_before`.
    let (b, _) = page("");
    run(
        &b,
        "var d = document.createElement('div'); d.innerHTML = '<iframe sandbox src=/x.html></iframe>'; \
         window.before = 0; d.firstChild.removeAttribute('sandbox');",
    );
    assert_eq!(b.take_frame_requests(), vec![], "asked for before it was in the document");
    run(&b, "document.body.appendChild(d);");
    assert!(chrome_has_document("inserted", "connected_before"));
    assert_eq!(asked(&b), vec![None]);
}

#[test]
fn a_deep_copy_of_a_frame_is_asked_for_with_the_copys_attribute() {
    assert_eq!(
        three(
            "function (id, m) { var d = document.createElement('div'); d.innerHTML = m; \
               var c = d.cloneNode(true); document.body.appendChild(c); return c.firstChild; }",
            "copy"
        ),
        none_empty_empty()
    );
}

#[test]
fn a_frame_adopted_from_another_document_is_asked_for_with_its_attribute() {
    assert_eq!(
        three(
            "function (id, m) { var other = document.implementation.createHTMLDocument(''); other.body.innerHTML = m; \
               var f = document.adoptNode(other.body.firstChild); document.body.appendChild(f); return f; }",
            "adopted"
        ),
        none_empty_empty()
    );
}

/// Markup set before the engine's first scan is asked for when set, not at the scan.
#[test]
fn a_frame_set_as_inner_html_before_the_scan_is_asked_for_then() {
    let (b, _) = page("<div id=p></div>");
    run(&b, "document.getElementById('p').innerHTML = '<iframe id=f sandbox src=/x.html></iframe>'; document.getElementById('f').removeAttribute('sandbox');");
    b.scan_frames().unwrap();
    assert_eq!(asked(&b), vec![some("")]);
}

/// A frame that markup replaced has left the document: its request says so.
#[test]
fn a_frame_replaced_by_inner_html_has_left() {
    let (b, doc) = page("<div id=p><iframe id=f></iframe></div>");
    b.scan_frames().unwrap();
    let node = doc.get_element_by_id("f").unwrap().id.raw();
    let _ = b.take_frame_requests();
    run(&b, "document.getElementById('p').innerHTML = '<b>x</b>';");
    assert_eq!(b.take_frame_requests(), vec![FrameRequest { node, url: None, sandbox: None, initial: false, quiet: false }]);
}

/// A request that was not kept (too many are parked) does not spend the attribute as parsed.
#[test]
fn a_request_that_was_not_kept_does_not_spend_the_attribute_as_parsed() {
    let (b, _) = page("<iframe id=r sandbox></iframe>");
    run(
        &b,
        "for (var i = 0; i < 256; i++) document.body.appendChild(document.createElement('iframe')); \
         document.getElementById('r').removeAttribute('sandbox');",
    );
    b.scan_frames().unwrap();
    assert_eq!(b.take_frame_requests().len(), 256);
    run(&b, "document.getElementById('r').src = '/x.html';");
    assert_eq!(asked(&b), vec![some("")]);
}

/// Known difference, recorded: `document.write` inserts nothing in this engine yet, so a frame
/// written that way does not exist and nothing is asked for. Chrome (`inserted.write_*`) makes
/// the frame and treats its attribute as for any other.
#[test]
fn known_difference_document_write_makes_no_frame() {
    let (b, _) = page("");
    b.scan_frames().unwrap();
    run(&b, "document.write('<iframe id=w sandbox src=/x.html></iframe>'); window.made = String(document.getElementById('w'));");
    assert!(chrome_has_document("inserted", "write_none") && !chrome_has_document("inserted", "write_empty"));
    assert!(matches!(b.evaluate("made"), Ok(JsValue::String(s)) if s == "null"), "document.write made an element: ask for its frame");
    assert_eq!(b.take_frame_requests(), vec![]);
}
