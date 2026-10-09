//! Document members pages read without feature-testing (web_document.js) and
//! DOMMatrix (web_dommatrix.js).

use super::*;

const PAGE: &str = "<!DOCTYPE html><html><body>\
    <form id=f1 name=signup></form><form id=f2></form>\
    <img id=i1 src=a.png><img id=i2 src=b.png>\
    <a id=l1 href=/x name=top>x</a><a id=l2>no href</a><area id=ar href=/y>\
    <input name=q id=q><span name=q id=s></span>\
    <script id=s1></script></body></html>";

fn bound() -> DomBindings {
    let b = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    b.set_document(Rc::new(Document::parse_html(PAGE).unwrap())).unwrap();
    b.set_selector_matcher(Rc::new(|node, selector, _| {
        Some(selector.split(',').any(|s| node.tag_name() == Some(s.trim())))
    }));
    b.set_location(&url::Url::parse("https://example.test/a/b?x=1#h").unwrap()).unwrap();
    b
}

fn ev(b: &DomBindings, script: &str) -> String {
    match b.evaluate(script).unwrap() {
        JsValue::String(s) => s,
        JsValue::Boolean(x) => x.to_string(),
        JsValue::Number(n) => format!("{n}"),
        JsValue::Undefined => "undefined".into(),
        JsValue::Null => "null".into(),
        other => format!("{other:?}"),
    }
}

fn thrown(b: &DomBindings, script: &str) -> String {
    ev(b, &format!("(function () {{ try {{ {script} }} catch (e) {{ return e.name; }} return 'no error'; }})()"))
}

#[test]
fn document_location_is_window_location() {
    let b = bound();
    assert_eq!(
        ev(&b, "var { pathname, host, search } = document.location; [document.location === window.location, pathname, host, search].join()"),
        "true,/a/b,example.test,?x=1"
    );
}

#[test]
fn document_default_view_is_window() {
    let b = bound();
    assert_eq!(ev(&b, "[document.defaultView === window, typeof document.defaultView].join()"), "true,object");
}

#[test]
fn state_members_read_as_a_visible_focused_utf8_html_document() {
    let b = bound();
    assert_eq!(
        ev(&b, "[document.visibilityState, document.hidden, document.characterSet, document.charset, document.inputEncoding, document.contentType, document.compatMode, document.hasFocus(), String(document.fullscreenElement), document.fullscreenEnabled].join()"),
        "visible,false,UTF-8,UTF-8,UTF-8,text/html,CSS1Compat,true,null,false"
    );
    assert_eq!(ev(&b, "[document.doctype.nodeType, document.doctype.name].join()"), "10,html");
}

#[test]
fn a_document_without_a_doctype_is_in_quirks_mode() {
    let b = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    b.set_document(Rc::new(Document::parse_html("<html><body></body></html>").unwrap())).unwrap();
    assert_eq!(ev(&b, "document.compatMode + ':' + document.doctype"), "BackCompat:null");
}

#[test]
fn the_collections_are_html_collection_shaped() {
    let b = bound();
    assert_eq!(ev(&b, "[document.forms.length, document.forms[1].id, document.forms.item(0).id, document.forms instanceof HTMLCollection].join()"), "2,f2,f1,true");
    assert_eq!(ev(&b, "[document.images.length, document.scripts.length, document.embeds.length].join()"), "2,1,0");
    assert_eq!(ev(&b, "document.links.length + ':' + Array.prototype.map.call(document.links, function (e) { return e.id; }).join('+')"), "2:l1+ar", "links need an href: <a> and <area>");
    assert_eq!(ev(&b, "document.anchors.length + ':' + document.anchors[0].id"), "1:l1", "anchors need a name");
    assert_eq!(ev(&b, "[String(document.forms.namedItem('f2').id), String(document.forms.namedItem('nope'))].join()"), "f2,null");
    // A snapshot, taken when read.
    assert_eq!(ev(&b, "var before = document.images; document.body.appendChild(document.createElement('img')); [before.length, document.images.length].join()"), "2,3");
}

#[test]
fn getelementsbyname_matches_the_name_attribute() {
    let b = bound();
    assert_eq!(ev(&b, "Array.prototype.map.call(document.getElementsByName('q'), function (e) { return e.id; }).join()"), "q,s");
    assert_eq!(ev(&b, "document.getElementsByName('none').length"), "0");
}

#[test]
fn adoptnode_detaches_and_validates() {
    let b = bound();
    assert_eq!(ev(&b, "var n = document.getElementById('i1'); var r = document.adoptNode(n); [r === n, n.parentNode === null].join()"), "true,true");
    assert_eq!(thrown(&b, "document.adoptNode(document);"), "NotSupportedError");
    assert_eq!(thrown(&b, "document.adoptNode({});"), "TypeError");
}

#[test]
fn document_fonts_exists_resolves_and_holds_what_is_added() {
    let b = bound();
    assert_eq!(
        ev(&b, "[typeof document.fonts, document.fonts.status, document.fonts.size, document.fonts.check('12px Foo'), document.fonts instanceof FontFaceSet].join()"),
        "object,loaded,0,true,true"
    );
    b.evaluate("var log = []; document.fonts.ready.then(function (s) { log.push('ready:' + (s === document.fonts)); });").unwrap();
    b.evaluate("var f = new FontFace('Foo', 'url(foo.woff2)', { weight: '700' }); document.fonts.add(f); f.load().then(function (x) { log.push('loaded:' + (x === f) + ':' + f.status); });").unwrap();
    b.run_timers(100, 10).unwrap();
    assert_eq!(ev(&b, "log.join()"), "ready:true,loaded:true:loaded");
    assert_eq!(ev(&b, "[document.fonts.size, document.fonts.has(f), f.family, f.weight, f.style, document.fonts.delete(f), document.fonts.size].join()"), "1,true,Foo,700,normal,true,0");
    assert_eq!(ev(&b, "var seen = []; document.fonts.add(f); document.fonts.forEach(function (x) { seen.push(x.family); }); seen.join() + ':' + Array.from(document.fonts).length"), "Foo:1");
    assert_eq!(thrown(&b, "new FontFace('x');"), "TypeError");
}

