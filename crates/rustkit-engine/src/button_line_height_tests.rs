//! CB3: `button{font-size:16px;line-height:1;padding:0 4px;border:none}` is 22.8 px tall in RustKit
//! and 16 in Chrome (the settings page's blocklist remove button; 93 of the 128 geometry diffs at
//! 1280 are the shift below it). A button's content height is its `line-height`, so with no
//! vertical padding and no border the box is exactly the line box. Pinned from the CSS box model
//! (and the row's Chrome measurement of 16), so the pins do not depend on a font's `normal` height.
//! Reduced page: docs/diagnostics/2026-11-09/settings_button_line_height_1.html.

use super::*;

const PAGE: &str = "<html><head><style>body{margin:0;font:12.8px Arial}div{margin:8px}\
    .i{display:flex;align-items:center;gap:4px;padding:4px 8px;background:#ccc;width:max-content}\
    .r{background:none;border:none;padding:0 4px;font-size:16px;line-height:1}</style></head><body>\
    <div class=i id=item><span>example.com</span><button class=r id=rm>&times;</button></div>\
    <div><button class=r id=rmx>x</button></div>\
    <div><button id=lh1 style='font-size:16px;line-height:1'>x</button></div>\
    <div><button id=all style='font-size:16px;line-height:1;padding:0;border:none'>x</button></div>\
    <div><span id=sp style='display:inline-block;font-size:16px;line-height:1'>&times;</span></div>\
    <div><button id=lhp0 style='font-size:16px;line-height:1;padding:0'>x</button></div>\
    <div><button id=lhbn style='font-size:16px;line-height:1;border:none'>x</button></div>\
    <div><button id=lhpad style='font-size:16px;line-height:1;padding:4px 8px'>x</button></div>\
    <div><button id=lh20p0 style='font-size:16px;line-height:20px;padding:0'>x</button></div>\
    </body></html>";

fn heights() -> std::collections::HashMap<String, f64> {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 800, height: 400 })
        .expect("view");
    engine.load_html(view, PAGE).expect("load_html");
    let ids = ["item", "rm", "rmx", "lh1", "all", "sp", "lhp0", "lhbn", "lhpad", "lh20p0"];
    let expr = ids
        .iter()
        .map(|id| format!("document.getElementById('{id}').getBoundingClientRect().height"))
        .collect::<Vec<_>>()
        .join(" + ',' + ");
    let out = engine.execute_script(view, &format!("String({expr})")).unwrap();
    let text = out.strip_prefix("String(\"").and_then(|v| v.strip_suffix("\")")).unwrap_or(&out).to_string();
    ids.iter().map(|s| s.to_string()).zip(text.split(',').map(|n| n.parse::<f64>().unwrap_or(f64::NAN))).collect()
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 0.5
}

#[test]
fn a_button_with_line_height_1_no_vertical_padding_and_no_border_is_one_line_box_tall() {
    let h = heights();
    assert!(close(h["all"], 16.0), "padding:0, border:none, line-height:1 at 16px is 16 px, got {}", h["all"]);
    assert!(close(h["rm"], 16.0), "the settings remove button is 16 px, got {}", h["rm"]);
    assert!(close(h["rmx"], 16.0), "the same button alone is 16 px, got {}", h["rmx"]);
}

#[test]
fn a_buttons_height_is_three_independent_parts() {
    // CB3b: line (line-height) + vertical padding + vertical border, each the author's when
    // declared (zero and `none` included) and the UA's when not. Chrome for Testing 148.0.7778.216 at
    // 16px Arial, measured on Linux (Talos) AND on macOS (Atlas, 2026-11-10; identical tables):
    // line-height:1 only 22, padding:0 only 22, border:none only 20, padding:0 + border:none 18,
    // all three 16, line-height:1 + padding:0 20, line-height:1 + border:none 18, padding 4px 8px
    // 30. The explicit line-heights here keep `normal` (font-dependent) out of it, and the UA parts
    // are the named constants, so a change of the UA numbers changes one line.
    use rustkit_layout::{UA_BUTTON_BORDER_V as BORDER, UA_BUTTON_PADDING_V as PAD};
    let h = heights();
    let cases = [
        ("all", 16.0, "line-height:1, padding:0, border:none"),
        ("lh1", 16.0 + PAD + BORDER, "line-height:1 only: UA padding and border kept"),
        ("lhp0", 16.0 + BORDER, "line-height:1 + padding:0: UA border kept"),
        ("lhbn", 16.0 + PAD, "line-height:1 + border:none: UA padding kept"),
        ("lhpad", 16.0 + 8.0 + BORDER, "line-height:1 + padding:4px 8px: UA border kept"),
        ("lh20p0", 20.0 + BORDER, "line-height:20px + padding:0: UA border kept"),
    ];
    for (id, want, what) in cases {
        assert!(close(h[id], f64::from(want)), "{what}: {id} should be {want}, got {}", h[id]);
    }
}

