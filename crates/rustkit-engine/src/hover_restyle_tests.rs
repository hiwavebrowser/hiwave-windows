//! A pointer restyle (`:hover`, `:active`) that changes only what is
//! painted keeps the laid-out tree: the elements that gained or lost the
//! pseudo-class are built again with what is under them, and nothing else
//! of the page; the paint properties that changed are copied onto the boxes
//! that are already laid out, and the page is painted from them. A restyle
//! that changes anything layout reads is a full relayout, as before.
//!
//! Every test that takes the short path compares its result (display list
//! and laid-out tree) with what a full relayout gives for the same pointer
//! state.

use super::*;
use crate::script_net_tests::serve_routes;

/// A link, a box that grows under the pointer, and a card whose hover
/// rules change paint only, holding the layout modes a page mixes: wrapped
/// text, flex, grid, a list, a table, an inline-block.
const PAGE: &str = "<style>\
    body{margin:0;font:16px/20px sans-serif}\
    a{color:blue;text-decoration:none} a:hover{color:red;text-decoration:underline}\
    #grow{height:30px;background:tan} #grow:hover{height:50px}\
    .card{width:300px;padding:10px;border:2px solid gray;background:white}\
    .card:hover{background:#eee;border-color:black;box-shadow:0 2px 4px rgba(0,0,0,.5)}\
    .card:hover .title{color:green}\
    .row{display:flex;gap:10px} .row>div{flex:1;height:20px;background:silver}\
    .grid{display:grid;grid-template-columns:1fr 2fr;gap:4px}\
    </style>\
    <div style='height:30px'><a id=l href='/x'>a link with some words</a></div>\
    <div id=grow></div>\
    <div class=card id=c><div class=title>Title of the card that wraps onto a second line when it is this long</div>\
    <div class=row><div>a</div><div>b</div></div>\
    <div class=grid><span>one</span><span>two</span></div>\
    <ul><li>item <b>bold</b></li></ul>\
    <table><tr><td>cell</td><td>cell two</td></tr></table>\
    <span style='display:inline-block;width:40px;height:10px;background:navy'></span></div>";

/// Viewport points over the link, the growing box and the card's title.
const LINK: (f32, f32) = (10.0, 10.0);
const GROW: (f32, f32) = (10.0, 45.0);
const CARD: (f32, f32) = (150.0, 80.0);
/// Below everything: the body's own background.
const NOWHERE: (f32, f32) = (380.0, 580.0);

fn page() -> (Engine, EngineViewId) {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds {
            x: 0,
            y: 0,
            width: 400,
            height: 600,
        })
        .expect("view");
    engine.load_html(view, PAGE).expect("load_html");
    (engine, view)
}

/// Moves the pointer to `at` (viewport points): a `:hover` change takes
/// the short path or misses to a full relayout.
pub(crate) fn hover(engine: &mut Engine, view: EngineViewId, at: (f32, f32)) {
    engine.mouse_move_at_point(view, at.0, at.1);
}

/// Layouts the view skipped so far: one more after each short-path hit.
pub(crate) fn skipped(engine: &Engine, view: EngineViewId) -> u32 {
    engine.relayout_stats(view).expect("view").layouts_skipped
}

fn restyles(engine: &Engine, view: EngineViewId) -> u32 {
    engine
        .relayout_stats(view)
        .expect("view")
        .by_first_cause
        .get("hover_or_focus")
}

/// The display list and the laid-out tree, styles included.
fn painted(engine: &Engine, view: EngineViewId) -> (String, String) {
    let state = engine.views.get(&view).expect("view");
    (
        format!("{:?}", state.display_list.as_ref().expect("list").commands),
        format!("{:?}", state.layout.as_ref().expect("layout")),
    )
}

/// Fails with the text around the first place two painted states differ.
fn assert_same(got: &(String, String), want: &(String, String), what: &str) {
    for (name, got, want) in [("display list", &got.0, &want.0), ("tree", &got.1, &want.1)] {
        if got == want {
            continue;
        }
        let at = got.bytes().zip(want.bytes()).position(|(a, b)| a != b).unwrap_or(got.len().min(want.len()));
        let window = |s: &str| {
            let from = (0..=at.saturating_sub(300)).rev().find(|i| s.is_char_boundary(*i)).unwrap_or(0);
            let to = (at + 200).min(s.len());
            let to = (to..=s.len()).find(|i| s.is_char_boundary(*i)).unwrap_or(s.len());
            s[from..to].to_string()
        };
        panic!("{what}: the {name} differs at byte {at}\n   got  ...{}\n   want ...{}", window(got), window(want));
    }
}

/// What a full relayout gives for the pointer state the view is in.
fn painted_by_a_full_relayout(engine: &mut Engine, view: EngineViewId) -> (String, String) {
    let before = skipped(engine, view);
    engine.relayout(view).expect("relayout");
    assert_eq!(skipped(engine, view), before, "a relayout nobody named is a full one");
    painted(engine, view)
}

#[test]
fn a_hover_that_changes_a_colour_skips_layout() {
    let (mut engine, view) = page();
    let at_rest = painted(&engine, view);
    hover(&mut engine, view, LINK);
    assert_eq!(restyles(&engine, view), 1, "the sheet reads :hover on the link");
    assert_eq!(skipped(&engine, view), 1, "colour and underline are paint");
    let short = painted(&engine, view);
    assert_ne!(short.0, at_rest.0, "the link is painted in its hover colour");
    assert_eq!(short, painted_by_a_full_relayout(&mut engine, view));
}

/// #702: a paint-only pointer restyle republishes the lazy geometry source,
/// so the next script read rebuilds and sees hover paint — not a stale
/// pre-hover snapshot from an earlier `getComputedStyle` / box read.
#[test]
fn a_paint_only_hover_republishes_geometry_for_the_next_script_read() {
    let (mut engine, view) = page();
    let color = |engine: &mut Engine, view: EngineViewId| {
        let value = engine
            .execute_script(
                view,
                "String(getComputedStyle(document.getElementById('l')).color)",
            )
            .expect("script");
        value
            .strip_prefix("String(\"")
            .and_then(|v| v.strip_suffix("\")"))
            .unwrap_or(&value)
            .to_string()
    };
    let builds = |engine: &Engine, view: EngineViewId| {
        engine.views[&view]
            .bindings
            .as_ref()
            .expect("bindings")
            .geometry_builds()
    };

    let resting = color(&mut engine, view);
    let builds_after_rest = builds(&engine, view);
    assert!(builds_after_rest >= 1, "the first read builds a snapshot");

    hover(&mut engine, view, LINK);
    assert_eq!(skipped(&engine, view), 1, "paint-only hover");

    let hovered = color(&mut engine, view);
    assert_ne!(hovered, resting, "script sees the hover colour, not the old snapshot");
    assert_eq!(
        builds(&engine, view),
        builds_after_rest + 1,
        "the restyle dropped the stale source so the next read rebuilt"
    );
}

