//! `display: inherit`. `inherit_property` had no `display` arm, so the
//! keyword fell through to "keep what you have" and a `<div>` stayed
//! `block` whatever its parent was. Seen on apple's global nav 2026-10-28:
//! `.globalnav-flyout` and `.globalnav-menu-list` say `display: inherit`
//! under a `display: contents` item of a flex list, so in Chrome the ten
//! menu items are flex items of the list. Here the two wrappers made one
//! block flex item and the items stacked in it: only the first fitted in
//! the 44px bar.

use super::*;

fn loaded(html: &str) -> (Engine, EngineViewId) {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 800, height: 600 })
        .expect("view");
    engine.load_html(view, html).expect("load");
    (engine, view)
}

const PLACES: &str = "[].map.call(document.querySelectorAll('[id]'), function (e) { \
     var r = e.getBoundingClientRect(); \
     return e.id + ':' + getComputedStyle(e).display + ',' + r.x + ',' + r.y; }).join(' ')";

/// A wrapper that inherits its flex parent's `display` is a flex container
/// itself, so its children sit in a row. Chrome 148: `w:flex,0,0 a:block,0,0
/// b:block,50,0`.
#[test]
fn display_inherit_takes_the_parents_flex() {
    let (mut engine, view) = loaded(concat!(
        "<!doctype html><style>body{margin:0} #p{display:flex;width:300px}",
        " #w{display:inherit} #a,#b{width:50px;height:20px}</style>",
        "<body><div id=p><div id=w><div id=a></div><div id=b></div></div></div></body>",
    ));
    assert_eq!(
        engine.execute_script(view, PLACES).expect("script"),
        "String(\"p:flex,0,0 w:flex,0,0 a:block,0,0 b:block,50,0\")"
    );
}

/// It goes down level by level, and a wrapper under a block parent stays
/// block. Chrome 148: `q:block,0,0 v:block,0,0 c:block,0,0 d:block,0,20`
/// beside the flex chain `r:flex,0,40 u:flex,0,40 t:flex,0,40 e:block,0,40
/// f:block,50,40`.
#[test]
fn display_inherit_follows_the_chain_and_keeps_block_under_block() {
    let (mut engine, view) = loaded(concat!(
        "<!doctype html><style>body{margin:0} #q{display:block} #r{display:flex}",
        " .i{display:inherit} #c,#d,#e,#f{width:50px;height:20px}</style>",
        "<body><div id=q><div id=v class=i><div id=c></div><div id=d></div></div></div>",
        "<div id=r><div id=u class=i><div id=t class=i><div id=e></div><div id=f></div></div></div></div>",
        "</body>",
    ));
    assert_eq!(
        engine.execute_script(view, PLACES).expect("script"),
        "String(\"q:block,0,0 v:block,0,0 c:block,0,0 d:block,0,20 \
         r:flex,0,40 u:flex,0,40 t:flex,0,40 e:block,0,40 f:block,50,40\")"
    );
}

// A `display: contents` box hands its children a parent style whose `display`
// is the grandparent's, so that they blockify against the box they really sit
// in (#786). `inherit` must still read `contents`. Expected strings below are
// Chromium 143.0.7499.4 on the same pages.

/// apple's nav: flex list > contents item > two `inherit` wrappers > items.
/// All three wrappers compute `contents`, so the items are flex items of the
/// list. With the grandparent's `flex` the wrapper was one flex item holding
/// all three in its own row.
#[test]
fn display_inherit_under_contents_reads_contents_and_items_join_the_flex_list() {
    let (mut engine, view) = loaded(concat!(
        "<!doctype html><style>body{margin:0}",
        " #l{display:flex;width:300px;justify-content:space-between}",
        " #m{display:contents} .i{display:inherit} #a,#b,#c{width:50px;height:20px}</style>",
        "<body><div id=l><div id=m><div id=f class=i><div id=g class=i>",
        "<div id=a>a</div><div id=b>b</div><div id=c>c</div></div></div></div></div></body>",
    ));
    assert_eq!(
        engine.execute_script(view, PLACES).expect("script"),
        "String(\"l:flex,0,0 m:contents,0,0 f:contents,0,0 g:contents,0,0 \
         a:block,0,0 b:block,125,0 c:block,250,0\")"
    );
}

/// The same under a block grandparent: the wrapper is `contents`, not `block`.
#[test]
fn display_inherit_under_contents_in_a_block_parent_reads_contents() {
    let (mut engine, view) = loaded(concat!(
        "<!doctype html><style>body{margin:0} #l{display:block;width:300px}",
        " #m{display:contents} .i{display:inherit} #a,#b{width:50px;height:20px}</style>",
        "<body><div id=l><div id=m><div id=f class=i>",
        "<div id=a>a</div><div id=b>b</div></div></div></div></body>",
    ));
    assert_eq!(
        engine.execute_script(view, PLACES).expect("script"),
        "String(\"l:block,0,0 m:contents,0,0 f:contents,0,0 a:block,0,0 b:block,0,20\")"
    );
}

/// A child that names its own `display` below the `inherit` wrapper is its own
/// box: the flag stops at the first element that does not inherit it.
#[test]
fn a_block_child_of_an_inheriting_contents_wrapper_stays_a_block_flex_item() {
    let (mut engine, view) = loaded(concat!(
        "<!doctype html><style>body{margin:0} #l{display:flex;width:300px}",
        " #m{display:contents} #f{display:inherit} #g{display:block}",
        " #a,#b{width:50px;height:20px}</style>",
        "<body><div id=l><div id=m><div id=f><div id=g>",
        "<div id=a>a</div><div id=b>b</div></div></div></div></div></body>",
    ));
    assert_eq!(
        engine.execute_script(view, PLACES).expect("script"),
        "String(\"l:flex,0,0 m:contents,0,0 f:contents,0,0 g:block,0,0 a:block,0,0 b:block,0,20\")"
    );
}

/// The keyword's modifier comes along too (R1 on #817, after #840): a
/// wrapper inheriting from a list item reads `list-item`, one inheriting
/// from a `flow-root` box reads `flow-root`, and a list item that inherits
/// from a block parent reads `block`. Chrome: `l:list-item,40,0 m:list-item,40,0
/// f:flow-root,0,20 g:flow-root,0,20 h:block,0,40 i:block,0,40`.
#[test]
fn display_inherit_copies_the_display_modifier() {
    let (mut engine, view) = loaded(concat!(
        "<!doctype html><style>body{margin:0} ul{margin:0;padding:0 0 0 40px} li{height:20px}",
        " .i{display:inherit} #f{display:flow-root} #h{display:block} div{height:20px}</style>",
        "<body><ul><li id=l><div id=m class=i></div></li></ul>",
        "<div id=f><div id=g class=i></div></div>",
        "<div id=h><li id=i class=i></li></div></body>",
    ));
    assert_eq!(
        engine.execute_script(view, PLACES).expect("script"),
        "String(\"l:list-item,40,0 m:list-item,40,0 f:flow-root,0,20 g:flow-root,0,20 h:block,0,40 i:block,0,40\")"
    );
}
