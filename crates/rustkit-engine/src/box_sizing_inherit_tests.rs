//! `box-sizing: inherit`. The reset most sites start with is
//! `html { box-sizing: border-box } *, :before, :after { box-sizing: inherit }`,
//! and `inherit` on `box-sizing` did nothing here: every box on such a page
//! stayed `content-box`, so a `width: 100%` wrapper with side padding was
//! wider than the window. Seen on cnn 2026-10-12 (`section.layout__wrapper`
//! 1344 wide in a 1280 window; 1280 in Chromium on the same bytes).

use super::*;
use crate::script_net_tests::serve_routes;

/// What `script` answers on a page with the inheriting reset, `css` after
/// it and `body`, in an 800x600 view.
fn measured(css: &str, body: &str, script: &str) -> String {
    let page = format!(
        "<html><head><style>html{{box-sizing:border-box}} *,:before,:after{{box-sizing:inherit}} body{{margin:0}} {css}</style></head>\
         <body>{body}</body></html>"
    );
    let server = serve_routes(vec![("/", "text/html", page)]);
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 800, height: 600 })
        .expect("view");
    let url = Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap();
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    rt.block_on(engine.load_url(view, url)).expect("load_url");
    engine.execute_script(view, script).expect("script")
}

const WIDTHS: &str = "[].map.call(document.querySelectorAll('[id]'), function (e) { \
     return e.id + ':' + e.getBoundingClientRect().width; }).join(' ')";

/// cnn's wrapper: `width: 100%` and side padding fit the window, and what
/// is inside it is the window less the padding.
#[test]
fn a_full_width_wrapper_with_padding_fits_the_window_under_the_inheriting_reset() {
    assert_eq!(
        measured(
            ".wrap{width:100%;max-width:1440px;margin:0 auto;padding:0 32px}",
            "<section id=wrap class=wrap><div id=inner>x</div></section>",
            WIDTHS,
        ),
        "String(\"wrap:800 inner:736\")"
    );
}

/// The value comes down from `html` through every level, and a box that
/// says `content-box` hands that to its own children.
#[test]
fn box_sizing_inherit_takes_the_parents_value_at_every_level() {
    assert_eq!(
        measured(
            ".b{width:200px;padding:10px;border:5px solid}  .c{box-sizing:content-box}",
            "<div><div><div id=deep class=b>x</div></div></div>\
             <div id=own class='b c'><div id=child class=b>x</div></div>",
            WIDTHS,
        ),
        "String(\"deep:200 own:230 child:230\")"
    );
}

/// `getComputedStyle` reports what was inherited, on an element and on a
/// box that set nothing itself.
#[test]
fn the_inherited_box_sizing_is_what_computed_style_reports() {
    assert_eq!(
        measured(
            ".c{box-sizing:content-box}",
            "<p id=a>x</p><div class=c><p id=b>x</p></div>",
            "getComputedStyle(document.getElementById('a')).boxSizing + ' ' + \
             getComputedStyle(document.getElementById('b')).boxSizing",
        ),
        "String(\"border-box content-box\")"
    );
}

/// A page without the reset is as before: `box-sizing` is not inherited
/// unless a rule says `inherit`.
#[test]
fn box_sizing_is_not_inherited_without_the_keyword() {
    let page = "<html><head><style>body{margin:0} .p{box-sizing:border-box;width:300px;padding:10px} \
                .k{width:100px;padding:10px}</style></head>\
                <body><div id=p class=p><div id=k class=k>x</div></div></body></html>";
    let server = serve_routes(vec![("/", "text/html", page.to_string())]);
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 800, height: 600 })
        .expect("view");
    let url = Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap();
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    rt.block_on(engine.load_url(view, url)).expect("load_url");
    assert_eq!(engine.execute_script(view, WIDTHS).expect("script"), "String(\"p:300 k:120\")");
}

/// `initial` and `unset` (box-sizing is not inherited, so both mean the
/// initial value) put a box back to `content-box` under the reset, and its
/// children inherit that. Chromium 141: 230 and content-box for both boxes.
/// Before the `inherit` arm the reset did nothing, so the missing initial
/// value did not show.
#[test]
fn box_sizing_initial_and_unset_are_content_box_under_the_inheriting_reset() {
    for keyword in ["initial", "unset"] {
        assert_eq!(
            measured(
                &format!(
                    ".b{{width:200px;padding:10px;border:5px solid}} .x{{box-sizing:{keyword}}}"
                ),
                "<div id=a class='b x'><div id=k class=b>x</div></div>",
                "[].map.call(document.querySelectorAll('[id]'), function (e) { \
                 return e.id + ':' + e.getBoundingClientRect().width + ',' + \
                 getComputedStyle(e).boxSizing; }).join(' ')",
            ),
            "String(\"a:230,content-box k:230,content-box\")",
            "box-sizing: {keyword}"
        );
    }
}