#[test]
fn dommatrix_parses_and_composes() {
    let b = bound();
    assert_eq!(ev(&b, "var m = new DOMMatrix(); [m.isIdentity, m.is2D, m.a, m.d, String(m)].join()"), "true,true,1,1,matrix(1, 0, 0, 1, 0, 0)");
    assert_eq!(ev(&b, "var t = new DOMMatrix('matrix(1, 0, 0, 1, 40, 20)'); [t.e, t.f, t.m41, t.m42, t.is2D].join()"), "40,20,40,20,true");
    assert_eq!(ev(&b, "var u = new DOMMatrix('translate(10px, 5px) scale(2)'); [u.a, u.d, u.e, u.f].join()"), "2,2,10,5");
    assert_eq!(ev(&b, "String(new DOMMatrix('none').isIdentity) + String(new DOMMatrix('').isIdentity)"), "truetrue");
    assert_eq!(ev(&b, "var z = new DOMMatrix('translate3d(1px, 2px, 3px)'); [z.is2D, z.m43, String(z)].join()"), "false,3,matrix3d(1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 1, 2, 3, 1)");
    assert_eq!(ev(&b, "var s = new DOMMatrix([1, 0, 0, 1, 7, 8]); [s.e, s.f].join()"), "7,8");
    assert_eq!(thrown(&b, "new DOMMatrix('perspective(5px)');"), "SyntaxError");
    assert_eq!(thrown(&b, "new DOMMatrix([1, 2, 3]);"), "TypeError");
    assert_eq!(thrown(&b, "DOMMatrix();"), "TypeError");
}

#[test]
fn dommatrix_methods_return_new_matrices_and_self_methods_mutate() {
    let b = bound();
    assert_eq!(ev(&b, "var m = new DOMMatrix(); var t = m.translate(5, 6); [m.isIdentity, t.e, t.f, t instanceof DOMMatrix].join()"), "true,5,6,true");
    assert_eq!(ev(&b, "var s = new DOMMatrix().scale(3); [s.a, s.d, s.m33].join()"), "3,3,1");
    assert_eq!(ev(&b, "var r = new DOMMatrix().rotate(90); [Math.round(r.a), Math.round(r.b), Math.round(r.c), Math.round(r.d)].join()"), "0,1,-1,0");
    assert_eq!(ev(&b, "var p = new DOMMatrix().translate(10, 20).transformPoint({ x: 1, y: 2 }); [p.x, p.y].join()"), "11,22");
    assert_eq!(ev(&b, "var i = new DOMMatrix('matrix(2, 0, 0, 2, 10, 10)').inverse(); [i.a, i.d, i.e, i.f].join()"), "0.5,0.5,-5,-5");
    assert_eq!(ev(&b, "var mm = new DOMMatrix(); var ret = mm.translateSelf(3, 4); [ret === mm, mm.e, mm.f].join()"), "true,3,4");
    assert_eq!(ev(&b, "var k = new DOMMatrix('translate(10px, 0)').multiply(new DOMMatrix('scale(2)')); [k.a, k.e].join()"), "2,10", "multiply applies the argument first");
    assert_eq!(ev(&b, "var q = new DOMMatrix('scale(2)'); q.e = 9; [q.e, q.m41].join()"), "9,9");
    assert_eq!(ev(&b, "[DOMMatrix.fromMatrix({ a: 2, e: 3 }).a, DOMMatrix.fromMatrix({ a: 2, e: 3 }).e, new DOMMatrix() instanceof DOMMatrixReadOnly, typeof WebKitCSSMatrix].join()"), "2,3,true,function");
}

#[test]
fn a_matrix_read_back_from_a_computed_transform() {
    // apple.com's gallery: new DOMMatrix(getComputedStyle(el).transform).m41
    let b = bound();
    assert_eq!(ev(&b, "new DOMMatrix('matrix(1, 0, 0, 1, -320, 0)').m41"), "-320");
}

#[test]
fn document_implementation_creates_html_document() {
    let b = bound();
    assert_eq!(
        ev(&b, "[document.implementation instanceof DOMImplementation, document.implementation === document.implementation, document.implementation.hasFeature()].join()"),
        "true,true,true"
    );
    assert_eq!(
        ev(&b, "var doc = document.implementation.createHTMLDocument('inert'); [doc instanceof Document, doc.title, doc.body !== null, doc.head !== null, doc.documentElement !== null, doc.doctype.name].join()"),
        "true,inert,true,true,true,html"
    );
    assert_eq!(
        ev(&b, "var div = doc.createElement('div'); doc.body.appendChild(div); [doc.body.children.length, div.parentNode === doc.body].join()"),
        "1,true"
    );
    assert_eq!(
        ev(&b, "var plain = document.implementation.createHTMLDocument(); [plain.title, plain.body !== null].join()"),
        ",true"
    );
    assert_eq!(
        ev(&b, "var dt = document.implementation.createDocumentType('html', 'pub', 'sys'); [dt.nodeType, dt.name, dt.publicId, dt.systemId, dt instanceof DocumentType].join()"),
        "10,html,pub,sys,true"
    );
}

