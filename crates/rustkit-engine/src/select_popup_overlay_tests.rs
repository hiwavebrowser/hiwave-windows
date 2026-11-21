//! CH2.E6b: a `<select>`'s drop-down in the RustKit chrome is part of the
//! chrome frame's display list, so the chrome's region (CH3.A3b,
//! `painted_boxes`) and its transparent surface (CH3.A3c) cover it where
//! it reaches past the sidebar over the page column.
//!
//! The frame the app presents (and derives the region from) is the
//! `FrameCommit`, so these read the commit's list, not only the view's.

use super::*;
use crate::painted_region::painted_boxes;
use rustkit_layout::DisplayCommand;

/// A chrome-shaped document at 1280 x 800: a 220 px left sidebar with
/// `overflow: hidden` (chrome.html's), a 104 px toolbar, the main column,
/// and a right sidebar 180 px wide. A select in each sidebar whose options
/// are wider than the sidebar.
const CHROME: &str = r#"<!DOCTYPE html><html><head><style>
html, body { height: 100%; margin: 0; overflow: hidden; }
body { background: #1e293b; display: flex; font: 13px sans-serif; color: #eee; }
.sidebar { width: 220px; flex-shrink: 0; background: #0f172a; overflow: hidden; position: relative; }
.main { flex: 1; display: flex; flex-direction: column; background: #111827; }
.toolbar { height: 104px; background: #334155; }
.right { width: 180px; flex-shrink: 0; background: #0f172a; overflow: hidden; position: relative; }
select { display: block; margin: 120px 12px 0; width: calc(100% - 24px); height: 32px; font-size: 13px; }
</style></head><body>
<aside class="sidebar"><select id="mode">
  <option value="essentials">Essentials: full automation with undo, every workspace</option>
  <option value="balanced">Balanced: suggestions you confirm before anything moves</option>
  <option value="zen" selected>Zen: nothing happens unless you ask for it yourself</option>
</select></aside>
<div class="main"><div class="toolbar">toolbar</div></div>
<aside class="right"><select id="edge">
  <option>Right edge option one with a long label for width</option>
  <option>Right edge option two with a long label for width</option>
</select></aside>
<script>
  window.log = [];
  document.getElementById('mode').addEventListener('change', function (e) { log.push('change:' + e.target.value); });
</script>
</body></html>"#;

/// The page column of `CHROME`.
const HOLE: Rect = Rect { x: 220.0, y: 104.0, width: 880.0, height: 696.0 };
/// The left sidebar, the Mode select's container.
const SIDEBAR: Rect = Rect { x: 0.0, y: 0.0, width: 220.0, height: 800.0 };

fn chrome() -> Option<(Engine, EngineViewId)> {
    let mut engine = match Engine::new(EngineConfig::default()) {
        Ok(engine) => engine,
        Err(e) => {
            eprintln!("Skipping test: no engine ({e:?})");
            return None;
        }
    };
    let id = engine.create_headless_view(Bounds::new(0, 0, 1280, 800)).expect("headless view");
    engine.load_html(id, CHROME).expect("load_html");
    Some((engine, id))
}

/// The list of the frame the app would present now.
fn committed_list(engine: &Engine, id: EngineViewId) -> Arc<DisplayList> {
    let commit = match engine.presenter.as_ref() {
        Some(p) => p.latest(id).cloned(),
        None => engine.page_commits.get(&id).cloned(),
    };
    commit
        .and_then(|c| c.list)
        .or_else(|| engine.views[&id].display_list.clone())
        .expect("a frame")
}

fn popup_rect(engine: &Engine, id: EngineViewId) -> Rect {
    engine.views[&id].lay.select_popup.borrow().as_ref().expect("open popup").rect
}

fn open_at(engine: &mut Engine, id: EngineViewId, x: f32, y: f32) {
    engine.mouse_down_at_point(id, x, y);
    engine.click_at_point(id, x, y);
    assert!(engine.select_popup_open(id), "a click on the select opens its popup");
}

fn overlaps(a: &Rect, b: &Rect) -> Rect {
    let x0 = a.x.max(b.x);
    let y0 = a.y.max(b.y);
    let x1 = (a.x + a.width).min(b.x + b.width);
    let y1 = (a.y + a.height).min(b.y + b.height);
    Rect::new(x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0))
}

fn same(a: &Rect, b: &Rect) -> bool {
    (a.x, a.y, a.width, a.height) == (b.x, b.y, b.width, b.height)
}

fn covers(boxes: &[Rect], r: &Rect) -> bool {
    boxes.iter().any(|b| {
        b.x <= r.x + 0.5 && b.y <= r.y + 0.5 && b.x + b.width >= r.x + r.width - 0.5 && b.y + b.height >= r.y + r.height - 0.5
    })
}

/// The popup's commands are the last of the list, at the top of the
/// stacking order, nothing after them but state pops: an overlay item
/// that the frame's other content cannot paint over.
fn popup_is_the_last_item(list: &DisplayList, popup: Rect) {
    let start = list
        .commands
        .iter()
        .rposition(|c| matches!(c, DisplayCommand::PushStackingContext { z_index: i32::MAX, rect } if same(rect, &popup)))
        .expect("the popup's stacking context is in the frame's list");
    let tail = &list.commands[start..];
    assert!(
        tail.iter().any(|c| matches!(c, DisplayCommand::FillRect { rect, .. } if same(rect, &popup))),
        "the popup's box is filled inside it"
    );
    // Nothing that paints follows the popup's own commands.
    let end = start
        + tail
            .iter()
            .position(|c| matches!(c, DisplayCommand::PopStackingContext))
            .expect("the popup's context closes");
    for c in &list.commands[end + 1..] {
        assert!(
            matches!(
                c,
                DisplayCommand::PopClip
                    | DisplayCommand::PopTransform
                    | DisplayCommand::PopOpacity
                    | DisplayCommand::PopFilter
                    | DisplayCommand::PopStackingContext
                    | DisplayCommand::PopViewportFixed
                    | DisplayCommand::PopWindowSticky
            ),
            "a command paints after the popup: {c:?}"
        );
    }
    // And it is drawn outside every clip the page left open: a clip open at
    // its start would cut it to the sidebar.
    let mut depth = 0i32;
    for c in &list.commands[..start] {
        match c {
            DisplayCommand::PushClip(_) | DisplayCommand::PushClipRounded { .. } => depth += 1,
            DisplayCommand::PopClip => depth -= 1,
            _ => {}
        }
    }
    assert_eq!(depth, 0, "no clip of the page is open around the popup");
}

#[test]
fn the_mode_select_popup_reaches_past_the_sidebar_and_is_in_the_chrome_region() {
    let Some((mut engine, id)) = chrome() else { return };
    // The select: 12..208 x 120..152.
    open_at(&mut engine, id, 100.0, 136.0);
    let popup = popup_rect(&engine, id);
    assert!(
        popup.x + popup.width > SIDEBAR.x + SIDEBAR.width + 40.0,
        "the popup is as wide as its options, past the sidebar into the page column: {popup:?}"
    );
    assert!(popup.y >= 151.0 && popup.y <= 153.0, "it hangs under the control: {popup:?}");
    let list = committed_list(&engine, id);
    popup_is_the_last_item(&list, popup);
    let over = overlaps(&popup, &HOLE);
    assert!(over.width > 40.0, "part of it is over the page column: {over:?}");
    let boxes = painted_boxes(&list, (0.0, 0.0), HOLE);
    assert!(covers(&boxes, &over), "the region holds the popup's part over the page {over:?}: {boxes:?}");
}

#[test]
fn a_select_at_the_right_edge_opens_leftward_over_the_page_column_and_is_in_the_region() {
    let Some((mut engine, id)) = chrome() else { return };
    // The right sidebar's select: 1112..1268 x 120..152.
    open_at(&mut engine, id, 1190.0, 136.0);
    let popup = popup_rect(&engine, id);
    assert!(popup.x + popup.width <= 1280.0 + 0.5, "kept inside the window: {popup:?}");
    assert!(popup.x < HOLE.x + HOLE.width - 40.0, "it reaches left over the page column: {popup:?}");
    let list = committed_list(&engine, id);
    popup_is_the_last_item(&list, popup);
    let over = overlaps(&popup, &HOLE);
    let boxes = painted_boxes(&list, (0.0, 0.0), HOLE);
    assert!(covers(&boxes, &over), "the region holds {over:?}: {boxes:?}");
}

#[test]
fn closing_the_popup_takes_it_out_of_the_region() {
    let Some((mut engine, id)) = chrome() else { return };
    open_at(&mut engine, id, 100.0, 136.0);
    assert!(engine.close_select_popup(id));
    let boxes = painted_boxes(&committed_list(&engine, id), (0.0, 0.0), HOLE);
    assert!(boxes.is_empty(), "nothing over the page once it closed: {boxes:?}");
}

/// The frame the app presents now.
fn committed(engine: &Engine, id: EngineViewId) -> FrameCommit {
    match engine.presenter.as_ref() {
        Some(p) => p.latest(id).cloned(),
        None => engine.page_commits.get(&id).cloned(),
    }
    .expect("a frame")
}

#[test]
fn the_frame_says_a_drop_down_is_open_so_the_chrome_takes_every_press() {
    // A press outside an open drop-down only closes it (Chromium on macOS:
    // the native menu's tracking takes the press). The chrome over the page
    // is hit only in its region, so the frame must say the popup is open
    // for the app to make the whole view hit while it is.
    let Some((mut engine, id)) = chrome() else { return };
    assert!(!committed(&engine, id).modal_popup, "at rest");
    open_at(&mut engine, id, 100.0, 136.0);
    assert!(committed(&engine, id).modal_popup, "open");
    // A press on the page column reaches the chrome now: it closes the
    // popup, and the click does nothing else.
    engine.mouse_down_at_point(id, 700.0, 500.0);
    engine.click_at_point(id, 700.0, 500.0);
    assert!(!engine.select_popup_open(id), "the press outside closed it");
    assert!(!committed(&engine, id).modal_popup, "closed");
    let log = engine.execute_script(id, "log.join(' ')").expect("script");
    assert!(!log.contains("change"), "no pick: {log}");
}

/// The rgb of a colour, for comparing.
fn rgb(c: rustkit_css::Color) -> (u8, u8, u8) {
    (c.r, c.g, c.b)
}

#[test]
fn the_popup_takes_the_selects_background_so_white_option_text_shows() {
    // chrome.html's Mode select: `background: var(--accent); color: white`.
    // Chromium's drawn popup (the Windows/Linux look #899 follows) paints
    // the list in the select's background colour and its text in the
    // select's colour; a white list made the white labels invisible (only
    // the emoji showed, CH2.B3's second witness).
    let page = r#"<!DOCTYPE html><html><body style="margin:0">
<select id="m" style="position:absolute;left:12px;top:48px;width:195px;height:31px;font-size:13px;
  background:#06b6d4;color:white;border:none;appearance:none">
  <option>Essentials</option><option>Balanced</option><option selected>Zen</option>
</select></body></html>"#;
    let mut engine = match Engine::new(EngineConfig::default()) {
        Ok(engine) => engine,
        Err(e) => {
            eprintln!("Skipping test: no engine ({e:?})");
            return;
        }
    };
    let id = engine.create_headless_view(Bounds::new(0, 0, 800, 600)).expect("headless view");
    engine.load_html(id, page).expect("load_html");
    open_at(&mut engine, id, 100.0, 60.0);
    let popup = popup_rect(&engine, id);
    let list = committed_list(&engine, id);
    let fill = list
        .commands
        .iter()
        .rev()
        .find_map(|c| match c {
            DisplayCommand::FillRect { rect, color } if same(rect, &popup) => Some(*color),
            _ => None,
        })
        .expect("the popup's fill");
    assert_eq!(rgb(fill), (6, 182, 212), "the list is the select's background");
    let essentials = list
        .commands
        .iter()
        .rev()
        .find_map(|c| match c {
            DisplayCommand::Text { text, color, .. } if text == "Essentials" => Some(*color),
            _ => None,
        })
        .expect("the row's text");
    assert_eq!(rgb(essentials), (255, 255, 255), "the text is the select's colour");
    assert_ne!(rgb(essentials), rgb(fill), "and shows on the list");
}

#[test]
fn a_select_without_a_background_keeps_the_white_list() {
    let Some((mut engine, id)) = chrome() else { return };
    open_at(&mut engine, id, 100.0, 136.0);
    let popup = popup_rect(&engine, id);
    let list = committed_list(&engine, id);
    let fill = list
        .commands
        .iter()
        .rev()
        .find_map(|c| match c {
            DisplayCommand::FillRect { rect, color } if same(rect, &popup) => Some(*color),
            _ => None,
        })
        .expect("the popup's fill");
    // The UA select's background is Field (white) in RustKit and Chromium.
    assert_eq!(rgb(fill), (255, 255, 255));
}
