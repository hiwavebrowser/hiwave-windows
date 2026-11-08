//! CH2.E22: `:focus`, `:focus-within` and `:focus-visible` in the main
//! cascade. The focused element (the page's, a button included) matches
//! `:focus`; it and the elements above it match `:focus-within`; it matches
//! `:focus-visible` when it was focused from the keyboard, or by a click on
//! a control that takes typed text, or by script after either. A focus
//! change restyles the elements the sheets' focus rules can match through
//! the pointer restyle's short path: a paint-only rule skips layout.
//!
//! Expected values are the oracle Chrome's (143.0.7499.4), from
//! `tools/parity_oracle/focus_pseudo_log.mjs` on
//! `tools/parity_oracle/focus_pseudo_page.html` (the page below):
//!
//! ```text
//! at rest:  i.border=rgb(0, 0, 255) f.bg=rgb(255, 255, 255) b.bg=rgb(221, 221, 221) b.outline=none
//! click #i: active=i i.border=rgb(255, 0, 0) f.bg=rgb(255, 255, 0) i:focus-visible=true
//! click #d: active=BODY i.border=rgb(0, 0, 255) f.bg=rgb(255, 255, 255)
//! click #b: active=b b.bg=rgb(0, 255, 255) b.outline=none 0px b:focus-visible=false
//! Tab:      active=i i.border=rgb(255, 0, 0) f.bg=rgb(255, 255, 0)
//! Tab:      active=b b.bg=rgb(0, 255, 255) b.outline=solid 3px b:focus-visible=true
//! Shift+Tab: active=i
//! script b.focus() after a mouse focus: b:focus-visible=false
//! script i.focus() after a mouse focus: i:focus-visible=true
//! script b.focus() on a fresh page: b.outline=solid 3px (no input yet)
//! ```

use super::*;

const PAGE: &str = "<!doctype html><html><head><style>\
    body { margin: 0; font: 16px/20px sans-serif; }\
    #f { height: 60px; padding: 4px; background: rgb(255, 255, 255); }\
    #f:focus-within { background: rgb(255, 255, 0); }\
    input { border: 2px solid rgb(0, 0, 255); outline: none; }\
    input:focus { border-color: rgb(255, 0, 0); }\
    #b { width: 80px; height: 24px; outline: none; background: rgb(221, 221, 221); }\
    #b:focus { background: rgb(0, 255, 255); }\
    #b:focus-visible { outline: 3px solid rgb(0, 128, 0); }\
    #d { height: 40px; }\
    </style></head><body>\
    <form id=\"f\"><input id=\"i\"></form>\
    <div style=\"height: 30px\"><button id=\"b\">Go</button></div>\
    <div id=\"d\">plain text</div>\
    </body></html>";

fn page_with(html: &str) -> (Engine, EngineViewId) {
    let mut engine = Engine::new_page(EngineConfig::default(), None, Arc::new(AtomicU64::new(0))).expect("engine");
    let id = engine.create_page_view(Bounds::new(0, 0, 800, 600)).expect("page view");
    engine.load_html(id, html).expect("load_html");
    js(&mut engine, id, "window.$ = function (i) { return document.getElementById(i); }; 0");
    (engine, id)
}

fn page() -> (Engine, EngineViewId) {
    page_with(PAGE)
}

fn js(engine: &mut Engine, id: EngineViewId, script: &str) -> String {
    let out = engine.execute_script(id, &format!("String(eval({script:?}))")).expect("script");
    out.strip_prefix("String(\"")
        .and_then(|s| s.strip_suffix("\")"))
        .map(|s| s.replace("\\\"", "\""))
        .unwrap_or(out)
}

fn css(engine: &mut Engine, id: EngineViewId, el: &str, property: &str) -> String {
    js(engine, id, &format!("getComputedStyle($('{el}')).getPropertyValue('{property}')"))
}

/// The page's active element, by id (`BODY` for the body).
fn active(engine: &mut Engine, id: EngineViewId) -> String {
    js(engine, id, "document.activeElement ? (document.activeElement.id || document.activeElement.nodeName) : 'null'")
}

/// The centre of an element, in viewport points.
fn centre(engine: &mut Engine, id: EngineViewId, el: &str) -> (f32, f32) {
    let r = js(engine, id, &format!("var r = $('{el}').getBoundingClientRect(); (r.x + r.width / 2) + ',' + (r.y + r.height / 2)"));
    let (x, y) = r.split_once(',').expect("x,y");
    (x.parse().expect("x"), y.parse().expect("y"))
}

fn click(engine: &mut Engine, id: EngineViewId, el: &str) {
    let (x, y) = centre(engine, id, el);
    engine.mouse_move_at_point(id, x, y);
    engine.mouse_down_at_point(id, x, y);
    engine.click_at_point(id, x, y);
}

