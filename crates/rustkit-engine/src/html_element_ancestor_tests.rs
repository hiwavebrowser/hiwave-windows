//! The `<html>` element as an ancestor in a selector.
//!
//! The box tree is built from `<body>`, and body was given no ancestors, so
//! a selector whose ancestor part has to match the root element by class,
//! id or tag (`.client-js .x`, `html.dark .x`, `#top .x`, `html > body .x`)
//! matched nothing. Pages hang their feature and theme classes there:
//! en.wikipedia.org shows its contents column with
//! `.vector-feature-toc-pinned-clientpref-1 .vector-column-start .vector-toc-landmark`.
//!
//! Every `nav` below is `display: none` until its own rule shows it. The
//! expected rows are what the oracle Chromium 143 computes for this page.

use super::*;

const PAGE: &str = r#"<!doctype html>
<html class="client-js p1 other" id="top" lang="en" data-theme="dark"><head><style>
nav { display: none; }
.p0 body:not(.s) .t .a, .p0 .s .c2 .a, .p1 .col .a { display: block; }
.p1 .col .b { display: block; }
html.p1 .c { display: block; }
.p0 .t .d, .p1 .col .d { display: block; }
.p0 body:not(.s) .t .e, .p1 .col .e { display: block; }
.col .f { display: block; }
body:not(.s) .col .g { display: block; }
#top .h { display: block; }
:root.p1 .i { display: block; }
html > body .j { display: block; }
.p1 > body > .col .k { display: block; }
[data-theme=dark] .l { display: block; }
html[lang=en] .m { display: block; }
html:not(.zz) .n { display: block; }
.zz .o { display: block; }
.p1.other .p { display: block; }
.client-js .col > .q { display: block; }
.p1 > .col .r { display: block; }
html.p0 .s2 { display: block; }
.p1 body.skin .t2 { display: block; }
.other.p1 > .skin nav.u { display: block; }
html .v { display: block; }
:root .w { display: block; }
.p1 .x::before { content: "x-before"; }
.x { display: block; }
</style></head><body class="skin">
<div class="col">
<nav class="a">a</nav><nav class="b">b</nav><nav class="c">c</nav><nav class="d">d</nav><nav class="e">e</nav>
<nav class="f">f</nav><nav class="g">g</nav><nav class="h">h</nav><nav class="i">i</nav><nav class="j">j</nav>
<nav class="k">k</nav><nav class="l">l</nav><nav class="m">m</nav><nav class="n">n</nav><nav class="o">o</nav>
<nav class="p">p</nav><nav class="q">q</nav><nav class="r">r</nav><nav class="s2">s2</nav><nav class="t2">t2</nav>
<nav class="u">u</nav><nav class="v">v</nav><nav class="w">w</nav><nav class="x"></nav>
</div>
</body></html>"#;

fn shown(html: &str) -> Vec<String> {
    let document = Rc::new(Document::parse_html(html).expect("html"));
    let engine = Engine::new(EngineConfig::default()).expect("engine");
    let layout = engine.build_layout_from_document(&document, &[]);
    fn texts(b: &LayoutBox, out: &mut Vec<String>) {
        if let BoxType::Text(t) = &b.box_type {
            if !t.trim().is_empty() {
                out.push(t.trim().to_string());
            }
        }
        if let Some(content) = &b.style.content {
            out.push(content.clone());
        }
        for c in &b.children {
            texts(c, out);
        }
    }
    let mut out = Vec::new();
    texts(&layout, &mut out);
    out.dedup();
    out
}

#[test]
fn a_selector_that_names_the_root_element_as_an_ancestor_matches() {
    assert_eq!(
        shown(PAGE).join(" "),
        "a b c d e f g h i j k l m n p q t2 u v w x-before"
    );
}

/// Pass before the fix: the root element's own classes are not its
/// descendants', and a page without any class on `<html>` is as before.
#[test]
fn a_root_class_is_not_matched_as_a_descendant_or_without_being_there() {
    let page = r#"<html class="p1"><head><style>
        nav { display: none; }
        .p1.col .a { display: block; }
        .col .p1 .b { display: block; }
        .p2 .c { display: block; }
        .col .d { display: block; }
        </style></head><body><div class="col">
        <nav class="a">a</nav><nav class="b">b</nav><nav class="c">c</nav><nav class="d">d</nav>
        </div></body></html>"#;
    assert_eq!(shown(page).join(" "), "d");
}

/// The root element is in every ancestor chain, so the attribute and
/// `:not()` parts of a compound that it matches by tag or class must be
/// tested against it: otherwise `html[dir=rtl] .x`, `html[lang=de] .x` and
/// `html:not(.js) .x` match on every page (Prometheus, R1 on #614: rows a,
/// b, d and e are his probe). His `[dir=rtl] .c` is not here: it matches
/// through `body` and `div`, whose attributes the chain does not carry, as
/// it did before the root was an ancestor. The expected row is the oracle
/// Chromium 143's for this page.
const ROOT_ATTRIBUTES: &str = r#"<!doctype html>
<html lang="en" dir="ltr" class="p1" data-theme="dark" data-n="a b"><head><style>
nav { display: none; }
html[lang=fr] .a { display: block; }
html[dir=rtl] .b { display: block; }
html[dir=rtl] > body .c { display: block; }
html:not(.p1) .d { display: block; }
html[lang=en] .e { display: block; }
html[lang] .f { display: block; }
html[data-missing] .g { display: block; }
html[data-theme="light"] .h { display: block; }
html[data-theme^=da] .i { display: block; }
html[data-n~=b] .j { display: block; }
html[data-n~=c] .k { display: block; }
html:not(.p2) .l { display: block; }
html:not([dir=rtl]) .m { display: block; }
html:not([dir=ltr]) .n { display: block; }
html:not(.p2, .p1) .o { display: block; }
html:not(body) .p { display: block; }
:root:not(html) .q { display: block; }
.p1[lang|=en] > body .r { display: block; }
.p1[lang=de] > body .s { display: block; }
html:is([lang=de], [lang=fr]) .t { display: block; }
html:is([lang=de], [lang=en]) .u { display: block; }
html:not(.p1) body .v, html.p1[dir=ltr] .v { display: block; }
html:not(:lang(fr)) .w { display: block; }
</style></head><body>
<div class="col">
<nav class="a">a</nav><nav class="b">b</nav><nav class="c">c</nav><nav class="d">d</nav><nav class="e">e</nav>
<nav class="f">f</nav><nav class="g">g</nav><nav class="h">h</nav><nav class="i">i</nav><nav class="j">j</nav>
<nav class="k">k</nav><nav class="l">l</nav><nav class="m">m</nav><nav class="n">n</nav><nav class="o">o</nav>
<nav class="p">p</nav><nav class="q">q</nav><nav class="r">r</nav><nav class="s">s</nav><nav class="t">t</nav>
<nav class="u">u</nav><nav class="v">v</nav><nav class="w">w</nav>
</div>
</body></html>"#;

#[test]
fn the_root_elements_attributes_and_negations_are_tested_when_it_is_the_ancestor() {
    assert_eq!(shown(ROOT_ATTRIBUTES).join(" "), "e f i j l m p r u v w");
}