#[test]
fn a_hover_over_a_card_restyles_its_descendants_without_layout() {
    let (mut engine, view) = page();
    let at_rest = painted(&engine, view);
    hover(&mut engine, view, CARD);
    assert_eq!(restyles(&engine, view), 1);
    assert_eq!(
        skipped(&engine, view),
        1,
        "background, border colour, shadow and a descendant's colour are paint"
    );
    let short = painted(&engine, view);
    assert_ne!(short.0, at_rest.0);
    assert_eq!(short, painted_by_a_full_relayout(&mut engine, view));
}

#[test]
fn leaving_puts_the_resting_paint_back() {
    let (mut engine, view) = page();
    let at_rest = painted(&engine, view);
    hover(&mut engine, view, CARD);
    hover(&mut engine, view, LINK);
    hover(&mut engine, view, NOWHERE);
    assert_eq!(skipped(&engine, view), 3);
    assert_eq!(painted(&engine, view), at_rest);
}

#[test]
fn a_hover_that_changes_a_size_is_laid_out() {
    let (mut engine, view) = page();
    let at_rest = painted(&engine, view);
    hover(&mut engine, view, GROW);
    assert_eq!(restyles(&engine, view), 1);
    assert_eq!(skipped(&engine, view), 0, "height is layout");
    assert_eq!(LAST_RESTYLE_MISS.with(|why| why.borrow().clone()), "style: height");
    let grown = painted(&engine, view);
    assert_ne!(grown.1, at_rest.1, "the card moved down");
    assert_eq!(grown, painted_by_a_full_relayout(&mut engine, view));
    // And the short path works again from the new layout.
    hover(&mut engine, view, LINK);
    assert_eq!(skipped(&engine, view), 0, "leaving the box shrinks it: layout");
    hover(&mut engine, view, NOWHERE);
    assert_eq!(skipped(&engine, view), 1);
    assert_eq!(painted(&engine, view), at_rest);
}

#[test]
fn a_hover_whose_listener_writes_to_the_page_is_laid_out() {
    let server = serve_routes(vec![(
        "/",
        "text/html",
        "<html><head><style>body{margin:0} #b{height:50px;background:tan} #b:hover{background:red}</style></head>\
         <body><div id=b></div><p id=out></p><script>\
         document.getElementById('b').addEventListener('mouseover', function () { \
           document.getElementById('out').textContent = 'over'; });</script></body></html>"
            .to_string(),
    )]);
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
    hover(&mut engine, view, (10.0, 10.0));
    assert_eq!(skipped(&engine, view), 0, "the listener added text: layout");
    assert_eq!(painted(&engine, view), painted_by_a_full_relayout(&mut engine, view));
}

/// Forty cards in a 300px column, each 40px tall, whose rules read where a
/// card and its children sit among their siblings. `NOWHERE` is beside them.
fn cards(extra_css: &str) -> (Engine, EngineViewId) {
    let mut html = String::from(
        "<style>body{margin:0;font:16px/20px sans-serif}\
         #list{width:300px}\
         .card{height:38px;border:1px solid gray;color:black}\
         .card:hover{background:#eee;color:maroon}\
         .card:nth-child(3):hover{border-color:red}\
         .card:hover span:nth-of-type(2){background:yellow}\
         .card:hover b:last-child{color:green}\
         .card a{color:blue} .card a:hover{text-decoration:underline}\
         .card:active{background:#ccd}",
    );
    html.push_str(extra_css);
    html.push_str("</style><div id=list>");
    for i in 0..40 {
        html.push_str(&format!(
            "<div class=card><span>card {i}</span> <span>second</span> <a href='/x'>link</a> <b>last</b></div>"
        ));
    }
    html.push_str("</div><p id=after>after the cards</p>");
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds {
            x: 0,
            y: 0,
            width: 400,
            height: 600,
        })
        .expect("view");
    engine.load_html(view, &html).expect("load_html");
    (engine, view)
}

/// A point over the text of card `n` (0-based), left of its link.
fn card(n: usize) -> (f32, f32) {
    (20.0, n as f32 * 40.0 + 20.0)
}

fn walked() -> usize {
    LAST_RESTYLE_WALKED.with(|walked| walked.get())
}

#[test]
fn a_hover_builds_the_hovered_card_and_not_the_page() {
    let (mut engine, view) = cards("");
    let at_rest = painted(&engine, view);
    hover(&mut engine, view, card(2));
    assert_eq!(skipped(&engine, view), 1);
    // Two builds of: the list, the card, its four children and their text.
    assert!(walked() < 40, "the two builds went into {} nodes of about 500", walked());
    let short = painted(&engine, view);
    assert_ne!(short.0, at_rest.0);
    assert_eq!(short, painted_by_a_full_relayout(&mut engine, view), "the third card, by position");
}

#[test]
fn a_move_from_one_card_to_another_restyles_both() {
    let (mut engine, view) = cards("");
    let at_rest = painted(&engine, view);
    hover(&mut engine, view, card(2));
    hover(&mut engine, view, card(7));
    assert_eq!(skipped(&engine, view), 2);
    assert!(walked() < 80, "{} nodes", walked());
    let short = painted(&engine, view);
    assert_same(&short, &painted_by_a_full_relayout(&mut engine, view), "after the move");
    hover(&mut engine, view, NOWHERE);
    assert_eq!(skipped(&engine, view), 3);
    assert_eq!(painted(&engine, view), at_rest);
}

#[test]
fn a_hover_inside_a_hovered_card_restyles_the_inner_element() {
    let (mut engine, view) = cards("");
    hover(&mut engine, view, card(5));
    // The link is the third child: past "card 5 second ".
    let link = engine
        .execute_script(
            view,
            "(function(){var r=document.querySelectorAll('.card')[5].querySelector('a').getBoundingClientRect();\
             return Math.round(r.left+r.width/2)+','+Math.round(r.top+r.height/2)})()",
        )
        .expect("script");
    let link = link.trim_start_matches("String(\"").trim_end_matches("\")");
    let (x, y) = link.split_once(',').expect("x,y");
    hover(&mut engine, view, (x.parse().unwrap(), y.parse().unwrap()));
    assert_eq!(skipped(&engine, view), 2, "the card stays hovered; the link gains it");
    assert!(walked() < 20, "{} nodes: the link alone", walked());
    assert_eq!(painted(&engine, view), painted_by_a_full_relayout(&mut engine, view));
}

