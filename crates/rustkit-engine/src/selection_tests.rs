//! CH2.E2: the selection model, mouse selection and the painted highlight.
//!
//! Page views (no GPU texture): the display list is read as built.

use super::*;
use rustkit_layout::{DisplayCommand, SELECTION_COLOR};

fn page(html: &str) -> (Engine, EngineViewId) {
    let mut engine = Engine::new_page(EngineConfig::default(), None, Arc::new(AtomicU64::new(0))).expect("engine");
    let view = engine.create_page_view(Bounds::new(0, 0, 600, 400)).expect("page view");
    engine.load_html(view, html).expect("load");
    (engine, view)
}

fn read(engine: &mut Engine, view: EngineViewId, expr: &str) -> String {
    let value = engine.execute_script(view, &format!("String({expr})")).unwrap();
    value
        .strip_prefix("String(\"")
        .and_then(|v| v.strip_suffix("\")"))
        .unwrap_or(&value)
        .to_string()
}

fn commands(engine: &Engine, view: EngineViewId) -> Vec<DisplayCommand> {
    engine.views[&view].display_list.as_ref().expect("display list").commands.clone()
}

/// The selection rects painted, as `(x, y, width, height)`.
fn highlight(engine: &Engine, view: EngineViewId) -> Vec<(f32, f32, f32, f32)> {
    commands(engine, view)
        .iter()
        .filter_map(|c| match c {
            DisplayCommand::SolidColor(color, r) if *color == SELECTION_COLOR => Some((r.x, r.y, r.width, r.height)),
            _ => None,
        })
        .collect()
}

/// Where character boundary `index` of the painted run containing `needle`
/// is, and the line's middle y.
fn char_x(engine: &Engine, view: EngineViewId, needle: &str, index: usize) -> (f32, f32) {
    for c in commands(engine, view) {
        if let DisplayCommand::Text { text, x, y, advances, font_size, font_family, .. } = c {
            if text.contains(needle) {
                let adv = advances.unwrap_or_else(|| {
                    text.chars()
                        .map(|ch| rustkit_layout::measure_text_advanced(&ch.to_string(), &font_family, font_size, Default::default(), Default::default()).width)
                        .collect()
                });
                return (x + adv[..index].iter().sum::<f32>(), y + font_size / 2.0);
            }
        }
    }
    panic!("no text run with {needle:?}");
}

const PARAGRAPH: &str = "<html><body style='margin:0'><p id=p style='margin:0;font-size:20px'>alpha beta gamma</p>\
    <p id=q style='margin:0;font-size:20px'>second <b>line</b> here</p></body></html>";

#[test]
fn dragging_across_two_words_selects_and_paints_them() {
    let (mut engine, view) = page(PARAGRAPH);
    assert!(highlight(&engine, view).is_empty(), "nothing selected yet");
    let (x0, y) = char_x(&engine, view, "alpha", 0);
    let (x10, _) = char_x(&engine, view, "alpha", 10);
    engine.mouse_move_at_point(view, x0 + 1.0, y);
    engine.mouse_down_at_point(view, x0 + 1.0, y);
    engine.mouse_move_at_point(view, (x0 + x10) / 2.0, y);
    engine.mouse_move_at_point(view, x10 + 1.0, y);
    engine.click_at_point(view, x10 + 1.0, y);
    assert_eq!(read(&mut engine, view, "getSelection().toString()"), "alpha beta");
    assert_eq!(read(&mut engine, view, "getSelection().anchorOffset + ',' + getSelection().focusOffset"), "0,10");
    let rects = highlight(&engine, view);
    assert_eq!(rects.len(), 1, "{rects:?}");
    let (rx, _, rw, rh) = rects[0];
    assert!((rx - x0).abs() < 0.5 && (rx + rw - x10).abs() < 0.5, "{rects:?} vs {x0}..{x10}");
    assert!(rh > 0.0);
    // The highlight is behind the glyphs.
    let list = commands(&engine, view);
    let fill = list.iter().position(|c| matches!(c, DisplayCommand::SolidColor(color, _) if *color == SELECTION_COLOR));
    let text = list.iter().position(|c| matches!(c, DisplayCommand::Text { text, .. } if text.contains("alpha")));
    assert!(fill < text, "selection before text");
}

