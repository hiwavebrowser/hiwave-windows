//! CH2.B31: the UA heading font-sizes are RELATIVE to the parent's computed
//! font-size (html.css: `h1 { font-size: 2em }`, h2 1.5em, h3 1.17em, h4 1em,
//! h5 .83em, h6 .67em). RustKit wrote them as px computed at 16px, so a
//! heading under any author font-size other than 16px had the wrong size,
//! and with it the wrong em margins (which resolve against the heading's own
//! font-size since CH2.B30).
//!
//! Chrome for Testing 148.0.7778.216 no longer applies the old
//! `:is(article, aside, nav, section) h1 { font-size: 1.5em }` rule: an h1
//! in a section, an article and an aside > section all compute 32px. The
//! last row pins that.
//!
//! Expected values are getComputedStyle on the same markup in CfT 148
//! (impl-board2 probe, 2026-11-08).

use super::*;

const PAGE: &str = "<!doctype html><body>\
<div style=\"font-size:10px\"><h1 id=a>a</h1><h2 id=b>b</h2><h3 id=c>c</h3><h4 id=d>d</h4><h5 id=e>e</h5><h6 id=f>f</h6></div>\
<section><h1 id=s>s</h1></section><article><h1 id=ar>ar</h1></article><aside><section><h1 id=ss>ss</h1></section></aside>\
<h1 id=top>top</h1></body>";

/// (id, font-size, margin-top = margin-bottom), Chromium 148.
const WANT: &[(&str, f64, f64)] = &[
    ("a", 20.0, 13.4),
    ("b", 15.0, 12.45),
    ("c", 11.7, 11.7),
    ("d", 10.0, 13.3),
    ("e", 8.3, 13.861),
    ("f", 6.7, 15.611),
    ("s", 32.0, 21.44),
    ("ar", 32.0, 21.44),
    ("ss", 32.0, 21.44),
    ("top", 32.0, 21.44),
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
fn ua_heading_font_sizes_are_relative_to_the_parent() {
    let ids: Vec<String> = WANT.iter().map(|w| format!("'{}'", w.0)).collect();
    let script = format!(
        "[{}].map(function (i) {{ var c = getComputedStyle(document.getElementById(i)); \
         return [c.fontSize, c.marginTop, c.marginBottom].map(parseFloat).join(','); }}).join(';')",
        ids.join(",")
    );
    let got = eval(PAGE, &script);
    let rows: Vec<&str> = got.split(';').collect();
    assert_eq!(rows.len(), WANT.len(), "script result: {got}");
    let wrong: Vec<String> = WANT
        .iter()
        .zip(rows)
        .filter_map(|((id, fs, m), row)| {
            let v: Vec<f64> = row.split(',').map(|s| s.parse().unwrap_or(f64::NAN)).collect();
            let want = [*fs, *m, *m];
            let ok = v.len() == 3 && v.iter().zip(want).all(|(g, w)| (g - w).abs() < 0.01);
            (!ok).then(|| format!("#{id}: {row}, Chromium {fs},{m},{m}"))
        })
        .collect();
    assert!(wrong.is_empty(), "{} of {} wrong:\n{}", wrong.len(), WANT.len(), wrong.join("\n"));
}
