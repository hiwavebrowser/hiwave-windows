//! `:has()`, the relational pseudo-class (Selectors 4 §4.5).
//!
//! It used to be hard-coded to never match, so every `:has()` rule was
//! dropped and every `:not(:has(x))` matched everything. A census of 37
//! real stylesheets found ~292 `:has()` rules across the top sites.
//!
//! Every `nav` below is `display: none` until its own rule shows it. A row
//! ending in 1 is one Chromium shows; the rows ending in 0 it does not.

use super::*;

/// The texts of the boxes the build kept, in tree order.
fn shown_in(engine: &Engine, document: &Document) -> Vec<String> {
    let layout = engine.build_layout_from_document(document, &[]);
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

fn shown(html: &str) -> String {
    let document = Document::parse_html(html).expect("html");
    let engine = Engine::new(EngineConfig::default()).expect("engine");
    shown_in(&engine, &document).join(" ")
}

const COMBINATORS: &str = r#"<!doctype html>
<html><head><style>
nav { display: none; }
nav.a:has(i.t) { display: block; }
nav.b:has(> i.t) { display: block; }
nav.c:has(+ i.t) { display: block; }
nav.d:has(~ i.t) { display: block; }
nav.e:has(> span .t) { display: block; }
nav.f:has(+ div > .t) { display: block; }
nav.k:has(.u, > .t) { display: block; }
</style></head><body>
<div><nav class="a"><span><i class="t"></i></span>a1</nav></div>
<div><nav class="a"><span><i class="u"></i></span>a0</nav></div>
<div><nav class="b"><i class="t"></i>b1</nav></div>
<div><nav class="b"><span><i class="t"></i></span>b0</nav></div>
<div><nav class="c">c1</nav><i class="t"></i></div>
<div><nav class="c">c0</nav><b></b><i class="t"></i></div>
<div><i class="t"></i><nav class="c">c00</nav></div>
<div><nav class="d">d1</nav><b></b><i class="t"></i></div>
<div><i class="t"></i><nav class="d">d0</nav></div>
<div><nav class="d">d00<i class="t"></i></nav></div>
<div><nav class="e"><span><b><i class="t"></i></b></span>e1</nav></div>
<div><nav class="e"><b><i class="t"></i></b>e0</nav></div>
<div><nav class="e"><span></span><i class="t"></i>e00</nav></div>
<div><nav class="f">f1</nav><div><i class="t"></i></div></div>
<div><nav class="f">f0</nav><div><b><i class="t"></i></b></div></div>
<div><nav class="f">f00</nav><p></p><div><i class="t"></i></div></div>
<div><nav class="k"><b><i class="u"></i></b>k1</nav></div>
<div><nav class="k"><i class="t"></i>k2</nav></div>
<div><nav class="k"><b><i class="t"></i></b>k0</nav></div>
</body></html>"#;

/// Each relative combinator searches where Selectors 4 says: `:has(x)` and
/// `:has(> x)` the anchor's descendants and children, `:has(+ x)` and
/// `:has(~ x)` its next and its later siblings; a longer relative selector
/// is matched from the anchor outward.
#[test]
fn each_relative_combinator_searches_the_right_elements() {
    assert_eq!(shown(COMBINATORS), "a1 b1 c1 d1 e1 f1 k1 k2");
}

const POSITIONS: &str = r#"<!doctype html>
<html><head><style>
nav { display: none; }
.g:has(.t) nav { display: block; }
.h:has(> .t) > nav { display: block; }
.j:has(.t) + nav { display: block; }
.l:has(.t) ~ nav { display: block; }
</style></head><body>
<div class="g"><i class="t"></i><div><nav>g1</nav></div></div>
<div class="g"><i class="u"></i><div><nav>g0</nav></div></div>
<div class="h"><i class="t"></i><nav>h1</nav></div>
<div class="h"><b><i class="t"></i></b><nav>h0</nav></div>
<div><div class="j"><i class="t"></i></div><nav>j1</nav></div>
<div><div class="j"><i class="u"></i></div><nav>j0</nav></div>
<div><div class="l"><i class="t"></i></div><p></p><nav>l1</nav></div>
<div><div class="l"></div><p></p><nav>l0</nav></div>
</body></html>"#;

/// `:has()` on a compound left of a combinator is tested against that
/// ancestor or sibling, not against the subject.
#[test]
fn a_has_on_an_ancestor_or_a_sibling_is_tested_against_that_element() {
    assert_eq!(shown(POSITIONS), "g1 h1 j1 l1");
}

const NESTED: &str = r#"<!doctype html>
<html><head><style>
nav { display: none; }
nav.n:not(:has(.t)) { display: block; }
nav.m:is(:has(> .t)) { display: block; }
nav.w:where(.w:has(.t), .never) { display: block; }
.o:not(:has(.t)) > nav { display: block; }
.q:is(:has(.t)) nav { display: block; }
</style></head><body>
<nav class="n"><i class="u"></i>n1</nav>
<nav class="n"><b><i class="t"></i></b>n0</nav>
<nav class="m"><i class="t"></i>m1</nav>
<nav class="m"><b><i class="t"></i></b>m0</nav>
<nav class="w"><b><i class="t"></i></b>w1</nav>
<nav class="w">w0</nav>
<div class="o"><nav>o1</nav></div>
<div class="o"><i class="t"></i><nav>o0</nav></div>
<div class="q"><i class="t"></i><nav>q1</nav></div>
<div class="q"><nav>q0</nav></div>
</body></html>"#;

/// `:has()` inside `:not()`, `:is()` and `:where()`, in the subject and on
/// an ancestor. `:not(:has(x))` used to match every element.
#[test]
fn has_nests_inside_not_is_and_where() {
    assert_eq!(shown(NESTED), "n1 m1 w1 o1 q1");
}

const INVALID: &str = r#"<!doctype html>
<html><head><style>
nav { display: none; }
nav.x:has(:has(.t)) { display: block; }
nav.y:has(:has(.t)), nav.y2 { display: block; }
nav.v:not(:has(:has(.t))) { display: block; }
nav.e:has() { display: block; }
nav.p:has(::before) { display: block; }
nav:is(.z:has(:has(.t)), .z) { display: block; }
</style></head><body>
<nav class="x"><b><i class="t"></i></b>x0</nav>
<nav class="y"><b><i class="t"></i></b>y0</nav>
<nav class="y2">y20</nav>
<nav class="v">v0</nav>
<nav class="e">e0</nav>
<nav class="p">p0</nav>
<nav class="z"><b><i class="t"></i></b>z1</nav>
</body></html>"#;

/// `:has()` inside `:has()` is invalid (Selectors 4 §4.5), so the rule is
/// dropped with its whole list, and `:not()` of it does not turn into
/// "everything". `:is()` forgives the bad member and keeps the rest.
#[test]
fn has_inside_has_is_invalid() {
    assert_eq!(shown(INVALID), "z1");
    for selector in [
        ":has(:has(.t))",
        "a:not(:has(:has(b)))",
        ":has()",
        "a:has(b), :has(:has(c))",
    ] {
        assert!(
            !SelectorMatcher::selector_list_is_valid(selector),
            "{selector} is invalid"
        );
    }
    for selector in [
        ":has(.t)",
        ":has(> a, + b, ~ c)",
        "a:not(:has(b))",
        ":is(:has(:has(a)), b)",
    ] {
        assert!(
            SelectorMatcher::selector_list_is_valid(selector),
            "{selector} is valid"
        );
    }
}

/// Selectors 4 §17: `:has()` takes the specificity of its most specific
/// argument, like `:is()`.
#[test]
fn has_takes_its_most_specific_arguments_specificity() {
    let m = SelectorMatcher;
    assert_eq!(m.selector_specificity(":has(#x)"), (1, 0, 0));
    assert_eq!(m.selector_specificity("div:has(.a, #b)"), (1, 0, 1));
    assert_eq!(m.selector_specificity("li:has(> a.b)"), (0, 1, 2));
    assert_eq!(m.selector_specificity(".c:has(+ p span)"), (0, 1, 2));
    // In the cascade: (1,0,1) beats a later (0,2,1).
    let page = r#"<!doctype html><html><head><style>
        nav:has(#k) { display: block; }
        nav.p.q { display: none; }
        </style></head><body>
        <nav class="p q"><i id="k"></i>s1</nav>
        <nav class="p q"><i></i>s0</nav>
        </body></html>"#;
    assert_eq!(shown(page), "s1");
}

/// A rule that reads `:has()` is matched afresh for every element: its
/// answer depends on the element's subtree or siblings, which the match
/// sharing key does not hold.
#[test]
fn a_has_rule_is_never_shared() {
    for selector in [
        ".a:has(.b)",
        ".a:has(.b) .c",
        ".a:not(:has(.b))",
        ":is(.a:has(> .b)) > .c",
    ] {
        let mut reads = SelectorReads::default();
        assert!(
            reads.note(&SelectorMatcher.prepared_selector(selector)),
            "{selector} is positional"
        );
    }
    // Look-alike siblings under look-alike parents: only the subtree differs.
    let page = r#"<!doctype html><html><head><style>
        nav { display: none; } .r:has(.t) > nav { display: block; } nav:has(.t) { display: block; }
        </style></head><body>
        <div class="r"><nav>r0</nav></div><div class="r"><i class="t"></i><nav>r1</nav></div>
        <div class="r"><nav>r00</nav></div>
        <section><nav>u0</nav><nav><i class="t"></i>u1</nav><nav>u00</nav></section>
        </body></html>"#;
    assert_eq!(shown(page), "r1 u1");
}

/// A failing multi-step relative selector searches each subtree once per
/// anchor, not once per element that matched a step on the way down: on a
/// complete binary tree of 511 divs, `div:has(div div .missing)` used to
/// test 35,334 compounds (each nested `div` restarted the search below it).
#[test]
fn a_failing_multi_step_has_searches_each_subtree_once_per_anchor() {
    fn tree(depth: usize) -> String {
        match depth {
            0 => String::new(),
            _ => format!("<div>{}{}</div>", tree(depth - 1), tree(depth - 1)),
        }
    }
    let page = format!(
        "<!doctype html><html><head><style>div:has(div div .missing) {{ color: red; }}</style></head><body>{}</body></html>",
        tree(9)
    );
    let document = Document::parse_html(&page).expect("html");
    let engine = Engine::new(EngineConfig::default()).expect("engine");
    HAS_COMPOUND_TESTS.with(|n| n.set(0));
    engine.build_layout_from_document(&document, &[]);
    let tested = HAS_COMPOUND_TESTS.with(|n| n.get());
    // Each of the 511 anchors tests each node under it at most once per
    // step: 3 steps * 3586 (the nodes under all anchors, summed).
    assert!(tested <= 3 * 3586, "{tested} compound tests");
}

/// A slotted element is matched against its host's light-tree chain, which
/// is its DOM parents: `:has()` on that ancestor is decided on the host.
#[test]
fn a_has_on_the_host_reaches_its_slotted_children() {
    let page = r#"<!doctype html><html><head><style>
        nav { display: none; } .host:has(> .t) > nav { display: block; }
        </style></head><body>
        <div class="host"><template shadowrootmode="open"><p><slot></slot></p></template><i class="t"></i><nav>s1</nav></div>
        <div class="host"><template shadowrootmode="open"><p><slot></slot></p></template><nav>s0</nav></div>
        </body></html>"#;
    assert_eq!(shown(page), "s1");
}

/// The DOM bindings' matcher (`querySelector`, `matches`) answers the same.
#[test]
fn script_queries_match_has_against_the_live_tree() {
    let document = Document::parse_html(
        "<div id=a><p class=t></p></div><div id=b><span><p class=t></p></span></div><div id=c></div><p id=d></p>",
    )
    .expect("html");
    let m = SelectorMatcher;
    let no_checked = HashMap::new();
    let ask = |id: &str, selector: &str| {
        m.node_matches(
            &document.get_element_by_id(id).unwrap(),
            selector,
            &no_checked,
        )
    };
    assert_eq!(ask("a", "div:has(> .t)"), Some(true));
    assert_eq!(ask("b", "div:has(> .t)"), Some(false));
    assert_eq!(ask("b", "div:has(.t)"), Some(true));
    assert_eq!(ask("c", "div:not(:has(*))"), Some(true));
    assert_eq!(ask("c", "div:has(+ p)"), Some(true));
    assert_eq!(ask("b", "div:has(+ p)"), Some(false));
    assert_eq!(ask("a", "div:has(~ #d)"), Some(true));
    assert_eq!(ask("a", ":has(:has(p))"), None);
}

/// Appending the child that makes `:has()` match restyles the anchor, and
/// removing it restyles it back, through the same engine's caches.
#[test]
fn a_dom_change_under_the_anchor_restyles_it_both_ways() {
    let document = Document::parse_html(
        r#"<!doctype html><html><head><style>
        nav { display: none; } nav:has(.on) { display: block; }
        .box:not(:has(> .on)) + p { display: none; }
        </style></head><body><div class="box" id="box"><nav id="n">shown</nav></div><p>after</p></body></html>"#,
    )
    .expect("html");
    let engine = Engine::new(EngineConfig::default()).expect("engine");
    assert_eq!(shown_in(&engine, &document), Vec::<String>::new());

    let nav = document.get_element_by_id("n").expect("nav");
    let mut attributes = HashMap::new();
    attributes.insert("class".to_string(), "on".to_string());
    let on = document.create_node(NodeType::Element {
        tag_name: "i".into(),
        namespace: String::new(),
        attributes,
    });
    nav.append_child(on.clone());
    assert_eq!(shown_in(&engine, &document), vec!["shown"]);

    // Moved up to the box: the nav loses it, the paragraph gains it.
    on.remove_from_parent();
    document
        .get_element_by_id("box")
        .unwrap()
        .append_child(on.clone());
    assert_eq!(shown_in(&engine, &document), vec!["after"]);

    on.remove_from_parent();
    assert_eq!(shown_in(&engine, &document), Vec::<String>::new());
}

/// docs/diagnostics/2026-10-20/css_has.html (PR #747): four boxes, all
/// green in Chromium; any red is a fail.
#[test]
fn the_css_census_has_page_paints_four_green_boxes() {
    let page = r#"<!doctype html><!-- :has() and the :not(:has()) inversion (ebay/github/x).
 PASS: four 100x100 boxes green at pixels (60,60) (170,60) (280,60) (390,60). FAIL: any red. -->
<html><head><style>
body { margin: 0; } .p { position: absolute; top: 10px; width: 100px; height: 100px; background: red; }
#p1:has(> .x) { background: green; }
#p2 { background: green; } #p2:not(:has(> .x)) { background: red; }
#p3:not(:has(> .x)) { background: green; }
#p4 { background: green; } #p4:has(.none) { background: red; }
</style></head><body>
<div class="p" id="p1" style="left:10px"><i class="x"></i></div><div class="p" id="p2" style="left:120px"><i class="x"></i></div>
<div class="p" id="p3" style="left:230px"><i class="y"></i></div><div class="p" id="p4" style="left:340px"><i class="y"></i></div>
</body></html>"#;
    let document = Document::parse_html(page).expect("html");
    let engine = Engine::new(EngineConfig::default()).expect("engine");
    let layout = engine.build_layout_from_document(&document, &[]);
    fn find(b: &LayoutBox, raw: usize) -> Option<&LayoutBox> {
        if b.node_id == Some(raw) {
            return Some(b);
        }
        b.children.iter().find_map(|c| find(c, raw))
    }
    let colors: Vec<String> = ["p1", "p2", "p3", "p4"]
        .iter()
        .map(|id| {
            let raw = document.get_element_by_id(id).unwrap().id.raw();
            let c = find(&layout, raw).expect("box").style.background_color;
            format!("{id}:{},{},{}", c.r, c.g, c.b)
        })
        .collect();
    assert_eq!(
        colors.join(" "),
        "p1:0,128,0 p2:0,128,0 p3:0,128,0 p4:0,128,0"
    );
}

/// A script that appends and removes the child: `getComputedStyle` follows.
#[cfg(feature = "headless")]
#[test]
fn a_script_append_and_remove_restyles_the_anchor() {
    use crate::script_net_tests::serve_routes;
    let page = "<html><head><style>#a{color:rgb(0, 0, 255)} #a:has(.on){color:rgb(0, 128, 0)}</style></head>\
                <body><div id=a>x</div></body></html>";
    let server = serve_routes(vec![("/", "text/html", page.to_string())]);
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds {
            x: 0,
            y: 0,
            width: 400,
            height: 200,
        })
        .expect("view");
    let url = Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(engine.load_url(view, url)).expect("load_url");
    let color = "getComputedStyle(document.getElementById('a')).color";
    assert_eq!(
        engine.execute_script(view, color).unwrap(),
        "String(\"rgb(0, 0, 255)\")"
    );
    engine
        .execute_script(
            view,
            "var i = document.createElement('i'); i.className = 'on'; \
             document.getElementById('a').appendChild(i); 1",
        )
        .unwrap();
    assert_eq!(
        engine.execute_script(view, color).unwrap(),
        "String(\"rgb(0, 128, 0)\")"
    );
    engine
        .execute_script(view, "document.querySelector('.on').remove(); 1")
        .unwrap();
    assert_eq!(
        engine.execute_script(view, color).unwrap(),
        "String(\"rgb(0, 0, 255)\")"
    );
}
