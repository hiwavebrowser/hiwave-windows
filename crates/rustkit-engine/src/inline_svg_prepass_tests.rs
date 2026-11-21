//! LAT4b: the inline svg pre-pass before every build (`cache_svg_paint_styles`
//! and `cache_inline_svgs`). Each inline `<svg>` that `<use>`s a sprite
//! symbol walked the whole document to find it, twice per relayout (the
//! pre-pass and the box build), and the author paint styles re-read every
//! rule of every sheet. On a sprite-icon page that cost was paid by every
//! relayout, a hover restyle's included.
//!
//! The cost pins run on a reduced page shaped like the one LAT4 measured:
//! 20 sheets of 250 rules, 150 sprite icons, about 1.5k nodes. The
//! correctness pins check that what is cached follows the DOM: a symbol
//! changed after its first use, a symbol parsed or inserted after the
//! icon, nested `<use>`, `xlink:href`, a class rule, `currentColor`.

use super::*;

/// 20 sheets x 250 rules (of which 50 are svg paint rules), a hidden
/// sprite sheet of 30 symbols, 150 icons naming them, and filler.
fn reduced_page() -> String {
    let mut html = String::from("<!DOCTYPE html><html><head>");
    for sheet in 0..20 {
        html.push_str("<style>");
        for r in 0..250 {
            let n = sheet * 250 + r;
            if n % 100 == 0 {
                html.push_str(&format!(".k{} path{{fill:#00{:02x}00}}\n", n / 100, n % 256));
            } else {
                html.push_str(&format!(".c{n} {{color:#{:06x};margin-left:{}px}}\n", n * 37 % 0xffffff, n % 7));
            }
        }
        html.push_str("</style>");
    }
    html.push_str("</head><body>");
    html.push_str(r#"<svg style="display:none" xmlns="http://www.w3.org/2000/svg">"#);
    for k in 0..30 {
        html.push_str(&format!(
            r#"<symbol id="i{k}" viewBox="0 0 10 10"><path d="M0 0h{w}v10H0z"/></symbol>"#,
            w = 1 + k % 9
        ));
    }
    html.push_str("</svg>");
    for i in 0..150 {
        html.push_str(&format!(
            r##"<span class="c{i}"><svg class="k{}" width="10" height="10"><use href="#i{}"/></svg></span>"##,
            i % 50,
            i % 30
        ));
    }
    for i in 0..320 {
        html.push_str(&format!("<div class=\"c{}\"><p>row {i}</p></div>", i * 7));
    }
    html.push_str("</body></html>");
    html
}

fn view_with(html: &str, width: u32, height: u32) -> (Engine, EngineViewId) {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width, height })
        .expect("view");
    engine.load_html(view, html).expect("load_html");
    (engine, view)
}

fn markup_builds() -> usize {
    INLINE_SVG_MARKUP_BUILDS.with(std::cell::Cell::get)
}

/// A relayout with nothing changed does no svg work worth the name: the
/// pre-pass is at most 2 ms (debug build) and serializes no svg.
#[test]
fn a_relayout_with_nothing_changed_redoes_no_inline_svg_work() {
    let html = reduced_page();
    let (mut engine, view) = view_with(&html, 800, 600);
    let nodes = engine.views[&view].document.as_ref().expect("document").root().subtree_len();
    assert!((1400..2200).contains(&nodes), "the reduced page has about 1.5k nodes: {nodes}");
    // One more to settle anything the load left (late caches).
    engine.relayout(view).expect("relayout");
    let mut svg_ms = Vec::new();
    let before = markup_builds();
    for _ in 0..3 {
        engine.relayout(view).expect("relayout");
        svg_ms.push(engine.relayout_stats(view).expect("view").last_svg_ms);
    }
    let builds = markup_builds() - before;
    eprintln!("LAT4b reduced page: {nodes} nodes, svg_ms per relayout {svg_ms:?}, markups built {builds}");
    assert_eq!(builds, 0, "no inline svg is serialized again when nothing changed");
    let best = svg_ms.iter().cloned().fold(f64::INFINITY, f64::min);
    assert!(best <= 2.0, "pre-build svg work with nothing changed: {svg_ms:?} ms (want <= 2)");
}

/// A pointer restyle (`:hover` on a paint property) does no svg work.
#[test]
fn a_hover_restyle_redoes_no_inline_svg_work() {
    let html = reduced_page().replace(
        "</head>",
        "<style>span:hover{background:#eee}</style></head>",
    );
    let (mut engine, view) = view_with(&html, 800, 600);
    engine.relayout(view).expect("relayout");
    let before = markup_builds();
    for x in [3.0, 25.0, 45.0, 3.0] {
        engine.mouse_move_at_point(view, x, 5.0);
        let _ = engine.relayout(view);
    }
    assert_eq!(markup_builds() - before, 0, "hover over icons serializes no svg");
}

