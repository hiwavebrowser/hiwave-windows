//! Row REPLACED-FLOAT-AVOID, CSS 2.1 §9.5: the border box of a block-level replaced
//! element, a table, or a block that establishes a new block formatting context must
//! not overlap the margin box of a float in the same formatting context. It goes beside
//! the float when it fits in the width the float leaves, else down until it fits. Its
//! own margins may lie over the float; an auto margin takes the width the float leaves.
//! A plain block still lies under the float (the control case), and a cleared block
//! replaced box is placed below the float with its margins collapsed as #1335 set out.
//!
//! Oracle: Chrome for Testing 148.0.7778.216 through
//! `tools/parity_oracle/replaced_float_avoid_log.mjs`, receipt
//! `docs/diagnostics/2026-11-19/replaced_float_avoid_cft148.json`, which this file
//! reads: every element with an id in a 400px-wide container, as `id=x,y,width,height`.

use super::*;

const RECEIPT: &str =
    include_str!("../../../docs/diagnostics/2026-11-19/replaced_float_avoid_cft148.json");

fn layout_of(body: &str) -> String {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds {
            x: 0,
            y: 0,
            width: 800,
            height: 600,
        })
        .expect("view");
    let page = format!(
        "<!doctype html><style>body{{margin:0}}#c{{width:400px}}</style><div id=c>{body}</div>"
    );
    engine.load_html(view, &page).expect("load");
    let got = engine
        .execute_script(
            view,
            "Array.prototype.map.call(document.querySelectorAll('#c [id]'), function (e) { \
             var r = e.getBoundingClientRect(); \
             return e.id + '=' + r.x + ',' + r.y + ',' + r.width + ',' + r.height; }).join(' ')",
        )
        .expect("script");
    got.trim_start_matches("String(\"")
        .trim_end_matches("\")")
        .to_string()
}

fn same(got: &str, want: &str) -> bool {
    let parse = |s: &str| -> Vec<(String, Vec<f32>)> {
        s.split_whitespace()
            .filter_map(|r| {
                let (id, rest) = r.split_once('=')?;
                Some((
                    id.to_string(),
                    rest.split(',').filter_map(|v| v.parse().ok()).collect(),
                ))
            })
            .collect()
    };
    let (g, w) = (parse(got), parse(want));
    g.len() == w.len()
        && g.iter().zip(&w).all(|((gi, gv), (wi, wv))| {
            gi == wi && gv.len() == wv.len() && gv.iter().zip(wv).all(|(a, b)| (a - b).abs() < 0.5)
        })
}

#[test]
fn boxes_that_avoid_floats_are_placed_as_chrome_places_them() {
    let receipt: serde_json::Value = serde_json::from_str(RECEIPT).unwrap();
    assert_eq!(receipt["chrome"], "148.0.7778.216");
    let cases = receipt["cases"].as_array().unwrap();
    let wrong: Vec<String> = cases
        .iter()
        .filter_map(|case| {
            let label = case["label"].as_str().unwrap();
            let want = case["want"].as_str().unwrap();
            let got = layout_of(case["body"].as_str().unwrap());
            (!same(&got, want)).then(|| format!("{label}:\n  ours   {got}\n  Chrome {want}"))
        })
        .collect();
    assert!(
        wrong.is_empty(),
        "{} of {} wrong:\n{}",
        wrong.len(),
        cases.len(),
        wrong.join("\n")
    );
}
