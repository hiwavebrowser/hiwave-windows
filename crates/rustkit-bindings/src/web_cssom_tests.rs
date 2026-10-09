//! The script-visible CSSOM (web_cssom.js): document.styleSheets,
//! `<style>.sheet`, constructable CSSStyleSheet, CSSStyleRule and its style
//! declaration, document.adoptedStyleSheets, CSS.supports and CSS.escape.
//! The point of the slice is that `insertRule` on a `<style>`'s sheet lands
//! in that element's text, which the engine's style pipeline reads.

use super::*;

const PAGE: &str = r#"<!DOCTYPE html><html><head><title>T</title>
<style id="s1">.box { color: red; } /* note */ #list li { margin: 0 !important; padding: 1px }
@media (min-width: 10px) { .m { color: blue } }</style>
<link id="l1" rel="stylesheet" href="/a.css"><link rel="icon" href="/i.png"></head>
<body><div id="main" class="box"></div><style id="s2"></style></body></html>"#;

fn bound() -> DomBindings {
    let bindings = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    bindings
        .set_document(Rc::new(Document::parse_html(PAGE).unwrap()))
        .unwrap();
    bindings
        .set_location(&url::Url::parse("https://cssom.test/dir/page.html").unwrap())
        .unwrap();
    bindings
}

fn ev(bindings: &DomBindings, script: &str) -> String {
    match bindings.evaluate(script).unwrap() {
        JsValue::String(s) => s,
        JsValue::Boolean(b) => b.to_string(),
        JsValue::Number(n) => n.to_string(),
        JsValue::Null => "null".to_string(),
        JsValue::Undefined => "undefined".to_string(),
        other => panic!("{script} evaluated to {other:?}"),
    }
}

#[test]
fn document_style_sheets_lists_style_and_stylesheet_links_in_order() {
    let b = bound();
    assert_eq!(
        ev(
            &b,
            "var l = document.styleSheets, r = [l.length, l instanceof StyleSheetList]; \
             for (var i = 0; i < l.length; i++) r.push(l[i].ownerNode.id || l[i].ownerNode.localName); \
             r.push(l.item(0) === document.getElementById('s1').sheet, String(l.item(9)), l[1].href, \
                    String(l[0].href), l[0].type, l[0] instanceof CSSStyleSheet, l[0] instanceof StyleSheet, \
                    Array.from(l).length); \
             r.join(',')"
        ),
        "3,true,s1,l1,s2,true,null,https://cssom.test/a.css,null,text/css,true,true,3"
    );
    // A <style> added by script shows up, and keeps its sheet's identity.
    assert_eq!(
        ev(
            &b,
            "var s = document.createElement('style'); s.textContent = 'p{color:red}'; \
             var r = [String(s.sheet)]; document.head.appendChild(s); \
             r.push(document.styleSheets.length, s.sheet === s.sheet, s.sheet.cssRules.length); \
             s.remove(); r.push(document.styleSheets.length, String(s.sheet)); r.join(',')"
        ),
        "null,4,true,1,3,null"
    );
}

#[test]
fn css_rules_parse_style_rules_and_keep_at_rules_whole() {
    let b = bound();
    assert_eq!(
        ev(
            &b,
            "var rs = document.getElementById('s1').sheet.cssRules, r = [rs.length, rs instanceof CSSRuleList]; \
             r.push(rs[0] instanceof CSSStyleRule, rs[0] instanceof CSSRule, rs[0].type, rs[0].selectorText, \
                    rs[0].style.cssText, rs[0].cssText); \
             r.push(rs[1].selectorText, rs[1].style.getPropertyValue('margin'), \
                    rs[1].style.getPropertyPriority('margin'), rs[1].style.length, rs[1].style[1]); \
             r.push(rs[2] instanceof CSSStyleRule, rs[2].type, rs[2].cssText.indexOf('@media') === 0, \
                    rs[0].parentStyleSheet === document.getElementById('s1').sheet); \
             r.join('|')"
        ),
        "3|true|true|true|1|.box|color: red;|.box { color: red; }|#list li|0|important|2|padding|false|4|true|true"
    );
}

#[test]
fn insert_rule_on_a_style_sheet_writes_the_rule_into_the_element_text() {
    let b = bound();
    b.take_dirty();
    // The CSS-in-JS path: insertRule at the end, many times.
    assert_eq!(
        ev(
            &b,
            "var s = document.getElementById('s2'), sh = s.sheet, r = []; \
             r.push(sh.insertRule('.css-1 { color: green }', sh.cssRules.length)); \
             r.push(sh.insertRule('.css-2{display:flex}', 1)); \
             r.push(sh.cssRules.length, sh.cssRules[1].selectorText, s.textContent); \
             r.join('|')"
        ),
        "0|1|2|.css-2|.css-1 { color: green }\n.css-2{display:flex}"
    );
    assert_eq!(b.take_dirty(), DomDirty::Style);
    // In the middle, and default index 0: the text follows rule order.
    assert_eq!(
        ev(
            &b,
            "var s = document.getElementById('s2'), sh = s.sheet; \
             sh.insertRule('.a { top: 0 }'); sh.insertRule('.b { top: 1px }', 2); \
             Array.from(sh.cssRules).map(function (x) { return x.selectorText; }).join() + '|' + \
             (s.textContent.indexOf('.a') < s.textContent.indexOf('.css-1'))"
        ),
        ".a,.css-1,.b,.css-2|true"
    );
    // deleteRule rewrites the text too; bad input throws the DOM errors.
    assert_eq!(
        ev(
            &b,
            "var s = document.getElementById('s2'), sh = s.sheet, r = []; \
             sh.deleteRule(0); r.push(sh.cssRules.length, s.textContent.indexOf('.a {')); \
             try { sh.insertRule('.x { color: red }', 99); } catch (e) { r.push(e.name); } \
             try { sh.insertRule('not a rule'); } catch (e) { r.push(e.name); } \
             try { sh.deleteRule(99); } catch (e) { r.push(e.name); } \
             r.join()"
        ),
        "3,-1,IndexSizeError,SyntaxError,IndexSizeError"
    );
}