#[test]
fn a_drag_across_paragraphs_paints_each_run() {
    let (mut engine, view) = page(PARAGRAPH);
    let (x6, y) = char_x(&engine, view, "alpha", 6);
    let (x2, y2) = char_x(&engine, view, "second", 2);
    engine.mouse_down_at_point(view, x6 + 0.5, y);
    engine.mouse_move_at_point(view, x2 + 0.5, y2);
    engine.click_at_point(view, x2 + 0.5, y2);
    assert_eq!(read(&mut engine, view, "getSelection().toString()"), "beta gammase");
    assert_eq!(highlight(&engine, view).len(), 2);
}

#[test]
fn double_click_selects_a_word_and_triple_click_the_paragraph() {
    let (mut engine, view) = page(PARAGRAPH);
    read(&mut engine, view, "window.__dbl = 0, document.addEventListener('dblclick', function () { window.__dbl++; })");
    let (x8, y) = char_x(&engine, view, "alpha", 8);
    engine.mouse_down_with(view, x8, y, 1, false);
    engine.click_at_point(view, x8, y);
    engine.mouse_down_with(view, x8, y, 2, false);
    engine.click_at_point(view, x8, y);
    assert_eq!(read(&mut engine, view, "getSelection().toString()"), "beta");
    assert_eq!(read(&mut engine, view, "window.__dbl"), "1");
    assert_eq!(highlight(&engine, view).len(), 1);
    engine.mouse_down_with(view, x8, y, 3, false);
    engine.click_at_point(view, x8, y);
    assert_eq!(read(&mut engine, view, "getSelection().toString()"), "alpha beta gamma");
    assert_eq!(read(&mut engine, view, "getSelection().anchorNode.id"), "p");
}

#[test]
fn shift_click_extends_the_selection() {
    let (mut engine, view) = page(PARAGRAPH);
    let (x6, y) = char_x(&engine, view, "alpha", 6);
    let (x16, _) = char_x(&engine, view, "alpha", 16);
    engine.mouse_down_at_point(view, x6 + 0.5, y);
    engine.click_at_point(view, x6 + 0.5, y);
    assert_eq!(read(&mut engine, view, "getSelection().isCollapsed"), "true");
    engine.mouse_down_with(view, x16 + 1.0, y, 1, true);
    engine.click_at_point(view, x16 + 1.0, y);
    assert_eq!(read(&mut engine, view, "getSelection().toString()"), "beta gamma");
}

#[test]
fn add_range_paints_and_fires_selectionchange() {
    let (mut engine, view) = page(PARAGRAPH);
    read(&mut engine, view, "window.__changes = 0, document.addEventListener('selectionchange', function () { window.__changes++; })");
    read(
        &mut engine,
        view,
        "(function () { var r = document.createRange(); r.selectNodeContents(document.querySelector('b')); getSelection().addRange(r); })()",
    );
    assert_eq!(read(&mut engine, view, "window.__changes"), "1");
    assert_eq!(read(&mut engine, view, "getSelection().toString()"), "line");
    let rects = highlight(&engine, view);
    assert_eq!(rects.len(), 1, "{rects:?}");
    let (bx, _) = char_x(&engine, view, "line", 0);
    assert!((rects[0].0 - bx).abs() < 0.5, "{rects:?} at {bx}");
    read(&mut engine, view, "getSelection().removeAllRanges()");
    assert!(highlight(&engine, view).is_empty());
    assert_eq!(read(&mut engine, view, "window.__changes"), "2");
}

fn text_input(engine: &Engine, view: EngineViewId) -> (String, Option<(usize, usize)>) {
    commands(engine, view)
        .into_iter()
        .find_map(|c| match c {
            DisplayCommand::TextInput { value, selection, kind: rustkit_layout::TextControlKind::Text, .. } => Some((value, selection)),
            _ => None,
        })
        .expect("a text input")
}

const FIELD: &str = "<html><body style='margin:0'><input id=f value='hello world' style='font-size:16px;width:300px'></body></html>";

#[test]
fn select_all_in_an_input_paints_one_rect_spanning_the_text() {
    let (mut engine, view) = page(FIELD);
    engine.mouse_down_at_point(view, 20.0, 10.0);
    engine.click_at_point(view, 20.0, 10.0);
    assert_eq!(text_input(&engine, view).1, None, "a caret, nothing selected");
    engine.handle_text_key(view, 0x41, "a", true, false, false);
    engine.relayout(view).unwrap();
    let (value, selection) = text_input(&engine, view);
    assert_eq!(selection, Some((0, 11)));
    let measure = |t: &str| t.chars().count() as f32 * 8.0;
    let rects = rustkit_layout::forms::calculate_selection_rects(&value, 0, 11, 2.0, 2.0, 18.0, &measure);
    assert_eq!(rects.len(), 1);
    assert_eq!((rects[0].x, rects[0].width), (2.0, 88.0), "the whole value");
}

