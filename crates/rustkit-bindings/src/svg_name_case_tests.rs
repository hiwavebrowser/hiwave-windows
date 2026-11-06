//! SV1: what a script sees of a parsed SVG element. Expected values are
//! Chromium 143's answers on the same markup
//! (`trench/z/tools/sv1/sv1_probe.mjs`).

use super::*;

fn bound() -> DomBindings {
    let b = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    let doc = Rc::new(
        Document::parse_html(
            "<html><head></head><body>\
             <svg id=s viewbox='0 0 1 1' data-k=v><lineargradient id=g></lineargradient><g id=g2></g>\
             <foreignobject id=fo></foreignobject></svg></body></html>",
        )
        .unwrap(),
    );
    b.set_document(doc).unwrap();
    b
}

fn ev(b: &DomBindings, script: &str) -> String {
    match b.evaluate(script).unwrap() {
        JsValue::String(s) => s,
        other => format!("{other:?}"),
    }
}

#[test]
fn tag_name_keeps_the_case_of_a_parsed_svg_element() {
    let b = bound();
    assert_eq!(
        ev(
            &b,
            "var s = document.getElementById('s'), g = document.getElementById('g'); \
             [s.tagName, s.nodeName, s.localName, g.tagName, g.localName, \
              document.body.tagName].join('|')"
        ),
        "svg|svg|svg|linearGradient|linearGradient|BODY"
    );
}

#[test]
fn a_parsed_svg_element_has_the_svg_interfaces_and_namespace() {
    let b = bound();
    assert_eq!(
        ev(
            &b,
            "var s = document.getElementById('s'), g = document.getElementById('g'); \
             [s.namespaceURI, s instanceof SVGSVGElement, s instanceof SVGElement, \
              s instanceof HTMLElement, g instanceof SVGElement].join('|')"
        ),
        "http://www.w3.org/2000/svg|true|true|false|true"
    );
}

#[test]
fn attribute_reads_and_writes_keep_their_case_on_a_parsed_svg_element() {
    let b = bound();
    assert_eq!(
        ev(
            &b,
            "var s = document.getElementById('s'); \
             var r = [s.getAttribute('viewBox'), String(s.getAttribute('viewbox')), s.hasAttribute('VIEWBOX')]; \
             s.setAttribute('preserveAspectRatio', 'xMinYMin'); s.setAttribute('FooBar', '1'); \
             r.push(s.getAttribute('preserveAspectRatio'), s.getAttributeNames().sort().join()); \
             r.join('|')"
        ),
        "0 0 1 1|null|false|xMinYMin|FooBar,data-k,id,preserveAspectRatio,viewBox"
    );
}

#[test]
fn the_shared_html_and_svg_members_stay_on_a_parsed_svg_element() {
    let b = bound();
    assert_eq!(
        ev(
            &b,
            "var s = document.getElementById('s'); \
             [s.dataset.k, s.tabIndex, typeof s.style, typeof s.focus, typeof s.blur, \
              typeof s.nonce, typeof s.autofocus, typeof s.click, typeof s.innerText, \
              typeof s.offsetWidth].join('|')"
        ),
        "v|-1|object|function|function|string|boolean|undefined|undefined|undefined"
    );
}

#[test]
fn inner_html_parses_and_serializes_svg_names() {
    let b = bound();
    assert_eq!(
        ev(
            &b,
            "var r = [], d = document.createElement('div'); \
             d.innerHTML = '<svg viewbox=\"0 0 1 1\"><lineargradient gradientunits=x></lineargradient></svg>'; \
             r.push(d.innerHTML, d.firstChild.tagName, d.firstChild.firstChild.tagName); \
             var g2 = document.getElementById('g2'); \
             g2.innerHTML = '<clippath clippathunits=x><path></path></clippath>'; \
             r.push(g2.innerHTML, g2.firstChild.tagName, g2.firstChild.namespaceURI); \
             var fo = document.getElementById('fo'); \
             fo.innerHTML = '<p viewbox=1></p><lineargradient></lineargradient>'; \
             r.push(fo.innerHTML, fo.firstChild.namespaceURI, fo.lastChild.namespaceURI); \
             r.join('|')"
        ),
        "<svg viewBox=\"0 0 1 1\"><linearGradient gradientUnits=\"x\"></linearGradient></svg>|svg|linearGradient|\
         <clipPath clipPathUnits=\"x\"><path></path></clipPath>|clipPath|http://www.w3.org/2000/svg|\
         <p viewbox=\"1\"></p><lineargradient></lineargradient>|\
         http://www.w3.org/1999/xhtml|http://www.w3.org/1999/xhtml"
    );
}

#[test]
fn insert_adjacent_html_on_an_svg_element_parses_in_the_svg_namespace() {
    let b = bound();
    assert_eq!(
        ev(
            &b,
            "var g2 = document.getElementById('g2'); \
             g2.insertAdjacentHTML('beforeend', '<clippath clippathunits=x></clippath>'); \
             [g2.firstChild.tagName, g2.firstChild.namespaceURI, \
              g2.firstChild.getAttribute('clipPathUnits')].join('|')"
        ),
        "clipPath|http://www.w3.org/2000/svg|x"
    );
}
