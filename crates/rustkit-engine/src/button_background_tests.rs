//! CH2.B3: an author's `background: transparent` on a button survives to
//! paint.
//!
//! chrome.html's workspace expanders and tab lock buttons are
//! `border: none; background: transparent`, and Chromium 143 computes
//! `background-color: rgba(0, 0, 0, 0)` for all of them (chrome board,
//! default state). The painter used to swap any alpha-0 button background
//! for ButtonFace (239, 239, 239), so every one painted as a white block.
//! ButtonFace is the UA default, so it belongs in the UA style, where an
//! author rule can override it (the #83 fix for fields, extended to buttons).
//!
//! Page views (no GPU texture): the display list is read as built.

use super::*;
use rustkit_css::Color;
use rustkit_layout::DisplayCommand;

const BUTTON_FACE: Color = Color { r: 239, g: 239, b: 239, a: 1.0 };

fn button_backgrounds(html: &str) -> Vec<(String, Color)> {
    let mut engine =
        Engine::new_page(EngineConfig::default(), None, Arc::new(AtomicU64::new(0))).expect("engine");
    let view = engine.create_page_view(Bounds::new(0, 0, 600, 400)).expect("page view");
    engine.load_html(view, html).expect("load");
    engine.views[&view]
        .display_list
        .as_ref()
        .expect("display list")
        .commands
        .iter()
        .filter_map(|c| match c {
            DisplayCommand::Button { label, background_color, .. } => Some((label.clone(), *background_color)),
            _ => None,
        })
        .collect()
}

fn background_of(list: &[(String, Color)], label: &str) -> Color {
    list.iter()
        .find(|(l, _)| l.trim() == label)
        .unwrap_or_else(|| panic!("no button labelled {label:?} in {list:?}"))
        .1
}

#[test]
fn a_transparent_button_paints_no_background() {
    let list = button_backgrounds(
        r#"<body style="background:#1e293b">
        <button style="border:none;background:transparent;padding:4px">v</button>
        <button style="border:none;background:transparent;display:flex;width:28px;height:28px">L</button>
        <input type="button" value="I" style="border:none;background-color:transparent">
        </body>"#,
    );
    for label in ["v", "L", "I"] {
        let bg = background_of(&list, label);
        assert_eq!(bg.a, 0.0, "button {label:?}: Chromium paints none, got {bg:?}");
    }
}

#[test]
fn an_unstyled_button_keeps_buttonface() {
    let list = button_backgrounds(
        r#"<body><button>plain</button><input type="submit" value="Go"><input type="reset" value="R"></body>"#,
    );
    for label in ["plain", "Go", "R"] {
        assert_eq!(background_of(&list, label), BUTTON_FACE, "button {label:?}");
    }
}

/// The second half of the same rows: chrome.html's expander is
/// `<button>\n<32 spaces>▾\n<28 spaces></button>`, and the label kept every
/// space, so the button measured ~113 px and its `flex: 1` sibling (the
/// workspace name) got 0. Chromium 143: the label collapses to "v" and both
/// buttons below are 14 px wide (`padding: 4px; border: none`, 12px Arial).
#[test]
fn a_button_label_collapses_its_whitespace() {
    let mut engine =
        Engine::new_page(EngineConfig::default(), None, Arc::new(AtomicU64::new(0))).expect("engine");
    let view = engine.create_page_view(Bounds::new(0, 0, 600, 400)).expect("page view");
    engine
        .load_html(
            view,
            "<body style=\"font:12px Arial\">\
             <button style=\"border:none;padding:4px\">\n                    v\n            </button>\
             <button style=\"border:none;padding:4px\">v</button></body>",
        )
        .expect("load");
    let buttons: Vec<(String, f32)> = engine.views[&view]
        .display_list
        .as_ref()
        .expect("display list")
        .commands
        .iter()
        .filter_map(|c| match c {
            DisplayCommand::Button { label, rect, .. } => Some((label.clone(), rect.width)),
            _ => None,
        })
        .collect();
    assert_eq!(buttons.len(), 2, "{buttons:?}");
    assert_eq!(buttons[0].0, "v", "label keeps no whitespace: {buttons:?}");
    assert!((buttons[0].1 - buttons[1].1).abs() < 0.5, "same width as a bare label: {buttons:?}");
}

#[test]
fn an_authored_button_colour_is_kept() {
    let list = button_backgrounds(r#"<body><button style="background:rgb(1,2,3)">c</button></body>"#);
    assert_eq!(background_of(&list, "c"), Color { r: 1, g: 2, b: 3, a: 1.0 });
}
