//! HTML IDL attribute reflection (web_reflect.js): link.href and friends.

use super::*;

const PAGE: &str = "<html><head>\
    <link id=fav class=js-site-favicon type=image/svg+xml href=/fav.svg rel='icon shortcut' data-base-href=/x crossorigin>\
    <base href=/sub/>\
    <meta id=m name=viewport content='width=device-width' http-equiv=refresh>\
    </head><body>\
    <a id=a href=/p target=_blank rel='noopener nofollow' download=f.txt>link</a>\
    <img id=i src=/pic.png alt=cat loading=lazy>\
    <img id=i2>\
    <iframe id=fr src=frame.html name=nm sandbox='allow-scripts allow-forms'></iframe>\
    <form id=f action=submit method=POST></form><form id=f2></form>\
    <script id=s src=/app.js type=module async nomodule></script>\
    <div id=d title=tip lang=fr dir=rtl hidden tabindex=3 contenteditable><p id=p></p></div>\
    <button id=b></button><span id=sp></span>\
    </body></html>";

fn bound() -> DomBindings {
    let b = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    b.set_document(Rc::new(Document::parse_html(PAGE).unwrap())).unwrap();
    b.set_location(&url::Url::parse("https://example.test/dir/page?x=1").unwrap()).unwrap();
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

#[test]
fn base_uri_is_the_documents_or_the_first_base_href() {
    let b = bound();
    assert_eq!(ev(&b, "document.baseURI"), "https://example.test/sub/");
    assert_eq!(ev(&b, "document.getElementById('d').baseURI"), "https://example.test/sub/");
    assert_eq!(ev(&b, "document.querySelector('base').href"), "https://example.test/sub/");
}

#[test]
fn link_reflects_its_attributes_and_resolves_the_url() {
    let b = bound();
    // The case that threw on github: link.href.indexOf(...).
    assert_eq!(ev(&b, "var l = document.getElementById('fav'); l.href"), "https://example.test/fav.svg", "absolute path against the base");
    assert_eq!(ev(&b, "l.href.indexOf('-dark.svg')"), "-1");
    assert_eq!(ev(&b, "[l.rel, l.type, l.relList.length, l.relList.contains('icon'), l.getAttribute('data-base-href'), l.crossOrigin, l.disabled, l.media === '', l.as === ''].join()"), "icon shortcut,image/svg+xml,2,true,/x,anonymous,false,true,true");
    assert_eq!(ev(&b, "l.href = 'other.svg'; [l.getAttribute('href'), l.href].join()"), "other.svg,https://example.test/sub/other.svg");
    assert_eq!(ev(&b, "var n = document.createElement('link'); n.rel = 'stylesheet'; n.href = '/a.css'; n.disabled = true; [n.rel, n.href, n.hasAttribute('disabled'), String(n.crossOrigin)].join()"), "stylesheet,https://example.test/a.css,true,null");
    assert_eq!(ev(&b, "document.createElement('link').href"), "");
}

#[test]
fn rellist_edits_the_rel_attribute() {
    let b = bound();
    assert_eq!(ev(&b, "var r = document.getElementById('a').relList; r.add('external'); r.remove('nofollow'); [document.getElementById('a').rel, r.length, r.item(0), r[1], r.toString(), r.toggle('x'), r.toggle('x'), r.replace('noopener', 'opener')].join()"), "noopener external,2,noopener,external,noopener external,true,false,true");
    assert_eq!(ev(&b, "var bad; try { r.add('a b'); } catch (e) { bad = e.name; } bad"), "InvalidCharacterError");
    assert_eq!(ev(&b, "Array.from(r).join()"), "opener,external");
}

#[test]
fn meta_script_and_img_reflect() {
    let b = bound();
    assert_eq!(ev(&b, "var m = document.getElementById('m'); [m.name, m.content, m.httpEquiv].join()"), "viewport,width=device-width,refresh");
    assert_eq!(ev(&b, "var s = document.getElementById('s'); [s.type, s.async, s.defer, s.noModule, String(s.crossOrigin), s.src].join()"), "module,true,false,true,null,https://example.test/app.js");
    assert_eq!(ev(&b, "s.defer = true; s.async = false; [s.hasAttribute('defer'), s.hasAttribute('async')].join()"), "true,false");
    assert_eq!(ev(&b, "var i = document.getElementById('i'); [i.alt, i.loading, i.decoding, i.currentSrc, i.complete, i.naturalWidth, i.naturalHeight].join()"), "cat,lazy,auto,https://example.test/pic.png,false,0,0");
    assert_eq!(ev(&b, "var i2 = document.getElementById('i2'); [i2.complete, i2.currentSrc === '', i2.loading].join()"), "true,true,eager");
    assert_eq!(ev(&b, "i.alt = 'dog'; [i.getAttribute('alt'), typeof i.decode].join()"), "dog,function");
}

#[test]
fn anchor_iframe_and_form_reflect() {
    let b = bound();
    assert_eq!(ev(&b, "var a = document.getElementById('a'); [a.target, a.download, a.text].join()"), "_blank,f.txt,link");
    assert_eq!(ev(&b, "a.target = '_self'; a.getAttribute('target')"), "_self");
    assert_eq!(ev(&b, "var fr = document.getElementById('fr'); [fr.src, fr.name, fr.sandbox.length, fr.sandbox.contains('allow-forms'), String(fr.contentWindow), String(fr.contentDocument)].join()"), "https://example.test/sub/frame.html,nm,2,true,null,null");
    assert_eq!(ev(&b, "var f = document.getElementById('f'); [f.action, f.method, f.enctype, f.noValidate, f.autocomplete].join()"), "submit,post,application/x-www-form-urlencoded,false,on");
    // form.action is web_forms.js's (the attribute as written); not overridden here.
    assert_eq!(ev(&b, "var f2 = document.getElementById('f2'); [f2.action === '' || f2.action === document.URL, f2.method].join()"), "true,get");
}

#[test]
fn every_html_element_has_the_global_attributes() {
    let b = bound();
    assert_eq!(ev(&b, "var d = document.getElementById('d'); [d.title, d.lang, d.dir, d.hidden, d.tabIndex, d.contentEditable, d.isContentEditable, d.draggable, d.spellcheck, d.translate].join()"), "tip,fr,rtl,true,3,true,true,false,true,true");
    assert_eq!(ev(&b, "var p = document.getElementById('p'); [p.isContentEditable, p.contentEditable, p.tabIndex, p.title === '', p.dir === '', p.hidden].join()"), "true,inherit,-1,true,true,false", "contenteditable is inherited");
    assert_eq!(ev(&b, "[document.getElementById('b').tabIndex, document.getElementById('a').tabIndex, document.getElementById('sp').tabIndex, document.createElement('a').tabIndex].join()"), "0,0,-1,-1");
    assert_eq!(ev(&b, "var sp = document.getElementById('sp'); sp.title = 'x'; sp.hidden = true; sp.tabIndex = 2; [sp.getAttribute('title'), sp.hasAttribute('hidden'), sp.getAttribute('tabindex'), sp.draggable].join()"), "x,true,2,false");
    assert_eq!(ev(&b, "sp.slot = 'named'; [sp.slot, sp.getAttribute('slot')].join()"), "named,named");
}

#[test]
fn existing_members_are_not_overridden() {
    let b = bound();
    // a.href and script.src are the DOM layer's; they still resolve and reflect.
    assert_eq!(ev(&b, "[document.getElementById('a').href, document.getElementById('s').src].join()"), "https://example.test/p,https://example.test/app.js");
}
