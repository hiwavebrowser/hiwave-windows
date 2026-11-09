//! CH2.E19: `:hover`, `:active`, `:focus-within` and `:checked ~` on the
//! author rules that colour inline svg. The svg paint pass
//! (`cache_svg_paint_styles`) matched every rule against the DOM's own
//! attributes, with no pointer, focus or checkedness state, so
//! `button:hover svg path { fill: red }` never turned the icon red: the
//! RustKit chrome's toolbar icons kept their resting colour under the
//! pointer.
//!
//! Oracle (Chromium 143.0.7499.4, headless), on the page below: every path
//! is `rgb(0, 0, 0)` at rest; `#b1:hover` makes p1 `rgb(255, 0, 0)` and
//! leaving reverts it; focusing `#i` makes p2 `rgb(0, 128, 0)` and a blur
//! reverts it; checking `#cb` makes p3 `rgb(0, 0, 255)`; pressing `#b2`
//! makes p4 `rgb(255, 165, 0)`.
//!
//! A hover that changes only svg paint takes the pointer restyle's short
//! path (no layout), and paints what a full relayout paints.

use super::*;
use rustkit_layout::DisplayCommand;

const PAGE: &str = concat!(
    "<!doctype html><style>body{margin:0}",
    "button{display:block;width:60px;height:40px;padding:0;border:0;margin:0}",
    "svg path{fill:rgb(0,0,0)}",
    "#b1:hover svg path{fill:rgb(255,0,0)}",
    "#w:focus-within svg path{fill:rgb(0,128,0)}",
    "#cb:checked ~ label svg path{fill:rgb(0,0,255)}",
    "#b2:active svg path{fill:rgb(255,165,0)}",
    "</style>",
    "<button id=b1><svg width=20 height=20><path d='M0 0H20V20H0Z'/></svg></button>",
    "<div id=w><input id=i><svg width=20 height=20><path d='M0 0H20V20H0Z'/></svg></div>",
    "<input type=checkbox id=cb><label for=cb><svg width=20 height=20><path d='M0 0H20V20H0Z'/></svg></label>",
    "<button id=b2><svg width=20 height=20><path d='M0 0H20V20H0Z'/></svg></button>",
);

/// Inside `#b1`, and below everything.
const ON_B1: (f32, f32) = (30.0, 20.0);
const NOWHERE: (f32, f32) = (390.0, 290.0);

const BLACK: (u8, u8, u8) = (0, 0, 0);
const RED: (u8, u8, u8) = (255, 0, 0);
const GREEN: (u8, u8, u8) = (0, 128, 0);
const BLUE: (u8, u8, u8) = (0, 0, 255);
const ORANGE: (u8, u8, u8) = (255, 165, 0);

fn page(html: &str) -> (Engine, EngineViewId) {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 400, height: 300 })
        .expect("view");
    engine.load_html(view, html).expect("load_html");
    (engine, view)
}

/// The fill colours the display list paints, in order, without the
/// page's own backgrounds: the svg paths' (each path a 20x20 square).
fn path_fills(engine: &Engine, view: EngineViewId) -> Vec<(u8, u8, u8)> {
    let list = engine.views[&view].display_list.as_ref().expect("a display list");
    let mut out = Vec::new();
    for c in &list.commands {
        let color = match c {
            DisplayCommand::FillPolygon { color, .. } => color,
            DisplayCommand::FillRect { color, rect } if rect.width == 20.0 && rect.height == 20.0 => color,
            DisplayCommand::SolidColor(color, rect) if rect.width == 20.0 && rect.height == 20.0 => color,
            _ => continue,
        };
        out.push((color.r, color.g, color.b));
    }
    out
}

fn skipped(engine: &Engine, view: EngineViewId) -> u32 {
    engine.relayout_stats(view).expect("view").layouts_skipped
}

fn list(engine: &Engine, view: EngineViewId) -> String {
    format!("{:?}", engine.views[&view].display_list.as_ref().expect("list").commands)
}