#[test]
fn a_press_restyles_the_pressed_card() {
    let (mut engine, view) = cards("");
    hover(&mut engine, view, card(4));
    let hovered = painted(&engine, view);
    let (x, y) = card(4);
    engine.mouse_down_at_point(view, x, y);
    assert_eq!(skipped(&engine, view), 2, "a background is paint");
    let pressed = painted(&engine, view);
    assert_ne!(pressed.0, hovered.0);
    assert_same(&pressed, &painted_by_a_full_relayout(&mut engine, view), "pressed");
}

/// Before S22 a sibling combinator anywhere in the sheets sent every
/// pointer restyle down the full path; now the next card is a root of the
/// two builds, beside the hovered one.
#[test]
fn a_hover_rule_with_a_sibling_combinator_takes_the_short_path() {
    let (mut engine, view) = cards(".card:hover + .card{background:pink}");
    let at_rest = painted(&engine, view);
    hover(&mut engine, view, card(2));
    assert_eq!(skipped(&engine, view), 1, "a background is paint, on the next card too");
    assert!(walked() < 60, "the two builds went into {} nodes: two cards", walked());
    let hovered = painted(&engine, view);
    assert_ne!(hovered.0, at_rest.0);
    assert_same(&hovered, &painted_by_a_full_relayout(&mut engine, view), "the third card and the fourth");
    hover(&mut engine, view, NOWHERE);
    assert_eq!(painted(&engine, view), at_rest, "off the cards");
}

/// A field in a 40px row and, under it, a box whose hover rule changes
/// paint only; `extra_css` adds each test's `:active` or `:focus` rules.
fn field_page(extra_css: &str) -> (Engine, EngineViewId) {
    let html = format!(
        "<style>body{{margin:0;font:16px/20px sans-serif}} #x{{height:30px;background:tan}} #x:hover{{background:red}} {extra_css}</style>\
         <div style='height:40px'><input id=i></div><div id=x>x</div>"
    );
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds {
            x: 0,
            y: 0,
            width: 400,
            height: 600,
        })
        .expect("view");
    engine.load_html(view, &html).expect("load_html");
    (engine, view)
}

/// Points over the field and over the box under it.
const FIELD: (f32, f32) = (20.0, 10.0);
const BOX: (f32, f32) = (20.0, 55.0);

/// Whether the laid-out tree carries a focused control with its caret.
fn caret_drawn(painted: &(String, String)) -> bool {
    painted.1.contains("focused_caret: Some(")
}

/// Why the last pointer restyle on this thread missed (empty on a hit).
pub(crate) fn last_miss() -> String {
    LAST_RESTYLE_MISS.with(|why| why.borrow().clone())
}

#[test]
fn a_press_that_focuses_a_field_paints_its_caret() {
    let (mut engine, view) = field_page("input:active{background:#eee}");
    assert!(!caret_drawn(&painted(&engine, view)));
    engine.mouse_down_at_point(view, FIELD.0, FIELD.1);
    assert!(engine.focused_node(view).is_some(), "the press focused the field");
    assert_eq!(restyles(&engine, view), 1, "the sheet reads :active on the field");
    assert_eq!(skipped(&engine, view), 0, "the focus moved since the last layout: the full path");
    // Since the selection model (#895) the press that focuses the field places the
    // caret and relayouts with the focus already in place, so the pointer restyle
    // that follows is a paint-only hit, not a "focus moved" miss. The observable
    // contract stays: one restyle, nothing skipped, caret painted.
    let pressed = painted(&engine, view);
    assert!(caret_drawn(&pressed), "the field is painted focused, with its caret");
    assert_same(&pressed, &painted_by_a_full_relayout(&mut engine, view), "pressed");
}

#[test]
fn a_hover_after_a_click_into_a_field_paints_the_focus() {
    let (mut engine, view) = field_page("");
    engine.click_at_point(view, FIELD.0, FIELD.1);
    assert!(engine.focused_node(view).is_some(), "the click focused the field");
    // No rule read the click, so nothing was laid out for it: the focus is
    // not painted until the next relayout (as on develop, where that
    // relayout is always a full one).
    assert_eq!(restyles(&engine, view), 0);
    assert!(!caret_drawn(&painted(&engine, view)));
    hover(&mut engine, view, BOX);
    assert_eq!(restyles(&engine, view), 1);
    assert_eq!(skipped(&engine, view), 0, "the focus moved since the last layout: the full path");
    assert_eq!(last_miss(), "focus moved");
    let hovered = painted(&engine, view);
    assert!(caret_drawn(&hovered), "the hover's relayout paints the focus and the caret");
    assert_same(&hovered, &painted_by_a_full_relayout(&mut engine, view), "hovered after the click");
    // That layout recorded the focus: the next paint-only move takes the
    // short path and keeps the caret.
    hover(&mut engine, view, NOWHERE);
    assert_eq!(skipped(&engine, view), 1);
    let left = painted(&engine, view);
    assert!(caret_drawn(&left));
    assert_same(&left, &painted_by_a_full_relayout(&mut engine, view), "left the box");
}

#[test]
fn a_press_elsewhere_blurs_the_field_in_paint() {
    let (mut engine, view) = field_page("#x:active{background:red}");
    engine.mouse_down_at_point(view, FIELD.0, FIELD.1);
    engine.relayout(view).expect("relayout");
    assert!(caret_drawn(&painted(&engine, view)), "the focused field is laid out and painted");
    engine.mouse_down_at_point(view, BOX.0, BOX.1);
    assert_eq!(engine.focused_node(view), None, "a press on the box takes the focus away");
    assert_eq!(skipped(&engine, view), 0, "the focus moved since the last layout: the full path");
    assert_eq!(last_miss(), "focus moved");
    let blurred = painted(&engine, view);
    assert!(!caret_drawn(&blurred), "no caret on a field that lost the focus");
    assert_same(&blurred, &painted_by_a_full_relayout(&mut engine, view), "blurred");
}

#[test]
fn focus_rules_after_a_click_paint_as_a_full_relayout_does() {
    // `:focus`, `:focus-visible` and `:focus-within` are not matched by
    // the engine yet (the same on develop): what is checked is that the
    // short path never shows something a full relayout does not, with the
    // caret, when a page has those rules.
    let (mut engine, view) = field_page(
        "#i:focus{background:yellow} #i:focus-visible{outline:2px solid blue} div:focus-within{background:pink}",
    );
    engine.click_at_point(view, FIELD.0, FIELD.1);
    hover(&mut engine, view, BOX);
    assert_eq!(skipped(&engine, view), 0, "the focus moved: the full path");
    let hovered = painted(&engine, view);
    assert!(caret_drawn(&hovered));
    assert_same(&hovered, &painted_by_a_full_relayout(&mut engine, view), "focus rules, hovered");
    hover(&mut engine, view, NOWHERE);
    assert_eq!(skipped(&engine, view), 1);
    assert_same(&painted(&engine, view), &painted_by_a_full_relayout(&mut engine, view), "focus rules, left");
}