/// The reset's `:before, :after { box-sizing: inherit }` half: a generated
/// box takes its element's value, so its width is its border box. Chromium
/// 141: the inline-block that holds it is 100 wide (130 as content-box).
#[test]
fn a_pseudo_element_inherits_box_sizing_from_its_element() {
    for pseudo in ["before", "after"] {
        assert_eq!(
            measured(
                &format!(
                    "#p::{pseudo}{{content:'';display:block;width:100px;padding:10px;border:5px solid}} \
                     #q{{box-sizing:content-box}} \
                     #q::{pseudo}{{content:'';display:block;width:100px;padding:10px;border:5px solid}}"
                ),
                "<span id=p style='display:inline-block'></span>\
                 <span id=q style='display:inline-block'></span>",
                WIDTHS,
            ),
            "String(\"p:100 q:130\")",
            "::{pseudo}"
        );
    }
}

/// Why the reset never reached a `::before`: a rule's selector list was
/// taken whole, so only its LAST member could name the pseudo-element.
/// Each line is one list; the 50px box shows the member was matched.
#[test]
fn a_pseudo_element_member_is_matched_anywhere_in_a_selector_list() {
    let width = |css: &str| {
        measured(css, "<span id=p class=c style='display:inline-block'></span>", WIDTHS)
    };
    const BOX: &str = "{content:'';display:block;width:50px}";
    for list in [
        "#p:before,.zz:after",
        ".zz:after,#p:before",
        "#p::before,.zz",
        ".zz,#p::after,b",
        ".zz:before,.c:before,.zz",
    ] {
        assert_eq!(width(&format!("{list}{BOX}")), "String(\"p:50\")", "{list}");
    }
    // A member for another element, or with no pseudo-element, makes no box.
    for list in ["i,.zz:before", ".zz:before,.zz:after", "i:hover,.zz:after,i"] {
        assert_eq!(width(&format!("{list}{BOX}")), "String(\"p:0\")", "{list}");
    }
    // Bare members, alone and in a list.
    for list in [":before", "::after", "*,:before,:after", ".zz,:after"] {
        assert_eq!(
            width(&format!("{list}{{margin-left:7px}} #p{{margin:0}} #p::before,#p::after{BOX}")),
            "String(\"p:57\")",
            "{list}"
        );
    }
}

/// A list member that names the pseudo-element cascades with ITS OWN
/// specificity when it is the one that matches (Selectors 4 §17), as an
/// element's rule does. With every member matched, each took the list's
/// maximum: `.c::before, #zz .c::before { width: 50px }` beat a later
/// `.d::before { width: 20px }`. Chromium 141: 20 (the #684 second review).
#[test]
fn a_pseudo_element_list_member_cascades_with_its_own_specificity() {
    let width = |css: &str, class: &str| {
        measured(
            css,
            &format!("<span id=p class='{class}' style='display:inline-block'></span>"),
            WIDTHS,
        )
    };
    for pseudo in ["before", "after"] {
        let made = format!(".c::{pseudo}{{content:'';display:block;height:5px}}");
        for (list, want) in [
            // The review's page, and the same with the matching member last.
            (format!(".c::{pseudo}, #zz .c::{pseudo}"), 20),
            (format!("#zz .c::{pseudo}, .c::{pseudo}"), 20),
            (format!("#zz::{pseudo}, .c::{pseudo}"), 20),
            // A member with no pseudo-element lends it nothing.
            (format!("#zz, .c::{pseudo}"), 20),
            (format!(".c:{pseudo}, #zz i"), 20),
            // The more specific member matches: the list wins.
            (format!(".c::{pseudo}, #p::{pseudo}"), 50),
            (format!("#p.c::{pseudo}, .zz::{pseudo}"), 50),
        ] {
            assert_eq!(
                width(&format!("{made} {list}{{width:50px}} .d::{pseudo}{{width:20px}}"), "c d"),
                format!("String(\"p:{want}\")"),
                "{list}"
            );
        }
    }
    // The one-colon spelling is a pseudo-element too, (0,0,1): the old
    // reset `:before { … }` must not beat `span::before`.
    assert_eq!(
        width(
            ".c::before{content:'';display:block;height:5px} \
             span::before{width:20px} :before{width:50px} span:before{margin-left:3px}",
            "c"
        ),
        "String(\"p:23\")"
    );
    // The ordinary shape of it.
    assert_eq!(
        width(
            ".icon::before{content:'';display:block;height:5px} \
             .icon::before, .nav .menu .icon::before{width:12px} .icon-big::before{width:24px}",
            "icon icon-big"
        ),
        "String(\"p:24\")"
    );
}