#[test]
fn text_written_by_script_reparses_and_rule_style_writes_reach_the_text() {
    let b = bound();
    assert_eq!(
        ev(
            &b,
            "var s = document.getElementById('s2'), sh = s.sheet, r = [sh.cssRules.length]; \
             s.textContent = 'a{color:red} b{color:blue}'; r.push(sh.cssRules.length, s.sheet === sh); \
             var st = sh.cssRules[1].style; st.setProperty('margin-top', '2px', 'important'); \
             st.color = 'green'; r.push(st.color, st.marginTop, st.getPropertyPriority('margin-top')); \
             r.push(s.textContent.indexOf('margin-top: 2px !important') > 0, st.removeProperty('color'), \
                    sh.cssRules[1].cssText); \
             sh.cssRules[0].selectorText = 'i'; r.push(s.textContent.indexOf('i {') === 0); \
             r.join('|')"
        ),
        "0|2|true|green|2px|important|true|green|b { margin-top: 2px !important; }|true"
    );
}

#[test]
fn constructed_sheets_replace_and_adopt() {
    let b = bound();
    assert_eq!(
        ev(
            &b,
            "var s = new CSSStyleSheet(), r = [s.cssRules.length, String(s.ownerNode), String(s.href), s.disabled]; \
             s.replaceSync('p { color: red } div { color: blue }'); r.push(s.cssRules.length); \
             s.insertRule('span { top: 0 }', 2); s.deleteRule(0); \
             r.push(Array.from(s.cssRules).map(function (x) { return x.selectorText; }).join('+')); \
             s.disabled = true; r.push(s.disabled); \
             r.push(Array.isArray(document.adoptedStyleSheets), document.adoptedStyleSheets.length); \
             document.adoptedStyleSheets = [s]; r.push(document.adoptedStyleSheets[0] === s); \
             document.adoptedStyleSheets = [...document.adoptedStyleSheets, new CSSStyleSheet({ disabled: true })]; \
             r.push(document.adoptedStyleSheets.length, document.adoptedStyleSheets[1].disabled); \
             try { document.adoptedStyleSheets = [{}]; } catch (e) { r.push(e.name); } \
             try { document.getElementById('s1').sheet.replaceSync('a{}'); } catch (e) { r.push(e.name); } \
             r.push(document.styleSheets.length); \
             r.join('|')"
        ),
        "0|null|null|false|2|div+span|true|true|0|true|2|true|TypeError|NotAllowedError|3"
    );
    // replace() resolves with the sheet.
    ev(
        &b,
        "globalThis.got = ''; var s = new CSSStyleSheet(); \
         s.replace('a { color: red }').then(function (x) { got = (x === s) + ',' + x.cssRules.length; }); '';",
    );
    assert_eq!(ev(&b, "got"), "true,1");
}

#[test]
fn css_supports_answers_from_the_engine_property_list() {
    let b = bound();
    assert_eq!(
        ev(
            &b,
            "[CSS.supports('display', 'grid'), CSS.supports('display', 'flexx'), CSS.supports('position', 'sticky'), \
              CSS.supports('gap', '1px'), CSS.supports('margin-inline', 'auto'), CSS.supports('--x', 'y'), \
              CSS.supports('nonsense-prop', '1'), CSS.supports('color', ''), CSS.supports('color', 'red;x'), \
              CSS.supports('display: flex'), CSS.supports('(display: grid) and (gap: 1px)'), \
              CSS.supports('(display: grid) and (frob: 1)'), CSS.supports('(frob: 1) or (color: red)'), \
              CSS.supports('not (frob: 1)'), CSS.supports('selector(.a > b)'), CSS.supports('garbage'), \
              CSS.supports('DISPLAY', 'block'), CSS.supports('width', 'inherit')].join()"
        ),
        "true,false,true,true,true,true,false,false,false,true,true,false,true,true,true,false,true,true"
    );
}

#[test]
fn css_escape_follows_cssom_serialize_an_identifier() {
    let b = bound();
    assert_eq!(
        ev(
            &b,
            r#"[CSS.escape('a b'), CSS.escape('1a'), CSS.escape('-1a'), CSS.escape('-'), CSS.escape('a\u0000'),
               CSS.escape('a:b.c#d'), CSS.escape('\u007f'), CSS.escape('é_-x'), CSS.escape('--a')].join('|')"#
        ),
        "a\\ b|\\31 a|-\\31 a|\\-|a\u{fffd}|a\\:b\\.c\\#d|\\7f |é_-x|--a"
    );
}