/// The colours of the solid fills in the view's display list.
fn fills(engine: &Engine, view: EngineViewId) -> Vec<(u8, u8, u8)> {
    let list = engine.views[&view].display_list.as_ref().expect("display list");
    let mut out = Vec::new();
    for c in &list.commands {
        if let rustkit_layout::DisplayCommand::FillRect { color, .. }
        | rustkit_layout::DisplayCommand::FillPolygon { color, .. } = c
        {
            out.push((color.r, color.g, color.b));
        }
    }
    out
}

const BLUE: (u8, u8, u8) = (0, 0, 255);
const RED: (u8, u8, u8) = (255, 0, 0);

fn run(engine: &mut Engine, view: EngineViewId, script: &str) {
    engine.execute_script(view, script).expect("script");
    engine.relayout(view).expect("relayout");
}

#[test]
fn a_symbol_changed_after_its_first_use_paints_changed() {
    let (mut engine, view) = view_with(
        r##"<!DOCTYPE html><body><svg style="display:none"><symbol id="s" viewBox="0 0 10 10"><rect width="10" height="10" fill="#ff0000"/></symbol></svg><svg width="20" height="20"><use href="#s"/></svg></body>"##,
        200,
        100,
    );
    assert!(fills(&engine, view).contains(&RED), "{:?}", fills(&engine, view));
    run(&mut engine, view, "document.querySelector('#s rect').setAttribute('fill', '#0000ff')");
    let got = fills(&engine, view);
    assert!(got.contains(&BLUE) && !got.contains(&RED), "the edited symbol paints: {got:?}");
    // Its subtree changed too: a child added to the symbol paints.
    run(
        &mut engine,
        view,
        "var r = document.createElementNS('http://www.w3.org/2000/svg', 'rect'); r.setAttribute('width', '5'); r.setAttribute('height', '5'); r.setAttribute('fill', '#ff0000'); document.getElementById('s').appendChild(r)",
    );
    let got = fills(&engine, view);
    assert!(got.contains(&BLUE) && got.contains(&RED), "the added rect paints: {got:?}");
}

#[test]
fn a_use_names_a_symbol_parsed_after_it() {
    let (engine, view) = view_with(
        r##"<!DOCTYPE html><body><svg width="20" height="20"><use href="#s"/></svg><svg style="display:none"><symbol id="s" viewBox="0 0 10 10"><rect width="10" height="10" fill="#0000ff"/></symbol></svg></body>"##,
        200,
        100,
    );
    assert!(fills(&engine, view).contains(&BLUE), "{:?}", fills(&engine, view));
}

#[test]
fn a_use_names_a_symbol_script_inserts_later() {
    let (mut engine, view) = view_with(
        r##"<!DOCTYPE html><body><svg width="20" height="20"><use href="#s"/></svg><div id="late"></div></body>"##,
        200,
        100,
    );
    assert!(!fills(&engine, view).contains(&BLUE));
    run(
        &mut engine,
        view,
        r##"document.getElementById('late').innerHTML = '<svg style="display:none"><symbol id="s" viewBox="0 0 10 10"><rect width="10" height="10" fill="#0000ff"/></symbol></svg>'"##,
    );
    assert!(fills(&engine, view).contains(&BLUE), "{:?}", fills(&engine, view));
    // And a second element with the id earlier in the tree takes over.
    run(
        &mut engine,
        view,
        r##"document.body.insertAdjacentHTML('afterbegin', '<svg style="display:none"><symbol id="s" viewBox="0 0 10 10"><rect width="10" height="10" fill="#ff0000"/></symbol></svg>')"##,
    );
    let got = fills(&engine, view);
    assert!(got.contains(&RED) && !got.contains(&BLUE), "the first element with the id wins: {got:?}");
}

#[test]
fn a_nested_use_and_an_xlink_href_resolve() {
    let (mut engine, view) = view_with(
        r##"<!DOCTYPE html><body><svg style="display:none"><symbol id="inner" viewBox="0 0 10 10"><rect width="10" height="10" fill="#0000ff"/></symbol><symbol id="outer" viewBox="0 0 10 10"><use href="#inner"/></symbol><symbol id="x" viewBox="0 0 10 10"><rect width="10" height="10" fill="#ff0000"/></symbol></svg><svg width="20" height="20"><use href="#outer"/></svg><svg width="20" height="20"><use xlink:href="#x"/></svg></body>"##,
        200,
        100,
    );
    let got = fills(&engine, view);
    assert!(got.contains(&BLUE), "nested use: {got:?}");
    assert!(got.contains(&RED), "xlink:href: {got:?}");
    // The innermost symbol changes: the outer icon follows.
    run(&mut engine, view, "document.querySelector('#inner rect').setAttribute('fill', '#00ff00')");
    let got = fills(&engine, view);
    assert!(got.contains(&(0, 255, 0)) && !got.contains(&BLUE), "nested change: {got:?}");
}

