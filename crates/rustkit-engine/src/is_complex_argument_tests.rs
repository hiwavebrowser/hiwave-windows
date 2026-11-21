//! `:is()`, `:where()` and `:not()` with a complex selector (one with a
//! combinator) as an argument.
//!
//! The matcher used to read each argument as a compound on the element
//! itself, and an argument with a combinator never matched
//! (`keys_for_compound`, `any_compound_in_list_matches`); now such a member
//! is a prepared complex selector with the element as its subject
//! (`ListMember::Complex`). Tailwind v4 writes its dark variant as
//! `.dark\:hidden:is(.dark *)`: on railway.com the page puts `class="dark"`
//! on `<html>`, and Chrome hides the day hero image and shows the dusk one;
//! RustKit shows the day image and every `dark:` rule is lost (Pete's hand
//! test 10-27).
//!
//! Every `nav` is `display: none` until its own rule shows it. The expected
//! row is what the pinned Chrome 148 computes for this page.

use super::*;

const PAGE: &str = r#"<!doctype html>
<html class="dark"><head><style>
nav { display: none; }
.a:is(.dark *) { display: block; }
.b:where(.dark, .dark *) { display: block; }
.c:is(.x, .dark *) { display: block; }
.d:is(.dark > body > .col > *) { display: block; }
.e:is(.light *) { display: block; }
.f:not(.dark *) { display: block; }
:is(.col .g) { display: block; }
.h:is(.col > .i *) { display: block; }
</style></head><body>
<div class="col">
<nav class="a">a</nav><nav class="b">b</nav><nav class="c">c</nav><nav class="d">d</nav><nav class="e">e</nav>
<nav class="f">f</nav><nav class="g">g</nav><div class="i"><nav class="h">h</nav></div>
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
        for c in &b.children {
            texts(c, out);
        }
    }
    let mut out = Vec::new();
    texts(&layout, &mut out);
    out
}

#[test]
fn a_complex_selector_inside_is_where_and_not_matches_against_the_ancestors() {
    assert_eq!(shown(PAGE).join(" "), "a b c d g h");
}

/// What `shown` reads, from a headless view's current layout (the restyle
/// path after a script).
fn shown_in_view(engine: &Engine, view: EngineViewId) -> String {
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
    texts(engine.views[&view].layout.as_ref().expect("layout"), &mut out);
    out.join(" ")
}

