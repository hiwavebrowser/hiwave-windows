//! GR3: a grid item lays its children out at the width it ends up with.
//! Grid sizing measures an item more than once (min-/max-content while the
//! tracks are sized, then the used width); the children must be placed by
//! the LAST pass. Floats that fit side by side in the final width stay on
//! one line (`s_two`: two 100px floats in a 200-wide shrink-to-fit item,
//! 10 tall, not 20), in shrink-to-fit, centred and stretched items, and the
//! same holds for non-float content: inline-blocks, wrapped text in auto
//! columns, percentage widths and flex children.
//!
//! Expected boxes: docs/diagnostics/2026-11-12/grid_item_final_width_cft148.txt
//! for grid_item_final_width.html beside it (Chrome for Testing
//! 148.0.7778.216, getBoundingClientRect at 800x600).

use super::*;

const PAGE: &str = include_str!("../../../docs/diagnostics/2026-11-12/grid_item_final_width.html");
const CHROME: &str = include_str!("../../../docs/diagnostics/2026-11-12/grid_item_final_width_cft148.txt");

/// Boxes still off for a reason outside this row: two `auto` columns share
/// the free space equally from the items' MIN-content (100 for two 100px
/// floats), where Chrome first grows each to its max-content (200) and then
/// stretches (296 / 104; here 246 / 154). Track sizing, its own row (GR4).
/// They must still DIFFER; one that starts matching comes off the list.
const KNOWN: &[&str] = &["t_auto_a", "t_auto_b"];

#[test]
fn grid_items_lay_out_children_at_their_final_width() {
    let expected: Vec<(&str, [f32; 4])> = CHROME
        .lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .map(|l| {
            let mut it = l.split_whitespace();
            let id = it.next().unwrap();
            let mut v = [0.0; 4];
            for x in v.iter_mut() {
                *x = it.next().unwrap().parse().unwrap();
            }
            (id, v)
        })
        .collect();
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 800, height: 600 })
        .expect("view");
    engine.load_html(view, PAGE).expect("load");
    let ids: Vec<String> = expected.iter().map(|(id, _)| format!("'{id}'")).collect();
    let got = engine
        .execute_script(
            view,
            &format!(
                "[{}].map(function (i) {{ var r = document.getElementById(i).getBoundingClientRect(); return [r.x, r.width, r.y, r.height].join(','); }}).join(' ')",
                ids.join(",")
            ),
        )
        .expect("script");
    let got: Vec<Vec<f32>> = got
        .trim_start_matches("String(\"")
        .trim_end_matches("\")")
        .split(' ')
        .map(|v| v.split(',').map(|n| n.parse().unwrap()).collect())
        .collect();
    assert_eq!(expected.len(), 27, "the Chrome table has 27 boxes");
    assert_eq!(got.len(), expected.len(), "the page answered for every box");
    // Text runs are measured with CoreText, so a wrapped-text box may be a
    // fraction off; 0.5 px still catches a float or line in the wrong place.
    let wrong: Vec<String> = expected
        .iter()
        .zip(&got)
        .filter(|((id, e), g)| {
            let off = e.iter().zip(g.iter()).any(|(e, g)| (e - g).abs() > 0.5);
            off != KNOWN.contains(id)
        })
        .map(|((id, e), g)| {
            let listed = if KNOWN.contains(id) { ", listed in KNOWN" } else { "" };
            format!("{id}: {g:?} (Chromium {e:?}{listed})")
        })
        .collect();
    assert!(wrong.is_empty(), "{} of {} boxes differ (x, width, y, height):\n{}", wrong.len(), expected.len(), wrong.join("\n"));
}