#[test]
fn the_flex_row_around_the_remove_button_is_the_button_plus_its_padding() {
    // .i: 4px padding above and below a 16 px button (the span is shorter): 24 px, not 30.8.
    let h = heights();
    assert!(close(h["item"], 24.0), "the item row is 24 px, got {}", h["item"]);
}

#[test]
fn an_inline_block_with_line_height_1_is_the_control() {
    let h = heights();
    assert!(close(h["sp"], 16.0), "a line-height:1 inline-block at 16px is 16 px, got {}", h["sp"]);
}

/// Batch 57 found #1037 growing the form-controls buttons from 19 to 24 px: the case's reset sets
/// `html { line-height: 1.5 }`, and the button INHERITED it (1.5 x 13.33 = 20 px line, + 0 padding
/// + 4 border = 24). Chrome's UA `font: -webkit-small-control` on form controls resets the line
/// to `normal`, so only a line-height declared on the button itself counts: with the page's
/// `* { padding: 0; box-sizing: border-box }` a bare button is 15 + 0 + 4 = 19 (Chrome for Testing
/// 148, websuite/micro/form-controls baseline, buttons and input type=submit/button/reset).
#[test]
fn a_buttons_line_is_normal_unless_the_button_itself_declares_one() {
    const PAGE: &str = "<html><head><style>\
        html { font-size: 16px; line-height: 1.5; }\
        * { margin: 0; padding: 0; box-sizing: border-box; }\
        #own { line-height: 1; }\
        </style></head><body>\
        <div><button id=b>Short</button></div>\
        <div><input id=s type=submit value=Submit></div>\
        <div><input id=t type=button value=Button></div>\
        <div><input id=r type=reset value=Reset></div>\
        <div><button id=own style='font-size:16px'>x</button></div>\
        </body></html>";
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 800, height: 400 })
        .expect("view");
    engine.load_html(view, PAGE).expect("load_html");
    let ids = ["b", "s", "t", "r", "own"];
    let expr = ids
        .iter()
        .map(|id| format!("document.getElementById('{id}').getBoundingClientRect().height"))
        .collect::<Vec<_>>()
        .join(" + ',' + ");
    let out = engine.execute_script(view, &format!("String({expr})")).unwrap();
    let text = out.strip_prefix("String(\"").and_then(|v| v.strip_suffix("\")")).unwrap_or(&out).to_string();
    let h: Vec<f64> = text.split(',').map(|n| n.parse().unwrap_or(f64::NAN)).collect();
    for (i, id) in ["b", "s", "t", "r"].iter().enumerate() {
        assert!(close(h[i], 19.0), "#{id}: a bare button under html{{line-height:1.5}} + *{{padding:0}} is 19 px, got {}", h[i]);
    }
    // The button's OWN line-height still counts: 16 (1 x 16px) + 0 padding + 4 border.
    assert!(close(h[4], 20.0), "#own: line-height:1 on the button itself is 16 + 0 + 4 = 20, got {}", h[4]);
}

/// The campaign case itself (websuite/micro/form-controls, its shared reset inlined): every button
/// and `input type=submit/button` is the 19 px Chrome for Testing 148 measured
/// (baselines/chrome-148/micro/form-controls/layout-rects.json: 35.875/93.66/167.03 wide buttons,
/// two inputs, all 19 tall at y 475), and the `.test4` button with an explicit 200 x 50 stays 50
/// under `box-sizing: border-box` (a declared height and the three-part sum do not double-count).
#[test]
fn the_form_controls_case_buttons_are_the_height_chrome_measured() {
    const CASE: &str = include_str!("../../../websuite/micro/form-controls/index.html");
    const RESET: &str = include_str!("../../../websuite/common/parity-reset.css");
    let link = "<link rel=\"stylesheet\" href=\"../../common/parity-reset.css\">";
    assert!(CASE.contains(link), "the case still links the shared reset");
    let page = CASE.replace(link, &format!("<style>{RESET}</style>"));
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 1280, height: 800 })
        .expect("view");
    engine.load_html(view, &page).expect("load_html");
    let out = engine
        .execute_script(
            view,
            "String(Array.prototype.map.call(document.querySelectorAll(\
             'button, input[type=submit], input[type=button], input[type=reset]'),\
             function (e) { return Math.round(e.getBoundingClientRect().height); }).join(','))",
        )
        .unwrap();
    let text = out.strip_prefix("String(\"").and_then(|v| v.strip_suffix("\")")).unwrap_or(&out);
    let heights: Vec<f64> = text.split(',').filter_map(|n| n.parse().ok()).collect();
    assert!(heights.len() >= 7, "the case has at least 7 buttons: {text}");
    for (i, h) in heights.iter().enumerate() {
        let want = if *h == 50.0 { 50.0 } else { 19.0 };
        assert!(close(*h, want), "button {i} is {h} px, Chrome measured 19 (or the explicit 50): {text}");
    }
    assert_eq!(heights.iter().filter(|h| **h == 50.0).count(), 1, "exactly the .test4 button is 50: {text}");
}
