//! Z plan CH2.B1: chrome.html's sidebar laid out 1105px tall in an 800px
//! viewport. Reduced: `html, body { height: 100% } body { display: flex }`.
//! The body is a single-line ROW whose height is a percentage the block pass
//! resolves (800), but the flex line's cross size came from the block
//! pre-pass's stack of the items (800 + 18 = 818, 1100 + 18 = 1118), so every
//! stretched or `height: 100%` item grew past the viewport. A body with
//! `height: 800px` was right all along.
//!
//! Expected values: Chromium 143.0.7499.4 on these exact pages at 1280x800
//! (body, #s, #m heights, then #s's children).

#![cfg(all(target_os = "macos", feature = "headless"))]

use super::*;

const CHAIN: &str = "<style>html,body{height:100%;margin:0;overflow:hidden}body{display:flex}";
const READ: &str = "(function(){function h(e){return Math.round(e.getBoundingClientRect().height)}\
    var s=document.getElementById('s');return 'body='+h(document.body)+' s='+h(s)+\
    ' m='+h(document.getElementById('m'))+' kids='+Array.prototype.map.call(s.children,h).join(',');})()";

fn read(html: &str) -> String {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let id = engine
        .create_headless_view(Bounds::new(0, 0, 1280, 800))
        .expect("view");
    engine.load_html(id, html).expect("load");
    let v = engine.execute_script(id, READ).expect("script");
    v.strip_prefix("String(\"")
        .and_then(|v| v.strip_suffix("\")"))
        .unwrap_or(&v)
        .to_string()
}

fn page(sidebar: &str, kids: &str) -> String {
    format!(
        "{CHAIN}#s{{width:220px;overflow:hidden;flex-shrink:0;{sidebar}}}#m{{flex:1}}</style>\
         <body><aside id=s>{kids}</aside><div id=m>x</div></body>"
    )
}

#[test]
fn percent_height_row_sizes_its_line_to_the_body() {
    let tall = "<div style=height:1100px></div>";
    let cases = [
        ("height:100%", tall, "body=800 s=800 m=800 kids=1100"),
        ("", tall, "body=800 s=800 m=800 kids=1100"),
        ("height:100%;display:flex;flex-direction:column", tall, "body=800 s=800 m=800 kids=800"),
        (
            "height:100%;display:flex;flex-direction:column",
            "<section style=height:300px></section>\
             <section style=\"flex:1;overflow:hidden;display:flex;flex-direction:column\">\
             <div style=height:800px></div></section>",
            "body=800 s=800 m=800 kids=300,500",
        ),
    ];
    let wrong: Vec<String> = cases
        .iter()
        .filter_map(|(sidebar, kids, chrome)| {
            let got = read(&page(sidebar, kids));
            (got != *chrome).then(|| format!("#s{{{sidebar}}}: {got} (Chromium 143: {chrome})"))
        })
        .collect();
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// The same row with a pixel height was always right: the control.
#[test]
fn pixel_height_row_control() {
    let html = "<style>html,body{margin:0;overflow:hidden}body{height:800px;display:flex}\
        #s{width:220px;height:100%;overflow:hidden;flex-shrink:0}#m{flex:1}</style>\
        <body><aside id=s><div style=height:1100px></div></aside><div id=m>x</div></body>";
    assert_eq!(read(html), "body=800 s=800 m=800 kids=1100");
}