#[test]
fn dragging_in_an_input_selects_part_of_its_value() {
    let (mut engine, view) = page(FIELD);
    let content_x = {
        let tree = engine.views[&view].layout.clone().unwrap();
        let raw = engine.views[&view].document.as_ref().unwrap().get_element_by_id("f").unwrap().id.raw();
        box_of_node(&tree, raw).unwrap().dimensions.content.x
    };
    engine.mouse_down_at_point(view, content_x + 0.5, 10.0);
    engine.mouse_move_at_point(view, content_x + 150.0, 10.0);
    let (_, selection) = text_input(&engine, view);
    let (s, e) = selection.expect("a selection");
    assert_eq!(s, 0);
    assert!(e > 0 && e <= 11, "{e}");
    engine.click_at_point(view, content_x + 150.0, 10.0);
    // A double-click selects the word under the pointer.
    engine.mouse_down_with(view, content_x + 1.0, 10.0, 2, false);
    engine.click_at_point(view, content_x + 1.0, 10.0);
    assert_eq!(text_input(&engine, view).1, Some((0, 5)));
}

const EDITABLE: &str = "<html><body style='margin:0'><div id=e contenteditable style='font-size:20px;min-height:30px'>hi</div>\
    <p style='font-size:20px'>outside</p></body></html>";

fn caret(engine: &Engine, view: EngineViewId) -> Option<f32> {
    commands(engine, view).iter().find_map(|c| match c {
        DisplayCommand::Caret { x, .. } => Some(*x),
        _ => None,
    })
}

#[test]
fn typing_in_a_contenteditable_div_edits_it_and_fires_input() {
    let (mut engine, view) = page(EDITABLE);
    read(&mut engine, view, "window.__inputs = [], document.getElementById('e').addEventListener('input', function (e) { window.__inputs.push(e.inputType); })");
    let (x2, y) = char_x(&engine, view, "hi", 2);
    engine.mouse_down_at_point(view, x2 + 2.0, y);
    engine.click_at_point(view, x2 + 2.0, y);
    assert_eq!(read(&mut engine, view, "document.activeElement.id"), "e", "the host takes the focus");
    assert!(engine.focused_node(view).is_some());
    let at = caret(&engine, view).expect("a caret in the host");
    assert!((at - x2).abs() < 0.5, "caret after 'hi': {at} vs {x2}");
    assert!(engine.handle_text_key(view, 0x41, "!", false, false, false));
    assert_eq!(read(&mut engine, view, "document.getElementById('e').textContent"), "hi!");
    let (x3, _) = char_x(&engine, view, "hi!", 3);
    let at = caret(&engine, view).expect("the caret moved");
    assert!((at - x3).abs() < 0.5, "caret after 'hi!': {at} vs {x3}");
    assert!(engine.handle_text_key(view, 0x08, "", false, false, false));
    assert_eq!(read(&mut engine, view, "document.getElementById('e').textContent"), "hi");
    engine.submit_focused_form(view);
    assert!(engine.handle_text_key(view, 0x0D, "", false, false, false));
    assert_eq!(read(&mut engine, view, "document.getElementById('e').innerHTML"), "hi<br><br>");
    assert_eq!(read(&mut engine, view, "window.__inputs.join()"), "insertText,deleteContentBackward,insertParagraph");
    // The page outside the host is not editable.
    let (ox, oy) = char_x(&engine, view, "outside", 3);
    engine.mouse_down_at_point(view, ox, oy);
    engine.click_at_point(view, ox, oy);
    assert!(engine.focused_node(view).is_none());
    assert!(caret(&engine, view).is_none());
    engine.handle_text_key(view, 0x41, "x", false, false, false);
    assert_eq!(read(&mut engine, view, "document.body.textContent.indexOf('x') < 0"), "true");
}

