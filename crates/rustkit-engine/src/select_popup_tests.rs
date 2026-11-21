//! The `<select>` drop-down's model (CH2.E6): placement, highlight, keys,
//! type-to-select, scrolling and the commands that paint it. No engine:
//! these run everywhere. The engine wiring is in `select_popup_engine_tests`.

use super::*;

fn opt(label: &str) -> SelectOption {
    SelectOption {
        node: 0,
        label: label.to_string(),
        disabled: false,
        default_selected: false,
        group: None,
    }
}

fn opts(labels: &[&str]) -> Vec<SelectOption> {
    labels.iter().enumerate().map(|(i, l)| SelectOption { node: i + 10, ..opt(l) }).collect()
}

fn font() -> PopupFont {
    PopupFont { size: 13.0, family: "sans-serif".into(), weight: 400, color: Color::new(0, 0, 0, 1.0), background: Color::new(255, 255, 255, 1.0) }
}

fn popup(options: Vec<SelectOption>, selected: Option<usize>, anchor: Rect) -> SelectPopup {
    SelectPopup::open(1, options, selected, anchor, Rect::new(0.0, 0.0, 800.0, 600.0), font(), |s| {
        s.chars().count() as f32 * 7.0
    })
}

fn texts(cmds: &[DisplayCommand]) -> Vec<String> {
    cmds.iter()
        .filter_map(|c| match c {
            DisplayCommand::Text { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn the_popup_opens_under_the_control_with_a_row_per_option() {
    let anchor = Rect::new(10.0, 20.0, 120.0, 22.0);
    let p = popup(opts(&["One", "Two", "Three"]), Some(1), anchor);
    assert_eq!(p.rect.x, 10.0);
    assert_eq!(p.rect.y, 42.0, "the top edge sits on the control's bottom edge");
    assert!(p.rect.width >= 120.0);
    assert_eq!(p.visible_rows, 3);
    assert_eq!(p.highlighted, Some(1), "the selected option is highlighted at open");
    let cmds = p.commands();
    assert_eq!(texts(&cmds), ["One", "Two", "Three"]);
    assert!(matches!(cmds.first(), Some(DisplayCommand::PushStackingContext { z_index: i32::MAX, .. })));
    assert!(matches!(cmds.last(), Some(DisplayCommand::PopStackingContext)));
}

#[test]
fn the_popup_opens_above_the_control_when_there_is_no_room_below() {
    let anchor = Rect::new(10.0, 570.0, 120.0, 22.0);
    let p = popup(opts(&["One", "Two", "Three"]), Some(0), anchor);
    assert_eq!(p.rect.y + p.rect.height, 570.0, "the bottom edge sits on the control's top edge");
}

#[test]
fn group_labels_get_rows_and_disabled_options_are_greyed() {
    let mut o = opts(&["Plain", "A", "B"]);
    o[1].group = Some("Letters".into());
    o[2].group = Some("Letters".into());
    o[2].disabled = true;
    let p = popup(o, Some(0), Rect::new(0.0, 0.0, 100.0, 20.0));
    let cmds = p.commands();
    assert_eq!(texts(&cmds), ["Plain", "Letters", "A", "B"]);
    let color_of = |label: &str| {
        cmds.iter().find_map(|c| match c {
            DisplayCommand::Text { text, color, .. } if text == label => Some(*color),
            _ => None,
        })
    };
    assert_eq!(color_of("B"), Some(DISABLED_TEXT));
    assert_eq!(color_of("A"), Some(Color::new(0, 0, 0, 1.0)));
    assert_eq!(color_of("Plain"), Some(HIGHLIGHT_TEXT), "the selected option is highlighted");
    // A click on the group label's row is on no option.
    let label_y = p.rect.y + 1.0 + p.row_height * 1.5;
    assert_eq!(p.option_at(p.rect.x + 5.0, label_y), None);
    let b_y = p.rect.y + 1.0 + p.row_height * 3.5;
    assert_eq!(p.option_at(p.rect.x + 5.0, b_y), Some(2));
}

#[test]
fn hover_moves_the_highlight_but_not_onto_a_disabled_option() {
    let mut o = opts(&["One", "Two", "Three"]);
    o[2].disabled = true;
    let mut p = popup(o, Some(0), Rect::new(0.0, 0.0, 100.0, 20.0));
    let row_y = |r: usize| p.rect.y + 1.0 + p.row_height * (r as f32 + 0.5);
    let (y1, y2) = (row_y(1), row_y(2));
    assert!(p.hover(5.0, y1));
    assert_eq!(p.highlighted, Some(1));
    assert!(!p.hover(5.0, y2));
    assert_eq!(p.highlighted, Some(1));
}

#[test]
fn arrow_keys_skip_disabled_options_and_stop_at_the_ends() {
    let mut o = opts(&["One", "Two", "Three", "Four"]);
    o[1].disabled = true;
    let mut p = popup(o.clone(), Some(0), Rect::new(0.0, 0.0, 100.0, 20.0));
    assert!(p.step(1));
    assert_eq!(p.highlighted, Some(2));
    assert!(p.step(1));
    assert_eq!(p.highlighted, Some(3));
    assert!(!p.step(1), "the last option stays highlighted");
    assert!(p.step(-1));
    assert!(p.step(-1));
    assert_eq!(p.highlighted, Some(0));
    // Closed-select helpers.
    assert_eq!(next_enabled(&o, Some(0), 1), Some(2));
    assert_eq!(next_enabled(&o, Some(3), 1), None);
    assert_eq!(edge_enabled(&o, true), Some(3));
}

#[test]
fn typing_a_letter_jumps_to_the_next_option_starting_with_it() {
    let o = opts(&["Apple", "Banana", "blueberry", "Cherry", "Bean"]);
    assert_eq!(typeahead(&o, Some(0), "b"), Some(1));
    assert_eq!(typeahead(&o, Some(1), "b"), Some(2));
    assert_eq!(typeahead(&o, Some(4), "b"), Some(1), "wraps around");
    assert_eq!(typeahead(&o, Some(0), "z"), None);
    let mut p = popup(o, Some(0), Rect::new(0.0, 0.0, 100.0, 20.0));
    let now = std::time::Instant::now();
    assert!(p.type_char('c', now));
    assert_eq!(p.highlighted, Some(3));
    // Typed quickly, a prefix.
    let mut p2 = popup(opts(&["Bar", "Bean", "Bead"]), Some(0), Rect::new(0.0, 0.0, 100.0, 20.0));
    p2.type_char('b', now);
    p2.type_char('e', now);
    p2.type_char('a', now);
    p2.type_char('d', now);
    assert_eq!(p2.highlighted, Some(2));
}

#[test]
fn many_options_scroll_and_the_highlight_stays_in_view() {
    let labels: Vec<String> = (0..50).map(|i| format!("Item {i}")).collect();
    let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
    let mut p = popup(opts(&refs), Some(40), Rect::new(0.0, 0.0, 100.0, 20.0));
    assert_eq!(p.visible_rows, MAX_VISIBLE_ROWS);
    assert!(p.first_row <= 40 && 40 < p.first_row + p.visible_rows, "the selected option shows at open");
    let shown = texts(&p.commands());
    assert_eq!(shown.len(), MAX_VISIBLE_ROWS);
    assert!(shown.contains(&"Item 40".to_string()));
    assert!(p.scroll_rows(-100));
    assert_eq!(p.first_row, 0);
    assert_eq!(texts(&p.commands())[0], "Item 0");
    assert!(p.step(1));
    assert_eq!(p.highlighted, Some(41));
    assert!(p.first_row <= 41 && 41 < p.first_row + p.visible_rows);
}

#[test]
fn the_displayed_option_follows_the_selectedness_rule() {
    let mut o = opts(&["One", "Two", "Three"]);
    o[0].disabled = true;
    assert_eq!(displayed_index(&o, |_| false), Some(1), "the first enabled one");
    assert_eq!(displayed_index(&o, |x| x.label != "One"), Some(2), "the last selected one");
}

// R1 on #899, nit 6: a `label` attribute is shown with its whitespace
// collapsed, as text labels are.
#[test]
fn label_attributes_are_collapsed_like_text() {
    let doc = rustkit_dom::Document::parse_html(
        "<select id=s><option label='  Two \n words '>x</option><option> a\n b </option></select>",
    )
    .unwrap();
    let s = doc.get_element_by_id("s").unwrap();
    let labels: Vec<String> = options_of(&s).into_iter().map(|o| o.label).collect();
    assert_eq!(labels, ["Two words", "a b"]);
}