fn tab(engine: &mut Engine, id: EngineViewId, shift: bool) {
    engine.handle_text_key(id, 0x09, "", false, shift, false);
}

fn skipped(engine: &Engine, id: EngineViewId) -> u32 {
    engine.relayout_stats(id).expect("view").layouts_skipped
}

const BLUE: &str = "rgb(0, 0, 255)";
const RED: &str = "rgb(255, 0, 0)";
const WHITE: &str = "rgb(255, 255, 255)";
const YELLOW: &str = "rgb(255, 255, 0)";
const GREY: &str = "rgb(221, 221, 221)";
const CYAN: &str = "rgb(0, 255, 255)";

#[test]
fn input_focus_border_follows_a_click_and_a_blur() {
    let (mut engine, id) = page();
    assert_eq!(css(&mut engine, id, "i", "border-top-color"), BLUE, "at rest");
    click(&mut engine, id, "i");
    assert_eq!(active(&mut engine, id), "i");
    assert_eq!(css(&mut engine, id, "i", "border-top-color"), RED, "input:focus");
    click(&mut engine, id, "d");
    assert_eq!(active(&mut engine, id), "BODY");
    assert_eq!(css(&mut engine, id, "i", "border-top-color"), BLUE, "blurred");
}

#[test]
fn form_focus_within_paints_the_ancestor() {
    let (mut engine, id) = page();
    assert_eq!(css(&mut engine, id, "f", "background-color"), WHITE);
    click(&mut engine, id, "i");
    assert_eq!(css(&mut engine, id, "f", "background-color"), YELLOW, "form:focus-within");
    click(&mut engine, id, "d");
    assert_eq!(css(&mut engine, id, "f", "background-color"), WHITE, "nothing focused within");
}

#[test]
fn a_clicked_button_is_focused_without_a_ring() {
    let (mut engine, id) = page();
    click(&mut engine, id, "b");
    assert_eq!(active(&mut engine, id), "b", "a click focuses a button");
    assert_eq!(css(&mut engine, id, "b", "background-color"), CYAN, "#b:focus");
    assert_eq!(css(&mut engine, id, "b", "outline-style"), "none", "mouse focus on a button: no :focus-visible");
    click(&mut engine, id, "d");
    assert_eq!(css(&mut engine, id, "b", "background-color"), GREY);
}

#[test]
fn tab_moves_the_focus_with_a_ring() {
    let (mut engine, id) = page();
    tab(&mut engine, id, false);
    assert_eq!(active(&mut engine, id), "i", "the first Tab: the input");
    assert_eq!(css(&mut engine, id, "i", "border-top-color"), RED);
    assert_eq!(css(&mut engine, id, "f", "background-color"), YELLOW);
    tab(&mut engine, id, false);
    assert_eq!(active(&mut engine, id), "b", "the second Tab: the button");
    assert_eq!(css(&mut engine, id, "b", "background-color"), CYAN);
    assert_eq!(css(&mut engine, id, "b", "outline-style"), "solid", "keyboard focus: :focus-visible");
    assert_eq!(css(&mut engine, id, "b", "outline-width"), "3px");
    assert_eq!(css(&mut engine, id, "i", "border-top-color"), BLUE);
    tab(&mut engine, id, true);
    assert_eq!(active(&mut engine, id), "i", "Shift+Tab: back to the input");
    assert_eq!(css(&mut engine, id, "b", "outline-style"), "none");
}

#[test]
fn script_focus_is_visible_as_the_last_focus_was() {
    // No input yet: script's focus shows the ring.
    let (mut engine, id) = page();
    js(&mut engine, id, "$('b').focus(); 0");
    assert_eq!(css(&mut engine, id, "b", "outline-style"), "solid", "fresh page");
    // After a mouse focus: a button's does not, a text input's does.
    let (mut engine, id) = page();
    click(&mut engine, id, "b");
    click(&mut engine, id, "d");
    js(&mut engine, id, "$('b').focus(); 0");
    assert_eq!(css(&mut engine, id, "b", "background-color"), CYAN);
    assert_eq!(css(&mut engine, id, "b", "outline-style"), "none", "after a mouse focus");
    js(&mut engine, id, "$('i').focus(); 0");
    assert_eq!(js(&mut engine, id, "$('i').matches(':focus-visible')"), "true", "a text input always");
    assert_eq!(css(&mut engine, id, "i", "border-top-color"), RED);
}