#[test]
fn a_label_click_that_focuses_a_textarea_paints_its_caret() {
    let html = "<style>body{margin:0;font:16px/20px sans-serif} label{display:block;height:20px} label:active{color:red}\
                #x{height:30px;background:tan} #x:hover{background:red}</style>\
                <label for=t>name</label><div style='height:60px'><textarea id=t rows=2></textarea></div><div id=x>x</div>";
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds {
            x: 0,
            y: 0,
            width: 400,
            height: 600,
        })
        .expect("view");
    engine.load_html(view, html).expect("load_html");
    // The press restyles the label; the release gives the textarea the focus.
    engine.click_at_point(view, 10.0, 10.0);
    assert!(engine.focused_node(view).is_some(), "the label's click focused its textarea");
    hover(&mut engine, view, (20.0, 95.0));
    let hovered = painted(&engine, view);
    assert!(caret_drawn(&hovered), "the textarea is painted focused, with its caret");
    assert_same(&hovered, &painted_by_a_full_relayout(&mut engine, view), "hovered after the label click");
    hover(&mut engine, view, NOWHERE);
    let left = painted(&engine, view);
    assert!(caret_drawn(&left));
    assert_same(&left, &painted_by_a_full_relayout(&mut engine, view), "left the box");
}

/// The review's cost page: 300 inline-block cards, about 5,700 boxes, whose
/// hover rules change paint only; with `miss`, one of them changes a height.
fn large_page(miss: bool) -> String {
    let mut html = String::from(
        "<style>body{margin:0;font:14px/18px sans-serif}\
         .card{width:360px;padding:6px;border:1px solid #999;margin:4px;display:inline-block;vertical-align:top}\
         .card .t{font-weight:bold} .row{display:flex;gap:4px} .row>span{flex:1;background:#eee}\
         #nohover{height:30px;background:#ccc}\
         .card:hover{background:#eef;border-color:#00f} .card:hover .t{color:#c00} .card a:hover{text-decoration:underline}",
    );
    if miss {
        html.push_str(".card:hover .row{height:30px}");
    }
    html.push_str("</style><div id=nohover>top bar no rule</div>");
    for i in 0..300 {
        html.push_str(&format!(
            "<div class=card><div class=t>Card {i} title with some words that wrap</div>\
             <p>Body text for card {i}, a sentence or two of text that wraps across lines in the card. <a href='#'>link {i}</a></p>\
             <div class=row><span>a</span><span>b</span><span>c</span></div><ul><li>one</li><li>two</li></ul></div>"
        ));
    }
    html
}

/// Timing, not a check: `--release -- --ignored --nocapture
/// zz_timing_of_a_hover_on_a_large_page`, once with the default and once
/// with `RUSTKIT_RESTYLE_REPAINT=0` (every pointer restyle laid out, as
/// before the short path). Prints the median milliseconds of a pointer move
/// onto a card and off it, of the same on the page whose hover changes a
/// height, and of a relayout nobody named.
#[test]
#[ignore = "timing"]
fn zz_timing_of_a_hover_on_a_large_page() {
    fn boxes(b: &LayoutBox) -> usize {
        1 + b.children.iter().map(boxes).sum::<usize>()
    }
    fn median(mut ms: Vec<f64>) -> f64 {
        ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
        ms[ms.len() / 2]
    }
    for miss in [false, true] {
        let mut engine = Engine::new(EngineConfig::default()).expect("engine");
        let view = engine
            .create_headless_view(Bounds { x: 0, y: 0, width: 1280, height: 800 })
            .expect("view");
        engine.load_html(view, &large_page(miss)).expect("load_html");
        let count = boxes(engine.views.get(&view).unwrap().layout.as_ref().unwrap());
        let mut moves = Vec::new();
        let mut split: [Vec<f64>; 4] = Default::default();
        for rep in 0..30 {
            for at in [([100.0, 470.0, 840.0][rep % 3], 60.0), (100.0, 15.0)] {
                let before = skipped(&engine, view);
                let started = std::time::Instant::now();
                hover(&mut engine, view, at);
                moves.push(started.elapsed().as_secs_f64() * 1000.0);
                if skipped(&engine, view) > before {
                    let last = LAST_RESTYLE_SPLIT.with(|last| last.get());
                    for (part, ms) in split.iter_mut().zip(last) {
                        part.push(ms);
                    }
                }
            }
        }
        if !split[0].is_empty() {
            let [builds, list, snapshot, render] = split.map(median);
            println!(
                "HOVERTIMING short path split: builds+compare+patch {builds:.2} display_list {list:.1} snapshot {snapshot:.1} render {render:.1} ms"
            );
        }
        let taken = skipped(&engine, view);
        let mut relayouts = Vec::new();
        for _ in 0..15 {
            let started = std::time::Instant::now();
            engine.relayout(view).expect("relayout");
            relayouts.push(started.elapsed().as_secs_f64() * 1000.0);
        }
        println!(
            "HOVERTIMING miss_page={miss} boxes={count} moves={} short_path={taken} move_median_ms={:.1} relayout_median_ms={:.1}",
            moves.len(),
            median(moves),
            median(relayouts)
        );
    }
}

/// #782 R2 (fix 3): a pointer restyle builds only the changed element and
/// the elements above it; their siblings got empty placeholder boxes, so
/// the `@container` walk never saw an earlier sibling's float and answered
/// the hovered element's query as if there were none. The full build
/// answers it false (since #782 HOLD 4 a float makes every later container
/// in its formatting context unknown), so the short path painted a colour
/// a full relayout does not. Chromium 148 paints red here (the container is
/// 300px, and the walk declines it as a trade-off); what this test holds
/// is that the short path agrees with the full relayout.
#[test]
fn a_hover_restyle_sees_an_earlier_siblings_float_in_a_container_query() {
    const FLOAT_PAGE: &str = "<style>\
        body{margin:0;font:16px/20px sans-serif}\
        .f{float:left;width:20px;height:20px;background:gray}\
        .pf::before{content:'';float:left;width:20px;height:20px}\
        .q{container-type:inline-size}\
        .h:hover{color:blue}\
        @container (min-width: 100px) { .h:hover{color:red} }\
        </style>\
        <div class=f></div>\
        <div class=q><span id=h class=h>hover me with some words</span></div>\
        <div style='overflow:hidden'><div class=pf></div>\
        <div class=q><span id=g class=h>and me with some words</span></div></div>";
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds {
            x: 0,
            y: 0,
            width: 400,
            height: 600,
        })
        .expect("view");
    engine.load_html(view, FLOAT_PAGE).expect("load_html");
    let colour = |engine: &mut Engine, id: &str| {
        let value = engine
            .execute_script(
                view,
                &format!("String(getComputedStyle(document.getElementById('{id}')).color)"),
            )
            .expect("script");
        value
            .strip_prefix("String(\"")
            .and_then(|v| v.strip_suffix("\")"))
            .unwrap_or(&value)
            .to_string()
    };
    // The auto-width container starts a context beside the float, so the
    // full build calls its width unknown (#782 HOLD 4; Chrome: 380px, red).
    // #h: an earlier sibling element floats; #g: an earlier sibling's
    // `::before` does.
    for (id, at) in [("h", (60.0, 8.0)), ("g", (60.0, 28.0))] {
        hover(&mut engine, view, NOWHERE);
        let before = skipped(&engine, view);
        hover(&mut engine, view, at);
        assert_eq!(skipped(&engine, view), before + 1, "#{id}: colour is paint");
        let short = painted(&engine, view);
        assert_eq!(colour(&mut engine, id), "rgb(0, 0, 255)", "#{id}: the container query is false after a float");
        assert_same(&short, &painted_by_a_full_relayout(&mut engine, view), id);
    }
}

