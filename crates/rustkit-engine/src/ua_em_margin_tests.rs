//! Chrome board re-rank 2026-11-07 (hub trench/z/BOARD-RERANK-2026-11-07.md):
//! the UA stylesheet's `em` margins resolve against the ELEMENT'S OWN
//! font-size (html.css: `p { margin-block: 1em }`, `h2 { margin-block:
//! 0.83em }`, `blockquote { margin: 1em 40px }`, …). RustKit wrote them as px
//! computed at 16px, so `.subtitle { font-size: 0.9rem }` on a `<p>` kept 16px
//! margins where Chromium has 14.4px, and every box below the subtitle on
//! settings.html and report.html sat 1.6px low (all 9 rows of those pages).
//!
//! Expected values are Chrome for Testing 148.0.7778.216 on the same markup
//! (getComputedStyle margins, and the two gaps of the board reduction).

use super::*;

const PAGE: &str = "<!doctype html><style>body{margin:0;font-size:10px}.s{font-size:14.4px}h1.t{font-size:28px}</style>\
<p id=p>x</p><p id=ps class=s>x</p><h1 id=h1t class=t>x</h1><h2 id=h2 class=s>x</h2><h3 id=h3 class=s>x</h3>\
<h4 id=h4 class=s>x</h4><h5 id=h5 class=s>x</h5><h6 id=h6 class=s>x</h6><ul id=ul><li>x</li></ul>\
<ol id=ol class=s><li>x</li></ol><dl id=dl><dd>x</dd></dl><blockquote id=bq>x</blockquote>\
<figure id=fig>x</figure><pre id=pre>x</pre><hr id=hr class=s><menu id=menu><li>x</li></menu>";

/// (id, margin-top, margin-right, margin-bottom, margin-left), Chromium 148.
const WANT: &[(&str, f64, f64, f64, f64)] = &[
    ("p", 10.0, 0.0, 10.0, 0.0),
    ("ps", 14.4, 0.0, 14.4, 0.0),
    ("h1t", 18.76, 0.0, 18.76, 0.0),
    ("h2", 11.952, 0.0, 11.952, 0.0),
    ("h3", 14.4, 0.0, 14.4, 0.0),
    ("h4", 19.152, 0.0, 19.152, 0.0),
    ("h5", 24.048, 0.0, 24.048, 0.0),
    ("h6", 33.552, 0.0, 33.552, 0.0),
    ("ul", 10.0, 0.0, 10.0, 0.0),
    ("ol", 14.4, 0.0, 14.4, 0.0),
    ("dl", 10.0, 0.0, 10.0, 0.0),
    ("bq", 10.0, 40.0, 10.0, 40.0),
    ("fig", 10.0, 40.0, 10.0, 40.0),
    ("pre", 10.0, 0.0, 10.0, 0.0),
    ("hr", 7.2, 0.0, 7.2, 0.0),
    ("menu", 10.0, 0.0, 10.0, 0.0),
];

fn eval(page: &str, script: &str) -> String {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 800, height: 600 })
        .expect("view");
    engine.load_html(view, page).expect("load");
    let got = engine.execute_script(view, script).expect("script");
    got.trim_start_matches("String(\"").trim_end_matches("\")").to_string()
}

#[test]
fn ua_em_margins_resolve_against_the_elements_own_font_size() {
    let ids: Vec<String> = WANT.iter().map(|w| format!("'{}'", w.0)).collect();
    let script = format!(
        "[{}].map(function (i) {{ var c = getComputedStyle(document.getElementById(i)); \
         return [c.marginTop, c.marginRight, c.marginBottom, c.marginLeft].map(parseFloat).join(','); }}).join(';')",
        ids.join(",")
    );
    let got = eval(PAGE, &script);
    let rows: Vec<&str> = got.split(';').collect();
    assert_eq!(rows.len(), WANT.len(), "script result: {got}");
    let wrong: Vec<String> = WANT
        .iter()
        .zip(rows)
        .filter_map(|((id, t, r, b, l), row)| {
            let v: Vec<f64> = row.split(',').map(|s| s.parse().unwrap_or(f64::NAN)).collect();
            let want = [*t, *r, *b, *l];
            let ok = v.len() == 4 && v.iter().zip(want).all(|(g, w)| (g - w).abs() < 0.01);
            (!ok).then(|| format!("#{id}: {row}, Chromium {t},{r},{b},{l}"))
        })
        .collect();
    assert!(wrong.is_empty(), "{} of {} wrong:\n{}", wrong.len(), WANT.len(), wrong.join("\n"));
}

/// The board reduction (settings.html / report.html header): an h1 with an
/// author bottom margin, then `<p class=subtitle>` whose font-size the author
/// set. The collapsed gap is max(8, 1em of the p) = 14.4, and its own author
/// bottom margin stays 24. RustKit laid the gap out at 16.
#[test]
fn a_restyled_paragraph_collapses_with_its_own_em_margin() {
    let page = "<!doctype html><style>body{margin:0}h1{margin:0 0 0.5rem;font-size:1.75rem}\
                .subtitle{font-size:0.9rem;margin-bottom:1.5rem}</style>\
                <h1>Settings</h1><p class=subtitle id=sub>Configure</p><div id=after style=\"height:10px\"></div>";
    let got = eval(
        page,
        "(function () { var h = document.querySelector('h1').getBoundingClientRect(), \
         s = document.getElementById('sub').getBoundingClientRect(), \
         a = document.getElementById('after').getBoundingClientRect(); \
         return [s.top - h.bottom, a.top - s.bottom].join(','); })()",
    );
    let v: Vec<f64> = got.split(',').map(|s| s.parse().unwrap_or(f64::NAN)).collect();
    // Chromium 148: 14.390625 (LayoutUnit-snapped 14.4) and 24.
    assert!(
        v.len() == 2 && (v[0] - 14.4).abs() < 0.05 && (v[1] - 24.0).abs() < 0.05,
        "gaps {got}, Chromium 14.39,24"
    );
}