#[test]
fn a_paint_only_focus_change_skips_layout() {
    let (mut engine, id) = page();
    engine.relayout(id).expect("relayout");
    let before = skipped(&engine, id);
    js(&mut engine, id, "$('b').focus(); 0");
    assert_eq!(skipped(&engine, id), before + 1, "background and outline are paint: no layout");
    assert_eq!(css(&mut engine, id, "b", "background-color"), CYAN);
    js(&mut engine, id, "$('b').blur(); 0");
    assert_eq!(skipped(&engine, id), before + 2, "the blur too");
    assert_eq!(css(&mut engine, id, "b", "background-color"), GREY);
}

#[test]
fn a_focus_rule_that_changes_layout_lays_out() {
    let html = PAGE.replace("#b:focus { background: rgb(0, 255, 255); }", "#b:focus { width: 120px; }");
    let (mut engine, id) = page_with(&html);
    engine.relayout(id).expect("relayout");
    let before = skipped(&engine, id);
    js(&mut engine, id, "$('b').focus(); 0");
    assert_eq!(skipped(&engine, id), before, "a width change is laid out");
    assert_eq!(js(&mut engine, id, "$('b').getBoundingClientRect().width"), "120");
}

#[test]
fn the_short_path_paints_as_a_full_relayout_does() {
    let (mut engine, id) = page();
    engine.relayout(id).expect("relayout");
    let painted = |engine: &Engine| {
        let view = engine.views.get(&id).expect("view");
        format!("{:?}", view.display_list.as_ref().expect("list").commands)
    };
    for script in ["$('b').focus(); 0", "$('i').focus(); 0", "$('i').blur(); 0"] {
        js(&mut engine, id, script);
        let short = painted(&engine);
        engine.relayout(id).expect("relayout");
        assert_eq!(short, painted(&engine), "after {script}");
    }
}

#[test]
fn the_ua_focus_visible_outline_is_chromiums() {
    // Chromium 143 (`focus_pseudo_log.mjs`'s ring probe): every focused
    // element matching `:focus-visible` has `outline: auto 1px
    // rgb(0, 95, 204)`, offset 0 (a link's 1px); an author `outline: none`
    // turns it off.
    let (mut engine, id) = page_with(
        "<body style='margin:20px'><input id=i><input id=n style='outline:none'><a id=a href='#x'>link</a><button id=b>B</button></body>",
    );
    assert_eq!(css(&mut engine, id, "i", "outline-style"), "none", "at rest");
    click(&mut engine, id, "i");
    assert_eq!(css(&mut engine, id, "i", "outline-style"), "auto", "a clicked field: :focus-visible");
    assert_eq!(css(&mut engine, id, "i", "outline-color"), "rgb(0, 95, 204)");
    assert_eq!(css(&mut engine, id, "i", "outline-offset"), "0px");
    click(&mut engine, id, "n");
    assert_eq!(css(&mut engine, id, "n", "outline-style"), "none", "the author's none wins");
    assert_eq!(css(&mut engine, id, "i", "outline-style"), "none", "blurred");
    click(&mut engine, id, "b");
    assert_eq!(css(&mut engine, id, "b", "outline-style"), "none", "a clicked button: no ring");
    tab(&mut engine, id, true);
    tab(&mut engine, id, false);
    assert_eq!(active(&mut engine, id), "b");
    assert_eq!(css(&mut engine, id, "b", "outline-style"), "auto", "a tabbed button: the ring");
    tab(&mut engine, id, true);
    assert_eq!(active(&mut engine, id), "a");
    assert_eq!(css(&mut engine, id, "a", "outline-offset"), "1px", "a link's ring is 1px out");
}

/// Whether the view's display list paints the UA focus ring: an `auto`
/// outline in rgb(0, 95, 204), drawn as a rounded ring (radius >= 3).
fn paints_the_ring(engine: &Engine, id: EngineViewId) -> bool {
    let ring = rustkit_css::Color::new(0, 95, 204, 1.0);
    let view = engine.views.get(&id).expect("view");
    view.display_list.as_ref().expect("list").commands.iter().any(|c| {
        matches!(c, rustkit_layout::DisplayCommand::RoundedBorder { colors, .. } if colors.iter().all(|c| *c == ring))
    })
}

#[test]
fn a_clicked_field_paints_the_ring_and_an_outline_none_field_does_not() {
    // The renderer's own ring is gone: the UA outline is what shows a
    // focused text field.
    let (mut engine, id) = page_with(
        "<body style='margin:20px'><input id=i><br><input id=n style='outline:none'><div id=d style='height:40px'>x</div></body>",
    );
    assert!(!paints_the_ring(&engine, id), "at rest");
    click(&mut engine, id, "i");
    assert!(paints_the_ring(&engine, id), "a clicked field paints its auto outline");
    click(&mut engine, id, "n");
    assert!(!paints_the_ring(&engine, id), "an outline:none field paints none (and the blurred one neither)");
}

