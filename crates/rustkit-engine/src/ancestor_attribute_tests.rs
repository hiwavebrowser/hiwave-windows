//! Attribute selectors and `:not()` on an ancestor that is not the root.
//!
//! The ancestor chain held each element's tag, classes and id, and only the
//! root's attributes (#614), so `[data-anim] > .x` asked nothing of the
//! parent and matched every `.x` that had one. microsoft.com ships
//! `[animation-intersection]:not([animation-view=disabled])[animation-enter=effect-1] > *
//! { opacity: 0 }`: it matched the page's root container, and the first
//! build that painted `opacity` (#633) drew a blank frame.
//!
//! Every `nav` below is `display: none` until its own rule shows it. A row
//! ending in 1 has the parent its rule asks for; the rows ending in 0 do
//! not. The expected row is what the oracle Chromium 143 computes for this
//! page.

use super::*;

const PAGE: &str = r#"<!doctype html>
<html><head><style>
nav { display: none; }
[data-anim] > .a { display: block; }
[data-anim=fade] > .b { display: block; }
[data-anim]:not([data-view=off]) > .c { display: block; }
[data-anim]:not([data-view=off])[data-enter=e1] > * { display: block; }
div[data-x] .e { display: block; }
.wrap:not([data-off]) .f { display: block; }
section:not(#main) .g { display: block; }
[data-n~=b] .h { display: block; }
[data-t^=da] .i { display: block; }
[lang|=en] > .j { display: block; }
[data-u$=z] .k { display: block; }
[data-v*=mid] .l { display: block; }
:is([data-a="1"], [data-a="2"]) > .m { display: block; }
[data-anim] [data-enter=e2] .n { display: block; }
[data-q="x y"] > .o { display: block; }
:not([data-anim]) > .p { display: block; }
.card[data-state=open] .r { display: block; }
</style></head><body>
<div data-anim><nav class="a">a1</nav></div>
<div><nav class="a">a0</nav></div>
<div data-anim="fade"><nav class="b">b1</nav></div>
<div data-anim="slide"><nav class="b">b0</nav></div>
<div data-anim><nav class="c">c1</nav></div>
<div data-anim data-view="off"><nav class="c">c0</nav></div>
<div><nav class="c">c00</nav></div>
<div data-anim data-enter="e1"><nav>d1</nav></div>
<div data-anim data-enter="e1" data-view="off"><nav>d0</nav></div>
<div data-enter="e1"><nav>d00</nav></div>
<div data-x><div><nav class="e">e1</nav></div></div>
<div><div><nav class="e">e0</nav></div></div>
<div class="wrap"><nav class="f">f1</nav></div>
<div class="wrap" data-off><nav class="f">f0</nav></div>
<section><nav class="g">g1</nav></section>
<section id="main"><nav class="g">g0</nav></section>
<div data-n="a b"><nav class="h">h1</nav></div>
<div data-n="ab c"><nav class="h">h0</nav></div>
<div data-t="dark"><nav class="i">i1</nav></div>
<div data-t="light"><nav class="i">i0</nav></div>
<div lang="en-US"><nav class="j">j1</nav></div>
<div lang="fr"><nav class="j">j0</nav></div>
<div data-u="xyz"><nav class="k">k1</nav></div>
<div data-u="zyx"><nav class="k">k0</nav></div>
<div data-v="amidb"><nav class="l">l1</nav></div>
<div data-v="none"><nav class="l">l0</nav></div>
<div data-a="2"><nav class="m">m1</nav></div>
<div data-a="3"><nav class="m">m0</nav></div>
<div data-anim><div data-enter="e2"><nav class="n">n1</nav></div></div>
<div><div data-enter="e2"><nav class="n">n0</nav></div></div>
<div data-anim><div data-enter="e3"><nav class="n">n00</nav></div></div>
<div data-q="x y"><nav class="o">o1</nav></div>
<div data-q="x"><nav class="o">o0</nav></div>
<div><nav class="p">p1</nav></div>
<div data-anim><nav class="p">p0</nav></div>
<div class="card" data-state="open"><nav class="r">r1</nav></div>
<div class="card" data-state="closed"><nav class="r">r0</nav></div>
<div class="other" data-state="open"><nav class="r">r00</nav></div>
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
fn an_attribute_or_negation_on_an_ancestor_is_tested_against_that_ancestor() {
    assert_eq!(
        shown(PAGE).join(" "),
        "a1 b1 c1 d1 e1 f1 g1 h1 i1 j1 k1 l1 m1 n1 o1 p1 r1"
    );
}
