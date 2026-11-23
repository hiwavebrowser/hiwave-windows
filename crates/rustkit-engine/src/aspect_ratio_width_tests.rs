//! A block with `aspect-ratio`, an auto width and a definite height takes its
//! width from the height (css-sizing-4 §5.1), as in Chrome.
//!
//! RustKit filled the containing block instead: `height:100px; aspect-ratio:1`
//! was 600 wide in a 600px parent, where Chrome makes it 100. Found through
//! the empty-block change (a container query under such a box was measured
//! at 600 and fired where Chrome's does not).
//!
//! Numbers: Chrome for Testing 148.0.7778.216 (linux64), `getBoundingClientRect`,
//! all inside a 600px-wide parent.

use super::*;
use rustkit_layout::{Dimensions, Rect};

fn by_id<'a>(b: &'a LayoutBox, id: &str) -> Option<&'a LayoutBox> {
    if b.identity
        .as_ref()
        .is_some_and(|i| i.selector == format!("#{id}"))
    {
        return Some(b);
    }
    b.children.iter().find_map(|c| by_id(c, id))
}

/// (width, height) of each `#id`'s border box.
fn sizes(body: &str, ids: &[&str]) -> Vec<(String, f32, f32)> {
    let e = Engine::new(EngineConfig::default()).expect("engine");
    let html = format!(
        r#"<!doctype html><html><body style="margin:0;font:16px/20px sans-serif"><div style="width:600px">{body}</div></body></html>"#
    );
    let d = Document::parse_html(&html).expect("parse");
    let mut root = e.build_layout_from_document(&d, &[]);
    root.layout_with_collapse(
        &Dimensions {
            content: Rect::new(0.0, 0.0, 600.0, 0.0),
            ..Default::default()
        },
        &mut rustkit_layout::MarginCollapseContext::new(),
        &mut rustkit_layout::FloatContext::new(),
    );
    ids.iter()
        .map(|id| {
            let b = by_id(&root, id).unwrap_or_else(|| panic!("#{id} has no box"));
            let r = b.dimensions.border_box();
            (id.to_string(), r.width, r.height)
        })
        .collect()
}

#[test]
fn an_auto_width_follows_a_definite_height_across_the_ratio() {
    let got = sizes(
        r#"<div id="plain" style="height:100px;aspect-ratio:1">x</div>
<div id="cb" style="height:100px;aspect-ratio:2;padding:10px">x</div>
<div id="bb" style="height:100px;aspect-ratio:2;padding:10px;box-sizing:border-box">x</div>
<div id="maxw" style="height:100px;aspect-ratio:2;max-width:150px">x</div>
<div id="minw" style="height:100px;aspect-ratio:1;min-width:300px">x</div>
<div id="wide" style="height:100px;aspect-ratio:10">x</div>
<div id="ratioauto" style="height:100px;aspect-ratio:auto 1">x</div>"#,
        &["plain", "cb", "bb", "maxw", "minw", "wide", "ratioauto"],
    );
    let want = [
        ("plain", 100.0, 100.0),
        // content-box: the ratio is the content box's (200x100), plus 20 padding.
        ("cb", 220.0, 120.0),
        // border-box: the ratio is the border box's.
        ("bb", 200.0, 100.0),
        ("maxw", 150.0, 100.0),
        ("minw", 300.0, 100.0),
        // Not clamped to the containing block.
        ("wide", 1000.0, 100.0),
        ("ratioauto", 100.0, 100.0),
    ];
    let want: Vec<(String, f32, f32)> = want.iter().map(|(i, w, h)| (i.to_string(), *w, *h)).collect();
    assert_eq!(got, want);
}

/// A percentage height against an indefinite containing block is not
/// definite: the width fills the parent and the height follows it (Chrome:
/// 600x600).
///
/// LIMIT: the width side holds (600), but the height does not follow it:
/// RustKit derives a height from the ratio only for `height: auto`, and a
/// percentage of an indefinite height behaves as auto everywhere except
/// there (20, the line). A height-side follow-up, outside the width fix.
#[test]
#[ignore = "a percentage height of an indefinite parent takes no height from the ratio (height-side follow-up)"]
fn a_percentage_height_of_an_auto_height_parent_does_not_drive_the_width() {
    let got = sizes(r#"<div id="pct" style="height:50%;aspect-ratio:1">x</div>"#, &["pct"]);
    assert_eq!(got, vec![("pct".to_string(), 600.0, 600.0)]);
}