const KEYS_PAGE: &str = "<body style='margin:0'>\
    <button id=b>B</button><a id=a href='#x'>L</a>\
    <div style='visibility:hidden'><button id=v>v</button></div>\
    <fieldset disabled><button id=fd>fd</button></fieldset>\
    <div inert><button id=in>in</button></div>\
    <button id=last>last</button>\
    <script>window.log = []; ['keydown', 'keyup', 'click'].forEach(function (t) {\
      document.addEventListener(t, function (e) { log.push(t + ':' + e.target.id); }, true); });</script></body>";

#[test]
fn tab_skips_hidden_inert_and_disabled_fieldset_controls() {
    // Chromium 143: b, a, last (a shadow root's button comes between a and
    // last there; shadow trees are not walked here).
    let (mut engine, id) = page_with(KEYS_PAGE);
    let mut order = Vec::new();
    for _ in 0..3 {
        tab(&mut engine, id, false);
        order.push(active(&mut engine, id));
    }
    assert_eq!(order, ["b", "a", "last"]);
}

#[test]
fn keys_go_to_a_tab_focused_button_and_enter_and_space_click_it() {
    // Chromium 143: Enter is `keydown click keyup`; Space clicks at its
    // keyup.
    let (mut engine, id) = page_with(KEYS_PAGE);
    tab(&mut engine, id, false);
    assert_eq!(active(&mut engine, id), "b");
    assert_eq!(js(&mut engine, id, "log.join(' ')"), "keydown:", "the first Tab's keydown: the body (no id)");
    js(&mut engine, id, "log.length = 0; 0");
    tab(&mut engine, id, false);
    assert_eq!(js(&mut engine, id, "log.join(' ')"), "keydown:b", "Tab's keydown at the focused button");
    tab(&mut engine, id, true);
    js(&mut engine, id, "log.length = 0; 0");
    assert!(engine.handle_text_key(id, 0x0D, "", false, false, false), "Enter is taken");
    assert_eq!(js(&mut engine, id, "log.join(' ')"), "keydown:b click:b");
    js(&mut engine, id, "log.length = 0; 0");
    assert!(engine.handle_text_key(id, 0, " ", false, false, false), "Space's keydown is taken: no scroll");
    assert_eq!(js(&mut engine, id, "log.join(' ')"), "keydown:b", "no click at the keydown");
    engine.handle_key_up(id, 0, " ", false, false, false);
    assert_eq!(js(&mut engine, id, "log.join(' ')"), "keydown:b keyup:b click:b", "the keyup clicks");
}

/// Chromium 143 (standards mode; `*:hover` is the quirks-mode hover quirk's
/// case, a doctype keeps it out): the universal selector with a state
/// pseudo-class matches as `:focus` alone does. `*:focus{outline:none}`
/// takes the UA ring away after a click, a Tab and script's `focus()`.
const STAR_PAGE: &str = "<!doctype html><html><head><style>body { margin: 0 }\
    *:focus { outline: none }\
    *:hover { background-color: rgb(255, 0, 0) }\
    #d { height: 40px }</style></head><body>\
    <input id=i><button id=b>B</button><div id=w><div id=d>x</div></div><div id=e style='height:40px'>e</div></body></html>";

#[test]
fn star_focus_outline_none_takes_the_ring_away() {
    let (mut engine, id) = page_with(STAR_PAGE);
    js(&mut engine, id, "$('b').focus(); 0");
    assert_eq!(css(&mut engine, id, "b", "outline-style"), "none", "script focus");
    assert!(!paints_the_ring(&engine, id), "script focus: no ring painted");
    click(&mut engine, id, "i");
    assert_eq!(active(&mut engine, id), "i");
    assert_eq!(css(&mut engine, id, "i", "outline-style"), "none", "a click");
    assert!(!paints_the_ring(&engine, id), "a click: no ring painted");
    tab(&mut engine, id, false);
    assert_eq!(active(&mut engine, id), "b");
    assert_eq!(css(&mut engine, id, "b", "outline-style"), "none", "Tab");
    assert!(!paints_the_ring(&engine, id), "Tab: no ring painted");
}

#[test]
fn star_hover_matches_the_hovered_chain() {
    let (mut engine, id) = page_with(STAR_PAGE);
    let (x, y) = centre(&mut engine, id, "d");
    engine.mouse_move_at_point(id, x, y);
    assert_eq!(css(&mut engine, id, "d", "background-color"), "rgb(255, 0, 0)", "the hovered element");
    assert_eq!(css(&mut engine, id, "w", "background-color"), "rgb(255, 0, 0)", "its parent");
    assert_eq!(css(&mut engine, id, "e", "background-color"), "rgba(0, 0, 0, 0)", "not its sibling");
}