#[test]
fn an_icon_follows_a_class_rule_when_its_class_changes() {
    let (mut engine, view) = view_with(
        r##"<!DOCTYPE html><style>.on{fill:#0000ff}</style><body><svg style="display:none"><symbol id="s" viewBox="0 0 10 10"><rect width="10" height="10"/></symbol></svg><svg id="icon" fill="#ff0000" width="20" height="20"><use href="#s"/></svg></body>"##,
        200,
        100,
    );
    assert!(fills(&engine, view).contains(&RED), "{:?}", fills(&engine, view));
    run(&mut engine, view, "document.getElementById('icon').classList.add('on')");
    let got = fills(&engine, view);
    assert!(got.contains(&BLUE) && !got.contains(&RED), "class added: {got:?}");
    run(&mut engine, view, "document.getElementById('icon').classList.remove('on')");
    let got = fills(&engine, view);
    assert!(got.contains(&RED) && !got.contains(&BLUE), "class removed: {got:?}");
}

#[test]
fn current_color_follows_the_parents_colour() {
    let (mut engine, view) = view_with(
        r##"<!DOCTYPE html><style>.b{color:#0000ff}</style><body><svg style="display:none"><symbol id="s" viewBox="0 0 10 10"><rect width="10" height="10" fill="currentColor"/></symbol></svg><span id="p" style="color:#ff0000"><svg width="20" height="20"><use href="#s"/></svg></span></body>"##,
        200,
        100,
    );
    assert!(fills(&engine, view).contains(&RED), "{:?}", fills(&engine, view));
    run(&mut engine, view, "var p = document.getElementById('p'); p.style.color = ''; p.className = 'b'");
    let got = fills(&engine, view);
    assert!(got.contains(&BLUE) && !got.contains(&RED), "currentColor follows: {got:?}");
}

/// A DOM change away from the icons (a script writes a text node) checks
/// each icon's own subtrees and serializes none of them.
#[test]
fn a_dom_change_elsewhere_serializes_no_icon() {
    let html = reduced_page().replace("<body>", "<body><p id=\"t\">0</p>");
    let (mut engine, view) = view_with(&html, 800, 600);
    engine.relayout(view).expect("relayout");
    let before = markup_builds();
    let relayouts = engine.relayout_stats(view).expect("view").relayouts;
    let mut svg_ms = Vec::new();
    for i in 1..4 {
        engine
            .execute_script(view, &format!("document.getElementById('t').textContent = '{i}'"))
            .expect("script");
        // The script's flush laid the page out.
        svg_ms.push(engine.relayout_stats(view).expect("view").last_svg_ms);
    }
    eprintln!("LAT4b reduced page, text change elsewhere: svg_ms per relayout {svg_ms:?}");
    assert_eq!(markup_builds() - before, 0, "no icon is serialized again");
    assert_eq!(engine.relayout_stats(view).expect("view").relayouts, relayouts + 3, "one relayout per script");
}

/// A paint rule keyed on an HTML ancestor's class (the kept per-element
/// styles must see the ancestor change), and one keyed on a sibling (no
/// per-element styles are kept then).
#[test]
fn paint_rules_on_an_ancestor_and_on_a_sibling_follow_changes() {
    let (mut engine, view) = view_with(
        r##"<!DOCTYPE html><style>.on rect{fill:#0000ff} .mark + svg rect{fill:#00ff00}</style><body><div id="a"><svg width="20" height="20"><rect width="10" height="10" fill="#ff0000"/></svg></div><div><i id="m"></i><svg width="20" height="20"><rect width="10" height="10" fill="#ff0000"/></svg></div></body>"##,
        200,
        100,
    );
    assert!(!fills(&engine, view).contains(&BLUE));
    run(&mut engine, view, "document.getElementById('a').className = 'on'");
    assert!(fills(&engine, view).contains(&BLUE), "ancestor class: {:?}", fills(&engine, view));
    run(&mut engine, view, "document.getElementById('a').className = ''");
    assert!(!fills(&engine, view).contains(&BLUE), "ancestor class removed: {:?}", fills(&engine, view));
    run(&mut engine, view, "document.getElementById('m').className = 'mark'");
    assert!(fills(&engine, view).contains(&(0, 255, 0)), "sibling class: {:?}", fills(&engine, view));
}
