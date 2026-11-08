//! SV1: the parser's SVG names (HTML §13.2.6.5, "adjust SVG attributes" and
//! the element name table). Expected values are Chromium 143's answers on
//! the same markup (`trench/z/tools/sv1/sv1_probe.mjs`). Where Chromium also
//! moves a breakout element out of the `<svg>`, only its namespace is pinned.

use super::*;

const SVG: &str = "http://www.w3.org/2000/svg";
const HTML: &str = "http://www.w3.org/1999/xhtml";

const PAGE: &str = r##"<!DOCTYPE html><body>
<svg id=s viewbox="0 0 1 1" PRESERVEASPECTRATIO=none data-k=v>
<lineargradient id=g gradientunits=userSpaceOnUse></lineargradient><clippath id=c></clippath>
<g id=g2></g><foreignobject id=fo><div id=h viewbox=1></div><svg id=inner></svg></foreignobject>
<desc id=de><i id=di></i></desc><a id=a1 xlink:href="#x"></a><font id=f1></font>
</svg><svg id=b1><div id=br></div></svg><svg id=b2><font id=f2 color=red></font></svg>
<div id=plain viewbox=1></div>
<table id=tb1><svg id=fost viewbox="0 0 3 3"><path id=fp></path></svg></table>
</body>"##;

/// `(tag, namespace, sorted attribute names)`.
fn read(doc: &Document, id: &str) -> (String, String, Vec<String>) {
    let node = doc.get_element_by_id(id).unwrap_or_else(|| panic!("no #{id}"));
    match &node.node_type {
        NodeType::Element { tag_name, namespace, attributes } => {
            let mut names: Vec<String> = attributes.keys().cloned().collect();
            names.sort();
            (tag_name.clone(), namespace.clone(), names)
        }
        _ => panic!("#{id} is not an element"),
    }
}

fn check(doc: &Document, id: &str, tag: &str, namespace: &str, attrs: &[&str]) {
    let (t, n, a) = read(doc, id);
    assert_eq!((t.as_str(), n.as_str()), (tag, namespace), "#{id} tag and namespace");
    assert_eq!(a, attrs, "#{id} attribute names");
}

#[test]
fn a_parsed_svg_element_is_in_the_svg_namespace_with_adjusted_attributes() {
    let doc = Document::parse_html(PAGE).unwrap();
    check(&doc, "s", "svg", SVG, &["data-k", "id", "preserveAspectRatio", "viewBox"]);
    check(&doc, "g2", "g", SVG, &["id"]);
    check(&doc, "a1", "a", SVG, &["id", "xlink:href"]);
    check(&doc, "f1", "font", SVG, &["id"]);
}

#[test]
fn a_parsed_svg_element_name_is_adjusted_from_the_table() {
    let doc = Document::parse_html(PAGE).unwrap();
    check(&doc, "g", "linearGradient", SVG, &["gradientUnits", "id"]);
    check(&doc, "c", "clipPath", SVG, &["id"]);
    check(&doc, "fo", "foreignObject", SVG, &["id"]);
}

#[test]
fn an_html_integration_point_holds_html_and_a_nested_svg_is_svg_again() {
    let doc = Document::parse_html(PAGE).unwrap();
    check(&doc, "h", "div", HTML, &["id", "viewbox"]);
    check(&doc, "inner", "svg", SVG, &["id"]);
    check(&doc, "de", "desc", SVG, &["id"]);
    check(&doc, "di", "i", HTML, &["id"]);
}

#[test]
fn a_breakout_tag_stays_html_and_font_breaks_out_only_with_its_attributes() {
    let doc = Document::parse_html(PAGE).unwrap();
    check(&doc, "br", "div", HTML, &["id"]);
    check(&doc, "f2", "font", HTML, &["color", "id"]);
}

#[test]
fn html_elements_keep_lowercase_attributes() {
    let doc = Document::parse_html(PAGE).unwrap();
    check(&doc, "plain", "div", HTML, &["id", "viewbox"]);
}

#[test]
fn an_svg_foster_parented_out_of_a_table_is_still_svg() {
    let doc = Document::parse_html(PAGE).unwrap();
    check(&doc, "fost", "svg", SVG, &["id", "viewBox"]);
    check(&doc, "fp", "path", SVG, &["id"]);
}

#[test]
fn a_fragment_in_an_svg_context_is_svg_and_foreign_object_context_is_html() {
    let doc = Document::parse_html("<body></body>").unwrap();
    let in_g = doc
        .parse_fragment_in("<clippath clippathunits=x></clippath>", "g", false, true)
        .unwrap();
    assert_eq!(in_g[0].tag_name(), Some("clipPath"));
    assert_eq!(in_g[0].get_attribute("clipPathUnits"), Some("x"));

    let in_fo = doc
        .parse_fragment_in("<p viewbox=1></p>", "foreignObject", false, false)
        .unwrap();
    assert_eq!(in_fo[0].tag_name(), Some("p"));
    assert_eq!(in_fo[0].get_attribute("viewbox"), Some("1"));

    let in_div = doc.parse_fragment("<svg viewbox='0 0 1 1'></svg><lineargradient></lineargradient>", "div").unwrap();
    assert_eq!(in_div[0].get_attribute("viewBox"), Some("0 0 1 1"));
    assert_eq!(in_div[1].tag_name(), Some("lineargradient"));
}