/// #782 follow-up (Atlas #868): with an `@container` rule on the page, the
/// two builds of a pointer restyle built every sibling of the path, so each
/// hover walked the whole document twice. The walk reads only what comes
/// before the hovered element, and only until a float is on record there:
/// later siblings, and earlier ones once a float is known, are skipped.
/// The short path still paints what a full relayout does.
#[test]
fn a_container_query_hover_skips_siblings_the_walk_cannot_read() {
    const CQ: &str = "#list{container-type:inline-size}\
        @container (min-width: 100px) { .card:hover{color:navy} }";
    // Measured: 74, 314 and 50 nodes; 966 for the third card before.
    // Without a float the earlier cards are still built.
    let float = ".card:first-child::before{content:'';float:left;width:1px;height:1px}";
    for (float, at, most, what) in [
        ("", card(2), 120, "no float, the third card"),
        ("", card(12), 400, "no float, the 13th card"),
        (float, card(12), 120, "a float in the first card, the 13th card"),
    ] {
        let (mut engine, view) = cards(&format!("{CQ}{float}"));
        let at_rest = painted(&engine, view);
        hover(&mut engine, view, at);
        assert_eq!(skipped(&engine, view), 1, "{what}: colour is paint");
        assert!(walked() < most, "{what}: the two builds went into {} nodes of about 1000", walked());
        let short = painted(&engine, view);
        assert_ne!(short.0, at_rest.0, "{what}");
        assert_same(&short, &painted_by_a_full_relayout(&mut engine, view), what);
    }
}

// ---------------------------------------------------------------------------
// S22 (reddit, Pete's hand test on 209f6a64): each pointer move with a
// `:hover` change cost a full build and layout (`build_ms=93`, 380 on the
// logged-in feed), because one `:hover` rule with a sibling combinator, or
// with the pseudo-class inside `:not()` / `:is()`, anywhere in the sheets
// sent every pointer restyle down the full path. Chrome restyles the
// elements whose hover state changed and the elements their matched rules
// reach, and lays out only what moved.
// ---------------------------------------------------------------------------

/// A feed of `posts` posts, six elements each, whose sheet reads `:hover`
/// the way a site's does: on the post and under it, on a button, inside
/// `:not()`, and left of a sibling combinator.
fn feed(posts: usize, extra_css: &str) -> String {
    let mut html = String::from(
        "<style>body{margin:0;font:14px/18px sans-serif}\
         #feed{width:600px}\
         .post{padding:8px;border:1px solid #ccc;border-top-width:0;color:black}\
         .post:first-child{border-top-width:1px}\
         .post:hover{background:#f6f7f8}\
         .post:hover .title{color:#0079d3}\
         .title{font-weight:bold} .meta{color:#777;font-size:12px}\
         .btn{display:inline-block;padding:2px 6px;margin-right:4px;background:#eee}\
         .btn:hover{background:#ddd}\
         .btn:not(:hover){color:#333}\
         .post:hover + .post{border-top-color:#f6f7f8}",
    );
    html.push_str(extra_css);
    html.push_str("</style><div id=feed>");
    for i in 0..posts {
        html.push_str(&format!(
            "<div class=post id=p{i}><div class=title>Post {i}: a title that says something</div>\
             <div class=meta>posted by someone</div>\
             <div class=actions><span class=btn>up</span><span class=btn>comment</span></div></div>"
        ));
    }
    html.push_str("</div>");
    html
}

/// Beside the feed: the body's own background.
const BESIDE: (f32, f32) = (700.0, 100.0);

/// The feed in a view tall enough that the first forty posts are in it.
fn feed_page(posts: usize, extra_css: &str) -> (Engine, EngineViewId) {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds {
            x: 0,
            y: 0,
            width: 800,
            height: 3000,
        })
        .expect("view");
    engine.load_html(view, &feed(posts, extra_css)).expect("load_html");
    (engine, view)
}

fn js_string(engine: &mut Engine, view: EngineViewId, js: &str) -> String {
    let value = engine.execute_script(view, js).expect("script");
    value
        .strip_prefix("String(\"")
        .and_then(|v| v.strip_suffix("\")"))
        .unwrap_or(&value)
        .to_string()
}

/// The viewport centre of the element `js` evaluates to.
fn centre(engine: &mut Engine, view: EngineViewId, js: &str) -> (f32, f32) {
    let value = js_string(
        engine,
        view,
        &format!("(function(){{var r=({js}).getBoundingClientRect();return Math.round(r.left+r.width/2)+','+Math.round(r.top+r.height/2)}})()"),
    );
    let (x, y) = value.split_once(',').expect("x,y");
    (x.parse().unwrap(), y.parse().unwrap())
}

fn computed(engine: &mut Engine, view: EngineViewId, js: &str, property: &str) -> String {
    js_string(engine, view, &format!("String(getComputedStyle({js}).getPropertyValue('{property}'))"))
}

fn last_build_ms(engine: &Engine, view: EngineViewId) -> f64 {
    engine.relayout_stats(view).expect("view").last_build_ms
}

fn median(mut ms: Vec<f64>) -> f64 {
    ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
    ms[ms.len() / 2]
}

fn boxes(b: &LayoutBox) -> usize {
    1 + b.children.iter().map(boxes).sum::<usize>()
}