fn list_by_a_full_relayout(engine: &mut Engine, view: EngineViewId) -> String {
    engine.relayout(view).expect("relayout");
    list(engine, view)
}

#[test]
fn every_icon_is_black_at_rest() {
    let (engine, view) = page(PAGE);
    assert_eq!(path_fills(&engine, view), vec![BLACK; 4]);
}

/// `button:hover svg path`: red under the pointer, black again after.
#[test]
fn a_hover_rule_colours_the_icon_and_reverts() {
    let (mut engine, view) = page(PAGE);
    crate::hover_restyle_tests::hover(&mut engine, view, ON_B1);
    assert_eq!(path_fills(&engine, view), vec![RED, BLACK, BLACK, BLACK], "hovered");
    crate::hover_restyle_tests::hover(&mut engine, view, NOWHERE);
    assert_eq!(path_fills(&engine, view), vec![BLACK; 4], "left");
}

/// The hover that changes only the icon's colour is a paint-only restyle:
/// the layout is skipped (no miss to a full relayout), and the frame is
/// the one a full relayout paints.
#[test]
fn an_icon_hover_takes_the_short_restyle_path() {
    let (mut engine, view) = page(PAGE);
    let skip0 = skipped(&engine, view);
    crate::hover_restyle_tests::hover(&mut engine, view, ON_B1);
    assert_eq!(path_fills(&engine, view)[0], RED, "the hover coloured the icon");
    assert_eq!(skipped(&engine, view), skip0 + 1, "the hover skipped layout");
    let painted = list(&engine, view);
    assert_eq!(painted, list_by_a_full_relayout(&mut engine, view));
    let skip1 = skipped(&engine, view);
    crate::hover_restyle_tests::hover(&mut engine, view, NOWHERE);
    assert_eq!(skipped(&engine, view), skip1 + 1, "leaving skipped layout");
    let painted = list(&engine, view);
    assert_eq!(painted, list_by_a_full_relayout(&mut engine, view));
}

/// `:focus-within svg path`: green while the input has focus.
#[test]
fn a_focus_within_rule_colours_the_icon() {
    let (mut engine, view) = page(PAGE);
    engine.execute_script(view, "document.getElementById('i').focus()").unwrap();
    engine.relayout(view).expect("relayout");
    assert_eq!(path_fills(&engine, view), vec![BLACK, GREEN, BLACK, BLACK], "focused");
    engine.execute_script(view, "document.getElementById('i').blur()").unwrap();
    engine.relayout(view).expect("relayout");
    assert_eq!(path_fills(&engine, view), vec![BLACK; 4], "blurred");
}

/// `:checked ~ label svg path`: blue once the box is checked by script.
#[test]
fn a_checked_sibling_rule_colours_the_icon() {
    let (mut engine, view) = page(PAGE);
    engine.execute_script(view, "document.getElementById('cb').checked = true").unwrap();
    engine.relayout(view).expect("relayout");
    assert_eq!(path_fills(&engine, view), vec![BLACK, BLACK, BLUE, BLACK]);
}

/// `:active svg path`: orange while the button is pressed.
#[test]
fn an_active_rule_colours_the_icon_while_pressed() {
    let (mut engine, view) = page(PAGE);
    let at = {
        // #b2 is the last block: below the label's line.
        let layout = engine.views[&view].layout.clone().expect("layout");
        fn find(b: &LayoutBox, tag: &str, id: &str) -> Option<(f32, f32)> {
            if b.identity.as_ref().is_some_and(|i| i.tag == tag && i.selector.ends_with(id)) {
                return Some((b.dimensions.content.x + 5.0, b.dimensions.content.y + 5.0));
            }
            b.children.iter().find_map(|c| find(c, tag, id))
        }
        find(&layout, "button", "#b2").expect("the second button")
    };
    engine.mouse_move_at_point(view, at.0, at.1);
    engine.mouse_down_at_point(view, at.0, at.1);
    assert_eq!(path_fills(&engine, view), vec![BLACK, BLACK, BLACK, ORANGE], "pressed");
    engine.click_at_point(view, at.0, at.1);
}