#[test]
fn text_control_offsets_are_characters_on_a_non_ascii_value() {
    let (mut engine, view) = page("<html><body style='margin:0'><input id=f value='café latte' style='font-size:16px;width:300px'></body></html>");
    let (content_x, style) = {
        let tree = engine.views[&view].layout.clone().unwrap();
        let raw = engine.views[&view].document.as_ref().unwrap().get_element_by_id("f").unwrap().id.raw();
        let b = box_of_node(&tree, raw).unwrap();
        (b.dimensions.content.x, b.style.clone())
    };
    let width = |t: &str| -> f32 {
        t.chars()
            .map(|c| rustkit_layout::measure_text_advanced(&c.to_string(), &style.font_family, 16.0, style.font_weight, style.font_style).width)
            .sum()
    };
    // A click just after "é", then a key: no panic, the text goes there.
    let x = content_x + width("café") + 0.5;
    engine.mouse_down_at_point(view, x, 10.0);
    engine.click_at_point(view, x, 10.0);
    engine.handle_text_key(view, 0x31, "!", false, false, false);
    assert_eq!(engine.edit_value_in(view, engine.focused_node(view).unwrap().raw()).unwrap().0, "café! latte");
    // A double-click on "café" selects its 4 characters; select-all, all 11.
    engine.mouse_down_with(view, content_x + 1.0, 10.0, 2, false);
    engine.click_at_point(view, content_x + 1.0, 10.0);
    assert_eq!(text_input(&engine, view).1, Some((0, 4)));
    engine.handle_text_key(view, 0x41, "a", true, false, false);
    engine.relayout(view).unwrap();
    assert_eq!(text_input(&engine, view).1, Some((0, 11)));
}

#[test]
fn byte_and_char_offsets_convert_on_boundaries() {
    use selection::{byte_of_char, char_of_byte};
    assert_eq!(byte_of_char("café x", 4), 5);
    assert_eq!(byte_of_char("café", 9), 5);
    assert_eq!(char_of_byte("café x", 5), 4);
    assert_eq!(char_of_byte("café", 4), 3, "inside é rounds down");
    assert_eq!(char_of_byte("café", 99), 4);
}

// R1 HOLD @ be0dd001: every selection write into the byte-indexed
// `TextEditState` and every read feeding `calculate_selection_rects` must
// convert between characters and bytes. These run on non-ASCII values.

const WIDE: &str = "<html><body style='margin:0'><input id=f value='héllo wörld' style='font-size:16px;width:300px'></body></html>";
const EMOJI: &str = "<html><body style='margin:0'><input id=f value='a😀b c' style='font-size:16px;width:300px'></body></html>";

/// The input's content-box x and a measure of text in its font, summed per
/// character the way `control_char_at` hit-tests.
fn field_metrics(engine: &Engine, view: EngineViewId) -> (f32, impl Fn(&str) -> f32) {
    let tree = engine.views[&view].layout.clone().unwrap();
    let raw = engine.views[&view].document.as_ref().unwrap().get_element_by_id("f").unwrap().id.raw();
    let b = box_of_node(&tree, raw).unwrap();
    let style = b.style.clone();
    let measure = move |t: &str| -> f32 {
        t.chars()
            .map(|c| rustkit_layout::measure_text_advanced(&c.to_string(), &style.font_family, 16.0, style.font_weight, style.font_style).width)
            .sum()
    };
    (b.dimensions.content.x, measure)
}

fn field_value(engine: &Engine, view: EngineViewId) -> String {
    engine.edit_value_in(view, engine.focused_node(view).expect("focused").raw()).expect("edit state").0
}

#[test]
fn non_ascii_click_after_e_acute_then_typing_inserts_there() {
    let (mut engine, view) = page(WIDE);
    let (cx, w) = field_metrics(&engine, view);
    let x = cx + w("hé") + 0.5;
    engine.mouse_down_at_point(view, x, 10.0);
    engine.click_at_point(view, x, 10.0);
    assert!(engine.handle_text_key(view, 0x58, "x", false, false, false));
    assert_eq!(field_value(&engine, view), "héxllo wörld");
    // The caret followed the insert: one more key lands after the 'x'.
    assert!(engine.handle_text_key(view, 0x59, "y", false, false, false));
    assert_eq!(field_value(&engine, view), "héxyllo wörld");
}

#[test]
fn non_ascii_click_after_an_emoji_then_typing_inserts_there() {
    let (mut engine, view) = page(EMOJI);
    let (cx, w) = field_metrics(&engine, view);
    let x = cx + w("a😀") + 0.5;
    engine.mouse_down_at_point(view, x, 10.0);
    engine.click_at_point(view, x, 10.0);
    assert!(engine.handle_text_key(view, 0x58, "x", false, false, false));
    assert_eq!(field_value(&engine, view), "a😀xb c");
    // A double-click on "xb", past the emoji, selects that word (characters
    // 2..4, bytes 5..7); typing replaces exactly it.
    engine.mouse_down_with(view, cx + w("a😀x") + 1.0, 10.0, 2, false);
    engine.click_at_point(view, cx + w("a😀x") + 1.0, 10.0);
    assert_eq!(text_input(&engine, view).1, Some((2, 4)), "in characters");
    assert!(engine.handle_text_key(view, 0x5A, "z", false, false, false));
    assert_eq!(field_value(&engine, view), "a😀z c");
}