/// (a) A move onto a post of a 200-post feed (1,200 elements) builds the
/// post and the one after it, not the page: under 5% of the boxes, and a
/// small fraction of a full build's time (measured 3.9 ms against 39 ms,
/// a tenth; gated at 15% for a loaded machine).
#[test]
fn a_hover_on_a_long_feed_builds_a_few_of_its_boxes() {
    let (mut engine, view) = feed_page(200, "");
    let total = boxes(engine.views.get(&view).unwrap().layout.as_ref().unwrap());
    assert!(total > 1200, "{total} boxes");
    let at_rest = painted(&engine, view);
    let mut full = Vec::new();
    for _ in 0..5 {
        engine.relayout(view).expect("relayout");
        full.push(last_build_ms(&engine, view));
    }
    let mut short = Vec::new();
    let mut walked_most = 0;
    for n in 2..=11 {
        let at = centre(&mut engine, view, &format!("document.getElementById('p{n}')"));
        let before = skipped(&engine, view);
        hover(&mut engine, view, at);
        assert_eq!(skipped(&engine, view), before + 1, "post {n}: every hover rule is paint");
        assert!(walked() * 20 < total, "post {n}: the two builds went into {} of {total} boxes", walked());
        walked_most = walked_most.max(walked());
        short.push(last_build_ms(&engine, view));
    }
    assert_same(&painted(&engine, view), &painted_by_a_full_relayout(&mut engine, view), "post 11 hovered");
    // (d) Leaving puts everything back.
    hover(&mut engine, view, BESIDE);
    assert_eq!(painted(&engine, view), at_rest, "off the feed");
    let (short, full) = (median(short), median(full));
    println!(
        "S22 feed: boxes={total} walked_most={walked_most} build_ms per move {short:.2} (short path) vs {full:.2} (full build)"
    );
    assert!(short * 100.0 < full * 15.0, "a move's builds took {short:.2} ms against a full build's {full:.2} ms");
}

/// (b) `.a:hover .b`: the descendant is restyled, by the short path; so is
/// a subject the rule names inside `:is()`.
#[test]
fn a_hover_rule_on_a_descendant_restyles_it_on_the_short_path() {
    let (mut engine, view) = feed_page(20, "#feed :is(.title:hover, .meta:hover){text-decoration:underline}");
    let title = "document.getElementById('p3').querySelector('.title')";
    assert_eq!(computed(&mut engine, view, title, "color"), "rgb(0, 0, 0)");
    let at = centre(&mut engine, view, "document.getElementById('p3').querySelector('.meta')");
    hover(&mut engine, view, at);
    assert_eq!(skipped(&engine, view), 1, "colours and an underline are paint");
    assert_eq!(computed(&mut engine, view, title, "color"), "rgb(0, 121, 211)", "the post is hovered: its title");
    let meta = "document.getElementById('p3').querySelector('.meta')";
    assert!(
        computed(&mut engine, view, meta, "text-decoration-line").contains("underline"),
        "the :is() member on the hovered meta line"
    );
    assert_same(&painted(&engine, view), &painted_by_a_full_relayout(&mut engine, view), "meta hovered");
}

/// (c) `.a:hover + .b` (after #878's sibling walk): the next post is
/// restyled with the hovered one, on the short path; the post after it is
/// not touched. `:not(:hover)` on the button flips on the button alone.
#[test]
fn a_hover_rule_with_a_sibling_combinator_restyles_the_sibling_on_the_short_path() {
    let (mut engine, view) = feed_page(20, "");
    let border = |engine: &mut Engine, n: usize| {
        computed(engine, view, &format!("document.getElementById('p{n}')"), "border-top-color")
    };
    assert_eq!(border(&mut engine, 4), "rgb(204, 204, 204)");
    let at_rest = painted(&engine, view);
    let at = centre(&mut engine, view, "document.getElementById('p3')");
    hover(&mut engine, view, at);
    assert_eq!(skipped(&engine, view), 1, "a border colour is paint");
    assert_eq!(border(&mut engine, 4), "rgb(246, 247, 248)", "the next post, by `+`");
    assert_eq!(border(&mut engine, 5), "rgb(204, 204, 204)", "the one after is not reached");
    assert!(walked() < 60, "the two builds went into {} nodes: two posts, twice", walked());
    assert_same(&painted(&engine, view), &painted_by_a_full_relayout(&mut engine, view), "post 3 hovered");

    // The first button of post 3: `:not(:hover)` gives way on it.
    let btn = "document.getElementById('p3').querySelector('.btn')";
    assert_eq!(computed(&mut engine, view, btn, "color"), "rgb(51, 51, 51)");
    let at = centre(&mut engine, view, btn);
    hover(&mut engine, view, at);
    assert_eq!(skipped(&engine, view), 2, "the button's colours are paint");
    assert_eq!(computed(&mut engine, view, btn, "color"), "rgb(0, 0, 0)", "the post's colour shows through");
    assert_same(&painted(&engine, view), &painted_by_a_full_relayout(&mut engine, view), "button hovered");

    // (d) Leaving reverts the sibling too.
    hover(&mut engine, view, BESIDE);
    assert_eq!(skipped(&engine, view), 3);
    assert_eq!(border(&mut engine, 4), "rgb(204, 204, 204)");
    assert_eq!(painted(&engine, view), at_rest, "off the feed");
}

/// (c) `.a:hover ~ .b .c`: every later post's meta line, on the short path.
#[test]
fn a_hover_rule_with_a_general_sibling_combinator_restyles_every_later_sibling() {
    let (mut engine, view) = feed_page(20, ".post:hover ~ .post .meta{color:red}");
    let meta = |engine: &mut Engine, n: usize| {
        computed(engine, view, &format!("document.getElementById('p{n}').querySelector('.meta')"), "color")
    };
    let at = centre(&mut engine, view, "document.getElementById('p15')");
    hover(&mut engine, view, at);
    assert_eq!(skipped(&engine, view), 1, "a colour is paint");
    assert_eq!(meta(&mut engine, 14), "rgb(119, 119, 119)", "an earlier post");
    assert_eq!(meta(&mut engine, 16), "rgb(255, 0, 0)", "the next post");
    assert_eq!(meta(&mut engine, 19), "rgb(255, 0, 0)", "the last post");
    assert_same(&painted(&engine, view), &painted_by_a_full_relayout(&mut engine, view), "post 15 hovered");
    hover(&mut engine, view, BESIDE);
    assert_eq!(meta(&mut engine, 19), "rgb(119, 119, 119)", "off the feed");
}

