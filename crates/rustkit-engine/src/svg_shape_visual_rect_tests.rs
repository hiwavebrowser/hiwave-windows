//! SVG-SHAPE-TRANSFORM: an inline `<svg>`'s shapes export the rect Chrome measures under a CSS
//! transform. `getBoundingClientRect()` on a shape is post-transform; the layout export gives
//! every transformed CSS box a `visual_border_box` but gave the shapes under it only their
//! untransformed rect. The inspector's expanded DOM toggle (`.dom-toggle.expanded svg { transform:
//! rotate(90deg) }`, path `M9 6l6 6-6 6` in a 24 viewBox at 10px) is 28 of the 31 geometry diffs
//! left on the Windows chrome board's inspector rows. Chrome for Testing 148.0.7778.216 (win64),
//! docs/diagnostics/2026-11-21/svg_shape_transform_cft148.json
//! (tools/parity_oracle/svg_shape_transform_log.mjs, the same markup).

use super::*;

fn path_rect(svg_style: &str, wrapper_style: &str) -> (f64, f64, f64, f64) {
    let page = format!(
        "<!doctype html><style>body{{margin:0}}</style>\
         <div id=w style=\"position:absolute;left:20px;top:30px;width:16px;height:16px;{wrapper_style}\">\
         <svg id=s viewBox=\"0 0 24 24\" style=\"display:block;margin:3px;width:10px;height:10px;{svg_style}\">\
         <path id=p d=\"M9 6l6 6-6 6\"/></svg></div>"
    );
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 400, height: 200 })
        .expect("view");
    engine.load_html(view, &page).expect("load_html");
    let dir = std::env::temp_dir().join(format!("svg_shape_visual_rect_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join(format!("{}.json", svg_style.len() * 31 + wrapper_style.len()));
    engine.export_layout_json(view, file.to_str().unwrap()).expect("export");
    let json: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    let _ = std::fs::remove_file(&file);
    fn find<'a>(v: &'a serde_json::Value) -> Option<&'a serde_json::Value> {
        if v.get("type").and_then(|t| t.as_str()) == Some("svg_shape")
            && v.get("tag").and_then(|t| t.as_str()) == Some("path")
        {
            return Some(v);
        }
        v.get("children")?.as_array()?.iter().find_map(find)
    }
    let shape = find(&json["root"]).expect("the path is exported as an svg_shape");
    // The oracle reads the visual rect where there is one, else `border_box`.
    let r = shape.get("visual_border_box").unwrap_or(&shape["border_box"]);
    (r["x"].as_f64().unwrap(), r["y"].as_f64().unwrap(), r["width"].as_f64().unwrap(), r["height"].as_f64().unwrap())
}

fn assert_rect(label: &str, got: (f64, f64, f64, f64), chrome: (f64, f64, f64, f64)) {
    let close = |a: f64, b: f64| (a - b).abs() < 0.05;
    assert!(
        close(got.0, chrome.0) && close(got.1, chrome.1) && close(got.2, chrome.2) && close(got.3, chrome.3),
        "{label}: Chrome's path rect is {chrome:?}, got {got:?}"
    );
}

#[test]
fn a_shape_in_a_rotated_svg_exports_its_rotated_rect() {
    assert_rect("rotate(90deg) on the svg", path_rect("transform:rotate(90deg)", ""), (25.5, 36.75, 5.0, 2.5));
}

#[test]
fn a_shape_under_a_rotated_ancestor_exports_its_rotated_rect() {
    assert_rect("rotate(90deg) on the wrapper", path_rect("", "transform:rotate(90deg)"), (25.5, 36.75, 5.0, 2.5));
}

#[test]
fn a_shape_in_a_translated_svg_moves_with_it() {
    assert_rect("translate(5px, 7px) on the svg", path_rect("transform:translate(5px,7px)", ""), (31.75, 42.5, 2.5, 5.0));
}

#[test]
fn an_untransformed_shape_keeps_its_rect() {
    assert_rect("no transform", path_rect("", ""), (26.75, 35.5, 2.5, 5.0));
}

// #1415 R1 nit 1: the cases above all pivot on the svg's centre, so they do not pin a
// transform-origin elsewhere or the order the wrapper's and the svg's transforms compose in.

#[test]
fn a_shape_under_an_ancestor_rotated_about_its_corner_exports_its_rotated_rect() {
    assert_rect(
        "rotate(90deg) on the wrapper about its top left",
        path_rect("", "transform:rotate(90deg);transform-origin:0 0"),
        (9.5, 36.75, 5.0, 2.5),
    );
}

#[test]
fn a_rotated_svg_in_a_translated_ancestor_moves_and_turns() {
    assert_rect(
        "translate(5px, 7px) on the wrapper, rotate(90deg) on the svg",
        path_rect("transform:rotate(90deg)", "transform:translate(5px,7px)"),
        (30.5, 43.75, 5.0, 2.5),
    );
}

#[test]
fn the_svgs_transform_applies_inside_its_ancestors() {
    // The svg's translate(5px, 0) is taken in the wrapper's rotated frame: down, not right.
    assert_rect(
        "rotate(90deg) on the wrapper about its top left, translate(5px, 0) on the svg",
        path_rect("transform:translate(5px,0)", "transform:rotate(90deg);transform-origin:0 0"),
        (9.5, 41.75, 5.0, 2.5),
    );
}