const COMBINATORS: &str = r#"<!doctype html>
<html><head><style>
nav { display: none; }
.d:is(.a .b) { display: block; }
.c:is(.a > .b) { display: block; }
.s:where(.p + .b) { display: block; }
.n:not(.a .b) { display: block; }
.nest:is(:is(.a .b)) { display: block; }
.nn:not(:is(.a .b)) { display: block; }
.y.z { display: none; }
.y:is(#x .y, .z) { display: block; }
.anc:is(.a *) > nav { display: block; }
.sib:is(.a *) ~ nav { display: block; }
.tw:is(.a *):is(.q + *) { display: block; }
</style></head><body>
<div class="a">
  <nav class="b d">d-in</nav>
  <div><nav class="b c">c-deep</nav></div>
  <nav class="b c">c-direct</nav>
  <nav class="p"></nav><nav class="b s">s-adjacent</nav>
  <nav class="b n">n-in</nav>
  <nav class="b nest">nest-in</nav>
  <nav class="b nn">nn-in</nav>
  <div id="x"><nav class="y z">yz</nav></div>
  <div class="anc"><nav>under-anc</nav></div>
  <nav class="sib"></nav><nav>after-sib</nav>
  <nav class="q"></nav><nav class="tw">tw</nav>
</div>
<div class="o">
  <nav class="b d">d-out</nav>
  <nav class="b s">s-out</nav>
  <nav class="b n">n-out</nav>
  <nav class="b nest">nest-out</nav>
  <nav class="b nn">nn-out</nav>
  <nav class="y z">yz-out</nav>
  <div class="anc"><nav>under-anc-out</nav></div>
  <nav class="sib"></nav><nav>after-sib-out</nav>
</div>
</body></html>"#;

#[test]
fn each_combinator_inside_is_where_and_not_reads_the_chain_and_the_siblings() {
    // `.c:is(.a > .b)`: `c-deep` has a div between it and `.a`.
    // `:not(.a .b)` shows the nav outside `.a` only; `:not(:is(.a .b))` the same.
    // `:is(#x .y, .z)` is (1,1,0) whichever member matches (Selectors 4 §17:
    // the most specific argument), so it beats `.y.z` (0,2,0) outside `#x` too.
    // Left of a combinator: `.anc:is(.a *) > nav` and `.sib:is(.a *) ~ nav`
    // decide the member against the ancestor's or sibling's own chain.
    assert_eq!(
        shown(COMBINATORS).join(" "),
        "d-in c-direct s-adjacent nest-in yz under-anc after-sib tw n-out nn-out yz-out"
    );
}

/// Left of a combinator, an `:is()` that has a complex member is decided
/// against the ancestor's chain (`complex_alternatives_hold`), and so are
/// its compound members. Those were read by tag, classes and id only, so a
/// compound member that is only an attribute selector or a `:not()` matched
/// every ancestor (Castor A1 audit of #822): the theme toggle
/// `:is([data-theme="dark"], .dark *) > .child` painted the dark theme
/// under `data-theme="light"`.
const THEMES: &str = r#"<!doctype html>
<html><head><style>
nav { display: none; }
:is([data-theme="dark"], .dark *) > .k { display: block; }
:is([data-theme="dark"], .dark *) .d { display: block; }
:is(:not(.light), .dark *) > .n { display: block; }
:is(:is([data-x]), .dark *) > .q { display: block; }
</style></head><body>
<div data-theme="light" class="light">
  <nav class="k">k-light</nav><div><nav class="d">d-light</nav></div><nav class="n">n-light</nav><nav class="q">q-light</nav>
</div>
<div data-theme="dark" data-x>
  <nav class="k">k-dark</nav><div><nav class="d">d-dark</nav></div><nav class="n">n-dark</nav><nav class="q">q-dark</nav>
</div>
<div class="dark"><div class="light">
  <nav class="k">k-under-dark</nav><nav class="n">n-under-dark</nav>
</div></div>
</body></html>"#;

#[test]
fn a_compound_member_beside_a_complex_one_reads_attributes_and_not() {
    // `d-dark`: the descendant combinator reaches `[data-theme="dark"]` past
    // the inner div. `n-dark`: the parent is not `.light`. `*-under-dark`:
    // the `.dark *` member holds for the `.light` parent.
    assert_eq!(shown(THEMES).join(" "), "k-dark d-dark n-dark q-dark k-under-dark n-under-dark");
}

#[test]
fn a_theme_attribute_flipped_by_script_restyles_the_is_member() {
    let page = r#"<!doctype html>
<html><head><style>
nav { display: none; }
:is([data-theme="dark"], .dark *) > .k { display: block; }
</style></head><body>
<div id="t" data-theme="light"><nav class="k">k</nav></div>
</body></html>"#;
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine.create_headless_view(Bounds { x: 0, y: 0, width: 400, height: 300 }).expect("view");
    engine.load_html(view, page).expect("load_html");
    assert_eq!(shown_in_view(&engine, view), "");
    engine.execute_script(view, "document.getElementById('t').dataset.theme = 'dark'").expect("dark");
    assert_eq!(shown_in_view(&engine, view), "k", "data-theme=dark on the parent shows it");
    engine.execute_script(view, "document.getElementById('t').dataset.theme = 'light'").expect("light");
    assert_eq!(shown_in_view(&engine, view), "", "and light hides it again");
}

#[test]
fn tailwind_dark_variant_follows_the_dark_class_on_html_through_restyles() {
    // Tailwind v4 as served: `.dark\:bg-black:is(.dark *)`, the escape as
    // the sheet parser keeps it.
    let page = r#"<!doctype html>
<html><head><style>
nav { display: none; }
.dark\:bg-black:is(.dark *) { display: block; }
.dark\:hidden:is(.dark *) { display: none; }
.day { display: block; }
</style></head><body>
<nav class="dark:bg-black">dusk</nav>
<nav class="day dark:hidden">day</nav>
</body></html>"#;
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine.create_headless_view(Bounds { x: 0, y: 0, width: 400, height: 300 }).expect("view");
    engine.load_html(view, page).expect("load_html");
    assert_eq!(shown_in_view(&engine, view), "day");
    engine.execute_script(view, "document.documentElement.classList.add('dark')").expect("add");
    assert_eq!(shown_in_view(&engine, view), "dusk", "adding dark on <html> restyles the variants");
    engine.execute_script(view, "document.documentElement.classList.remove('dark')").expect("remove");
    assert_eq!(shown_in_view(&engine, view), "day", "removing it restyles them back");
}

#[test]
fn query_selector_sees_complex_members_too() {
    let page = r#"<!doctype html><html class="dark"><body>
<div class="a"><p id="in"></p></div><p id="out"></p></body></html>"#;
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine.create_headless_view(Bounds { x: 0, y: 0, width: 400, height: 300 }).expect("view");
    engine.load_html(view, page).expect("load_html");
    let got = engine
        .execute_script(
            view,
            "[document.querySelectorAll('p:is(.a *)').length, \
              document.querySelectorAll('p:not(.a p)').length, \
              document.getElementById('in').matches(':is(.dark .a > p)'), \
              document.getElementById('out').matches(':where(.a + p)')].join(' ')",
        )
        .expect("script");
    assert_eq!(got, "String(\"1 1 true true\")");
}

/// The R1 routine's case list, one rule per case. Selectors 4 §4.2 (`:is`),
/// §4.3 (`:not`), §4.4 (`:where`): the argument is a <complex-real-selector-list>
/// whose members are matched with the element as their subject, so a
/// combinator inside the argument reaches the element's real ancestors and
/// earlier siblings. Chromium agrees (the expected text below is the set of
/// navs Chrome shows for this page).
const R1_CASES: &str = r#"<!doctype html>
<html><head><style>
nav { display: none; }
.t1:is(.dark *) { display: block; }
.t2:where(.a > .b) { display: block; }
.t3:not(.a .b) { display: block; }
.t4:is(.a + .b) { display: block; }
.t5:is(.a ~ .b) { display: block; }
.t6:is(:not(.a .b)) { display: block; }
</style></head><body>
<div class="dark"><nav class="t1">t1-dark</nav></div>
<div class="light"><nav class="t1">t1-light</nav></div>
<div class="a"><nav class="b t2">t2-child</nav><div><nav class="b t2">t2-grandchild</nav></div></div>
<div class="a"><div><nav class="b t3">t3-in</nav></div></div>
<div><nav class="b t3">t3-out</nav><nav class="t3">t3-not-b</nav></div>
<div><nav class="a"></nav><nav class="b t4">t4-adjacent</nav><nav class="b t4">t4-second</nav></div>
<div><nav class="a"></nav><p></p><nav class="b t5">t5-later</nav></div>
<div><nav class="b t5">t5-before</nav><nav class="a"></nav></div>
<div class="a"><nav class="b t6">t6-in</nav></div>
<div><nav class="b t6">t6-out</nav></div>
</body></html>"#;

#[test]
fn r1_is_where_not_complex_members_match_like_chromium() {
    // t1: `.dark *` holds only under `.dark`.
    // t2: `.a > .b` needs `.a` as the parent, not a grandparent.
    // t3: `:not(.a .b)` excludes only a `.b` inside `.a`.
    // t4: `.a + .b` only right after `.a`; t5: `.a ~ .b` anywhere after it,
    //     never before it.
    // t6: `:is(:not(.a .b))` is `:not(.a .b)`.
    assert_eq!(
        shown(R1_CASES).join(" "),
        "t1-dark t2-child t3-out t3-not-b t4-adjacent t5-later t6-out"
    );
}

#[test]
fn r1_specificity_of_complex_members() {
    let s = |sel: &str| SelectorMatcher.selector_specificity(sel);
    // §17: `:is()`/`:not()` take their most specific argument, `:where()` zero.
    assert_eq!(s(":is(.dark *)"), (0, 1, 0));
    assert_eq!(s(":where(.a > .b)"), (0, 0, 0));
    assert_eq!(s("p:where(.a > .b)"), (0, 0, 1));
    assert_eq!(s(":not(.a .b)"), (0, 2, 0));
    assert_eq!(s(":is(.a + .b)"), (0, 2, 0));
    assert_eq!(s(":is(.a ~ .b, #c d)"), (1, 0, 1));
    assert_eq!(s(":is(:not(.a .b))"), (0, 2, 0));
    // Tailwind v4's dark variant: one escaped class plus `:is(.dark *)`.
    assert_eq!(s(r".dark\:bg-black:is(.dark *)"), (0, 2, 0));
    assert_eq!(s(r".md\:hover\:x:is(.dark *)"), (0, 2, 0));
    // A hex escape and the one whitespace that ends it: `.\31 0` is the
    // class `10`, so `.\31 0:is(.a *)` is (0,2,0), and `.\31  p` is a
    // class then a descendant `p`, (0,1,1).
    assert_eq!(s(".\\31 0:is(.a *)"), (0, 2, 0));
    assert_eq!(s(".\\31  p"), (0, 1, 1));
    // `\` then a newline is not an escape (CSS Syntax §4.3.8): the class
    // ends at the backslash and the `p` after it is a type selector.
    assert_eq!(s(".a\\\np"), (0, 1, 1));
}

#[test]
fn r1_where_complex_member_adds_no_specificity_in_the_cascade() {
    // `.w:where(.a > .b)` is (0,1,0), so the later `.w` (0,1,0) wins on
    // source order; `.i:is(.a > .b)` is (0,3,0) and beats the later `.i.i`
    // (0,2,0).
    let page = r#"<!doctype html><html><head><style>
nav { display: none; }
.w:where(.a > .b) { display: block; }
.w { display: none; }
.i:is(.a > .b) { display: block; }
.i.i { display: none; }
</style></head><body><div class="a"><nav class="b w">w</nav><nav class="b i">i</nav></div></body></html>"#;
    assert_eq!(shown(page).join(" "), "i");
}

#[test]
fn r1_tailwind_dark_variant_beats_its_base_utility() {
    // `.bg-white` (0,1,0) then `.dark\:hidden:is(.dark *)` (0,2,0): under
    // `.dark` the variant wins whatever the order, as Tailwind relies on.
    let page = r#"<!doctype html><html class="dark"><head><style>
nav { display: none; }
.dark\:block:is(.dark *) { display: block; }
.shown { display: block; }
.dark\:hidden:is(.dark *) { display: none; }
.late { display: block; }
</style></head><body>
<nav class="dark:block">dusk</nav>
<nav class="shown dark:hidden">day</nav>
<nav class="dark:hidden late">late</nav>
</body></html>"#;
    assert_eq!(shown(page).join(" "), "dusk");
}