/// (e) A hover that changes layout (padding) still lays the page out: the
/// hovered post grows and the posts after it move down, as a full relayout
/// places them.
#[test]
fn a_hover_that_changes_padding_moves_the_following_siblings() {
    let (mut engine, view) = feed_page(20, ".post:hover{padding:12px}");
    let top = |engine: &mut Engine, n: usize| -> f32 {
        js_string(
            engine,
            view,
            &format!("String(document.getElementById('p{n}').getBoundingClientRect().top)"),
        )
        .parse()
        .expect("top")
    };
    let (p3, p4, p9) = (top(&mut engine, 3), top(&mut engine, 4), top(&mut engine, 9));
    let at = centre(&mut engine, view, "document.getElementById('p3')");
    hover(&mut engine, view, at);
    assert_eq!(skipped(&engine, view), 0, "padding is layout");
    assert_eq!(top(&mut engine, 3), p3, "the hovered post stays where it is");
    assert_eq!(top(&mut engine, 4), p4 + 8.0, "the next post moves down by the padding");
    assert_eq!(top(&mut engine, 9), p9 + 8.0, "and so does every later one");
    assert_same(&painted(&engine, view), &painted_by_a_full_relayout(&mut engine, view), "post 3 hovered");
    hover(&mut engine, view, BESIDE);
    assert_eq!(top(&mut engine, 9), p9, "off the feed");
}

/// Timing, not a check: `--features headless -- --ignored --nocapture
/// zz_cascade_timing_of_a_feed_hover`. Logs the "Cascade timing" line of
/// every build (the two of a pointer restyle, then a full one) on the
/// 200-post feed, to see where a move's build time goes.
#[test]
#[ignore = "timing"]
fn zz_cascade_timing_of_a_feed_hover() {
    std::env::set_var("RUSTKIT_CASCADE_TIMING", "1");
    let _ = tracing_subscriber::fmt()
        .with_env_filter("rustkit_engine=info")
        .with_writer(std::io::stderr)
        .try_init();
    let (mut engine, view) = feed_page(200, "");
    for n in 2..=4 {
        let at = centre(&mut engine, view, &format!("document.getElementById('p{n}')"));
        eprintln!("--- move onto post {n}");
        hover(&mut engine, view, at);
        eprintln!("build_ms={:.2} walked={}", last_build_ms(&engine, view), walked());
    }
    eprintln!("--- full relayout");
    engine.relayout(view).expect("relayout");
    eprintln!("build_ms={:.2}", last_build_ms(&engine, view));
}

// ---------------------------------------------------------------------------
// LAT3 (Pete's flag-on log, 11-05): every one of a top-20 marketplace's 59 hover restyles
// missed the short path with "a rule reads the pseudo-class where no
// compound shows it" and cost a full build and layout (210 ms). The sheet's
// only such rules are two of the shape
// `[data-x]:not(:has([data-x]:hover)):hover::after`: `:hover` inside
// `:has()`, which the pointer index did not file, so any `:hover` change
// could change any element. A `:hover` inside a complex `:not()` member
// (`.a:not(.b:hover .a)`) missed the same way. Chrome restyles the
// `:has()` subjects above the changed element and the elements a `:not()`
// member's compounds name, and nothing else.
// ---------------------------------------------------------------------------

/// Clears the last miss, so a hit leaves it empty.
fn clear_miss() {
    LAST_RESTYLE_MISS.with(|why| why.borrow_mut().clear());
}

const ANYWHERE_MISS: &str = "a rule reads the pseudo-class where no compound shows it";

/// A marketplace's shape: a debug outline drawn on an element carrying an attribute
/// that no element of the page carries. It can never match, so no move
/// restyles anything for it.
#[test]
fn a_has_hover_rule_that_matches_nothing_keeps_the_short_path() {
    let (mut engine, view) = feed_page(
        20,
        "[data-dbg=indigo]:not(:has([data-dbg]:hover)):hover::after,\
         [data-dbg]:not(:has([data-dbg]:hover)):hover::after{content:'';outline:2px solid red}",
    );
    let at_rest = painted(&engine, view);
    let at = centre(&mut engine, view, "document.getElementById('p3')");
    clear_miss();
    hover(&mut engine, view, at);
    assert_eq!(last_miss(), "", "the short path was taken");
    assert_eq!(skipped(&engine, view), 1, "every rule that can match is paint");
    assert!(walked() < 60, "the two builds went into {} nodes: two posts, twice", walked());
    assert_same(&painted(&engine, view), &painted_by_a_full_relayout(&mut engine, view), "post 3 hovered");
    hover(&mut engine, view, BESIDE);
    assert_eq!(skipped(&engine, view), 2);
    assert_eq!(painted(&engine, view), at_rest, "off the feed");
}

/// `:has(.btn:hover)`: the post whose button is hovered is the subject. It
/// is restyled (with what is under it) on the short path; the other posts
/// are not built. Chrome 143 (the probe in the PR body):
/// post 3's background is rgb(255, 238, 238) with its button hovered,
/// rgb(246, 247, 248) with its title hovered, post 4's stays transparent.
#[test]
fn a_has_hover_rule_restyles_its_subject_on_the_short_path() {
    let (mut engine, view) = feed_page(20, ".post:has(.btn:hover){background:#fee}");
    let bg = |engine: &mut Engine, n: usize| {
        computed(engine, view, &format!("document.getElementById('p{n}')"), "background-color")
    };
    let at_rest = painted(&engine, view);
    let btn = "document.getElementById('p3').querySelector('.btn')";
    let at = centre(&mut engine, view, btn);
    clear_miss();
    hover(&mut engine, view, at);
    assert_eq!(last_miss(), "", "the short path was taken");
    assert_eq!(skipped(&engine, view), 1, "a background is paint");
    assert_eq!(bg(&mut engine, 3), "rgb(255, 238, 238)", "the post holds the hovered button");
    assert_eq!(bg(&mut engine, 4), "rgba(0, 0, 0, 0)", "the next post does not");
    assert!(walked() < 60, "the two builds went into {} nodes: two posts, twice", walked());
    assert_same(&painted(&engine, view), &painted_by_a_full_relayout(&mut engine, view), "button hovered");

    let at = centre(&mut engine, view, "document.getElementById('p3').querySelector('.title')");
    clear_miss();
    hover(&mut engine, view, at);
    assert_eq!(last_miss(), "");
    assert_eq!(skipped(&engine, view), 2);
    assert_eq!(bg(&mut engine, 3), "rgb(246, 247, 248)", "the title is hovered, the button is not");
    assert_same(&painted(&engine, view), &painted_by_a_full_relayout(&mut engine, view), "title hovered");

    hover(&mut engine, view, BESIDE);
    assert_eq!(skipped(&engine, view), 3);
    assert_eq!(painted(&engine, view), at_rest, "off the feed");
}