#[test]
fn non_ascii_drag_across_a_multibyte_char_paints_at_the_right_x() {
    let (mut engine, view) = page(WIDE);
    let (cx, w) = field_metrics(&engine, view);
    // From after 'h' to after 'ö': characters 1..8 ("éllo wö").
    let (from, to) = (cx + w("h") + 0.5, cx + w("héllo wö") + 0.5);
    engine.mouse_down_at_point(view, from, 10.0);
    engine.mouse_move_at_point(view, to, 10.0);
    engine.click_at_point(view, to, 10.0);
    let (value, selection) = text_input(&engine, view);
    assert_eq!(value, "héllo wörld");
    assert_eq!(selection, Some((1, 8)), "characters, not bytes");
    // The painter measures from the value's start: the highlight begins
    // after 'h' and covers exactly "éllo wö".
    let rects = rustkit_layout::forms::calculate_selection_rects(&value, 1, 8, cx, 0.0, 18.0, &w);
    assert_eq!(rects.len(), 1, "{rects:?}");
    assert!((rects[0].x - (cx + w("h"))).abs() < 0.01, "{rects:?}");
    assert!((rects[0].width - w("éllo wö")).abs() < 0.01, "{rects:?}");
    // The edit state holds the same range: typing replaces exactly it.
    assert!(engine.handle_text_key(view, 0x58, "x", false, false, false));
    assert_eq!(field_value(&engine, view), "hxrld");
}

#[test]
fn non_ascii_shift_click_extends_the_field_selection() {
    let (mut engine, view) = page(WIDE);
    let (cx, w) = field_metrics(&engine, view);
    let (a, b) = (cx + w("hé") + 0.5, cx + w("héllo wö") + 0.5);
    engine.mouse_down_at_point(view, a, 10.0);
    engine.click_at_point(view, a, 10.0);
    engine.mouse_down_with(view, b, 10.0, 1, true);
    engine.click_at_point(view, b, 10.0);
    assert_eq!(text_input(&engine, view).1, Some((2, 8)));
    // A shift-click before the range extends it from its far end.
    engine.mouse_down_with(view, cx + w("h") + 0.5, 10.0, 1, true);
    engine.click_at_point(view, cx + w("h") + 0.5, 10.0);
    assert_eq!(text_input(&engine, view).1, Some((1, 8)), "after 'h' to after 'ö'");
    assert!(engine.handle_text_key(view, 0x58, "x", false, false, false));
    assert_eq!(field_value(&engine, view), "hxrld");
}

#[test]
fn non_ascii_triple_click_selects_the_whole_value() {
    let (mut engine, view) = page(EMOJI);
    let (cx, w) = field_metrics(&engine, view);
    engine.mouse_down_with(view, cx + w("a") + 1.0, 10.0, 3, false);
    engine.click_at_point(view, cx + w("a") + 1.0, 10.0);
    assert_eq!(text_input(&engine, view).1, Some((0, 5)), "a😀b c is 5 characters");
    assert!(engine.handle_text_key(view, 0x58, "x", false, false, false));
    assert_eq!(field_value(&engine, view), "x");
}

#[test]
fn non_ascii_get_selection_to_string_returns_the_dragged_substring() {
    let (mut engine, view) =
        page("<html><body style='margin:0'><p id=p style='margin:0;font-size:20px'>héllo wörld 😀 end</p></body></html>");
    // Characters 1..13: "éllo wörld 😀" (the emoji is two UTF-16 units).
    let (x1, y) = char_x(&engine, view, "héllo", 1);
    let (x13, _) = char_x(&engine, view, "héllo", 13);
    engine.mouse_down_at_point(view, x1 + 0.5, y);
    engine.mouse_move_at_point(view, x13 - 0.5, y);
    engine.click_at_point(view, x13 - 0.5, y);
    assert_eq!(read(&mut engine, view, "getSelection().toString()"), "éllo wörld 😀");
    assert_eq!(read(&mut engine, view, "[getSelection().anchorOffset, getSelection().focusOffset].join()"), "1,14");
    let rects = highlight(&engine, view);
    assert_eq!(rects.len(), 1, "{rects:?}");
    assert!((rects[0].0 - x1).abs() < 0.5, "{rects:?} from {x1}");
}
