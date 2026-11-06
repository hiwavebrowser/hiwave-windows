//! Z plan CH2.B8 (disclosed on #903): `html, body { height: 100% }` and
//! `.row { display: flex; height: 100% }` gave `.row` its content height
//! (102px) in a 300x200 viewport; Chromium 143: 200. The percentage chain
//! from the initial containing block through html and body resolves to the
//! viewport height for every display type of `.row`.
//!
//! Expected values: Chromium 143.0.7499.4 on these exact pages at 300x200
//! (`tools/ch2_b8/oracle.mjs`, `variants.mjs`).

#![cfg(all(target_os = "macos", feature = "headless"))]

use super::*;

const READ: &str = "(function(){function h(e){return Math.round(e.getBoundingClientRect().height)}\
    return 'html='+h(document.documentElement)+' body='+h(document.body)+' row='+h(document.querySelector('.row'));})()";

const SIDE: &str = "<aside class=side><section><div style='padding:8px;color:#000'>Workspaces</div>\
    <button style='background:rgb(255,0,0);width:120px;height:30px;border:0'>Open</button>\
    <div style='background:rgb(0,0,255);width:50px;height:20px'></div></section></aside>\
    <main class=main>main</main>";
const SIDE_CSS: &str = ".side{width:0;min-width:0;overflow:hidden;display:flex;flex-direction:column;\
    flex-shrink:0;background:#eee}.main{flex:1;background:#fff}";

fn read(html: &str) -> String {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let id = engine
        .create_headless_view(Bounds::new(0, 0, 300, 200))
        .expect("view");
    engine.load_html(id, html).expect("load");
    let v = engine.execute_script(id, READ).expect("script");
    v.strip_prefix("String(\"")
        .and_then(|v| v.strip_suffix("\")"))
        .unwrap_or(&v)
        .to_string()
}

fn doc(css: &str, body: &str) -> String {
    format!("<!doctype html><html><head><style>{css}</style></head><body>{body}</body></html>")
}

#[test]
fn percent_height_chain_reaches_the_row() {
    let text = "<div class=row><div>a</div></div>";
    let cases = [
        (
            "b2_page",
            doc(
                &format!("html,body{{margin:0;height:100%;background:#fff}}.row{{display:flex;height:100%}}{SIDE_CSS}"),
                &format!("<div class=row>{SIDE}</div>"),
            ),
        ),
        ("flex_row_text", doc("html,body{margin:0;height:100%}.row{display:flex;height:100%}", text)),
        (
            "flex_col_text",
            doc("html,body{margin:0;height:100%}.row{display:flex;flex-direction:column;height:100%}", text),
        ),
        ("block_text", doc("html,body{margin:0;height:100%}.row{height:100%}", text)),
        ("grid_text", doc("html,body{margin:0;height:100%}.row{display:grid;height:100%}", text)),
        (
            "no_doctype_flex",
            format!("<style>html,body{{margin:0;height:100%}}.row{{display:flex;height:100%}}</style>{text}"),
        ),
    ];
    let chrome = "html=200 body=200 row=200";
    let wrong: Vec<String> = cases
        .iter()
        .filter_map(|(name, html)| {
            let got = read(html);
            (got != chrome).then(|| format!("{name}: {got} (Chromium 143: {chrome})"))
        })
        .collect();
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// The document element's rect with an explicit height is that height,
/// whatever body's margin box does (body overflows html); with `auto` it is
/// body's margin box. The engine folded body's margin box into an explicit
/// height too: `html, body { height: 100% }` with body's default 8px margin
/// read html=216 (Chromium 200).
#[test]
fn an_explicit_html_height_is_the_root_rect() {
    let cases = [
        ("body_margin", doc("html,body{height:100%}.row{display:flex;height:100%}", "<div class=row><div>a</div></div>"), "html=200 body=200 row=200"),
        ("html50_body_tall", doc("html{height:50px}body{margin:8px}.row{height:120px}", "<div class=row>a</div>"), "html=50 body=120 row=120"),
        ("html300_body_short", doc("html{height:300px}body{margin:0}.row{height:20px}", "<div class=row>a</div>"), "html=300 body=20 row=20"),
        ("html_auto_body_margin", doc("body{margin:8px}.row{height:20px}", "<div class=row>a</div>"), "html=36 body=20 row=20"),
    ];
    let wrong: Vec<String> = cases
        .iter()
        .filter_map(|(name, html, chrome)| {
            let got = read(html);
            (got != *chrome).then(|| format!("{name}: {got} (Chromium 143: {chrome})"))
        })
        .collect();
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}