/// A `:has(:hover)` rule that sets a layout property still lays the page
/// out, for the property's sake and not for the index's.
#[test]
fn a_has_hover_rule_that_sets_a_width_is_laid_out() {
    let (mut engine, view) = feed_page(20, ".post:has(.btn:hover){width:500px}");
    let width = |engine: &mut Engine, n: usize| -> f32 {
        js_string(engine, view, &format!("String(document.getElementById('p{n}').getBoundingClientRect().width)"))
            .parse()
            .expect("width")
    };
    assert_eq!(width(&mut engine, 3), 600.0);
    let at = centre(&mut engine, view, "document.getElementById('p3').querySelector('.btn')");
    clear_miss();
    hover(&mut engine, view, at);
    assert_eq!(skipped(&engine, view), 0, "width is layout");
    assert_eq!(last_miss(), "style: width");
    assert_eq!(width(&mut engine, 3), 518.0, "500px plus padding and border");
    assert_eq!(width(&mut engine, 4), 600.0);
    assert_same(&painted(&engine, view), &painted_by_a_full_relayout(&mut engine, view), "button hovered");
}

/// `:hover` in a complex `:not()` member: the meta line is grey only under a
/// hovered post. The post is the compound the member names; restyling its
/// subtree reaches the meta line. Chrome 143: rgb(1, 2, 3) at rest,
/// rgb(119, 119, 119) under the hovered post.
#[test]
fn a_hover_inside_a_complex_not_member_takes_the_short_path() {
    let (mut engine, view) = feed_page(20, ".meta:not(.post:hover .meta){color:rgb(1,2,3)}");
    let meta = |engine: &mut Engine, n: usize| {
        computed(engine, view, &format!("document.getElementById('p{n}').querySelector('.meta')"), "color")
    };
    assert_eq!(meta(&mut engine, 3), "rgb(1, 2, 3)");
    let at_rest = painted(&engine, view);
    let at = centre(&mut engine, view, "document.getElementById('p3')");
    clear_miss();
    hover(&mut engine, view, at);
    assert_eq!(last_miss(), "", "the short path was taken");
    assert_eq!(skipped(&engine, view), 1, "a colour is paint");
    assert_eq!(meta(&mut engine, 3), "rgb(119, 119, 119)", "under the hovered post");
    assert_eq!(meta(&mut engine, 4), "rgb(1, 2, 3)");
    assert_same(&painted(&engine, view), &painted_by_a_full_relayout(&mut engine, view), "post 3 hovered");
    hover(&mut engine, view, BESIDE);
    assert_eq!(painted(&engine, view), at_rest, "off the feed");
}

/// The marketplace-shaped sheet over about 5,000 boxes: hover rules nested in
/// `:is()` / `:where()`, on a compound left of a combinator, with
/// `:not([disabled],[aria-disabled=true])`, a `::after`, and the
/// `:has(:hover)` debug rule. Every move takes the short path, and its two
/// builds take a small fraction of the full build (both are printed).
#[test]
fn a_marketplace_shaped_sheet_hovers_on_the_short_path_in_a_few_ms() {
    let mut html = String::from(
        "<style>body{margin:0;font:13px/16px sans-serif}\
         .grid{width:1200px}\
         .item{display:inline-block;width:280px;margin:4px;padding:6px;border:1px solid #ddd;vertical-align:top}\
         .item .title{color:#111} .item .price{font-weight:bold} .btn{display:inline-block;padding:2px 6px}\
         :is(.item:focus-within,.item:hover) .btn{background:#eef}\
         .grid :is(.item .title:hover,.item .title:focus-visible){color:#06c;text-decoration:underline}\
         :where(.item:hover) .price{color:#c00}\
         .btn:not([disabled],[aria-disabled=true]):hover:after{content:'';outline:1px solid #06c}\
         .item:hover:not(:active) .meta{color:#555}\
         .vl:is(:hover,:focus-within)>.pop{background:#ffe}\
         [data-dbg=indigo]:not(:has([data-dbg]:hover)):hover:after,\
         [data-dbg]:not(:has([data-dbg]:hover)):hover:after{content:'';outline:2px solid red}\
         </style><div class=grid>",
    );
    for i in 0..330 {
        html.push_str(&format!(
            "<div class=item id=i{i}><div class=title>Item {i} with a title of some words</div>\
             <div class=price>$ {i}.99</div><div class=meta>Free shipping <b>new</b></div>\
             <div class=vl><span class=pop>pop</span></div>\
             <span class=btn>Watch</span><span class=btn aria-disabled=true>Buy</span></div>"
        ));
    }
    html.push_str("</div>");
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 1280, height: 4000 })
        .expect("view");
    engine.load_html(view, &html).expect("load_html");
    let total = boxes(engine.views.get(&view).unwrap().layout.as_ref().unwrap());
    assert!(total > 5000, "{total} boxes");
    let mut full = Vec::new();
    let mut full_ms = Vec::new();
    for _ in 0..3 {
        let started = std::time::Instant::now();
        engine.relayout(view).expect("relayout");
        full_ms.push(started.elapsed().as_secs_f64() * 1000.0);
        full.push(last_build_ms(&engine, view));
    }
    let mut short = Vec::new();
    let mut move_ms = Vec::new();
    let mut split: [Vec<f64>; 4] = Default::default();
    for n in 10..20 {
        let target = if n % 2 == 0 { "" } else { ".querySelector('.title')" };
        let at = centre(&mut engine, view, &format!("document.getElementById('i{n}'){target}"));
        let before = skipped(&engine, view);
        clear_miss();
        let started = std::time::Instant::now();
        hover(&mut engine, view, at);
        move_ms.push(started.elapsed().as_secs_f64() * 1000.0);
        let last = LAST_RESTYLE_SPLIT.with(|last| last.get());
        for (part, ms) in split.iter_mut().zip(last) {
            part.push(ms);
        }
        assert_eq!(last_miss(), "", "item {n}: the short path was taken");
        assert_eq!(skipped(&engine, view), before + 1, "item {n}: every hover rule is paint");
        short.push(last_build_ms(&engine, view));
    }
    assert_same(&painted(&engine, view), &painted_by_a_full_relayout(&mut engine, view), "item 19's title hovered");
    let (short, full, move_ms, full_ms) = (median(short), median(full), median(move_ms), median(full_ms));
    let [builds, list, snapshot, render] = split.map(median);
    println!(
        "LAT3 marketplace shape: boxes={total} build_ms per move {short:.2} (short path) vs {full:.2} (full build); \
         whole move {move_ms:.1} ms (builds+patch {builds:.1}, display list {list:.1}, snapshot {snapshot:.1}, \
         render {render:.1}) vs whole relayout {full_ms:.1} ms"
    );
    // Measured 5.1-5.4 ms against 100 ms in a debug build: ~2 ms of each
    // of the two builds is the walk past the grid's 329 other items, which
    // the restyle build steps over one by one (#894's design, not changed
    // here). Gated at a tenth of the full build for a loaded machine.
    assert!(short * 10.0 < full, "a move's builds took {short:.2} ms (full build {full:.2} ms)");
}
