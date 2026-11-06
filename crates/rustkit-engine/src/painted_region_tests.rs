//! CH3.A3b: the boxes a frame paints over the page column, read from the
//! frame's own display list (`painted_region::painted_boxes`).
//!
//! The RustKit chrome is the whole window, stacked over the page, and is
//! shown only inside its region: the window less the page column (the
//! hole), plus whatever the chrome painted over the hole. Before this, the
//! "whatever" was a list of popup selectors the chrome document reported
//! by script; a popup missing from the list (the chrome's own context
//! menu, #917) was clipped at the hole's edge.
//!
//! Page views (no GPU texture): the display list is read as built.

use super::*;
use crate::painted_region::painted_boxes;
use rustkit_css::Color;
use rustkit_layout::DisplayCommand;

/// A chrome-shaped document at 1280 x 800: a 220 px sidebar, a 104 px
/// toolbar, the body's background over the whole window, the main column
/// over everything right of the sidebar, and `extra` on top.
fn chrome_like(extra: &str) -> String {
    format!(
        r#"<!DOCTYPE html><html><head><style>
        html, body {{ height: 100%; margin: 0; overflow: hidden; }}
        body {{ background: #1e293b; display: flex; font: 13px sans-serif; color: #eee; }}
        .sidebar {{ width: 220px; background: #0f172a; }}
        .main {{ flex: 1; display: flex; flex-direction: column; background: #111827; }}
        .toolbar {{ height: 104px; background: #334155; }}
        .menu {{ position: absolute; z-index: 9999; background: #222; border: 1px solid #555;
                 border-radius: 10px; box-shadow: 0 8px 32px rgba(0,0,0,0.5); padding: 6px; }}
        .menu button {{ display: block; width: 180px; height: 30px; border: none;
                        background: transparent; color: #ddd; text-align: left; }}
        </style></head><body>
        <div class="sidebar">side</div>
        <div class="main"><div class="toolbar">toolbar</div></div>
        {extra}
        </body></html>"#
    )
}

/// The page column of `chrome_like`.
const HOLE: Rect = Rect { x: 220.0, y: 104.0, width: 1060.0, height: 696.0 };

fn list_of(html: &str) -> Arc<DisplayList> {
    let mut engine =
        Engine::new_page(EngineConfig::default(), None, Arc::new(AtomicU64::new(0))).expect("engine");
    let view = engine.create_page_view(Bounds::new(0, 0, 1280, 800)).expect("page view");
    engine.load_html(view, html).expect("load");
    Arc::clone(engine.views[&view].display_list.as_ref().expect("display list"))
}

fn near(a: f32, b: f32) -> bool {
    (a - b).abs() <= 1.5
}

fn rect_near(r: &Rect, x: f32, y: f32, w: f32, h: f32) -> bool {
    near(r.x, x) && near(r.y, y) && near(r.width, w) && near(r.height, h)
}

#[test]
fn a_chrome_at_rest_paints_nothing_over_the_page_column() {
    // The body's background (propagated to the canvas) and the main
    // column both cover the page column: they are the window's backdrop,
    // not a surface over the page.
    let boxes = painted_boxes(&list_of(&chrome_like("")), (0.0, 0.0), HOLE);
    assert!(boxes.is_empty(), "{boxes:?}");
}

#[test]
fn an_open_menu_reaching_over_the_page_is_one_box_clipped_to_the_column() {
    // A context menu opened at the toolbar (600, 70), four rows tall: it
    // reaches from y 70 to 70 + 2*6 + 2 + 4*30 = 204, over the page from
    // y 104. Its rows, their text and its border are inside its own box.
    let menu = r#"<div class="menu" style="left:600px;top:70px">
        <button>Back</button><button>Forward</button><button>Reload</button><button>Copy</button></div>"#;
    let boxes = painted_boxes(&list_of(&chrome_like(menu)), (0.0, 0.0), HOLE);
    assert_eq!(boxes.len(), 1, "{boxes:?}");
    assert!(rect_near(&boxes[0], 600.0, 104.0, 194.0, 100.0), "{boxes:?}");
}

#[test]
fn a_menu_found_by_no_selector_is_found_by_its_paint() {
    // The reporter knew `.context-menu`, `.downloads-panel`, ...; any
    // other class (here a tooltip-like box over the page) was clipped.
    let tip = r#"<div style="position:absolute;left:900px;top:300px;width:120px;height:24px;background:#000">tip</div>"#;
    let boxes = painted_boxes(&list_of(&chrome_like(tip)), (0.0, 0.0), HOLE);
    assert_eq!(boxes.len(), 1, "{boxes:?}");
    assert!(rect_near(&boxes[0], 900.0, 300.0, 120.0, 24.0), "{boxes:?}");
}

#[test]
fn a_hidden_menu_paints_nothing() {
    for style in ["visibility:hidden", "opacity:0", "display:none"] {
        let menu = format!(
            r#"<div class="menu" style="left:600px;top:70px;{style}"><button>Back</button><button>Forward</button></div>"#
        );
        let boxes = painted_boxes(&list_of(&chrome_like(&menu)), (0.0, 0.0), HOLE);
        assert!(boxes.is_empty(), "{style}: {boxes:?}");
    }
}

#[test]
fn a_menu_inside_the_toolbar_strip_is_not_an_overlay() {
    // The strip is in the region already; only what reaches the page
    // column is reported.
    let menu = r#"<div class="menu" style="left:600px;top:10px"><button>Back</button></div>"#;
    let boxes = painted_boxes(&list_of(&chrome_like(menu)), (0.0, 0.0), HOLE);
    assert!(boxes.is_empty(), "{boxes:?}");
}

fn fill(x: f32, y: f32, w: f32, h: f32) -> DisplayCommand {
    DisplayCommand::SolidColor(Color::new(10, 20, 30, 1.0), Rect::new(x, y, w, h))
}

fn list(commands: Vec<DisplayCommand>) -> DisplayList {
    let mut list = DisplayList::new();
    list.commands = commands;
    list
}

#[test]
fn a_box_is_found_where_its_transform_puts_it() {
    let l = list(vec![
        DisplayCommand::PushTransform { matrix: [1.0, 0.0, 0.0, 1.0, 0.0, 300.0], origin: (0.0, 0.0) },
        fill(400.0, 0.0, 100.0, 50.0),
        DisplayCommand::PopTransform,
        fill(400.0, 0.0, 100.0, 50.0),
    ]);
    let boxes = painted_boxes(&l, (0.0, 0.0), HOLE);
    assert_eq!(boxes.len(), 1, "{boxes:?}");
    assert!(rect_near(&boxes[0], 400.0, 300.0, 100.0, 50.0), "{boxes:?}");
}

#[test]
fn a_clip_cuts_the_box_and_an_empty_clip_hides_it() {
    let l = list(vec![
        DisplayCommand::PushClip(Rect::new(0.0, 0.0, 1280.0, 250.0)),
        fill(400.0, 200.0, 100.0, 200.0),
        DisplayCommand::PushClip(Rect::new(0.0, 0.0, 0.0, 0.0)),
        fill(700.0, 200.0, 100.0, 20.0),
        DisplayCommand::PopClip,
        DisplayCommand::PopClip,
    ]);
    let boxes = painted_boxes(&l, (0.0, 0.0), HOLE);
    assert_eq!(boxes.len(), 1, "{boxes:?}");
    assert!(rect_near(&boxes[0], 400.0, 200.0, 100.0, 50.0), "{boxes:?}");
}

#[test]
fn a_faded_out_group_and_a_transparent_fill_paint_nothing() {
    let l = list(vec![
        DisplayCommand::PushOpacity(0.0),
        fill(400.0, 200.0, 100.0, 100.0),
        DisplayCommand::PopOpacity,
        DisplayCommand::SolidColor(Color::new(0, 0, 0, 0.0), Rect::new(600.0, 200.0, 100.0, 100.0)),
    ]);
    assert!(painted_boxes(&l, (0.0, 0.0), HOLE).is_empty());
}

#[test]
fn a_shadow_alone_is_not_a_surface() {
    // The chrome's surface is opaque: outside a box, its frame holds the
    // body's colour, so revealing a shadow's reach would show a block of
    // that colour, not a shadow over the page (disclosed, A3-composition).
    let l = list(vec![DisplayCommand::BoxShadow {
        offset_x: 0.0,
        offset_y: 8.0,
        blur_radius: 32.0,
        spread_radius: 0.0,
        color: Color::new(0, 0, 0, 0.5),
        rect: Rect::new(400.0, 200.0, 100.0, 100.0),
        border_radius: Default::default(),
        inset: false,
    }]);
    assert!(painted_boxes(&l, (0.0, 0.0), HOLE).is_empty());
}

#[test]
fn scrolled_content_moves_and_fixed_content_stays() {
    let l = list(vec![
        fill(400.0, 500.0, 100.0, 50.0),
        DisplayCommand::PushViewportFixed,
        fill(700.0, 500.0, 100.0, 50.0),
        DisplayCommand::PopViewportFixed,
    ]);
    let mut boxes = painted_boxes(&l, (0.0, 200.0), HOLE);
    boxes.sort_by(|a, b| a.x.total_cmp(&b.x));
    assert_eq!(boxes.len(), 2, "{boxes:?}");
    assert!(rect_near(&boxes[0], 400.0, 300.0, 100.0, 50.0), "{boxes:?}");
    assert!(rect_near(&boxes[1], 700.0, 500.0, 100.0, 50.0), "{boxes:?}");
}

#[test]
fn a_box_that_covers_most_of_the_column_is_a_backdrop() {
    // The rule that drops the body, the main column and a page-sized
    // placeholder, even when the app's hole and the document's columns
    // disagree by a few pixels.
    let l = list(vec![fill(221.5, 0.0, 1058.5, 800.0), fill(400.0, 200.0, 50.0, 50.0)]);
    let boxes = painted_boxes(&l, (0.0, 0.0), HOLE);
    assert_eq!(boxes.len(), 1, "{boxes:?}");
    assert!(rect_near(&boxes[0], 400.0, 200.0, 50.0, 50.0), "{boxes:?}");
}

#[test]
fn twenty_thousand_commands_take_one_linear_pass() {
    // The cost per new frame: one pass over the list, bounds only, no
    // pixels. Kept as a smoke check against an accidental quadratic.
    let mut commands = Vec::with_capacity(20_000);
    for i in 0..20_000 {
        let y = (i % 600) as f32;
        commands.push(fill(0.0, y, 200.0, 1.0)); // the sidebar: outside the column
    }
    commands.push(fill(400.0, 200.0, 50.0, 50.0));
    let l = list(commands);
    let started = std::time::Instant::now();
    let boxes = painted_boxes(&l, (0.0, 0.0), HOLE);
    let took = started.elapsed();
    assert_eq!(boxes.len(), 1, "{boxes:?}");
    assert!(took < std::time::Duration::from_millis(250), "{took:?}");
}
